//! Repair's "Resolve stuck funding: send all coins to yourself" (E0-04
//! §6.5, §2a.5, §16.10 C9): Mode B's way out of a funding that stays
//! `MaybeSent` (or `WillBeSent` with no proof) with no time bound, such as
//! a lock whose row a power loss lost and that a peer withholds.
//!
//! The self-spend is an engine `TxDraft` that spends every coin of the
//! funding accounts (BIP44, BIP32, DashPay receiving: the accounts the
//! library's asset-lock builds fund from), the inputs of every tracked
//! `Built` and `Broadcast` asset lock of the wallet among them, to one
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
//! journal's append-only `self_spend` table
//! ([`LeaseTable::record_self_spend`](super::LeaseTable::record_self_spend)),
//! and only then broadcast. A record that fails releases the draft unsent.
//! After a restart the row loads with its marker and the wallet's ChainLock
//! is read again, so the step derives the same status; no `NotSent` row is
//! written, and the step's marker keeps standing (a new funding needs a
//! new draft, as for any resolved marker).
//!
//! **Which coins.** Every coin of the funding accounts that consensus lets
//! the self-spend spend (mature, not frozen by key-wallet). The inputs of
//! tracked `Built`/`Broadcast` locks are reserved for every other build
//! (`coins.rs`); this draft names exactly them as exempt. A coin the user
//! locked, or one a pending payment of the engine reserves, is not
//! exempt: the draft refuses it (`send.outpoint_unavailable`), since the
//! library saw neither lock nor reservation, so a lost lock may have spent
//! it, and a self-spend without it might not conflict with that lock. The
//! user unlocks it (or waits for the payment) and repairs again.
//!
//! **When.** A coin a wallet transaction spends is no coin of the
//! self-spend, so the self-spend beats a lost lock only if every such
//! transaction is settled. The repair is refused while a transaction of
//! the funding accounts that spends the wallet's coins is not ChainLocked
//! (an earlier self-spend, a payment, or the lost lock itself, which the
//! wallet may have seen): it waits for that ChainLock, or the user
//! abandons the transaction, whose coins come back to the next repair. And
//! it is refused unless it spends an input of every tracked `Built` and
//! `Broadcast` row of the step, so it conflicts with each.
//!
//! The facade call under Tools ▸ Repair is DP1-02's (m4 contract,
//! "Tools ▸ Repair ... gets a facade call; engine-internal until then").

use std::collections::HashSet;
use std::sync::Arc;

use dashcore::OutPoint;

use super::ArtifactId;
use super::mode_b::StuckFundingRefusal;
use crate::platform::flows::is_funding_step;
use crate::send::{BroadcastOutcome, CoinSource, Recipient, TxDraft};
use crate::{EngineError, NetworkSession, WalletId};

impl From<StuckFundingRefusal> for EngineError {
    fn from(r: StuckFundingRefusal) -> Self {
        match r {
            StuckFundingRefusal::JournalUnavailable => EngineError::Storage(r.to_string()),
            _ => EngineError::InvalidArgument(r.to_string()),
        }
    }
}

impl NetworkSession {
    /// Repair's "Resolve stuck funding: send all coins to yourself" for
    /// the Mode B funding `step` of `wallet_id` (`registration/<draft>/funding`
    /// or `topup/<id>/funding`; module doc). `grant_id` is a `Spend` grant
    /// or a lease with a Spend budget, as for [`TxDraft::prepare`]: the
    /// self-spend's outflow is its fee.
    ///
    /// Refused (`invalid_argument`) unless the step reads `MaybeSent` or
    /// `WillBeSent` now with its marker standing and no call running under
    /// it, while a spend of the wallet's coins awaits its ChainLock, and
    /// when the self-spend would miss every input of one of the step's
    /// tracked rows (module doc, "When"); `storage` when the journal cannot
    /// record the self-spend. The
    /// draft's own errors are `TxDraft`'s. On success the self-spend is
    /// recorded and broadcast; the step keeps reading `MaybeSent` (or
    /// `WillBeSent`) until the wallet sees the self-spend ChainLocked.
    pub async fn resolve_stuck_funding(
        self: &Arc<Self>,
        wallet_id: WalletId,
        step: String,
        grant_id: String,
    ) -> Result<BroadcastOutcome, EngineError> {
        let draft = self.stuck_funding_draft(wallet_id, &step).await?;
        self.send_self_spend(&draft, &step, grant_id).await
    }

    /// The self-spend's draft (module doc): every spendable coin of the
    /// funding accounts to a fresh receive address, the fee subtracted,
    /// with the tracked locks' inputs exempt.
    pub(crate) async fn stuck_funding_draft(
        self: &Arc<Self>,
        wallet_id: WalletId,
        step: &str,
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
            if !this.leases.refresh_funding(wallet_id).await {
                return Err(StuckFundingRefusal::RowsUnknown.into());
            }
            let rows = this.leases.check_stuck_funding(wallet_id, &step)?;
            let wallet = this.wallet(&wallet_id).await?;
            if let Some(txid) = crate::coins::unsettled_spend(&wallet.state().await.core_wallet) {
                return Err(StuckFundingRefusal::SpendUnsettled(txid).into());
            }
            let snapshot = this.coin_snapshot(&wallet, wallet_id).await?;
            let coins: Vec<_> = snapshot
                .coins
                .iter()
                .filter(|c| c.send_account && c.utxo.is_spendable(snapshot.height))
                .collect();
            if coins.is_empty() {
                return Err(EngineError::InvalidArgument(
                    "the wallet holds no coin to spend to itself".into(),
                ));
            }
            let exempt: HashSet<OutPoint> = coins
                .iter()
                .filter(|c| c.lock_input)
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
            let draft = this.new_draft(wallet_id, exempt)?;
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

    /// Prepares `draft`, records its txid against `step`'s marker, then
    /// broadcasts it (module doc, "Order").
    pub(crate) async fn send_self_spend(
        self: &Arc<Self>,
        draft: &Arc<TxDraft>,
        step: &str,
        grant_id: String,
    ) -> Result<BroadcastOutcome, EngineError> {
        let prepared = draft.prepare(grant_id).await?;
        let Some(txid) = ArtifactId::parse(&prepared.summary().txid) else {
            let _ = draft.abandon(prepared).await;
            return Err(EngineError::Internal(
                "the self-spend's txid does not parse".into(),
            ));
        };
        let this = Arc::clone(self);
        let (wallet_id, owned) = (draft.wallet_id(), step.to_owned());
        let recorded = self
            .on_runtime(async move {
                let _op = this.enter().await?;
                Ok(this
                    .leases
                    .record_self_spend(wallet_id, &owned, txid.0)
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
