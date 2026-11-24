//! The one-time block-spend check (#268's migration) and its hold, at the
//! engine level. A coin abandoned before block spends were kept, given
//! back while a block had spent it, is held from every build until the
//! stored filters and blocks prove it spent or unspent; a gap falls back
//! to the owed rescan; the hold survives a restart and every build path.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::{Range, RangeInclusive};
use std::sync::Arc;

use dashcore::bip158::BlockFilter;
use dashcore::{Address, Block, BlockHash, OutPoint, ScriptBuf, Transaction};
use futures_util::future::BoxFuture;

use super::own_spend_disk_tests::{
    Accept, COIN, COINS, FUNDED_AT, TIP, abandon, chain_lock, coins, foreign, fresh_address,
    funding, prepare, send, spendable, wait_stored,
};
use super::{block_with, elsewhere, engine_wallet, mempool, mined, reopen};
use crate::block_spend_check::{CheckPass, StoredChain};
use crate::send::CoinSource;
use crate::session::NetworkSession;
use crate::{EngineError, WalletId};

/// An SPV store holding a filter and a block at each height of `blocks`.
struct FakeChain {
    blocks: BTreeMap<u32, Block>,
    filters: BTreeMap<u32, Vec<u8>>,
    tip: u32,
}

impl FakeChain {
    /// The chain `blocks`, its filters built with `prevouts`' scripts.
    fn new(blocks: BTreeMap<u32, Block>, prevouts: &HashMap<OutPoint, ScriptBuf>) -> Self {
        let filters = (blocks.iter())
            .map(|(h, b)| {
                let filter = BlockFilter::new_script_filter(b, |o| {
                    Ok::<_, dashcore::bip158::Error>(
                        prevouts.get(o).cloned().unwrap_or_else(elsewhere),
                    )
                })
                .unwrap();
                (*h, filter.content)
            })
            .collect();
        let tip = blocks.keys().last().copied().unwrap_or(0);
        Self {
            blocks,
            filters,
            tip,
        }
    }
}

impl StoredChain for FakeChain {
    fn filter_heights(&self) -> BoxFuture<'_, Result<Option<RangeInclusive<u32>>, ()>> {
        let range = (self.filters.keys().next().copied())
            .zip(self.filters.keys().last().copied())
            .map(|(a, b)| a..=b);
        Box::pin(async move { Ok(range) })
    }

    fn filters(&self, range: Range<u32>) -> BoxFuture<'_, Result<Vec<Vec<u8>>, ()>> {
        let out = range
            .filter_map(|h| self.filters.get(&h).cloned())
            .collect();
        Box::pin(async move { Ok(out) })
    }

    fn block(&self, height: u32) -> BoxFuture<'_, Result<Option<(BlockHash, Block)>, ()>> {
        let block = self
            .blocks
            .get(&height)
            .map(|b| (b.block_hash(), b.clone()));
        Box::pin(async move { Ok(block) })
    }

    fn header(&self, height: u32) -> BoxFuture<'_, Option<BlockHash>> {
        let hash = self.blocks.get(&height).map(Block::block_hash);
        Box::pin(async move { hash })
    }

    fn tip(&self) -> Option<u32> {
        Some(self.tip)
    }
}

/// The block at `height` of the stored chain: its coinbase, then `txs`.
fn stored_block(height: u32, txs: Vec<Transaction>) -> Block {
    let coinbase = Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::from(height.to_le_bytes().to_vec()),
            ..Default::default()
        }],
        output: vec![dashcore::TxOut {
            value: 500 * COIN,
            script_pubkey: elsewhere(),
        }],
        special_transaction_payload: None,
    };
    let mut all = vec![coinbase];
    all.extend(txs);
    block_with(all)
}

struct Given {
    _dir: tempfile::TempDir,
    engine: crate::Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    a: OutPoint,
    funded: Transaction,
    w: Transaction,
}

impl Given {
    fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce(Arc<NetworkSession>, Arc<crate::session::Manager>) -> BoxFuture<'static, R>
        + Send
        + 'static,
    ) -> R {
        let s = Arc::clone(&self.session);
        (self.engine)
            .block_on(self.session.on_runtime(async move {
                let manager = s.manager()?;
                Ok::<_, EngineError>(f(s, manager).await)
            }))
            .unwrap()
    }

    /// The check's start at a load, as on a store that keeps block spends
    /// for the first time.
    fn start_unchecked(&self) {
        (self.session.hub.block_spends.ignore_mark)
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let id = self.id;
        self.run(move |s, m| {
            Box::pin(async move { s.start_block_spend_check(&m, id, true).await.unwrap() })
        });
    }

    fn pass(&self) -> CheckPass {
        let id = self.id;
        self.run(move |s, m| {
            Box::pin(async move { s.block_spend_check_pass(&m, id).await.unwrap() })
        })
    }

    fn with_chain(&self, chain: FakeChain) {
        *self.session.hub.block_spends.fake_chain.write().unwrap() = Some(Arc::new(chain));
    }

    /// The stored chain from `from` to TIP + 1, W in the block at `w_at`.
    fn chain(&self, w_at: Option<u32>, from: u32) -> FakeChain {
        let prevouts: HashMap<OutPoint, ScriptBuf> = (self.funded.output.iter().enumerate())
            .map(|(v, o)| {
                (
                    OutPoint::new(self.funded.txid(), v as u32),
                    o.script_pubkey.clone(),
                )
            })
            .collect();
        let mut map = BTreeMap::new();
        for h in from..=TIP + 1 {
            let mut txs = match h {
                FUNDED_AT => vec![self.funded.clone()],
                TIP => vec![foreign()],
                _ => vec![],
            };
            if w_at == Some(h) {
                txs.push(self.w.clone());
            }
            map.insert(h, stored_block(h, txs));
        }
        FakeChain::new(map, &prevouts)
    }

    fn checking(&self) -> bool {
        let filter = crate::CoinFilter {
            include_locked: true,
            ..Default::default()
        };
        (self.engine.block_on(self.session.utxos(self.id, filter)))
            .unwrap()
            .iter()
            .any(|c| c.outpoint == self.a && c.checking && !c.spendable)
    }

    fn listed(&self) -> bool {
        let filter = crate::CoinFilter {
            include_locked: true,
            ..Default::default()
        };
        (self.engine.block_on(self.session.utxos(self.id, filter)))
            .unwrap()
            .iter()
            .any(|c| c.outpoint == self.a)
    }

    fn locked_in_key_wallet(&self) -> Option<bool> {
        let manager = self.session.manager().unwrap();
        let (id, a) = (self.id, self.a);
        self.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let info = wm.get_wallet_info(&id.0).unwrap();
            (info.core_wallet.accounts.all_funding_accounts().into_iter())
                .find_map(|acc| acc.utxos.get(&a).map(|u| u.is_locked))
        })
    }

    fn held(&self) -> bool {
        coins(&self.engine, &self.session, self.id)
            .iter()
            .any(|(o, _)| *o == self.a)
    }

    fn other_coin_spendable(&self) -> bool {
        let other = OutPoint::new(self.funded.txid(), 1);
        spendable(&self.engine, &self.session, self.id, other)
    }

    fn reopen(mut self) -> Self {
        self.session = reopen(&self.engine, self.session);
        self
    }
}

/// The pre-fix state: P paid from A, was abandoned and gave A back; W, a
/// foreign spend of A, was then mined at TIP + 1 in a block the wallet
/// never fetched, and ChainLocked. A is unspent in the store and the
/// wallet.
fn given_back() -> Given {
    let (dir, engine, session, id) = engine_wallet();
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let funded = funding(7, &to, COIN);
    mined(&engine, &session, id, FUNDED_AT, vec![funded.clone()]);
    mined(&engine, &session, id, TIP, vec![foreign()]);
    let a = OutPoint::new(funded.txid(), 0);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    let session = reopen(&engine, session);
    abandon(&engine, &session, id, p.txid());
    let session = reopen(&engine, session);
    // The abandon's owed rescan passes the blocks the wallet fetched.
    mined(&engine, &session, id, FUNDED_AT, vec![funded.clone()]);
    mined(&engine, &session, id, TIP, vec![foreign()]);
    assert!(
        session.rescan_progress().unwrap().is_none(),
        "the abandon's rescan is done"
    );
    let w = super::spend_of(a, vec![(elsewhere(), COIN - 1_000)]);
    let fx = Given {
        _dir: dir,
        engine,
        session,
        id,
        a,
        funded,
        w,
    };
    assert!(fx.held(), "A is back, unspent");
    fx
}

/// The wallet's ChainLock reaches W's block.
fn lock_w(fx: &Given) {
    let hash = stored_block(TIP + 1, vec![fx.w.clone()]).block_hash();
    chain_lock(&fx.engine, &fx.session, fx.id, TIP + 1, hash);
}

/// (k) An abandon suspect a stored block spent: held, not offered while
/// the check runs, other coins spendable; the check finds W's block, stores
/// the row with A spent, and the hold lifts. A stays spent after a restart.
#[test]
fn a_coin_an_abandon_gave_back_is_found_spent_in_a_stored_block() {
    let fx = given_back();
    lock_w(&fx);
    fx.start_unchecked();
    assert!(fx.checking(), "A is held for the check, and listed");
    assert_eq!(
        fx.locked_in_key_wallet(),
        Some(true),
        "locked in key-wallet"
    );
    assert!(
        !spendable(&fx.engine, &fx.session, fx.id, fx.a),
        "A is not offered"
    );
    assert!(fx.other_coin_spendable(), "other coins stay spendable");
    fx.with_chain(fx.chain(Some(TIP + 1), FUNDED_AT));
    assert_eq!(fx.pass(), CheckPass::Done);
    assert!(!fx.held(), "the found spend takes A out");
    assert!(!fx.listed(), "A is not listed");
    let rows = fx.session.hub.block_spends.rows(&fx.id);
    assert_eq!(rows.get(&fx.a).map(|r| r.spender_txid), Some(fx.w.txid()));
    let fx = fx.reopen();
    assert!(!fx.held(), "A stays spent after a restart");
    assert!(!spendable(&fx.engine, &fx.session, fx.id, fx.a));
    assert!(fx.session.hub.block_spends.checking(&fx.id).is_empty());
}

/// (k) No stored block spent the suspect: once the search covers the
/// range to the tip, the hold lifts and A is offered again.
#[test]
fn a_suspect_no_stored_block_spent_is_released_at_the_tip() {
    let fx = given_back();
    fx.start_unchecked();
    assert!(fx.checking());
    fx.with_chain(fx.chain(None, FUNDED_AT));
    assert_eq!(fx.pass(), CheckPass::Done);
    assert!(fx.listed() && !fx.checking(), "A is listed, not held");
    assert_eq!(
        fx.locked_in_key_wallet(),
        Some(false),
        "the hold lifts its lock"
    );
    assert!(spendable(&fx.engine, &fx.session, fx.id, fx.a));
}

/// (k) A spender above the wallet's ChainLock keeps the hold until the
/// lock covers it: only a final block re-spends a coin.
#[test]
fn a_spender_above_the_chain_lock_keeps_the_hold() {
    let fx = given_back();
    fx.start_unchecked();
    fx.with_chain(fx.chain(Some(TIP + 1), FUNDED_AT));
    assert_eq!(fx.pass(), CheckPass::Waiting);
    assert!(fx.checking(), "still held");
    assert!(!fx.session.hub.block_spends.has_row(&fx.id, &fx.a));
    lock_w(&fx);
    assert_eq!(fx.pass(), CheckPass::Done);
    assert!(!fx.held());
}

/// (k) Offline (no SPV store), the check waits and A stays held and
/// visible; a restart resumes it, locked again before any build, and it
/// completes once the store is there. Other coins stay spendable.
#[test]
fn the_check_survives_a_restart_and_stays_held_offline() {
    let fx = given_back();
    lock_w(&fx);
    fx.start_unchecked();
    assert_eq!(fx.pass(), CheckPass::Waiting, "no SPV store offline");
    assert!(fx.checking(), "held and visible offline");
    let fx = fx.reopen();
    assert_eq!(
        fx.locked_in_key_wallet(),
        Some(true),
        "locked again by the load, before any build"
    );
    assert!(fx.checking(), "the check resumes after a restart");
    assert!(!spendable(&fx.engine, &fx.session, fx.id, fx.a));
    assert!(fx.other_coin_spendable());
    fx.with_chain(fx.chain(Some(TIP + 1), FUNDED_AT));
    fx.run(|s, _m| Box::pin(async move { s.block_spend_checks_once().await.unwrap() }));
    assert!(!fx.held(), "the session's periodic pass completes it");
    assert!(fx.session.hub.block_spends.checking(&fx.id).is_empty());
}

/// (k) The store's filters start above the suspect's funding block: no
/// proof, so the owed rescan runs from it and sends are refused
/// (`RescanInProgress`) while it does.
#[test]
fn a_gap_in_the_stored_filters_falls_back_to_the_owed_rescan() {
    let fx = given_back();
    fx.start_unchecked();
    fx.with_chain(fx.chain(Some(TIP + 1), TIP));
    assert_eq!(fx.pass(), CheckPass::Rescanning);
    let other = OutPoint::new(fx.funded.txid(), 1);
    let (_draft, prepared) = prepare(
        &fx.engine,
        &fx.session,
        fx.id,
        CoinSource::Outpoints(vec![other]),
        COIN / 2,
        false,
        &Arc::new(Accept::default()),
    );
    assert!(
        matches!(prepared, Err(EngineError::RescanInProgress)),
        "{:?}",
        prepared.map(|_| ())
    );
    assert!(fx.checking(), "held while the rescan runs");
    assert_eq!(fx.pass(), CheckPass::Rescanning);
}

/// (k) A suspect spent only by an unconfirmed transaction is held too; the
/// check then finds W's block.
#[test]
fn a_suspect_an_unconfirmed_payment_spends_again_is_checked() {
    let fx = given_back();
    lock_w(&fx);
    let p2 = send(
        &fx.engine,
        &fx.session,
        fx.id,
        CoinSource::Outpoints(vec![fx.a]),
        COIN / 3,
        false,
    );
    mempool(&fx.engine, &fx.session, fx.id, &p2);
    assert!(!fx.held(), "P2 spends A");
    fx.start_unchecked();
    assert!(
        fx.session.hub.block_spends.checking(&fx.id).contains(&fx.a),
        "A is a suspect"
    );
    fx.with_chain(fx.chain(Some(TIP + 1), FUNDED_AT));
    assert_eq!(fx.pass(), CheckPass::Done);
    assert!(fx.session.hub.block_spends.has_row(&fx.id, &fx.a));
}

/// C7: a held coin stays locked through a fenced build's sweep of stale
/// locks (the asset-lock funding path that excludes denominations), is
/// never free to a library funding, and CoinJoin never takes it.
#[test]
fn a_held_coin_stays_out_of_fenced_builds_and_coinjoin() {
    let fx = given_back();
    fx.start_unchecked();
    let id = fx.id;
    let (free, mixable) = fx.run(move |s, _m| {
        Box::pin(async move {
            let fence = s.coin_fence(id).await.unwrap();
            let snapshot = s.fenced_coins(&fence).await.unwrap();
            s.exclude_denominations_under_fence(&fence, &snapshot)
                .await
                .unwrap();
            let free = snapshot.free_outpoints();
            drop(fence);
            let view = s.read_mix_view(id).await.unwrap();
            let mixable: BTreeSet<OutPoint> = (view.coins.iter())
                .filter(|c| !c.coin.checking && c.coin.utxo.is_spendable(view.height))
                .map(|c| c.coin.utxo.outpoint)
                .collect();
            (free, mixable)
        })
    });
    assert!(!free.contains(&fx.a), "a library funding is never handed A");
    assert!(!mixable.contains(&fx.a), "CoinJoin never takes A");
    assert_eq!(
        fx.locked_in_key_wallet(),
        Some(true),
        "the fence's sweep of stale locks passes over the hold"
    );
    assert!(!spendable(&fx.engine, &fx.session, fx.id, fx.a));
}

/// (k) W in the very block that funded A: the search watches A from its
/// funding block, whose filter holds A's script through its funding
/// output, and finds W there.
#[test]
fn a_spender_in_the_coins_funding_block_is_found() {
    let fx = given_back();
    lock_w(&fx);
    fx.start_unchecked();
    fx.with_chain(fx.chain(Some(FUNDED_AT), FUNDED_AT));
    assert_eq!(fx.pass(), CheckPass::Done);
    let rows = fx.session.hub.block_spends.rows(&fx.id);
    assert_eq!(rows.get(&fx.a).map(|r| r.height), Some(FUNDED_AT));
    assert!(!fx.held());
}
