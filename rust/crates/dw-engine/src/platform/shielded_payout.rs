//! A contact payment from the Shielded balance (C-funding D-C5, C5a): an
//! unshield to the contact's reserved DIP-15 address, through every gate
//! an unshield to Core has (P3) and the engine's dispatch hook. The
//! payout's row is written before its spend is built; the hook's accept
//! names the spend's transition in that row, durably, before it writes
//! the spend's artifact, and refuses the spend if it cannot (P6). The
//! journal's artifact is the payout's source record: its status settles
//! the payout (P7, P9), never the user's acknowledgement in Repair.

use std::sync::Arc;

use dashcore::Address;
use dashcore::address::Payload;
use dw_appdb::DpContactPayoutRow;
use dw_appdb::dispatch::ArtifactStatus;
use platform_wallet::wallet::shielded::ShieldedDispatchKind;

use super::VaultContactCrypto;
use super::errors::{ContactError, PlatformError, ShieldedError};
use super::flows::FlowKind;
use super::money_gate::Refusal;
use super::payment_locks::{ClaimRefused, Pair, PayCall, PayoutEvidence};
use super::payments::{ContactPayOutcome, ContactPayQuote, PaymentSource, PayoutState};
use super::registration_flow::b58;
use super::runtime::guard;
use super::shielded::{ShieldedActivity, ShieldedActivityKind, ShieldedSpendState};
use super::shielded_pool::{self as pool, MEMO_BYTES, Quoting, ShieldedLib, Spent, To};
use super::signers::LockedContactCrypto;
use super::trust::Rule5;
use crate::{NetworkSession, WalletId};

/// The wallet's shielded payout whose spend the library is building, and
/// the transition the hook named in its row (P6).
pub(crate) struct ArmedPayout {
    reference: String,
    named: Option<[u8; 32]>,
}

/// While it lives, the wallet's next accepted spend is the payout's.
pub(crate) struct Arm<'a> {
    session: &'a NetworkSession,
    wallet: WalletId,
}

impl Arm<'_> {
    fn named(&self) -> Option<[u8; 32]> {
        (guard(&self.session.platform.payments.armed).get(&self.wallet)).and_then(|a| a.named)
    }
}

impl Drop for Arm<'_> {
    fn drop(&mut self) {
        guard(&self.session.platform.payments.armed).remove(&self.wallet);
    }
}

/// A payout's reference: an id the engine makes (P6).
fn new_reference() -> String {
    format!("sh-{}", hex::encode(rand::random::<[u8; 16]>()))
}

fn gate_refusal(refusal: Refusal) -> ContactError {
    ContactError::Platform(refusal.platform())
}

/// `contact_pay_quote` from the Shielded balance past the payee and the
/// pair's lock: the unshield's withdrawal range and fee (P13, P14).
pub(crate) async fn quote(
    lib: &impl ShieldedLib,
    credits: u64,
) -> Result<ContactPayQuote, ContactError> {
    let quote = pool::unshield_plan(lib, credits).await?;
    Ok(ContactPayQuote {
        source: PaymentSource::Shielded,
        credits,
        expected_duffs: quote.expected_duffs.unwrap_or_default(),
        fee_credits: quote.fee_credits,
        grant: quote.grant,
    })
}

/// `contact_pay` from the Shielded balance, in P2's order. Every refusal
/// before the reservation takes no address; every refusal before the
/// lease keeps the grant.
#[cfg(test)]
pub(crate) async fn pay(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl ShieldedLib,
    call: PayCall,
) -> Result<ContactPayOutcome, ContactError> {
    let pair = session.payout_gates(wallet, &call).await?;
    pay_gated(session, wallet, lib, pair, call).await
}

/// [`pay`] past its payee, lock and rule-5 gates, which the caller ran.
pub(crate) async fn pay_gated(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl ShieldedLib,
    pair: Pair,
    call: PayCall,
) -> Result<ContactPayOutcome, ContactError> {
    // The source's gates (P3), as an unshield's: the hook, the wallet's
    // lock, an unrecorded spend, the withdrawal range.
    pool::unhooked(lib).await?;
    let lock = pool::wallet_lock(session, wallet);
    let _spending = lock.lock().await;
    // A payment that ended while this one waited for the lock may hold
    // the pair now: refused before the grant or an address.
    session.check_payment_lock(wallet, &pair, None)?;
    pool::confirm_unrecorded(session, wallet, call.confirm_unrecorded).await?;
    pool::unshield_plan(lib, call.credits).await?;
    let quoting = (call.credits, Quoting::Withdraw);
    let flow = FlowKind::ContactPayFromBalance;
    let armed = pool::arm(session, wallet, lib, quoting, &call.grant, flow).await?;
    let crypto = match armed.lease.contact_crypto() {
        Ok(crypto) => Some(crypto),
        Err(_) => session.background_crypto(wallet).await,
    };
    let address = session.payout_address(wallet, pair, crypto).await?;
    let reference = new_reference();
    let in_flight = match session.claim_payment(wallet, pair, &reference, &address) {
        Ok(in_flight) => in_flight,
        Err(ClaimRefused::Locked(e)) => return Err(e),
        Err(ClaimRefused::Spent) => {
            return Err(PlatformError::InvalidArgument {
                detail: "the contact's address went to another payment; pay again".into(),
            }
            .into());
        }
    };
    let row = DpContactPayoutRow {
        reference: reference.clone(),
        identity: b58(&pair.0),
        contact: b58(&pair.1),
        source: "shielded".into(),
        st_hash: None,
        address: address.to_string(),
        credits: call.credits,
        expected_duffs: armed.quote.expected_duffs.unwrap_or_default(),
        note: call.note,
        state: "pending".into(),
        since: crate::events::unix_now(),
    };
    if let Err(e) = session.record_payout(wallet, pair, row).await {
        session.platform.payments.give_back(wallet, pair, address);
        return Err(e);
    }
    let arm = session.arm_payout(wallet, &reference);
    let to = To::Core(address.clone());
    let sent = pool::send_armed(session, wallet, lib, armed, &to, [0; MEMO_BYTES]).await;
    let named = arm.named();
    drop(arm);
    #[cfg(test)]
    if let Some(pause) = session.platform.payments.settle_pause() {
        pause.mark_reached();
        pause.release.notified().await;
    }
    let settled = session
        .settle_payout_call(wallet, pair, (&reference, address), named, sent)
        .await;
    drop(in_flight);
    settled
}

impl NetworkSession {
    /// Makes `reference` the wallet's payout the hook names in its row
    /// until the returned guard drops. The caller holds the wallet's
    /// shielded lock and the passes, so the wallet's next accepted spend
    /// is the payout's.
    pub(crate) fn arm_payout(&self, wallet: WalletId, reference: &str) -> Arm<'_> {
        guard(&self.platform.payments.armed).insert(
            wallet,
            ArmedPayout {
                reference: reference.to_owned(),
                named: None,
            },
        );
        Arm {
            session: self,
            wallet,
        }
    }

    /// The hook's accept of a spend of `wallet` (P6): with a payout armed,
    /// only a withdrawal to Core, and only once its transition `st_hash`
    /// is named in the payout's row, durably. Any failure refuses the
    /// spend, so nothing is sent. Nothing to do with no payout armed.
    pub(crate) async fn accept_payout_spend(
        &self,
        wallet: WalletId,
        kind: &ShieldedDispatchKind,
        st_hash: [u8; 32],
    ) -> Result<(), String> {
        let reference = match guard(&self.platform.payments.armed).get(&wallet) {
            None => return Ok(()),
            Some(armed) if armed.named.is_some() => {
                return Err("the contact payout's spend was accepted already".into());
            }
            Some(armed) => armed.reference.clone(),
        };
        if !matches!(kind, ShieldedDispatchKind::Withdraw) {
            return Err("a contact payout's spend is a withdrawal to Core".into());
        }
        let (w, r, h) = (wallet.to_string(), reference.clone(), hex::encode(st_hash));
        #[cfg(test)]
        crate::coins::pause_at(&self.platform.payments.hold_st_hash).await;
        let written = self
            .appdb_op(move |db| db.set_dp_contact_payout_st_hash_durable(&w, &r, &h))
            .await;
        match written {
            Ok(true) => {}
            Ok(false) => return Err("the contact payout's row is not pending".into()),
            Err(e) => return Err(format!("the contact payout's row was not written: {e}")),
        }
        if let Some(armed) = guard(&self.platform.payments.armed).get_mut(&wallet)
            && armed.reference == reference
        {
            armed.named = Some(st_hash);
        }
        Ok(())
    }

    /// What the journal proves of the shielded payout whose transition is
    /// `st_hash` (P6, P7, P9): no artifact, never sent; executed, sent;
    /// not executed, not sent with its address consumed (P5). Anything
    /// else, a read failure included, is unknown.
    pub(crate) async fn shielded_payout_evidence(
        &self,
        wallet: WalletId,
        st_hash: [u8; 32],
    ) -> PayoutEvidence {
        let Ok(artifacts) = self.leases.shielded_artifacts(wallet).await else {
            return PayoutEvidence::Unknown;
        };
        match artifacts.iter().find(|a| a.st_hash == Some(st_hash)) {
            None => PayoutEvidence::NotSent { give_back: true },
            Some(a) if a.status == ArtifactStatus::Executed => PayoutEvidence::Sent,
            Some(a) if a.status == ArtifactStatus::NotExecuted => {
                PayoutEvidence::NotSent { give_back: false }
            }
            Some(_) => PayoutEvidence::Unknown,
        }
    }

    /// Settles a shielded payout from its own call's end. `Done` is sent.
    /// A spend whose transition never reached the journal (none named, or
    /// one with no artifact) was never sent: not sent, and its address
    /// goes back to the pair. Anything else holds the contact until the
    /// journal settles it: a pending result, a pool-floor refusal (P5)
    /// and any other failure once its artifact exists.
    async fn settle_payout_call(
        &self,
        wallet: WalletId,
        pair: Pair,
        (reference, address): (&str, Address),
        named: Option<[u8; 32]>,
        sent: Result<Spent, ShieldedError>,
    ) -> Result<ContactPayOutcome, ContactError> {
        let source = PaymentSource::Shielded;
        let outcome = |state| ContactPayOutcome {
            reference: reference.to_owned(),
            state,
        };
        let e = match sent {
            Ok(spent) if spent.state == ShieldedSpendState::Done => {
                let settled = self
                    .settle_payout(wallet, pair, (source, reference), PayoutState::Sent, None)
                    .await;
                return Ok(outcome(if settled {
                    PayoutState::Sent
                } else {
                    PayoutState::Pending
                }));
            }
            Ok(_) => {
                self.hold_payout(wallet, pair, source, reference).await;
                return Ok(outcome(PayoutState::Pending));
            }
            Err(e) => e,
        };
        let journaled = match named {
            None => Some(false),
            Some(hash) => self.leases.has_shielded_artifact(wallet, hash).await.ok(),
        };
        if journaled == Some(false) {
            let state = PayoutState::NotSent;
            let back = Some(address);
            self.settle_payout(wallet, pair, (source, reference), state, back)
                .await;
            return Err(e.into());
        }
        self.hold_payout(wallet, pair, source, reference).await;
        match e {
            e @ ShieldedError::PoolTooSmall { .. } => Err(e.into()),
            e => {
                tracing::warn!(wallet_id = %wallet, %reference, error = %e,
                    "a shielded contact payout failed after its spend was accepted; the contact stays locked");
                Err(PlatformError::BroadcastUnknown {
                    artifact: reference.to_owned(),
                }
                .into())
            }
        }
    }

    /// Settles the wallet's pending shielded payouts the journal now
    /// decides (P7, P9), after a pass read the library's outcomes into
    /// it. A payout whose call is under way is left to it.
    pub(crate) async fn settle_shielded_payouts(&self, wallet: WalletId) {
        let pending: Vec<(Pair, String)> = (self.platform.payments.locks_of(&wallet).into_iter())
            .filter(|(_, l)| l.source == PaymentSource::Shielded)
            .map(|(pair, l)| (pair, l.reference))
            .collect();
        for (pair, reference) in pending {
            self.resolve_payout(wallet, pair, &reference).await;
        }
    }

    /// The pair's reserved address for a payout, reserved now if it has
    /// none (P5: shared with the Core path), kept once rule 5's re-check
    /// passes (DP3-04 §4), as a Core contact payment's.
    async fn payout_address(
        self: &Arc<Self>,
        wallet: WalletId,
        pair: Pair,
        crypto: Option<VaultContactCrypto>,
    ) -> Result<Address, ContactError> {
        let payments = &self.platform.payments;
        let rule = self.trust.strict_rule_5();
        let (address, fresh) = match payments.reserved_checked(wallet, pair) {
            Some((address, false)) if rule == Rule5::Enforce => {
                payments.drop_reserved(wallet, pair, &address);
                (self.reserve_payout(wallet, pair, crypto).await?, true)
            }
            Some((address, _)) => (address, false),
            None => (self.reserve_payout(wallet, pair, crypto).await?, true),
        };
        let checked = match self.money_recheck(wallet, pair, &address, rule).await {
            Ok(checked) => checked,
            Err(refusal) => {
                payments.drop_reserved(wallet, pair, &address);
                return Err(gate_refusal(refusal));
            }
        };
        if fresh {
            payments.keep(wallet, pair, address.clone(), checked);
        }
        if (payments.reserved_checked(wallet, pair)).is_none_or(|(a, _)| a != address) {
            return Err(PlatformError::InvalidArgument {
                detail: "the contact's address went to another payment; pay again".into(),
            }
            .into());
        }
        Ok(address)
    }

    /// A new address of `pair`'s contact from the library (DP3-01 D7).
    async fn reserve_payout(
        &self,
        wallet: WalletId,
        pair: Pair,
        crypto: Option<VaultContactCrypto>,
    ) -> Result<Address, ContactError> {
        let platform_wallet = self.wallet(&wallet).await.map_err(PlatformError::from)?;
        let dashpay = platform_wallet.identity().dashpay();
        let reserved = match &crypto {
            Some(c) => dashpay.reserve_payment_address(&pair.0, &pair.1, c).await,
            None => (dashpay.reserve_payment_address(&pair.0, &pair.1, &LockedContactCrypto)).await,
        }
        .map_err(PlatformError::from)?;
        if !matches!(reserved.payload(), Payload::PubkeyHash(_)) {
            return Err(PlatformError::Internal {
                detail: format!("the contact's payment address {reserved} is not P2PKH"),
            }
            .into());
        }
        Ok(reserved)
    }

    /// Names the contact a payout paid on each withdrawal of `rows` (P10):
    /// by the payout row whose transition the row's artifact carries, else
    /// by the destination address through the payee scripts.
    pub(crate) async fn name_shielded_payouts(
        &self,
        wallet: WalletId,
        rows: &mut [ShieldedActivity],
    ) {
        if !rows
            .iter()
            .any(|r| r.kind == ShieldedActivityKind::Withdrawal)
        {
            return;
        }
        let payouts = self.platform.payments.payouts(&wallet);
        let artifacts = self
            .leases
            .shielded_artifacts(wallet)
            .await
            .unwrap_or_default();
        let w = wallet.to_string();
        let hashes: Vec<(String, String)> = (self.appdb_op(move |db| db.dp_contact_payouts(&w)))
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.source == "shielded")
            .filter_map(|r| Some((r.st_hash?, r.reference)))
            .collect();
        let Ok(manager) = self.manager() else {
            return;
        };
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let Some(info) = wm.get_wallet_info(&wallet.0) else {
            return;
        };
        let payees = self.payees(info, &wallet);
        let network = self.network.core_network();
        for row in rows
            .iter_mut()
            .filter(|r| r.kind == ShieldedActivityKind::Withdrawal)
        {
            let by_row = (hex::decode(&row.id).ok())
                .and_then(|id| artifacts.iter().find(|a| a.activity[..] == id[..]))
                .and_then(|a| a.st_hash)
                .and_then(|h| hashes.iter().find(|(s, _)| *s == hex::encode(h)))
                .and_then(|(_, reference)| payouts.iter().find(|p| p.reference == *reference))
                .map(|p| p.pair.1);
            let by_address = || {
                let address = row.counterparty.as_deref()?.parse::<Address<_>>().ok()?;
                let script = address.require_network(network).ok()?.script_pubkey();
                payees.sent_to(&script)
            };
            row.contact = by_row
                .or_else(by_address)
                .and_then(|c| payees.counterparty(&c));
        }
    }
}
