//! The per-contact payment lock (DP3-01, `dp_payment_lock`; DASHPAY §2.3
//! step 8): a contact payment whose broadcast outcome is unknown locks its
//! (identity, contact) pair until positive evidence settles it. Held in
//! memory for the sync `payment_lock` read (m4 rule 3), mirrored to
//! app.sqlite, loaded with the wallet's identity choices. Also the pair's
//! first broadcast under way (D20) and its reserved address no broadcast
//! handed out yet (D21), both for the session.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use dashcore::{Address, OutPoint, Txid};
use dpp::prelude::Identifier;
use dw_appdb::DpPaymentLockRow;
use key_wallet::transaction_checking::TransactionContext;

use super::contacts_model::identifier;
use super::errors::{ContactError, PlatformError};
use super::flows::DispatchState;
use super::lease::ArtifactId;
use super::payments::{LockResolution, PaymentLock};
use super::registration_flow::b58;
use super::runtime::guard;
use crate::{NetworkSession, PlatformChange, WalletId};

/// One side of a contact payment: our identity and the contact it pays.
pub(crate) type Pair = (Identifier, Identifier);

/// The session's payment locks, and the inputs of each locked payment
/// (the conflict evidence of D9 when the wallet lost its record of it,
/// stored with the lock: D20).
#[derive(Default)]
pub(crate) struct PaymentLocks {
    locks: Mutex<HashMap<WalletId, BTreeMap<Pair, PaymentLock>>>,
    inputs: Mutex<HashMap<Txid, Vec<OutPoint>>>,
    /// Each pair's payment whose first broadcast is under way (D20).
    in_flight: Mutex<HashMap<(WalletId, Pair), String>>,
    /// Each pair's reserved address that no broadcast handed out (D21).
    reserved: Mutex<HashMap<(WalletId, Pair), Address>>,
}

impl PaymentLocks {
    pub(crate) fn get(&self, id: &WalletId, pair: &Pair) -> Option<PaymentLock> {
        guard(&self.locks).get(id)?.get(pair).cloned()
    }

    pub(crate) fn forget(&self, id: &WalletId) {
        guard(&self.locks).remove(id);
        guard(&self.reserved).retain(|(w, _), _| w != id);
    }

    /// A restart: everything the session held in memory goes.
    #[cfg(test)]
    pub(crate) fn restart(&self) {
        guard(&self.locks).clear();
        guard(&self.inputs).clear();
        guard(&self.in_flight).clear();
        guard(&self.reserved).clear();
    }

    /// The pair's reserved address, if no broadcast handed it out.
    pub(crate) fn reserved(&self, id: WalletId, pair: Pair) -> Option<Address> {
        guard(&self.reserved).get(&(id, pair)).cloned()
    }

    /// Keeps `address` as the pair's reserved one, unless it has one.
    pub(crate) fn keep(&self, id: WalletId, pair: Pair, address: Address) {
        guard(&self.reserved).entry((id, pair)).or_insert(address);
    }
}

/// A pair's first broadcast under way (D20): no other payment to the pair
/// is claimed until it drops, after its outcome settled the lock.
pub(crate) struct InFlight {
    session: Arc<NetworkSession>,
    key: (WalletId, Pair),
    txid: String,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut in_flight = guard(&self.session.platform.payments.in_flight);
        if in_flight.get(&self.key) == Some(&self.txid) {
            in_flight.remove(&self.key);
        }
    }
}

/// Why [`NetworkSession::claim_payment`] refused.
pub(crate) enum ClaimRefused {
    /// Another payment to the pair is locked or under way.
    Locked(ContactError),
    /// The payment's address is no longer the pair's reserved one.
    Spent,
}

fn encode_inputs(inputs: &[OutPoint]) -> String {
    let inputs: Vec<String> = inputs.iter().map(ToString::to_string).collect();
    inputs.join(",")
}

fn decode_inputs(inputs: &str) -> Vec<OutPoint> {
    (inputs.split(',')).filter_map(|o| o.parse().ok()).collect()
}

/// The two ids of a call; the identity itself is `contact.self`.
pub(crate) fn pair_of(identity: &str, contact: &str) -> Result<Pair, ContactError> {
    let pair = (identifier(identity)?, identifier(contact)?);
    if pair.0 == pair.1 {
        return Err(ContactError::IsSelf);
    }
    Ok(pair)
}

impl NetworkSession {
    /// Loads the wallet's locks from app.sqlite into memory, where a lock
    /// held already (one whose row write failed or has not landed) stays.
    /// A read error keeps memory as it was; a row that does not parse is
    /// skipped.
    pub(crate) async fn load_payment_locks(&self, id: WalletId) {
        let wallet = id.to_string();
        let rows = match self.appdb_op(move |db| db.dp_payment_locks(&wallet)).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(wallet_id = %id, error = %e, "could not read the payment locks");
                return;
            }
        };
        let payments = &self.platform.payments;
        let mut locks = guard(&payments.locks);
        let wallet = locks.entry(id).or_default();
        for row in rows {
            let (Ok(pair), Ok(txid)) = (
                pair_of(&row.identity, &row.contact),
                row.txid.parse::<Txid>(),
            ) else {
                continue;
            };
            (guard(&payments.inputs).entry(txid)).or_insert_with(|| decode_inputs(&row.inputs));
            wallet.entry(pair).or_insert(PaymentLock {
                txid: row.txid,
                since: row.since,
            });
        }
    }

    /// The pair's lock, or its first broadcast under way, refused as
    /// `contact.payment_locked{txid}` unless it is `txid`'s own.
    pub(crate) fn check_payment_lock(
        &self,
        id: WalletId,
        pair: &Pair,
        txid: Option<&str>,
    ) -> Result<(), ContactError> {
        let in_flight = guard(&self.platform.payments.in_flight)
            .get(&(id, *pair))
            .cloned();
        let other = (self.platform.payments.get(&id, pair).map(|l| l.txid))
            .into_iter()
            .chain(in_flight)
            .find(|other| Some(other.as_str()) != txid);
        match other {
            Some(txid) => Err(ContactError::PaymentLocked { txid }),
            None => Ok(()),
        }
    }

    /// Claims `pair`'s reserved `address` for `txid`'s first broadcast
    /// (D7, D20): refused while another payment to the pair is locked or
    /// under way, or once the address went to another payment; else the
    /// address leaves the pair and the broadcast is under way until the
    /// returned guard drops. The lock is read under the in-flight map, so
    /// a payment settling meanwhile is seen either under way or locked.
    pub(crate) fn claim_payment(
        self: &Arc<Self>,
        id: WalletId,
        pair: Pair,
        txid: &str,
        address: &Address,
    ) -> Result<InFlight, ClaimRefused> {
        let payments = &self.platform.payments;
        let mut in_flight = guard(&payments.in_flight);
        let key = (id, pair);
        let other = (in_flight.get(&key).cloned())
            .into_iter()
            .chain(payments.get(&id, &pair).map(|l| l.txid))
            .find(|other| other != txid);
        if let Some(txid) = other {
            return Err(ClaimRefused::Locked(ContactError::PaymentLocked { txid }));
        }
        {
            let mut reserved = guard(&payments.reserved);
            if reserved.get(&key) != Some(address) {
                return Err(ClaimRefused::Spent);
            }
            reserved.remove(&key);
        }
        in_flight.insert(key, txid.to_owned());
        Ok(InFlight {
            session: Arc::clone(self),
            key,
            txid: txid.to_owned(),
        })
    }

    /// Locks `pair` behind `txid`, whose broadcast outcome is unknown
    /// (D8): memory first, then the row. A failed write is logged; the lock
    /// holds for the session. A lock of another payment is kept (the
    /// oldest unknown outcome; D20 lets no second one start).
    pub(crate) async fn lock_payment(
        &self,
        id: WalletId,
        pair: Pair,
        txid: Txid,
        inputs: Vec<OutPoint>,
    ) {
        let lock = PaymentLock {
            txid: txid.to_string(),
            since: crate::events::unix_now(),
        };
        {
            let mut locks = guard(&self.platform.payments.locks);
            let wallet = locks.entry(id).or_default();
            if let Some(held) = wallet.get(&pair) {
                // A repeat broadcast's unknown outcome: locked already.
                if held.txid != lock.txid {
                    tracing::warn!(wallet_id = %id, %txid, held = %held.txid,
                        "a second unknown payment to a locked contact");
                }
                return;
            }
            wallet.insert(pair, lock.clone());
        }
        let row = DpPaymentLockRow {
            identity: b58(&pair.0),
            contact: b58(&pair.1),
            txid: lock.txid,
            since: lock.since,
            inputs: encode_inputs(&inputs),
        };
        guard(&self.platform.payments.inputs).insert(txid, inputs);
        let wallet = id.to_string();
        let row = self
            .appdb_op(move |db| db.set_dp_payment_lock(&wallet, &row))
            .await;
        if let Err(e) = row {
            tracing::warn!(wallet_id = %id, %txid, error = %e, "could not store the payment lock");
        }
        self.signal_payments(id, &pair.0);
    }

    /// Clears `pair`'s lock if it is still `txid`'s.
    pub(crate) async fn clear_payment_lock(&self, id: WalletId, pair: Pair, txid: &str) {
        {
            let mut locks = guard(&self.platform.payments.locks);
            match locks.get_mut(&id) {
                Some(wallet) if wallet.get(&pair).is_some_and(|l| l.txid == txid) => {
                    wallet.remove(&pair);
                }
                // Every row is in memory (`load_payment_locks`).
                _ => return,
            }
        }
        if let Ok(parsed) = txid.parse::<Txid>() {
            guard(&self.platform.payments.inputs).remove(&parsed);
        }
        let (wallet, identity, contact, txid) =
            (id.to_string(), b58(&pair.0), b58(&pair.1), txid.to_owned());
        let row = self
            .appdb_op(move |db| db.clear_dp_payment_lock(&wallet, &identity, &contact, &txid))
            .await;
        if let Err(e) = row {
            tracing::warn!(wallet_id = %id, error = %e, "could not clear the payment lock");
        }
        self.signal_payments(id, &pair.0);
    }

    fn signal_payments(&self, id: WalletId, owner: &Identifier) {
        let identity = b58(owner);
        self.hub.pump.mark_platform(
            id,
            [
                PlatformChange::Payments {
                    identity: identity.clone(),
                },
                PlatformChange::Contacts { identity },
            ],
        );
    }

    /// `DashPay::payment_lock`: memory only.
    pub(crate) fn payment_lock_of(
        &self,
        id: WalletId,
        identity: &str,
        contact: &str,
    ) -> Result<Option<PaymentLock>, ContactError> {
        let _op = self.try_enter().map_err(PlatformError::from)?;
        self.require_wallet(&id).map_err(PlatformError::from)?;
        let pair = pair_of(identity, contact)?;
        Ok(self.platform.payments.get(&id, &pair))
    }

    /// `DashPay::resolve_payment_lock` (D9): `Sent` or `NotSent` on positive
    /// evidence only, which clears the lock; else `Unknown`.
    pub(crate) async fn resolve_payment_lock_of(
        &self,
        id: WalletId,
        identity: String,
        contact: String,
    ) -> Result<LockResolution, ContactError> {
        let _op = self.enter().await.map_err(PlatformError::from)?;
        self.require_wallet(&id).map_err(PlatformError::from)?;
        let pair = pair_of(&identity, &contact)?;
        let Some(lock) = self.platform.payments.get(&id, &pair) else {
            return Err(PlatformError::InvalidArgument {
                detail: "no payment lock for this contact".into(),
            }
            .into());
        };
        let txid: Txid = lock.txid.parse().map_err(|_| PlatformError::Internal {
            detail: format!("a payment lock names no txid: {}", lock.txid),
        })?;
        let resolution = self.lock_evidence(id, &txid);
        if resolution != LockResolution::Unknown {
            self.clear_payment_lock(id, pair, &lock.txid).await;
        }
        Ok(resolution)
    }

    /// E0-04 §16.6's evidence for the locked payment `txid`.
    fn lock_evidence(&self, id: WalletId, txid: &Txid) -> LockResolution {
        match self.leases.dispatch_status(id, ArtifactId::from_txid(txid)) {
            Some(DispatchState::Sent) => return LockResolution::Sent,
            Some(DispatchState::NotSent) => return LockResolution::NotSent,
            _ => {}
        }
        let history = self.hub.history.snapshot(&id);
        let ours = history.get(txid);
        // Network evidence only: the wallet records its own broadcast in
        // its mempool view before any peer took it (BL-76).
        if ours.is_some_and(|e| e.instant_locked || e.context != TransactionContext::Mempool) {
            return LockResolution::Sent;
        }
        let inputs: Vec<OutPoint> = match ours {
            Some(e) => e.tx.input.iter().map(|i| i.previous_output).collect(),
            None => guard(&self.platform.payments.inputs)
                .get(txid)
                .cloned()
                .unwrap_or_default(),
        };
        let conflicted = (history.iter())
            .filter(|(other, e)| {
                *other != txid && matches!(e.context, TransactionContext::InChainLockedBlock(_))
            })
            .any(|(_, e)| {
                e.tx.input
                    .iter()
                    .any(|i| inputs.contains(&i.previous_output))
            });
        if conflicted {
            LockResolution::NotSent
        } else {
            LockResolution::Unknown
        }
    }
}
