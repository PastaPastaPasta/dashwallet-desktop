//! A DashPay contact paid from the Platform balance (C-funding D-C5, C5b):
//! one address credit withdrawal from the payment account to the contact's
//! next DIP-15 Core address, as a `ToContact` move (`platform_moves.rs`).
//!
//! **Order** (P2). The arguments, the live payee, the pair's lock and rule 5
//! on the payee; the wallet's coin fence (so no Core payment reserves
//! meanwhile); then the move's own gates under the move lock (the proved
//! read under rule 5, `credits.move_pending`, the withdrawal range), the
//! grant's cap and the lease. Only then the leg ([`ContactLeg`]) reserves
//! the pair's address and re-checks rule 5 on it, claims the pair, and writes
//! the payout row durably, named by the move's id; the move record follows,
//! and only then is anything signed. A refusal before the reservation takes
//! no address; one before the lease keeps the grant.
//!
//! **Address** (P4, P5). The address comes only from the reservation inside
//! the same call: no host input names it, and "Move to Dash wallet" keeps
//! its own-address rule. It goes back to the pair only when nothing was
//! signed. Otherwise it is consumed, also when the payout is later proved
//! not sent.
//!
//! **Mainnet** ([`FEATURE`]). The flow is off on mainnet until its funds
//! review: the quote and the payment refuse `platform.feature_off` there
//! before anything is read, redeemed or written.
//!
//! **Lock** (P6, P7, P8). The row is the pair's lock while `pending`. It
//! settles `sent` once the payout's withdrawal document is proved, `not_sent`
//! once it can never execute (refused for good, or its nonce taken with its
//! document proved absent at a height at or above the nonce proof, whether a
//! "Move to Dash wallet" at its inputs and nonces took it or another device
//! did). Nothing else clears it.

use std::sync::Arc;

use dashcore::Address;
use dpp::identity::core_script::CoreScript;
use dw_appdb::DpContactPayoutRow;
use platform_wallet::PlatformWallet;

use super::VaultContactCrypto;
use super::credits::WithdrawAmount;
use super::errors::{ContactError, CreditsError, PlatformError};
use super::flows::GrantRequest;
use super::payment_locks::{InFlight, Pair, PayCall, Payout};
use super::payments::{
    ContactPayOutcome, ContactPayQuote, Counterparty, LockResolution, PaymentLock, PaymentSource,
    PayoutState,
};
use super::platform_addresses::{PlatformActivity, PlatformActivityKind};
use super::platform_moves::{self, MoveNet, MoveRecord, SdkMoves, Target};
use super::registration_flow::b58;
use crate::send::SendFailure;
use crate::{EngineError, NetworkSession, WalletId};

/// A refusal of the address step, as `contact_pay` answers it.
fn refusal(e: EngineError) -> ContactError {
    match e {
        EngineError::Contact(c) => c,
        EngineError::Send(SendFailure::VaultLocked) => PlatformError::SignerUnavailable.into(),
        EngineError::Send(SendFailure::PreparedTxSpent) => PlatformError::Unavailable.into(),
        e => PlatformError::from(e).into(),
    }
}

/// The stand-in `run` returns for a refusal the leg holds in full.
fn held_refusal() -> CreditsError {
    PlatformError::Internal {
        detail: "the contact leg refused".into(),
    }
    .into()
}

/// The payout's side of a `ToContact` move (module doc, Order): its
/// address, its claim on the pair and its row.
pub(crate) struct ContactLeg {
    session: Arc<NetworkSession>,
    wallet_id: WalletId,
    wallet: Arc<PlatformWallet>,
    pair: Pair,
    credits: u64,
    note: Option<String>,
    crypto: Option<VaultContactCrypto>,
    address: Option<Address>,
    in_flight: Option<InFlight>,
    since: u64,
    /// The leg's own refusal, in full.
    refused: Option<ContactError>,
    /// Where the payout stands when the call returns.
    state: PayoutState,
}

impl ContactLeg {
    /// After the lease, before the move record: reserves the pair's address
    /// and re-checks rule 5 on it, claims the pair for `record`, and writes
    /// its payout row durably (P2, P6). The address's script, which `record`
    /// now names.
    pub(crate) async fn claim(
        &mut self,
        record: &mut MoveRecord,
    ) -> Result<CoreScript, CreditsError> {
        match self.claim_inner(record).await {
            Ok(script) => Ok(script),
            Err(e) => {
                self.refused = Some(e);
                Err(held_refusal())
            }
        }
    }

    async fn claim_inner(&mut self, record: &mut MoveRecord) -> Result<CoreScript, ContactError> {
        let (session, id, pair) = (Arc::clone(&self.session), self.wallet_id, self.pair);
        let crypto = self.crypto.take();
        let address = (session
            .contact_address(id, &self.wallet, pair, crypto)
            .await)
            .map_err(refusal)?;
        let in_flight = match session.claim_payment(id, pair, &record.id, &address) {
            Ok(in_flight) => in_flight,
            Err(super::payment_locks::ClaimRefused::Locked(e)) => return Err(e),
            Err(super::payment_locks::ClaimRefused::Spent) => {
                return Err(PlatformError::Unavailable.into());
            }
        };
        self.in_flight = Some(in_flight);
        // The withdrawal's document id, so a restored wallet can find it
        // executed (P10's restore).
        let document = {
            let mut paid = record.clone();
            paid.to = address.to_string();
            platform_moves::payout_document(&paid, session.vault.network())
        };
        let row = DpContactPayoutRow {
            reference: record.id.clone(),
            identity: b58(&pair.0),
            contact: b58(&pair.1),
            source: "platform".into(),
            st_hash: document,
            address: address.to_string(),
            credits: self.credits,
            expected_duffs: record.expected_duffs,
            note: self.note.clone(),
            state: "pending".into(),
            since: self.since,
        };
        let wallet = id.to_string();
        let written = {
            let _rows = session.platform.payments.rows().await;
            let written = session
                .appdb_op(move |db| db.insert_dp_contact_payout_durable(&wallet, &row))
                .await;
            if let Ok(Some(holder)) = &written {
                // Another payment's row holds the pair: it is the lock.
                let source = match holder {
                    dw_appdb::PairHolder::Core(_) => PaymentSource::Core,
                    dw_appdb::PairHolder::Payout { source, .. } if source == "shielded" => {
                        PaymentSource::Shielded
                    }
                    dw_appdb::PairHolder::Payout { .. } => PaymentSource::Platform,
                };
                let lock = PaymentLock {
                    txid: holder.id().to_owned(),
                    since: self.since,
                    source,
                    reference: holder.id().to_owned(),
                };
                session.platform.payments.lock_payout(id, pair, lock);
            }
            written
        };
        let refused = match written {
            Ok(None) => None,
            Ok(Some(holder)) => Some(ContactError::PaymentLocked {
                txid: holder.id().to_owned(),
            }),
            Err(e) => {
                // A write that reported an error after its commit leaves no
                // row of a payout never signed.
                self.settle_row(&record.id, PayoutState::NotSent).await;
                Some(PlatformError::from(e).into())
            }
        };
        if let Some(e) = refused {
            session.platform.payments.give_back(id, pair, address);
            self.in_flight = None;
            return Err(e);
        }
        // Tests: the process stops between the payout row and the move
        // record (C5b's kill test).
        #[cfg(test)]
        if (session.platform.payments.stop_after_row)
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            panic!("the process stops");
        }
        session.platform.payments.put_payout(
            id,
            self.payout(
                &record.id,
                &address,
                PayoutState::Pending,
                record.expected_duffs,
            ),
        );
        record.to = address.to_string();
        let script = CoreScript::new(address.script_pubkey());
        self.address = Some(address);
        Ok(script)
    }

    fn payout(&self, reference: &str, address: &Address, state: PayoutState, duffs: u64) -> Payout {
        Payout {
            pair: self.pair,
            source: PaymentSource::Platform,
            reference: reference.to_owned(),
            script: Some(address.script_pubkey()),
            expected_duffs: duffs,
            state,
            since: self.since,
        }
    }

    /// Settles the row `reference` as `state`; `false` when the write
    /// failed (logged): the row stays `pending`, and so must memory, for a
    /// restart or the next read to settle it.
    async fn settle_row(&self, reference: &str, state: PayoutState) -> bool {
        let (wallet, reference, value) = (
            self.wallet_id.to_string(),
            reference.to_owned(),
            row_state(state),
        );
        let settled = self
            .session
            .appdb_op(move |db| db.settle_dp_contact_payout(&wallet, &reference, value))
            .await;
        if let Err(e) = &settled {
            tracing::warn!(wallet_id = %self.wallet_id, error = %e, "could not settle a payout row");
        }
        settled.is_ok()
    }

    /// The pair locked behind the payout `reference` (its row `pending`).
    fn lock(&self, reference: &str) {
        let lock = PaymentLock {
            txid: reference.to_owned(),
            since: self.since,
            source: PaymentSource::Platform,
            reference: reference.to_owned(),
        };
        (self.session.platform.payments).lock_payout(self.wallet_id, self.pair, lock);
        self.session.signal_payments(self.wallet_id, &self.pair.0);
    }

    /// The move record could not be written: nothing was signed, so the
    /// row settles not sent and the address goes back to the pair.
    pub(crate) async fn unrecorded(&mut self, record: &MoveRecord) {
        let _rows = self.session.platform.payments.rows().await;
        if !self.settle_row(&record.id, PayoutState::NotSent).await {
            // The row stands `pending` with no record: the restart clears it
            // and gives the address back (P6); until then it is the lock.
            self.lock(&record.id);
            self.in_flight = None;
            return;
        }
        self.session.platform.payments.settle_payout(
            self.wallet_id,
            &record.id,
            PayoutState::NotSent,
        );
        if let Some(address) = self.address.take() {
            (self.session.platform.payments).give_back(self.wallet_id, self.pair, address);
        }
        self.state = PayoutState::NotSent;
        self.in_flight = None;
    }

    /// The call's outcome for `record` (P5, P7): done, the row is sent;
    /// unable to land, not sent, its address back with the pair only when
    /// `give_back`; else the pair is locked behind it. The claim ends after.
    pub(crate) async fn outcome(&mut self, record: &MoveRecord, give_back: bool) {
        let (payments, id) = (&self.session.platform.payments, self.wallet_id);
        let _rows = payments.rows().await;
        // A final state whose row write failed stays the lock: memory
        // follows the row, and the next read settles both.
        let state = match final_state(record) {
            Some(state) if self.settle_row(&record.id, state).await => Some(state),
            _ => None,
        };
        match state {
            Some(state) => {
                payments.settle_payout(id, &record.id, state);
                if state == PayoutState::NotSent
                    && give_back
                    && let Some(address) = self.address.take()
                {
                    payments.give_back(id, self.pair, address);
                }
                self.state = state;
            }
            None => {
                let lock = PaymentLock {
                    txid: record.id.clone(),
                    since: self.since,
                    source: PaymentSource::Platform,
                    reference: record.id.clone(),
                };
                payments.lock_payout(id, self.pair, lock);
                self.state = PayoutState::Pending;
            }
        }
        self.session.signal_payments(id, &self.pair.0);
        self.in_flight = None;
    }
}

fn row_state(state: PayoutState) -> &'static str {
    match state {
        PayoutState::Pending => "pending",
        PayoutState::Sent => "sent",
        PayoutState::NotSent => "not_sent",
    }
}

/// Where a `ToContact` record leaves its payout: sent once done as itself,
/// not sent once it can never execute, else `None` (it may still).
fn final_state(record: &MoveRecord) -> Option<PayoutState> {
    use super::platform_addresses::PlatformActivityState as S;
    match record.state {
        S::Done if record.group.is_none() => Some(PayoutState::Sent),
        _ if record.holds() => None,
        S::Done | S::Pending => None,
        S::Failed | S::Replaced => Some(PayoutState::NotSent),
    }
}

/// Settles the payout row of each `ToContact` record in `moves` that its
/// record settled, and clears its lock (P7, P8). A row already settled, or
/// whose write fails, is left.
pub(crate) async fn settled(session: &NetworkSession, wallet_id: WalletId, moves: &[MoveRecord]) {
    let done: Vec<(&str, PayoutState)> = (moves.iter())
        .filter(|m| m.pays_contact())
        .filter_map(|m| Some((m.id.as_str(), final_state(m)?)))
        .collect();
    if done.is_empty() {
        return;
    }
    let payments = &session.platform.payments;
    let _rows = payments.rows().await;
    for (reference, state) in done {
        let known = payments.payout_state(&wallet_id, reference);
        if known.is_some_and(|k| k != PayoutState::Pending) {
            continue;
        }
        let (wallet, r, value) = (
            wallet_id.to_string(),
            reference.to_owned(),
            row_state(state),
        );
        match session
            .appdb_op(move |db| db.settle_dp_contact_payout(&wallet, &r, value))
            .await
        {
            Ok(_) => {
                if let Some(pair) = payments.settle_payout(wallet_id, reference, state) {
                    session.signal_payments(wallet_id, &pair.0);
                }
            }
            Err(e) => {
                tracing::warn!(wallet_id = %wallet_id, error = %e, "could not settle a payout row");
            }
        }
    }
}

/// After a load: the payout rows whose move records settled meanwhile
/// (a restart after the record changed but before the row did).
pub(crate) async fn settle_recorded(session: &NetworkSession, wallet_id: WalletId) {
    match platform_moves::records(session, wallet_id).await {
        Ok(moves) => settled(session, wallet_id, &moves).await,
        Err(e) => tracing::info!(wallet_id = %wallet_id, "reading the move records: {e}"),
    }
}

/// Names each `ContactPayment` row's contact from its payout row (P10).
pub(crate) async fn name_rows(
    session: &NetworkSession,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    rows: &mut [PlatformActivity],
) {
    if !rows
        .iter()
        .any(|r| r.kind == PlatformActivityKind::ContactPayment)
    {
        return;
    }
    let payouts = session.platform.payments.payouts(&wallet_id);
    let payees = {
        let wm = wallet.wallet_manager().read().await;
        (wm.get_wallet_info(&wallet.wallet_id())).map(|info| session.payees(info, &wallet_id))
    };
    for row in rows
        .iter_mut()
        .filter(|r| r.kind == PlatformActivityKind::ContactPayment)
    {
        let Some(payout) = payouts.iter().find(|p| p.reference == row.id) else {
            continue;
        };
        let contact = payout.pair.1;
        row.contact = Some(
            (payees.as_ref())
                .and_then(|p| p.counterparty(&contact))
                .unwrap_or_else(|| Counterparty {
                    identity: b58(&contact),
                    username: None,
                    display_name: None,
                    avatar_url: None,
                }),
        );
    }
}

/// The SDK's network and the wallet's platform wallet, for a call that
/// runs on the session's runtime already.
/// The mainnet flag of a contact payment from the Platform balance.
pub(crate) const FEATURE: &str = "platform_contact_payment";

/// Off on mainnet ([`FEATURE`]); on for testnet, devnet and regtest. On
/// mainnet the facade and `begin_flow` keep the refusal every network gave
/// before C5b.
pub(crate) fn refuse_on_mainnet(session: &NetworkSession) -> Result<(), PlatformError> {
    if session.vault.network() == dashcore::Network::Mainnet {
        return Err(PlatformError::FeatureOff {
            feature: FEATURE.into(),
        });
    }
    Ok(())
}

async fn live_net(
    session: &NetworkSession,
    wallet_id: WalletId,
) -> Result<(SdkMoves, Arc<PlatformWallet>), PlatformError> {
    let manager = session.manager().map_err(PlatformError::from)?;
    let wallet = super::names::platform_wallet(&manager, wallet_id).await?;
    let net = SdkMoves {
        sdk: manager.sdk_arc(),
        wallet: Arc::clone(&wallet),
    };
    Ok((net, wallet))
}

/// `contact_pay_quote` from the Platform balance past the payee's gates.
pub(crate) async fn quote_live(
    session: &NetworkSession,
    wallet_id: WalletId,
    credits: u64,
) -> Result<ContactPayQuote, ContactError> {
    refuse_on_mainnet(session)?;
    let (net, wallet) = live_net(session, wallet_id).await?;
    quote(&net, session, &wallet, wallet_id, credits).await
}

/// The quote of a payout of `credits` (P13, P14): a withdrawal of `credits`
/// to the contact, its fee source keeping three times the estimate (P8),
/// refused beside any move that holds (C-routes §0).
pub(crate) async fn quote(
    net: &impl MoveNet,
    session: &NetworkSession,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    credits: u64,
) -> Result<ContactPayQuote, ContactError> {
    refuse_on_mainnet(session)?;
    platform_moves::sends_active(net.sends_version().await)?;
    let (planned, expected_duffs) =
        platform_moves::contact_quote(net, session, wallet, wallet_id, credits).await?;
    let debit = planned.may_lose();
    Ok(ContactPayQuote {
        source: PaymentSource::Platform,
        credits,
        expected_duffs,
        fee_credits: planned.fee,
        grant: GrantRequest {
            max_duffs: 0,
            max_credits: debit,
        },
    })
}

/// `contact_pay` from the Platform balance on the session's runtime.
pub(crate) async fn pay_live(
    session: &Arc<NetworkSession>,
    wallet_id: WalletId,
    call: PayCall,
) -> Result<ContactPayOutcome, ContactError> {
    refuse_on_mainnet(session)?;
    let (net, wallet) = live_net(session, wallet_id).await?;
    pay(&net, session, &wallet, wallet_id, call).await
}

/// A payout of `call.credits` to `call.contact` (module doc, Order).
pub(crate) async fn pay(
    net: &impl MoveNet,
    session: &Arc<NetworkSession>,
    wallet: &Arc<PlatformWallet>,
    wallet_id: WalletId,
    call: PayCall,
) -> Result<ContactPayOutcome, ContactError> {
    refuse_on_mainnet(session)?;
    let pair = session.pay_gates(wallet_id, &call).await?;
    platform_moves::sends_active(net.sends_version().await)?;
    if !session.vault.has_wallet_secret(&wallet_id.0) {
        return Err(PlatformError::SignerUnavailable.into());
    }
    let crypto = session.background_crypto(wallet_id).await;
    let queued = wallet
        .identity()
        .dashpay()
        .drainable_contact_crypto_count()
        .await;
    if crypto.is_none() && queued > 0 {
        // The reservation's drain would refuse: unlock first (D6).
        return Err(PlatformError::SignerUnavailable.into());
    }
    // Before the move lock, as "Add from Dash wallet" takes them: no Core
    // payment to the contact reserves while this one does.
    let _fence = session.coin_fence(wallet_id).await.map_err(refusal)?;
    // A Core payment may have claimed the pair while this one waited for the
    // fence: refused here, before any grant is redeemed or address taken.
    session.check_payment_lock(wallet_id, &pair, None)?;
    let mut leg = ContactLeg {
        session: Arc::clone(session),
        wallet_id,
        wallet: Arc::clone(wallet),
        pair,
        credits: call.credits,
        note: call.note,
        crypto,
        address: None,
        in_flight: None,
        since: crate::events::unix_now(),
        refused: None,
        state: PayoutState::Pending,
    };
    let ask = (
        WithdrawAmount::Credits {
            credits: call.credits,
        },
        Target::Contact,
        None,
        None,
    );
    let ran = platform_moves::run(
        net,
        session,
        wallet,
        wallet_id,
        ask,
        &call.grant,
        Some(&mut leg),
    )
    .await;
    // One payout per `ContactPayFromBalance` lease (P15): once one claimed
    // its address, the lease ends, whatever the call answered.
    if (leg.address.is_some() || leg.state != PayoutState::Pending)
        && super::lease::parse_lease(&call.grant).is_some()
    {
        let _ = session.end_flow(call.grant.clone());
    }
    match ran {
        Ok((record, _)) => Ok(ContactPayOutcome {
            reference: record.id,
            state: leg.state,
        }),
        Err(e) => Err(leg.refused.take().unwrap_or_else(|| e.into())),
    }
}

/// A held payout this install has no move record of (a restored wallet):
/// `Sent` once its document, kept in the row, is proved; else `Unknown`.
async fn restored(
    net: &impl MoveNet,
    session: &NetworkSession,
    wallet_id: WalletId,
    reference: &str,
) -> LockResolution {
    let recorded = platform_moves::records(session, wallet_id).await;
    if recorded.map_or(true, |moves| moves.iter().any(|m| m.id == reference)) {
        return LockResolution::Unknown;
    }
    let (wallet, r) = (wallet_id.to_string(), reference.to_owned());
    let rows = session
        .appdb_op(move |db| db.dp_contact_payouts(&wallet))
        .await;
    let document = (rows.ok().into_iter().flatten())
        .find(|row| row.reference == r && row.source == "platform" && row.state == "pending")
        .and_then(|row| row.st_hash);
    let Some(document) = document else {
        return LockResolution::Unknown;
    };
    if platform_moves::restored_payout(session, net, &document).await != Some(true) {
        return LockResolution::Unknown;
    }
    let payments = &session.platform.payments;
    let _rows = payments.rows().await;
    let (wallet, r) = (wallet_id.to_string(), reference.to_owned());
    let value = row_state(PayoutState::Sent);
    match session
        .appdb_op(move |db| db.settle_dp_contact_payout(&wallet, &r, value))
        .await
    {
        Ok(_) => {
            if let Some(pair) = payments.settle_payout(wallet_id, reference, PayoutState::Sent) {
                session.signal_payments(wallet_id, &pair.0);
            }
            LockResolution::Sent
        }
        Err(e) => {
            tracing::warn!(%wallet_id, error = %e, "could not settle a restored payout row");
            LockResolution::Unknown
        }
    }
}

/// `resolve_payment_lock` of a Platform payout: the move's evidence read
/// again (P7, P8); `Sent` or `NotSent` once it settled the row.
pub(crate) async fn resolve(
    session: &NetworkSession,
    wallet_id: WalletId,
    pair: Pair,
    reference: &str,
) -> Result<LockResolution, ContactError> {
    let (net, wallet) = live_net(session, wallet_id).await?;
    resolve_with(&net, session, &wallet, wallet_id, pair, reference).await
}

pub(crate) async fn resolve_with(
    net: &impl MoveNet,
    session: &NetworkSession,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    pair: Pair,
    reference: &str,
) -> Result<LockResolution, ContactError> {
    platform_moves::activity(net, session, wallet, wallet_id).await?;
    let held = (session.platform.payments.get(&wallet_id, &pair))
        .is_some_and(|l| l.reference == reference);
    if held {
        return Ok(restored(net, session, wallet_id, reference).await);
    }
    Ok(
        match session
            .platform
            .payments
            .payout_state(&wallet_id, reference)
        {
            Some(PayoutState::Sent) => LockResolution::Sent,
            Some(PayoutState::NotSent) => LockResolution::NotSent,
            _ => LockResolution::Unknown,
        },
    )
}
