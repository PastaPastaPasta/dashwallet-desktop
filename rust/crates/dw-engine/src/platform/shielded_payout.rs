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
use super::payment_locks::{ClaimRefused, Pair, PayCall, PayoutEvidence};
use super::payments::{
    ContactPayOutcome, ContactPayQuote, Counterparty, PaymentSource, PayoutState,
};
use super::registration_flow::b58;
use super::runtime::guard;
use super::shielded::{ShieldedActivity, ShieldedActivityKind, ShieldedSpendState};
use super::shielded_pool::{self as pool, MEMO_BYTES, Quoting, ShieldedLib, Spent, To};
use super::signers::LockedContactCrypto;
use super::trust::Rule5;
use crate::{DashNetwork, NetworkSession, WalletId};

/// The flag that keeps this flow off on mainnet until its funds ruling:
/// there it refuses `platform.feature_off{feature: FEATURE}` as every
/// network did before it (`payout_source`).
pub(crate) const FEATURE: &str = "shielded_contact_payment";

/// Whether a contact payment from the Shielded balance runs on `network`:
/// not on mainnet; testnet, devnet and regtest pay.
pub(crate) fn enabled_on(network: &DashNetwork) -> bool {
    *network != DashNetwork::Mainnet
}

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
    /// Ends the arm: the transition the hook named, if any.
    fn disarm(self) -> Option<[u8; 32]> {
        (guard(&self.session.platform.payments.armed).remove(&self.wallet)).and_then(|a| a.named)
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

fn address_went_elsewhere() -> ContactError {
    PlatformError::InvalidArgument {
        detail: "the contact's address went to another payment; pay again".into(),
    }
    .into()
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
    let pair = session.pay_gates(wallet, &call).await?;
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
    if !enabled_on(&session.network) {
        return Err(PlatformError::FeatureOff {
            feature: FEATURE.into(),
        }
        .into());
    }
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
    // A `begin_flow` lease pays one payout: it ends once that one may
    // have been sent, so a repeated call cannot pay the contact again.
    let flow_lease = session.lease_for(&wallet, &call.grant).is_some();
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
        Err(ClaimRefused::Spent) => return Err(address_went_elsewhere()),
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
    let named = arm.disarm();
    if flow_lease {
        let _ = session.end_flow(call.grant);
    }
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
    /// is named in the payout's row, durably: the payout it named. Any
    /// failure refuses the spend, so nothing is sent. Nothing to do with no
    /// payout armed.
    pub(crate) async fn accept_payout_spend(
        &self,
        wallet: WalletId,
        kind: &ShieldedDispatchKind,
        st_hash: [u8; 32],
    ) -> Result<Option<String>, String> {
        let reference = match guard(&self.platform.payments.armed).get(&wallet) {
            None => return Ok(None),
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
        Ok(Some(reference))
    }

    /// After the hook wrote the artifact of the spend `st_hash`, which
    /// [`Self::accept_payout_spend`] named in payout `named`'s row:
    /// records, durably, that the library may now send it
    /// ([`dw_appdb::dp_payout_dispatched`]). A failure settles the artifact
    /// `not_executed` and refuses the spend. Nothing to do for no payout.
    pub(crate) async fn confirm_payout_dispatch(
        &self,
        wallet: WalletId,
        named: Option<String>,
        st_hash: [u8; 32],
    ) -> Result<(), String> {
        let Some(reference) = named else {
            return Ok(());
        };
        let scope = dw_appdb::local_scope(&wallet.to_string());
        let key = dw_appdb::dp_payout_dispatched(&reference);
        #[cfg(test)]
        let failing =
            (self.platform.payments.fail_dispatch_record).load(std::sync::atomic::Ordering::SeqCst);
        #[cfg(not(test))]
        let failing = false;
        let written = match failing {
            true => Err("injected".to_owned()),
            false => (self.appdb_op(move |db| db.set_setting_durable(&scope, &key, Some("1"))))
                .await
                .map_err(|e| e.to_string()),
        };
        let Err(e) = written else {
            return Ok(());
        };
        let unsent = ArtifactStatus::NotExecuted;
        let settled = (self.leases)
            .settle_shielded_artifact(wallet, st_hash, unsent, None, false)
            .await;
        if let Err(e) = settled {
            tracing::warn!(wallet_id = %wallet, error = %e, "a refused payout spend's artifact was not settled");
        }
        Err(format!(
            "the contact payout's dispatch was not recorded: {e}"
        ))
    }

    /// Whether a re-send of transition `st_hash` keeps P6: every payout row
    /// of `wallet` that names it holds its recorded dispatch
    /// ([`Self::confirm_payout_dispatch`]). A read that fails refuses.
    pub(crate) async fn payout_dispatch_recorded(
        &self,
        wallet: WalletId,
        st_hash: [u8; 32],
    ) -> Result<(), String> {
        let (w, named) = (wallet.to_string(), hex::encode(st_hash));
        let scope = dw_appdb::local_scope(&w);
        let unrecorded = self
            .appdb_op(move |db| {
                for row in db.dp_contact_payouts(&w)? {
                    if row.st_hash.as_deref() != Some(named.as_str()) {
                        continue;
                    }
                    let key = dw_appdb::dp_payout_dispatched(&row.reference);
                    if db.setting(&scope, &key)?.is_none() {
                        return Ok(Some(row.reference));
                    }
                }
                Ok(None)
            })
            .await
            .map_err(|e| format!("the contact payouts could not be read: {e}"))?;
        match unrecorded {
            None => Ok(()),
            Some(reference) => Err(format!(
                "the contact payout {reference}'s dispatch was not recorded"
            )),
        }
    }

    /// What the journal proves of the shielded payout `reference` whose
    /// transition is `st_hash` (P6, P7, P9): no artifact, never sent, but
    /// only while the hook never recorded its dispatch (else the journal is
    /// lost or older: unknown); settled executed from the library's reported
    /// outcome, sent; not executed, not sent with its address consumed (P5).
    /// Anything else, a read failure and an execution the library could not
    /// attribute included, is unknown.
    pub(crate) async fn shielded_payout_evidence(
        &self,
        wallet: WalletId,
        reference: &str,
        st_hash: [u8; 32],
    ) -> PayoutEvidence {
        let Ok(artifacts) = self.leases.shielded_artifacts(wallet).await else {
            return PayoutEvidence::Unknown;
        };
        let executed = |a: &dw_appdb::dispatch::ShieldedArtifact| {
            a.status == ArtifactStatus::Executed
                && a.disposition
                    .as_ref()
                    .is_some_and(|(kind, _)| kind == "executed")
        };
        match artifacts.iter().find(|a| a.st_hash == Some(st_hash)) {
            None => {
                let scope = dw_appdb::local_scope(&wallet.to_string());
                let key = dw_appdb::dp_payout_dispatched(reference);
                match self.appdb_op(move |db| db.setting(&scope, &key)).await {
                    Ok(None) => PayoutEvidence::NotSent { give_back: true },
                    Ok(Some(_)) | Err(_) => PayoutEvidence::Unknown,
                }
            }
            Some(a) if executed(a) => PayoutEvidence::Sent,
            Some(a) if a.status == ArtifactStatus::NotExecuted => {
                PayoutEvidence::NotSent { give_back: false }
            }
            Some(_) => PayoutEvidence::Unknown,
        }
    }

    /// Settles a shielded payout from its own call's end. `Done` is sent.
    /// A spend the journal proves unsent is not sent: one whose transition
    /// never reached it (none named, or one with no artifact and no
    /// recorded dispatch) gives its address back, one refused after its
    /// artifact (settled `not_executed`) keeps it consumed: not sent, and its address
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
        // A read failure is not proof: the contact stays held.
        let evidence = match named {
            None => PayoutEvidence::NotSent { give_back: true },
            Some(hash) => self.shielded_payout_evidence(wallet, reference, hash).await,
        };
        if let PayoutEvidence::NotSent { give_back } = evidence {
            let state = PayoutState::NotSent;
            let back = give_back.then_some(address);
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
                return Err(ContactError::Platform(refusal.platform()));
            }
        };
        if fresh {
            payments.keep(wallet, pair, address.clone(), checked);
        }
        if (payments.reserved_checked(wallet, pair)).is_none_or(|(a, _)| a != address) {
            return Err(address_went_elsewhere());
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
        if !enabled_on(&self.network) {
            return;
        }
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
            // The payout's row names its contact even once the payee
            // cache no longer has it; the address fallback needs the cache.
            row.contact = match by_row {
                Some(c) => Some(payees.counterparty(&c).unwrap_or_else(|| Counterparty {
                    identity: b58(&c),
                    username: None,
                    display_name: None,
                    avatar_url: None,
                })),
                None => by_address().and_then(|c| payees.counterparty(&c)),
            };
        }
    }
}
