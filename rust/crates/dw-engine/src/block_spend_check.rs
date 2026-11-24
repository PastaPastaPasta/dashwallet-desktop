//! The one-time block-spend check (#268's migration). Before the wallet
//! store kept block spends, a release could give back a coin a foreign
//! block had spent. The suspects are the coins this wallet gave back that
//! way: the inputs of the payments it abandoned. One that is unspent now,
//! or spent only by an
//! unconfirmed transaction, is held (`crate::block_spends`) while the SPV
//! store's filters and blocks are searched, from its funding height to the
//! tip as the search starts, for a block that spent it.
//!
//! A spender found at or below the wallet's ChainLock is stored as a row
//! with the coin spent; one above it keeps the coin held until the lock
//! covers it. A suspect no block spent is released only when the search
//! covered the whole range: every height has a stored filter, and every
//! filter match a stored block whose hash is the header chain's. Any gap
//! falls back to the wallet's owed rescan from the lowest suspect's funding
//! height, with sends refused while it runs, and the hold lifts once it is
//! done.
//!
//! The suspects are kept in `app.sqlite` until the check is done, so a
//! restart resumes it; offline it stays held. Whether a wallet was checked
//! is the wallet store's own mark: a store that keeps block spends for the
//! first time is checked once.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::Arc;

use std::ops::{Range, RangeInclusive};

use dashcore::bip158::{BlockFilter, FilterQuery};
use dashcore::{Block, BlockHash, OutPoint, ScriptBuf, Txid};
use futures_util::future::BoxFuture;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::changeset::BlockSpend;

use crate::{EngineError, NetworkSession, WalletId};

/// Wallet-scope setting: the suspects still held, `txid:vout@funded` joined
/// by `,`.
const CHECK_KEY: &str = "block_spend_check";
/// Wallet-scope setting: the fallback rescan from this checkpoint was
/// recorded; the hold lifts once the wallet's replay is done.
const CHECK_RESCAN_KEY: &str = "block_spend_check_rescan";
/// Filters read per store call.
const FILTER_CHUNK: u32 = 2_000;
#[cfg(not(test))]
const PASS_EVERY: std::time::Duration = std::time::Duration::from_secs(20);

/// What the check reads of the SPV store and header chain.
pub(crate) trait StoredChain: Send + Sync {
    /// The heights the store holds filters for; `Err` when SPV is not
    /// running.
    fn filter_heights(&self) -> BoxFuture<'_, Result<Option<RangeInclusive<u32>>, ()>>;
    fn filters(&self, range: Range<u32>) -> BoxFuture<'_, Result<Vec<Vec<u8>>, ()>>;
    fn block(&self, height: u32) -> BoxFuture<'_, Result<Option<(BlockHash, Block)>, ()>>;
    fn header(&self, height: u32) -> BoxFuture<'_, Option<BlockHash>>;
    fn tip(&self) -> Option<u32>;
}

struct SpvChain<'a> {
    spv: &'a platform_wallet::spv::SpvRuntime,
    hub: &'a crate::events::SessionHub,
}

impl StoredChain for SpvChain<'_> {
    fn filter_heights(&self) -> BoxFuture<'_, Result<Option<RangeInclusive<u32>>, ()>> {
        Box::pin(async move { self.spv.stored_filter_heights().await.map_err(drop) })
    }

    fn filters(&self, range: Range<u32>) -> BoxFuture<'_, Result<Vec<Vec<u8>>, ()>> {
        Box::pin(async move { self.spv.stored_filters(range).await.map_err(drop) })
    }

    fn block(&self, height: u32) -> BoxFuture<'_, Result<Option<(BlockHash, Block)>, ()>> {
        Box::pin(async move { self.spv.stored_block(height).await.map_err(drop) })
    }

    fn header(&self, height: u32) -> BoxFuture<'_, Option<BlockHash>> {
        Box::pin(self.hub.header_hash(height))
    }

    fn tip(&self) -> Option<u32> {
        self.hub.tracker().tip_height()
    }
}

/// What one pass over the suspects ended with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckPass {
    /// Nothing is held.
    Done,
    /// Still held: SPV is offline, the filters don't reach the tip, or a
    /// spender is above the ChainLock.
    Waiting,
    /// A gap in the stored filters or blocks: the owed rescan runs.
    Rescanning,
}

fn encode(suspects: &BTreeMap<OutPoint, u32>) -> String {
    (suspects.iter())
        .map(|(o, h)| format!("{}:{}@{h}", o.txid, o.vout))
        .collect::<Vec<_>>()
        .join(",")
}

fn decode(text: &str) -> BTreeMap<OutPoint, u32> {
    (text.split(',').filter(|s| !s.is_empty()))
        .filter_map(|s| {
            let (outpoint, funded) = s.split_once('@')?;
            let (txid, vout) = outpoint.split_once(':')?;
            Some((
                OutPoint::new(Txid::from_str(txid).ok()?, vout.parse().ok()?),
                funded.parse().ok()?,
            ))
        })
        .collect()
}

/// The height of the block that funded `o` in the history `txs`, the
/// wallet's birth when unknown.
fn funded_height(
    txs: &std::collections::HashMap<Txid, crate::history::TxEntry>,
    o: &OutPoint,
    birth: u32,
) -> u32 {
    (txs.get(&o.txid))
        .and_then(|e| e.context.block_info().map(|b| b.height()))
        .unwrap_or(birth)
}

enum Search {
    Found(BTreeMap<OutPoint, BlockSpend>),
    Waiting,
    Gap,
}

/// Searches `chain`'s stored filters and blocks, from `lowest` to the tip
/// as the search starts, for the blocks that spent `suspects` (module
/// doc). `Gap` when the store cannot prove the whole range.
async fn search(
    chain: &dyn StoredChain,
    suspects: &BTreeMap<OutPoint, u32>,
    scripts: &BTreeMap<OutPoint, ScriptBuf>,
    lowest: u32,
) -> Search {
    let Ok(stored) = chain.filter_heights().await else {
        return Search::Waiting;
    };
    let Some(tip) = chain.tip() else {
        return Search::Waiting;
    };
    let Some(stored) = stored else {
        return Search::Gap;
    };
    if *stored.end() < tip {
        return Search::Waiting;
    }
    if *stored.start() > lowest || scripts.len() < suspects.len() {
        return Search::Gap;
    }
    let mut found: BTreeMap<OutPoint, BlockSpend> = BTreeMap::new();
    let mut from = lowest.max(*stored.start());
    while from <= tip {
        let to = from.saturating_add(FILTER_CHUNK).min(tip + 1);
        let Ok(filters) = chain.filters(from..to).await else {
            return Search::Waiting;
        };
        if filters.len() as u32 != to - from {
            return Search::Gap;
        }
        for (height, bytes) in (from..to).zip(filters) {
            let watched: FilterQuery = (suspects.iter())
                .filter(|(o, funded)| **funded <= height && !found.contains_key(*o))
                .filter_map(|(o, _)| scripts.get(o).map(|s| s.as_bytes()))
                .collect();
            if watched.is_empty() {
                continue;
            }
            let Some(hash) = chain.header(height).await else {
                return Search::Gap;
            };
            let matched = BlockFilter::new(&bytes)
                .match_any(&hash, &watched)
                .unwrap_or(true);
            if !matched {
                continue;
            }
            let block = match chain.block(height).await {
                Ok(Some((stored_hash, block))) if stored_hash == hash => block,
                Ok(_) => return Search::Gap,
                Err(()) => return Search::Waiting,
            };
            for tx in &block.txdata {
                for input in &tx.input {
                    let o = input.previous_output;
                    if suspects.contains_key(&o) {
                        found.entry(o).or_insert(BlockSpend {
                            outpoint: o,
                            height,
                            block_hash: hash,
                            spender_txid: tx.txid(),
                        });
                    }
                }
            }
        }
        from = to;
    }
    Search::Found(found)
}

impl NetworkSession {
    /// The suspects still held for `wallet`, with their funding heights.
    async fn held_suspects(&self, wallet: WalletId) -> Result<BTreeMap<OutPoint, u32>, EngineError> {
        let scope = wallet.to_string();
        let text = self
            .appdb_op(move |db| db.setting(&scope, CHECK_KEY))
            .await?;
        Ok(text.as_deref().map(decode).unwrap_or_default())
    }

    async fn store_suspects(
        &self,
        wallet: WalletId,
        suspects: &BTreeMap<OutPoint, u32>,
    ) -> Result<(), EngineError> {
        let scope = wallet.to_string();
        let value = (!suspects.is_empty()).then(|| encode(suspects));
        self.appdb_op(move |db| {
            let rescan = match value {
                Some(_) => db.setting(&scope, CHECK_RESCAN_KEY)?,
                None => None,
            };
            db.set_settings_durable(
                &scope,
                &[
                    (CHECK_KEY, value.as_deref()),
                    (CHECK_RESCAN_KEY, rescan.as_deref()),
                ],
            )
        })
        .await
    }

    /// The coins this wallet's abandons gave back before the store kept
    /// block spends, that no row covers and that are unspent now or spent only by an
    /// unconfirmed transaction, with their funding heights (the wallet's
    /// birth when unknown).
    async fn block_spend_suspects(
        &self,
        manager: &crate::session::Manager,
        wallet: WalletId,
    ) -> Result<BTreeMap<OutPoint, u32>, EngineError> {
        let (unspent, birth) = {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let Some(info) = wm.get_wallet_info(&wallet.0) else {
                return Ok(BTreeMap::new());
            };
            let core = &info.core_wallet;
            let unspent: BTreeSet<OutPoint> = (core.accounts.all_funding_accounts().into_iter())
                .flat_map(|a| a.utxos.keys().copied().collect::<Vec<_>>())
                .collect();
            (unspent, core.birth_height())
        };
        let rows = self.hub.block_spends.rows(&wallet);
        let appdb = Arc::clone(&self.live()?.appdb);
        let (marks, sent, read) = tokio::task::spawn_blocking(move || {
            let (marks, read) = crate::tx_actions::load_abandoned(&appdb, wallet);
            let (sent, sent_read) = crate::tx_actions::read_sent(&appdb, wallet);
            let read = read && sent_read != crate::tx_actions::SentRead::Failed;
            (marks, sent, read)
        })
        .await?;
        if !read {
            return Err(EngineError::Storage("the abandoned or sent marks do not read".into()));
        }
        // A mark whose copy does not read back still gave its coins back:
        // the sent mark names them.
        let abandoned: BTreeSet<OutPoint> = (marks.copies.iter())
            .flat_map(|(_, tx)| tx.input.iter().map(|i| i.previous_output))
            .chain(
                (marks.unread.iter())
                    .filter_map(|t| sent.get(t))
                    .flat_map(|m| m.coins.iter().copied()),
            )
            .collect();
        Ok(self.hub.history.read(&wallet, |txs| {
            let mut suspects = BTreeMap::new();
            for o in abandoned {
                if rows.contains_key(&o) {
                    continue;
                }
                let spenders: Vec<_> = (txs.values())
                    .filter(|e| !e.is_abandoned())
                    .filter(|e| e.tx.input.iter().any(|i| i.previous_output == o))
                    .collect();
                let unconfirmed_only = !spenders.is_empty()
                    && spenders.iter().all(|e| e.context.block_info().is_none());
                if !(unspent.contains(&o) || unconfirmed_only) {
                    continue;
                }
                suspects.insert(o, funded_height(txs, &o, birth));
            }
            suspects
        }))
    }

    /// At each load of `wallet`, before any build: resumes a check still
    /// held, or, the first time the wallet store keeps block spends and
    /// after a load that read the whole history, finds the suspects, holds
    /// them and marks the store checked. The held
    /// coins are locked under the manager's write lock; the search runs
    /// apart ([`Self::block_spend_check_pass`]).
    pub(crate) async fn start_block_spend_check(
        &self,
        manager: &crate::session::Manager,
        wallet: WalletId,
        history_complete: bool,
    ) -> Result<(), EngineError> {
        let live = self.live()?;
        let mut suspects = self.held_suspects(wallet).await?;
        if suspects.is_empty() {
            let persister = Arc::clone(&live.persister);
            let done = tokio::task::spawn_blocking(move || persister.block_spends_migration_done(&wallet.0))
                .await?
                .map_err(|e| EngineError::Storage(format!("the block-spend check mark: {e}")))?;
            #[cfg(test)]
            let done = done
                && !(self.hub.block_spends.ignore_mark)
                    .swap(false, std::sync::atomic::Ordering::SeqCst);
            if done {
                return Ok(());
            }
            if !history_complete {
                tracing::warn!(%wallet, "history load incomplete: the block-spend check waits for a complete load");
                return Ok(());
            }
            suspects = self.block_spend_suspects(manager, wallet).await?;
            self.store_suspects(wallet, &suspects).await?;
            let persister = Arc::clone(&live.persister);
            tokio::task::spawn_blocking(move || persister.mark_block_spends_migration_done(&wallet.0))
                .await?
                .map_err(|e| EngineError::Storage(format!("the block-spend check mark: {e}")))?;
            if !suspects.is_empty() {
                tracing::info!(%wallet, suspects = suspects.len(), "checking coins given back before block spends were kept");
            }
        }
        self.hub
            .block_spends
            .set_checking(wallet, suspects.keys().copied().collect());
        self.hold_block_spends(manager, wallet).await;
        Ok(())
    }

    /// One pass of the check for `wallet` (module doc).
    pub(crate) async fn block_spend_check_pass(
        &self,
        manager: &crate::session::Manager,
        wallet: WalletId,
    ) -> Result<CheckPass, EngineError> {
        let live = self.live()?;
        let suspects = self.held_suspects(wallet).await?;
        if suspects.is_empty() {
            self.hub.block_spends.set_checking(wallet, BTreeSet::new());
            return Ok(CheckPass::Done);
        }
        let scope = wallet.to_string();
        let rescan = self
            .appdb_op(move |db| db.setting(&scope, CHECK_RESCAN_KEY))
            .await?;
        if rescan.is_some() {
            if live.replay.get(wallet).await?.is_some() {
                return Ok(CheckPass::Rescanning);
            }
            return self.settle_suspects(manager, wallet, BTreeMap::new()).await;
        }
        let chain_lock = {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let Some(info) = wm.get_wallet_info(&wallet.0) else {
                return Ok(CheckPass::Waiting);
            };
            crate::history_ops::applied_chainlock(info).unwrap_or(0)
        };
        let scripts: BTreeMap<OutPoint, ScriptBuf> = self.hub.history.read(&wallet, |txs| {
            (suspects.keys())
                .filter_map(|o| {
                    let out = txs.get(&o.txid)?.tx.output.get(o.vout as usize)?;
                    Some((*o, out.script_pubkey.clone()))
                })
                .collect()
        });
        let lowest = suspects.values().copied().min().unwrap_or(0);
        let spv = SpvChain {
            spv: manager.spv(),
            hub: &self.hub,
        };
        #[cfg(test)]
        let fake = self.hub.block_spends.stored_chain();
        #[cfg(test)]
        let chain: &dyn StoredChain = fake.as_deref().unwrap_or(&spv);
        #[cfg(not(test))]
        let chain: &dyn StoredChain = &spv;
        let found = match search(chain, &suspects, &scripts, lowest).await {
            Search::Found(found) => found,
            Search::Waiting => return Ok(CheckPass::Waiting),
            Search::Gap => return self.fall_back_to_rescan(manager, wallet, lowest).await,
        };
        let mut held = BTreeMap::new();
        for (o, spend) in found {
            if spend.height > chain_lock {
                held.insert(o, suspects[&o]);
                continue;
            }
            let persister = Arc::clone(&live.persister);
            tokio::task::spawn_blocking(move || persister.record_found_block_spend(&wallet.0, spend))
                .await?
                .map_err(|e| EngineError::Storage(format!("a found block spend: {e}")))?;
            self.hub.block_spends.insert(wallet, spend);
            tracing::info!(%wallet, outpoint = %o, height = spend.height, spender = %spend.spender_txid,
                "found the block that spent a coin given back");
        }
        self.settle_suspects(manager, wallet, held).await
    }

    /// Keeps only `held` under the check, lifts the hold of the rest and
    /// reapplies it under the manager's lock.
    async fn settle_suspects(
        &self,
        manager: &crate::session::Manager,
        wallet: WalletId,
        held: BTreeMap<OutPoint, u32>,
    ) -> Result<CheckPass, EngineError> {
        self.store_suspects(wallet, &held).await?;
        self.hub
            .block_spends
            .set_checking(wallet, held.keys().copied().collect());
        self.hold_block_spends(manager, wallet).await;
        self.hub.pump.mark_balances(wallet);
        Ok(match held.is_empty() {
            true => CheckPass::Done,
            false => CheckPass::Waiting,
        })
    }

    /// The search cannot prove a suspect unspent: the wallet's owed rescan
    /// from `lowest` replays every block since (sends are refused while it
    /// runs), and the hold lifts once it is done.
    async fn fall_back_to_rescan(
        &self,
        manager: &crate::session::Manager,
        wallet: WalletId,
        lowest: u32,
    ) -> Result<CheckPass, EngineError> {
        let checkpoint = lowest.saturating_sub(1);
        self.record_rewind(manager, wallet, checkpoint).await?;
        let scope = wallet.to_string();
        let value = checkpoint.to_string();
        self.appdb_op(move |db| db.set_setting_durable(&scope, CHECK_RESCAN_KEY, Some(&value)))
            .await?;
        self.rewind_recorded(manager, wallet, checkpoint).await?;
        tracing::warn!(%wallet, checkpoint, "the stored filters cannot prove the given-back coins unspent; rescanning");
        Ok(CheckPass::Rescanning)
    }

    /// Runs a pass for every loaded wallet with a check held, and checks
    /// their block-spend rows above the ChainLock against the header chain
    /// again, every [`PASS_EVERY`] until the session closes.
    #[cfg(not(test))]
    pub(crate) fn spawn_block_spend_checks(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.rt.spawn(async move {
            loop {
                tokio::time::sleep(PASS_EVERY).await;
                let Some(this) = weak.upgrade() else { return };
                if this.block_spend_checks_once().await.is_err() {
                    return;
                }
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn spawn_block_spend_checks(self: &Arc<Self>) {}

    /// One round of [`Self::spawn_block_spend_checks`]. `Err` once the
    /// session closes.
    pub(crate) async fn block_spend_checks_once(&self) -> Result<(), EngineError> {
        let _op = self.enter().await?;
        let manager = self.manager()?;
        for id in manager.list_wallet_ids_blocking() {
            let wallet = WalletId(id);
            if matches!(self.check_block_spends(&manager, wallet).await, Ok(true)) {
                self.hold_block_spends(&manager, wallet).await;
            }
            if !self.hub.block_spends.checking(&wallet).is_empty()
                && let Err(e) = self.block_spend_check_pass(&manager, wallet).await
            {
                tracing::warn!(%wallet, error = %e, "the block-spend check pass failed; retrying later");
            }
        }
        Ok(())
    }
}
