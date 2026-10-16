//! Contact payments (DP3-01, DASHPAY §2.3; decisions in
//! `docs/design/DP3-01-notes.md`): a `TxDraft` whose one recipient is a
//! DashPay contact. The estimate plans a same-size placeholder output; the
//! host authorizes after Confirm; `prepare` plans, redeems the grant into a
//! lease and charges it, and only then reserves the contact's next DIP-15
//! address, which it swaps into the planned output before signing. The
//! address stays reserved for the contact's next `prepare` until a
//! broadcast may have handed it to the network; one first broadcast per
//! contact at a time, and an unknown outcome locks the contact
//! (`platform/payment_locks.rs`).

use std::sync::Arc;

use dashcore::address::Payload;
use dashcore::hashes::Hash;
use dashcore::{Address, OutPoint, PubkeyHash};
use dw_vault::{GrantKind, VaultError};
use platform_wallet::wallet::platform_wallet::PlatformWallet;

use super::{
    EngineError, Phase, PreparedTx, Recipient, SendFailure, SpendCharge, TxDraft, lease_failure,
    validate_recipients, vault_failure,
};
use crate::platform::contacts_model::Payee;
use crate::platform::lease::Lease;
#[cfg(test)]
use crate::platform::money_gate::Point;
use crate::platform::money_gate::{Refusal, Subject};
use crate::platform::payment_locks::{ClaimRefused, InFlight, Pair, pair_of};
use crate::platform::signers::LockedContactCrypto;
use crate::platform::trust::Rule5;
use crate::platform::{
    ChannelState, ContactError, Eligibility, FlowKind, PlatformError, Relation, VaultContactCrypto,
};

/// m4 §6's `Recipient::Contact` (D1): pays `amount` duffs from our
/// `identity` to its DashPay `contact`, both Base58. `note` is stored with
/// the transaction as its message (D10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactRecipient {
    pub identity: String,
    pub contact: String,
    pub amount: u64,
    pub subtract_fee: bool,
    pub note: Option<String>,
}

/// The estimate's stand-in for the contact's address: P2PKH, the length of
/// every DIP-15 address, so the fee is the one the payment pays (D3). The
/// wallet owns no such key, so the spend cap counts the payment.
fn placeholder(network: dashcore::Network) -> Address {
    Address::new(
        network,
        Payload::PubkeyHash(PubkeyHash::from_byte_array([0; 20])),
    )
}

/// A refusal of the money-move gate (DP3-04 §6): a Lock during its wait
/// is the vault's, the others the contact's Platform refusal.
fn gate_refusal(refusal: Refusal) -> EngineError {
    match refusal {
        Refusal::Locked => SendFailure::VaultLocked.into(),
        refusal => ContactError::Platform(refusal.platform()).into(),
    }
}

/// Refuses a payment to a payee its relation or channel does not allow
/// (D2, D4). `pending`: a pending incoming request passes ("Accept and
/// pay" before its accept); otherwise it is `PendingIncoming`.
fn check_payee(payee: Payee, pending: bool) -> Result<(), ContactError> {
    match payee {
        Some((Relation::Contact, ChannelState::Broken)) => Err(ContactError::ChannelBroken),
        Some((Relation::Contact, _)) => Ok(()),
        Some((Relation::PendingIncoming, _)) if pending => Ok(()),
        Some((Relation::PendingIncoming, _)) => Err(ContactError::Ineligible {
            reason: Eligibility::PendingIncoming,
        }),
        _ => Err(PlatformError::InvalidArgument {
            detail: "not a contact of this identity".into(),
        }
        .into()),
    }
}

impl TxDraft {
    /// Makes `r` the draft's only recipient, validated offline from memory
    /// (D2): the amount as `set_recipients` checks it, the contact an
    /// established one or (for "Accept and pay") one whose request waits,
    /// its channel not broken and no payment to it locked.
    pub fn set_contact_recipient(&self, r: ContactRecipient) -> Result<(), EngineError> {
        let pair = pair_of(&r.identity, &r.contact)?;
        let network = self.network();
        let valid = validate_recipients(
            vec![Recipient {
                address: placeholder(network).to_string(),
                amount: r.amount,
                subtract_fee_from_amount: r.subtract_fee,
                label: None,
                message: r.note,
            }],
            network,
        )?;
        let _op = self.session.try_enter()?;
        check_payee(self.session.payee_state(self.wallet_id, &pair)?, true)?;
        (self.session).check_payment_lock(self.wallet_id, &pair, None)?;
        let mut state = self.state();
        state.recipients = valid;
        state.contact = Some(pair);
        Ok(())
    }

    /// `prepare` of a contact payment (D4–D7). Every refusal before the
    /// reservation leaves the contact's addresses alone. The relation is
    /// read live; the lock is checked before the trust gate's wait and
    /// again by `resolve` (DP3-04 §4).
    pub(super) async fn prepare_contact(
        self: &Arc<Self>,
        grant_id: String,
        pair: Pair,
    ) -> Result<Arc<PreparedTx>, EngineError> {
        let session = &self.session;
        let wallet_id = self.wallet_id;
        if !session.vault.has_wallet_secret(&wallet_id.0) {
            return Err(SendFailure::WatchOnly.into());
        }
        check_payee(session.payee_state_live(wallet_id, &pair).await?, false)?;
        session.check_payment_lock(wallet_id, &pair, None)?;
        // Rule 5 (DP3-04): before anything is taken, so a refusal keeps the
        // grant and the coin fence is not held across the wait.
        let payee = Subject::Payee(wallet_id, pair);
        session.money_gate(payee).await.map_err(gate_refusal)?;
        #[cfg(test)]
        session.platform.money_gate.reached(Point::Gated).await;
        let fence = session.coin_fence(wallet_id).await?;
        let given = (session.lease_for(&wallet_id, &grant_id))
            .transpose()
            .map_err(|e| lease_failure(e, 0))?;
        match &given {
            Some(lease) => {
                let flow = matches!(
                    lease.flow(),
                    FlowKind::AcceptAndPay | FlowKind::ContactPayment
                );
                if !flow || !lease.has_spend() {
                    return Err(SendFailure::GrantInvalid.into());
                }
                lease.spend_signer().map_err(|e| lease_failure(e, 0))?;
            }
            None => {
                if let Err(
                    e @ (VaultError::NoVault | VaultError::Locked | VaultError::MixingOnly),
                ) = (session.vault).check_grant(&grant_id, GrantKind::Spend, Some(&wallet_id.0))
                {
                    return Err(vault_failure(e));
                }
            }
        }
        let wallet = session.wallet(&wallet_id).await?;
        let crypto = self.payment_crypto(given.as_ref()).await;
        let queued = wallet
            .identity()
            .dashpay()
            .drainable_contact_crypto_count()
            .await;
        if crypto.is_none() && queued > 0 {
            // The reservation's drain would refuse: unlock first (D6).
            return Err(SendFailure::VaultLocked.into());
        }
        let mut r = self.resolve(&fence).await?;
        if r.contact != Some(pair) {
            return Err(EngineError::InvalidArgument(
                "the draft's recipient changed during prepare".into(),
            ));
        }
        let lease = match given {
            Some(lease) => lease,
            None => (session.begin_lease(wallet_id, FlowKind::ContactPayment, &[grant_id]))
                .await
                .map_err(|e| lease_failure(e, 0))?,
        };
        let (recipient_mine, change_mine, external_sent) = self.outflow_of(&r, &wallet).await;
        let outflow = external_sent.saturating_add(r.plan.fee);
        let (charge, signer) =
            (lease.charge_spend_signer(outflow)).map_err(|e| lease_failure(e, outflow))?;
        let charge = SpendCharge {
            lease,
            charge: Some(charge),
        };
        // Nothing refused so far took an address; from here one is taken.
        let address = self.payout_address(&wallet, pair, crypto).await?;
        r.recipients[0].address = address;
        self.sign_and_reserve(
            r,
            wallet,
            recipient_mine,
            change_mine,
            external_sent,
            signer,
            Some(charge),
            fence,
        )
        .await
    }

    /// The contact crypto the reservation drains through (D6): the lease's,
    /// else the wallet's background one; `None` without either.
    async fn payment_crypto(&self, lease: Option<&Lease>) -> Option<VaultContactCrypto> {
        if let Some(crypto) = lease.and_then(|l| l.contact_crypto().ok()) {
            return Some(crypto);
        }
        self.session.background_crypto(self.wallet_id).await
    }

    /// `pair`'s reserved address, reserved now if it has none (D7, D21),
    /// kept once rule 5's re-check passes (DP3-04 §4). The caller holds
    /// the wallet's coin fence, so no other `prepare` reserves meanwhile.
    async fn payout_address(
        &self,
        wallet: &PlatformWallet,
        pair: Pair,
        crypto: Option<VaultContactCrypto>,
    ) -> Result<Address, EngineError> {
        let (session, payments) = (&self.session, &self.session.platform.payments);
        let rule = session.trust.rule_5();
        let (address, fresh) = match payments.reserved_checked(self.wallet_id, pair) {
            // Kept while rule 5 did not apply: not paid once it does.
            Some((address, false)) if rule == Rule5::Enforce => {
                payments.drop_reserved(self.wallet_id, pair, &address);
                (self.reserve(wallet, pair, crypto).await?, true)
            }
            Some((address, _)) => (address, false),
            None => (self.reserve(wallet, pair, crypto).await?, true),
        };
        #[cfg(test)]
        session.platform.money_gate.reached(Point::Reserved).await;
        // The payee may have changed since the gate (§4 step 5): an address
        // taken from content that does not verify now is neither kept nor
        // paid.
        let subject = Subject::Payee(self.wallet_id, pair);
        let checked = match session.money_recheck(subject, rule).await {
            Ok(checked) => checked,
            Err(refusal) => {
                payments.drop_reserved(self.wallet_id, pair, &address);
                return Err(gate_refusal(refusal));
            }
        };
        if fresh {
            payments.keep(self.wallet_id, pair, address.clone(), checked);
        } else if payments
            .reserved_checked(self.wallet_id, pair)
            .is_none_or(|(a, _)| a != address)
        {
            // Another payment's broadcast claimed it during the re-check.
            return Err(SendFailure::PreparedTxSpent.into());
        }
        Ok(address)
    }

    /// A new address of `pair`'s contact from the library (D7).
    async fn reserve(
        &self,
        wallet: &PlatformWallet,
        pair: Pair,
        crypto: Option<VaultContactCrypto>,
    ) -> Result<Address, EngineError> {
        let dashpay = wallet.identity().dashpay();
        let reserved = match &crypto {
            Some(c) => dashpay.reserve_payment_address(&pair.0, &pair.1, c).await,
            None => (dashpay.reserve_payment_address(&pair.0, &pair.1, &LockedContactCrypto)).await,
        }
        .map_err(|e| match PlatformError::from(e) {
            // Crypto queued since the check, with no key to drain it (D6).
            PlatformError::SignerUnavailable => EngineError::from(SendFailure::VaultLocked),
            e => ContactError::Platform(e).into(),
        })?;
        // Every DIP-15 address is P2PKH: the planned output's length.
        if !matches!(reserved.payload(), Payload::PubkeyHash(_)) {
            return Err(EngineError::Internal(format!(
                "the contact's payment address {reserved} is not P2PKH"
            )));
        }
        Ok(reserved)
    }

    /// Before the first broadcast of a contact payment: refuses it while
    /// another payment to the contact is locked or under way, and takes
    /// its address off the contact, so no other payment pays it (D7, D8,
    /// D20). An address already taken is `send.prepared_tx_spent`. The
    /// guard marks the broadcast under way until it drops.
    pub(super) fn claim_contact(
        &self,
        prepared: &PreparedTx,
    ) -> Result<Option<InFlight>, EngineError> {
        let Some((pair, address)) = &prepared.contact else {
            return Ok(None);
        };
        match (self.session).claim_payment(self.wallet_id, *pair, &prepared.summary.txid, address) {
            Ok(in_flight) => Ok(Some(in_flight)),
            Err(ClaimRefused::Locked(e)) => Err(e.into()),
            Err(ClaimRefused::Spent) => Err(SendFailure::PreparedTxSpent.into()),
        }
    }

    /// Before a contact payment's first hand-off: its lock row (D23). If it
    /// cannot be written, or another payment's holds the contact, nothing
    /// is handed off: the payment is `Pending` again, its own row (should
    /// the write have landed after all) dropped, and its address back with
    /// the contact.
    pub(super) async fn record_contact(&self, prepared: &PreparedTx) -> Result<(), EngineError> {
        let Some((pair, address)) = &prepared.contact else {
            return Ok(());
        };
        let inputs: Vec<OutPoint> = prepared.summary.inputs.iter().map(|i| i.outpoint).collect();
        let recorded = (self.session)
            .record_payment(self.wallet_id, *pair, &prepared.summary.txid, &inputs)
            .await;
        if recorded.is_err() {
            // A write that committed yet reported an error must not leave
            // a row behind for a payment never handed off.
            (self.session)
                .clear_payment_lock(self.wallet_id, *pair, &prepared.summary.txid)
                .await;
            // Unchecked: under rule 5 the next `prepare` reserves anew.
            (self.session.platform.payments).keep(self.wallet_id, *pair, address.clone(), false);
            *prepared.phase() = Phase::Pending;
        }
        recorded
    }

    /// After a broadcast of a contact payment left it in `next`: a first
    /// one never sent returns its address to the contact and drops its row;
    /// unknown, the contact is locked behind it; accepted, its lock and row
    /// go (D7, D8, D23). Runs before the payment leaves `Broadcasting`,
    /// under its attempt guard, so a repeat settles after it (D24). A
    /// wallet removed or unloaded meanwhile is left alone: its row stays
    /// and its next load settles it on the evidence.
    pub(super) async fn settle_contact(&self, prepared: &PreparedTx, first: bool, next: Phase) {
        let Some((pair, address)) = &prepared.contact else {
            return;
        };
        if self.session.wallet(&self.wallet_id).await.is_err() {
            return;
        }
        #[cfg(test)]
        {
            if let Some(pause) = self.session.platform.payments.settle_pause() {
                pause
                    .reached
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                pause.release.notified().await;
            }
        }
        let txid = &prepared.summary.txid;
        match next {
            Phase::Pending | Phase::Released if first => {
                (self.session.platform.payments).keep(
                    self.wallet_id,
                    *pair,
                    address.clone(),
                    false,
                );
                (self.session)
                    .clear_payment_lock(self.wallet_id, *pair, txid)
                    .await;
            }
            Phase::Unknown => {
                if let Ok(parsed) = txid.parse() {
                    let inputs = prepared.summary.inputs.iter().map(|i| i.outpoint).collect();
                    (self.session)
                        .lock_payment(self.wallet_id, *pair, parsed, inputs)
                        .await;
                }
            }
            Phase::Sent => {
                (self.session)
                    .clear_payment_lock(self.wallet_id, *pair, txid)
                    .await;
            }
            Phase::Pending | Phase::Released | Phase::Broadcasting => {}
        }
    }
}
