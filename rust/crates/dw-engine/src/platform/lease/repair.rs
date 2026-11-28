//! Repair's "Resolve stuck funding" (E0-04 §6.5, §2a.5, §16.10 C9; the
//! renderer's label still reads "send all coins to yourself"): Mode B's way
//! out of a funding that stays `MaybeSent` (or `WillBeSent` with no proof)
//! with no time bound, such as a lock whose row a power loss lost and that
//! a peer withholds.
//!
//! The self-spend is an engine `TxDraft` that spends coins of the funding
//! accounts (BIP44, BIP32, DashPay receiving: the accounts the library's
//! asset-lock builds fund from), "Which coins" below, among them an input
//! of every tracked `Built` and `Broadcast` asset lock of the step, to one
//! fresh receive address of the wallet, under the caller's `Spend` grant or
//! lease as any `TxDraft`. A lost lock spent some of those coins, so once
//! the self-spend is ChainLocked the lock can no longer confirm, and the
//! step's status derives `NotSent` ([`super::mode_b::derive_funding`]'s
//! first row), which emits `DispatchResolved(NotSent)`. Nothing here
//! declares `NotSent`: negative evidence (unspent inputs, a transaction
//! the wallet never saw) proves nothing (§6.5).
//!
//! **Order.** The draft is prepared (signed, its inputs reserved), then
//! its txid is recorded against the step's standing marker in the
//! journal's append-only `self_spend` table, with the input the signed
//! transaction spends of each lock a call of the step signed (its
//! provenance: without it the ChainLocked self-spend shows nothing, review
//! P2b r3 R3-01)
//! ([`LeaseTable::record_self_spend`](super::LeaseTable::record_self_spend)),
//! and only then broadcast. A record that fails releases the draft unsent.
//! After a restart the row loads with its marker and the wallet's ChainLock
//! is read again, so the step derives the same status; no `NotSent` row is
//! written, and the step's marker keeps standing (a new funding needs a
//! new draft, as for any resolved marker).
//!
//! **Which coins.** Coins of the funding accounts that consensus lets the
//! self-spend spend (mature, not frozen by key-wallet), picked by the
//! step's own lock rows:
//! - While one of them spent an unmixed coin: every unmixed coin, and no
//!   counted denomination (the mixed coins a restored wallet holds there).
//!   No funding of this app selects one, and joining one to unmixed coins
//!   would link them. A lock that also spent a denomination is conflicted
//!   through its unmixed input.
//! - When every one of them spent only denominations: one denomination of
//!   each lock, none for a lock another's already conflicts, and no unmixed
//!   coin. One input conflicts with a lock, and the smallest denomination
//!   pays the fee; the self-spend links the denominations it spends (one per
//!   lock at most) and turns them into one ordinary coin.
//! - A step with locks of both kinds, which only an older build leaves:
//!   the live inputs of its locks.
//!   The one transaction links the mixed coins to the unmixed ones;
//!   refusing would hold them for good. A repair with only denominations
//!   left is refused.
//!
//! The inputs of
//! tracked `Built`/`Broadcast` locks are reserved for every other build
//! (`coins.rs`); this draft names exactly them as exempt. A coin the user
//! locked, or one a pending payment of the engine reserves, is not
//! exempt: the draft refuses it (`send.outpoint_unavailable`), picked or
//! not, as main's draft of every coin did, since the
//! library saw neither lock nor reservation, so a lost lock may have spent
//! it, and a self-spend without it might not conflict with that lock. The
//! user unlocks it (or waits for the payment) and repairs again.
//!
//! **A drain's step.** A shield step whose lock drained the CoinJoin
//! account (`shield_drain.rs`) spent no coin of the funding accounts: the
//! self-spend takes exactly the CoinJoin coins its locks' records name
//! that are still unspent, never pooled with other coins (that would link
//! them), and signs them as a CoinJoin payment is. It is refused while the
//! wallet mixes, and holds the wallet's claim on its CoinJoin account
//! (`CoinJoinRuntime::claim_sweep`) from the draft's prepare through its
//! broadcast, so no move of the account runs meanwhile; a coin a move's
//! record holds is refused as any reserved coin.
//!
//! **When.** A coin a wallet transaction spends is no coin of the
//! self-spend, so the self-spend beats a lost lock only if every such
//! transaction is settled. The repair is refused while a transaction of
//! the funding accounts that spends the wallet's coins is not ChainLocked
//! (an earlier self-spend or a payment): it waits for that ChainLock, or
//! the user abandons the transaction, whose coins come back to the next
//! repair. The step's own lock is not the user's to abandon: Resolve does
//! that itself (below). It
//! is refused unless it spends an input of every tracked `Built` and
//! `Broadcast` row of the step, so it conflicts with each. And it is
//! refused while an unattributed row of the step's funding type stands (a
//! lock re-added from the chain at an identity index that may not be its
//! own, `mode_b`'s "Unattributed rows"): that lock may be the step's, on
//! the chain whatever the self-spend does.
//!
//! **The lock the wallet recorded.** The step's own lock may stand in the
//! wallet as a payment that is not ChainLocked (dash-spv records what it
//! sends), and the "When" refusal would then wait for it forever if no
//! peer took it. While it is only in the mempool (one mined or
//! InstantSend-locked waits for its ChainLock as any other), Resolve
//! abandons it first, as an Activity Abandon would, for the step's own
//! locks only and nothing that spends them (Activity refuses it,
//! `PlatformFunding`; an abandon that would take anything else is refused
//! `SpendUnsettled`), and signs nothing: the stop of its rebroadcast is
//! asked (the pinned dash-spv has none, and keeps sending it until a
//! restart), and the coins it spent wait for the rescan every abandon runs
//! (`crate::rescan_hold`), so it is refused `LockAbandoned`. Once they are
//! back, Resolve again builds the self-spend from them. A resume of the
//! lock in between sends it again; the wallet keeps it abandoned, and the
//! next Resolve asks the stop again. The preview prices
//! nothing for such a step: it is refused `SpendUnsettled`.
//!
//! The facade call under Tools ▸ Repair is DP1-02's (m4 contract,
//! "Tools ▸ Repair ... gets a facade call; engine-internal until then").

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use dashcore::{OutPoint, Txid};

use super::ArtifactId;
use super::mode_b::StuckFundingRefusal;
use crate::coinjoin::CoinJoinFailure;
use crate::coins::UnsettledSpend;
use crate::platform::flows::is_funding_step;
use crate::send::{BroadcastOutcome, CoinSource, Recipient, TxDraft};
use crate::{EngineError, NetworkSession, WalletId};

impl From<StuckFundingRefusal> for EngineError {
    fn from(r: StuckFundingRefusal) -> Self {
        EngineError::StuckFunding(r)
    }
}

impl NetworkSession {
    /// Repair's "Resolve stuck funding" for the Mode B funding `step` of `wallet_id` (`registration/<draft>/funding`,
    /// `topup/<id>/funding` or `shield/<k>/funding`; module doc).
    /// `grant_id` is a `Spend` grant or a lease with a Spend budget, as for
    /// [`TxDraft::prepare`]: the self-spend's outflow is its fee.
    ///
    /// Refused (`stuck_funding`, the [`StuckFundingRefusal`] itself) unless
    /// the step reads `MaybeSent` or `WillBeSent` now with its marker
    /// standing and no call running under it, while a spend of the wallet's
    /// coins awaits its ChainLock or an unattributed row may be the step's
    /// lock, when the self-spend would miss every input of one of the
    /// step's locks (module doc, "When"), and when the journal cannot
    /// record the self-spend ([`StuckFundingRefusal::retry_later`] tells
    /// the passing refusals from the lasting ones). A drain's step is also
    /// refused `coinjoin.mixing_active` while the wallet mixes and
    /// `coinjoin.move_running` while a move of its CoinJoin coins runs.
    /// The draft's own errors are `TxDraft`'s. On success the self-spend is
    /// recorded and broadcast; the step keeps reading `MaybeSent` (or
    /// `WillBeSent`) until the wallet sees the self-spend ChainLocked.
    pub async fn resolve_stuck_funding(
        self: &Arc<Self>,
        wallet_id: WalletId,
        step: String,
        grant_id: String,
    ) -> Result<BroadcastOutcome, EngineError> {
        let draft = self.stuck_funding_draft_as(wallet_id, &step, true).await?;
        self.send_self_spend(&draft, &step, grant_id).await
    }

    /// The self-spend's draft (module doc): the funding accounts' coins
    /// [`funding_repair_coins`] picks to a fresh receive address, the fee
    /// subtracted, with the tracked locks' inputs exempt. Abandons nothing:
    /// a step whose own lock the wallet recorded unsettled is refused
    /// `SpendUnsettled` here.
    pub(crate) async fn stuck_funding_draft(
        self: &Arc<Self>,
        wallet_id: WalletId,
        step: &str,
    ) -> Result<Arc<TxDraft>, EngineError> {
        self.stuck_funding_draft_as(wallet_id, step, false).await
    }

    /// [`Self::stuck_funding_draft`]; `resolve`: for Resolve itself, which
    /// abandons the step's own locks the wallet recorded unsettled (module
    /// doc, "The lock the wallet recorded") and is refused `LockAbandoned`.
    pub(crate) async fn stuck_funding_draft_as(
        self: &Arc<Self>,
        wallet_id: WalletId,
        step: &str,
        resolve: bool,
    ) -> Result<Arc<TxDraft>, EngineError> {
        if !is_funding_step(step) {
            return Err(EngineError::InvalidArgument(format!(
                "{step} is not a funding step"
            )));
        }
        let this = Arc::clone(self);
        let step = step.to_owned();
        self.on_runtime(async move {
            let _op = this.enter().await?;
            // Resolve abandons and sends: behind the restore's money gate,
            // as an Activity Abandon and a send are.
            if resolve {
                this.refuse_while_restored()?;
            }
            // Not while a wallet loads, as an Activity Abandon.
            let _lifecycle = match resolve {
                true => Some(this.wallet_lifecycle.hold().await),
                false => None,
            };
            if !this.leases.refresh_funding(wallet_id).await {
                return Err(StuckFundingRefusal::RowsUnknown.into());
            }
            let rows = this.leases.check_stuck_funding(wallet_id, &step)?;
            let wallet = this.wallet(&wallet_id).await?;
            // The coins are read under the fence; the draft's prepare reads
            // them again under its own, so a coin locked in between is
            // refused there (`send.outpoint_unavailable`).
            let fence = this.coin_fence(wallet_id).await?;
            let unsettled = crate::coins::unsettled_spends(
                &crate::wallet_read::info_of(&wallet).await?.core_wallet,
            );
            let own: BTreeSet<Txid> = rows.iter().map(|(t, _)| *t).collect();
            if let Some((&txid, &state)) = unsettled.iter().find(|(t, state)| {
                !resolve || **state != UnsettledSpend::Mempool || !own.contains(*t)
            }) {
                let reason = if !resolve && state == UnsettledSpend::Mined && own.contains(&txid) {
                    StuckFundingRefusal::OwnLockUnfinalized(txid)
                } else {
                    StuckFundingRefusal::SpendUnsettled(txid)
                };
                return Err(reason.into());
            }
            let locks: BTreeSet<Txid> = unsettled.into_keys().collect();
            if let Some(lock) = locks.first().copied() {
                // Refused under the write lock (it settled meanwhile, or
                // something else of the wallet spends it): wait as for any
                // unsettled spend.
                match this.abandon_own_locks(wallet_id, &locks, &own).await {
                    Err(EngineError::TxActionRefused(_)) => {
                        return Err(StuckFundingRefusal::SpendUnsettled(lock).into());
                    }
                    other => other?,
                }
                return Err(StuckFundingRefusal::LockAbandoned(lock).into());
            }
            // An own lock abandoned before: its coins wait for the rescan.
            if resolve {
                this.restop_abandoned_locks(wallet_id, &own).await;
            }
            let held = this.hub.rescan_held(&wallet_id);
            if let Some((txid, _)) = (rows.iter()).find(|(_, i)| i.iter().any(|o| held.contains(o)))
            {
                return Err(StuckFundingRefusal::LockAbandoned(*txid).into());
            }
            let snapshot = this.coin_snapshot(&fence, &wallet).await?;
            drop(fence);
            // A drain's lock spent CoinJoin coins and none of the funding
            // accounts' (module doc); any other lock conflicts through a
            // funding account's coin.
            let recorded: HashSet<OutPoint> = (rows.iter())
                .flat_map(|(_, inputs)| inputs.iter().copied())
                .collect();
            let spends = |of: fn(&crate::coins::WalletCoin) -> bool| {
                (snapshot.coins.iter()).any(|c| of(c) && recorded.contains(&c.utxo.outpoint))
            };
            let coinjoin = spends(|c| c.coinjoin_account) && !spends(|c| c.send_account);
            if coinjoin && this.mixing(wallet_id) {
                return Err(CoinJoinFailure::MixingActive.into());
            }
            // A coin another step's lock spends stays out: the self-spend
            // would conflict with that funding too.
            let spendable = (snapshot.coins.iter()).filter(|c| {
                c.utxo.is_spendable(snapshot.height)
                    && (!c.lock_input || recorded.contains(&c.utxo.outpoint))
            });
            let coins: Vec<_> = if coinjoin {
                spendable
                    .filter(|c| c.coinjoin_account && recorded.contains(&c.utxo.outpoint))
                    .collect()
            } else {
                let mut coins =
                    funding_repair_coins(spendable.filter(|c| c.send_account).collect(), &rows)?;
                // A coin that stopped main's all-coin draft (locked, or
                // reserved other than as a lock's input) is in this one,
                // so its prepare refuses it as main's did.
                let stops: Vec<_> = (snapshot.coins.iter())
                    .filter(|c| {
                        c.send_account
                            && c.utxo.is_spendable(snapshot.height)
                            && (c.user_locked || (c.reserved && !c.lock_input))
                            && !coins.iter().any(|p| p.utxo.outpoint == c.utxo.outpoint)
                    })
                    .collect();
                coins.extend(stops);
                coins
            };
            if coins.is_empty() {
                return Err(EngineError::InvalidArgument(
                    "the wallet holds no coin to spend to itself".into(),
                ));
            }
            let exempt: HashSet<OutPoint> = coins
                .iter()
                .filter(|c| c.lock_input && recorded.contains(&c.utxo.outpoint))
                .map(|c| c.utxo.outpoint)
                .collect();
            let outpoints: HashSet<OutPoint> = coins.iter().map(|c| c.utxo.outpoint).collect();
            if let Some((txid, _)) = rows
                .iter()
                .find(|(_, inputs)| !inputs.iter().any(|o| outpoints.contains(o)))
            {
                return Err(StuckFundingRefusal::LockInputsSpent(*txid).into());
            }
            let total: u64 = coins.iter().map(|c| c.utxo.value()).sum();
            let outpoints = outpoints.into_iter().collect();
            let address = wallet.core().next_receive_address_for_account(0).await?;
            let draft = this.new_draft(wallet_id, exempt, coinjoin)?;
            draft.set_source(CoinSource::Outpoints(outpoints))?;
            draft.set_recipients(vec![Recipient {
                address: address.to_string(),
                amount: total,
                subtract_fee_from_amount: true,
                label: None,
                message: None,
            }])?;
            Ok(draft)
        })
        .await
    }

    /// Abandons `locks`, the stuck step's own locks the wallet recorded as
    /// unsettled spends, as an Activity Abandon would but past its
    /// `PlatformFunding` refusal for the step's locks `own` alone: their
    /// rebroadcast stops, and the coins they spent wait for the rescan
    /// (`crate::rescan_hold`). Nothing is signed.
    async fn abandon_own_locks(
        &self,
        wallet_id: WalletId,
        locks: &BTreeSet<Txid>,
        own: &BTreeSet<Txid>,
    ) -> Result<(), EngineError> {
        let manager = self.manager()?;
        if !manager.spv().is_started() {
            return Err(EngineError::SpvNotRunning);
        }
        let _batch = self.hub.abandon_batch(wallet_id);
        let mut done = BTreeSet::new();
        for lock in locks {
            if !done.contains(lock) {
                done.extend((self.abandon_inner_funding(&manager, wallet_id, *lock, own)).await?);
            }
        }
        Ok(())
    }

    /// Retires any remaining retries of the step's own abandoned locks,
    /// including those an earlier attempt may still have queued.
    async fn restop_abandoned_locks(&self, wallet_id: WalletId, own: &BTreeSet<Txid>) {
        let abandoned: Vec<Txid> = (own.iter())
            .filter(|t| (self.hub.history.get(&wallet_id, t)).is_some_and(|e| e.is_abandoned()))
            .copied()
            .collect();
        self.stop_rebroadcasts(wallet_id, abandoned.into_iter())
            .await;
    }

    /// Prepares `draft`, records its txid against `step`'s marker, then
    /// broadcasts it (module doc, "Order").
    pub(crate) async fn send_self_spend(
        self: &Arc<Self>,
        draft: &Arc<TxDraft>,
        step: &str,
        grant_id: String,
    ) -> Result<BroadcastOutcome, EngineError> {
        // A drain's self-spend: no mixing starts and no move of the CoinJoin
        // account runs until it is out (module doc).
        let _claim = (draft.spends_coinjoin())
            .then(|| self.coinjoin.claim_sweep(draft.wallet_id()))
            .transpose()?;
        let prepared = draft.prepare(grant_id).await?;
        // The inputs of the transaction that will be broadcast: its record
        // names the one it spends of each signed lock (review P2b r3).
        let signed = prepared
            .raw()
            .and_then(|raw| dashcore::consensus::deserialize::<dashcore::Transaction>(&raw).ok());
        let Some(signed) = signed.filter(|tx| {
            ArtifactId::parse(&prepared.summary().txid) == Some(ArtifactId::from_txid(&tx.txid()))
        }) else {
            let _ = draft.abandon(prepared).await;
            return Err(EngineError::Internal(
                "the self-spend's transaction does not parse".into(),
            ));
        };
        let txid = ArtifactId::from_txid(&signed.txid());
        let inputs: Vec<OutPoint> = signed.input.iter().map(|i| i.previous_output).collect();
        let this = Arc::clone(self);
        let (wallet_id, owned) = (draft.wallet_id(), step.to_owned());
        let recorded = self
            .on_runtime(async move {
                let _op = this.enter().await?;
                Ok(this
                    .leases
                    .record_self_spend(wallet_id, &owned, txid.0, &inputs)
                    .await?)
            })
            .await;
        if let Err(e) = recorded {
            // The record's error is the answer; a failed release only
            // leaves the inputs reserved until the draft is dropped.
            if let Err(abandon) = draft.abandon(prepared).await {
                tracing::warn!(step, error = %abandon, "releasing an unrecorded self-spend failed");
            }
            return Err(e);
        }
        draft.broadcast(prepared).await
    }
}

/// The funding accounts' coins (`send`, spendable) a self-spend takes for
/// the step's locks `rows` (module doc, "Which coins"): every coin but the
/// counted denominations while a lock spends an unmixed coin, else one
/// denomination of each lock not already conflicted, or every live input
/// of a step whose locks are of both kinds. A lock none of whose inputs is
/// left is refused.
fn funding_repair_coins<'a>(
    send: Vec<&'a crate::coins::WalletCoin>,
    rows: &[(dashcore::Txid, Vec<OutPoint>)],
) -> Result<Vec<&'a crate::coins::WalletCoin>, EngineError> {
    let coin = |o: &OutPoint| send.iter().find(|c| c.utxo.outpoint == *o);
    let mut unmixed_lock = false;
    let mut denominations = HashSet::new();
    for (txid, inputs) in rows {
        let live: Vec<_> = inputs.iter().filter_map(coin).collect();
        if live.is_empty() {
            return Err(StuckFundingRefusal::LockInputsSpent(*txid).into());
        }
        if live.iter().any(|c| !c.counted_denomination) {
            unmixed_lock = true;
        } else if !live
            .iter()
            .any(|c| denominations.contains(&c.utxo.outpoint))
        {
            denominations.insert(live[0].utxo.outpoint);
        }
    }
    if denominations.is_empty() {
        let unmixed: Vec<_> = (send.iter().copied())
            .filter(|c| !c.counted_denomination)
            .collect();
        if unmixed.is_empty() && !send.is_empty() {
            return Err(EngineError::InvalidArgument(
                "the wallet holds only mixed coins here, and the repair does not spend them".into(),
            ));
        }
        return Ok(unmixed);
    }
    // Locks of both kinds (an older build's): the step's locks' live
    // inputs, linking the mixed coins to the others.
    if unmixed_lock {
        let inputs: HashSet<&OutPoint> = rows.iter().flat_map(|(_, i)| i).collect();
        return Ok((send.into_iter())
            .filter(|c| inputs.contains(&c.utxo.outpoint) && !c.user_locked)
            .collect());
    }
    Ok(send
        .into_iter()
        .filter(|c| denominations.contains(&c.utxo.outpoint))
        .collect())
}

#[cfg(test)]
mod tests {
    use dashcore::{Address, OutPoint, TxOut, Txid};

    use super::funding_repair_coins;
    use crate::EngineError;
    use crate::coins::WalletCoin;
    use crate::platform::lease::mode_b::StuckFundingRefusal;

    fn coin(seed: u8, denomination: bool) -> WalletCoin {
        let address = Address::new(
            dashcore::Network::Regtest,
            dashcore::address::Payload::PubkeyHash(dashcore::PubkeyHash::from_byte_array(
                [seed; 20],
            )),
        );
        let txout = TxOut {
            value: if denomination {
                100_001_000
            } else {
                50_000_000
            },
            script_pubkey: address.script_pubkey(),
        };
        let outpoint = OutPoint::new(Txid::from_byte_array([seed; 32]), 0);
        WalletCoin {
            utxo: key_wallet::Utxo::new(outpoint, txout, address, 1, false),
            send_account: true,
            coinjoin_account: false,
            counted_denomination: denomination,
            is_change: false,
            block_time: None,
            chain_locked: false,
            foreign_incoming: false,
            user_locked: false,
            dust_locked: false,
            reserved: false,
            mixing_held: false,
            lock_input: false,
            unresolved_lock: false,
            awaiting_history: false,
            excluded: None,
        }
    }

    fn op(seed: u8) -> OutPoint {
        OutPoint::new(Txid::from_byte_array([seed; 32]), 0)
    }

    fn row(seed: u8, inputs: &[u8]) -> (Txid, Vec<OutPoint>) {
        (
            Txid::from_byte_array([seed + 100; 32]),
            inputs.iter().map(|s| op(*s)).collect(),
        )
    }

    fn picked(
        coins: &[WalletCoin],
        rows: &[(Txid, Vec<OutPoint>)],
    ) -> Result<Vec<u8>, EngineError> {
        let picked = funding_repair_coins(coins.iter().collect(), rows)?;
        Ok(picked.iter().map(|c| c.utxo.outpoint.txid[0]).collect())
    }

    /// A, B unmixed; D, E counted denominations.
    fn wallet() -> Vec<WalletCoin> {
        vec![coin(1, false), coin(2, false), coin(4, true), coin(5, true)]
    }

    #[test]
    fn a_lock_of_unmixed_coins_takes_every_unmixed_coin_and_no_denomination() {
        assert_eq!(picked(&wallet(), &[row(1, &[1])]).unwrap(), [1, 2]);
    }

    #[test]
    fn a_lock_that_spent_an_unmixed_coin_and_a_denomination_conflicts_through_the_unmixed_one() {
        assert_eq!(picked(&wallet(), &[row(1, &[1, 4])]).unwrap(), [1, 2]);
    }

    #[test]
    fn a_lock_of_denominations_alone_takes_those_denominations_alone() {
        assert_eq!(picked(&wallet(), &[row(1, &[4])]).unwrap(), [4]);
    }

    /// One denomination is enough to conflict with a lock and pay the fee:
    /// the other is not spent with it, which would link the two.
    #[test]
    fn a_lock_of_two_denominations_takes_one_of_them() {
        assert_eq!(picked(&wallet(), &[row(1, &[4, 5])]).unwrap(), [4]);
    }

    /// A lock already conflicted through a denomination another lock's
    /// repair spends adds none of its own.
    #[test]
    fn locks_sharing_a_denomination_take_it_alone() {
        assert_eq!(
            picked(&wallet(), &[row(1, &[4]), row(2, &[5, 4])]).unwrap(),
            [4]
        );
    }

    /// F-D: an older build's locks of both kinds are repaired through the
    /// live inputs of the step's locks, in either row order.
    #[test]
    fn locks_of_both_kinds_take_their_inputs() {
        for rows in [
            [row(1, &[1, 4]), row(2, &[4])],
            [row(2, &[4]), row(1, &[1, 4])],
            [row(1, &[1]), row(2, &[4])],
            [row(2, &[4]), row(1, &[1])],
        ] {
            assert_eq!(picked(&wallet(), &rows).unwrap(), [1, 4], "{rows:?}");
        }
    }

    /// F-D: the fallback takes no spent and no user-locked input.
    #[test]
    fn locks_of_both_kinds_take_no_spent_or_user_locked_input() {
        let mut coins = wallet();
        coins[0].user_locked = true;
        let rows = [row(1, &[1, 2, 9]), row(2, &[4])];
        assert_eq!(picked(&coins, &rows).unwrap(), [2, 4]);
    }

    #[test]
    fn a_lock_with_no_input_left_is_refused() {
        let refused = picked(&wallet(), &[row(1, &[9])]);
        assert!(
            matches!(
                refused,
                Err(EngineError::StuckFunding(
                    StuckFundingRefusal::LockInputsSpent(_)
                ))
            ),
            "{refused:?}"
        );
    }

    #[test]
    fn only_denominations_left_says_so() {
        let refused = picked(&[coin(4, true)], &[]);
        assert!(
            matches!(&refused, Err(EngineError::InvalidArgument(m)) if m.contains("only mixed coins")),
            "{refused:?}"
        );
    }
}
