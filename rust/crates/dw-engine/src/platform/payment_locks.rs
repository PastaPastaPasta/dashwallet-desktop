//! The per-contact payment lock (DP3-01, `dp_payment_lock`; DASHPAY §2.3
//! step 8): a contact payment whose broadcast outcome is unknown locks its
//! (identity, contact) pair until positive evidence settles it. Held in
//! memory for the sync `payment_lock` read (m4 rule 3), mirrored to
//! app.sqlite, loaded with the wallet's identity choices. A payment's row
//! is written before its first hand-off (D23), so the row, not memory, is
//! the record of every payment that may have gone out. Also the pair's
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
use crate::{EngineError, NetworkSession, PlatformChange, WalletId};

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
    /// Orders the lock changes (memory and row together): no clear's
    /// delete lands before an earlier write, nor a load between them
    /// (D23).
    rows: tokio::sync::Mutex<()>,
    /// Tests: holds the next contact settlement before it changes a lock.
    #[cfg(test)]
    pause_settle: Mutex<Option<Arc<crate::coins::TestPause>>>,
}

impl PaymentLocks {
    pub(crate) fn get(&self, id: &WalletId, pair: &Pair) -> Option<PaymentLock> {
        guard(&self.locks).get(id)?.get(pair).cloned()
    }

    /// Memory only: every lock's row was written before its payment's
    /// hand-off (D23), so the next load reads it back.
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

    /// Tests: holds the next contact settlement until `pause` is released.
    #[cfg(test)]
    pub(crate) fn pause_settle(&self, pause: Arc<crate::coins::TestPause>) {
        *guard(&self.pause_settle) = Some(pause);
    }

    #[cfg(test)]
    pub(crate) fn settle_pause(&self) -> Option<Arc<crate::coins::TestPause>> {
        guard(&self.pause_settle).take()
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
    /// held already stays, then settles each loaded one that has positive
    /// evidence (D9, D23): a payment that may have gone out before a restart
    /// or an unload is locked until it is `Sent` or provably `NotSent`. The
    /// row of a payment whose first broadcast is under way here is left to
    /// that broadcast's settlement. A read error keeps memory as it was; a
    /// row that does not parse is skipped.
    pub(crate) async fn load_payment_locks(&self, id: WalletId) {
        let _rows = self.platform.payments.rows.lock().await;
        let wallet = id.to_string();
        let rows = match self.appdb_op(move |db| db.dp_payment_locks(&wallet)).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(wallet_id = %id, error = %e, "could not read the payment locks");
                return;
            }
        };
        let payments = &self.platform.payments;
        let mut loaded = Vec::new();
        {
            let in_flight = guard(&payments.in_flight);
            let mut locks = guard(&payments.locks);
            let wallet = locks.entry(id).or_default();
            for row in rows {
                let (Ok(pair), Ok(txid)) = (
                    pair_of(&row.identity, &row.contact),
                    row.txid.parse::<Txid>(),
                ) else {
                    continue;
                };
                if in_flight.get(&(id, pair)) == Some(&row.txid) {
                    continue;
                }
                (guard(&payments.inputs).entry(txid)).or_insert_with(|| decode_inputs(&row.inputs));
                wallet.entry(pair).or_insert(PaymentLock {
                    txid: row.txid,
                    since: row.since,
                });
                loaded.push((pair, txid));
            }
        }
        for (pair, txid) in loaded {
            if self.lock_evidence(id, &txid) != LockResolution::Unknown {
                self.clear_held(id, pair, &txid.to_string()).await;
            }
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

    /// Writes `txid`'s lock row for `pair` before its first hand-off (D23),
    /// synced against power loss (D27): what a restart or a reload reads
    /// back as the pair's lock, so no payment that may have gone out lives
    /// in memory only. Refused, and
    /// nothing written, while another payment's row holds the pair
    /// (`contact.payment_locked`; that row becomes the pair's lock in memory,
    /// for `payment_lock` and `resolve_payment_lock`, should memory have
    /// lost it) or when the write or its sync fails: the hand-off must not
    /// happen.
    /// Memory is otherwise left to the outcome (`lock_payment`).
    pub(crate) async fn record_payment(
        &self,
        id: WalletId,
        pair: Pair,
        txid: &str,
        inputs: &[OutPoint],
    ) -> Result<(), EngineError> {
        let _rows = self.platform.payments.rows.lock().await;
        let row = DpPaymentLockRow {
            identity: b58(&pair.0),
            contact: b58(&pair.1),
            txid: txid.to_owned(),
            since: crate::events::unix_now(),
            inputs: encode_inputs(inputs),
        };
        let wallet = id.to_string();
        let held = self
            .appdb_op(move |db| {
                if db.set_dp_payment_lock_durable(&wallet, &row)?.is_none() {
                    return Ok(None);
                }
                let rows = db.dp_payment_locks(&wallet)?;
                Ok((rows.into_iter())
                    .find(|r| r.identity == row.identity && r.contact == row.contact))
            })
            .await?;
        let Some(held) = held else {
            return Ok(());
        };
        if let Ok(parsed) = held.txid.parse::<Txid>() {
            (guard(&self.platform.payments.inputs).entry(parsed))
                .or_insert_with(|| decode_inputs(&held.inputs));
        }
        guard(&self.platform.payments.locks)
            .entry(id)
            .or_default()
            .entry(pair)
            .or_insert(PaymentLock {
                txid: held.txid.clone(),
                since: held.since,
            });
        self.signal_payments(id, &pair.0);
        Err(ContactError::PaymentLocked { txid: held.txid }.into())
    }

    /// Locks `pair` behind `txid`, whose broadcast outcome is unknown
    /// (D8): memory, then the row again (its first write was the
    /// payment's record, D23; this one is idempotent). A failed write is
    /// logged. A lock of another payment is kept (the oldest unknown
    /// outcome; D20 lets no second one start).
    pub(crate) async fn lock_payment(
        &self,
        id: WalletId,
        pair: Pair,
        txid: Txid,
        inputs: Vec<OutPoint>,
    ) {
        let _rows = self.platform.payments.rows.lock().await;
        let mut lock = PaymentLock {
            txid: txid.to_string(),
            since: crate::events::unix_now(),
        };
        {
            let mut locks = guard(&self.platform.payments.locks);
            let wallet = locks.entry(id).or_default();
            match wallet.get(&pair) {
                None => {
                    wallet.insert(pair, lock.clone());
                }
                // A repeat broadcast's unknown outcome: locked already.
                Some(held) if held.txid == lock.txid => lock.since = held.since,
                Some(held) => {
                    tracing::warn!(wallet_id = %id, %txid, held = %held.txid,
                        "a second unknown payment to a locked contact");
                    return;
                }
            }
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
        match self
            .appdb_op(move |db| db.set_dp_payment_lock(&wallet, &row))
            .await
        {
            Ok(None) => {}
            Ok(Some(held)) => {
                tracing::warn!(wallet_id = %id, %txid, %held, "another payment's row holds the contact");
            }
            Err(e) => {
                tracing::warn!(wallet_id = %id, %txid, error = %e, "could not store the payment lock");
            }
        }
        self.signal_payments(id, &pair.0);
    }

    /// Clears `pair`'s lock, in memory and on disk, where it is still
    /// `txid`'s: its payment was accepted, or is provably unsent.
    pub(crate) async fn clear_payment_lock(&self, id: WalletId, pair: Pair, txid: &str) {
        let _rows = self.platform.payments.rows.lock().await;
        self.clear_held(id, pair, txid).await;
    }

    /// [`Self::clear_payment_lock`] under the caller's `rows` guard. The
    /// row goes even if memory never held it (a payment whose first
    /// broadcast did not hand it off, D23).
    async fn clear_held(&self, id: WalletId, pair: Pair, txid: &str) {
        let held = {
            let mut locks = guard(&self.platform.payments.locks);
            let wallet = locks.get_mut(&id);
            match wallet {
                Some(wallet) if wallet.get(&pair).is_some_and(|l| l.txid == txid) => {
                    wallet.remove(&pair);
                    true
                }
                _ => false,
            }
        };
        if let Ok(parsed) = txid.parse::<Txid>() {
            guard(&self.platform.payments.inputs).remove(&parsed);
        }
        let (wallet, identity, contact, txid) =
            (id.to_string(), b58(&pair.0), b58(&pair.1), txid.to_owned());
        let row = self
            .appdb_op(move |db| db.clear_dp_payment_lock(&wallet, &identity, &contact, &txid))
            .await;
        let cleared = match row {
            Ok(cleared) => cleared,
            Err(e) => {
                tracing::warn!(wallet_id = %id, error = %e, "could not clear the payment lock");
                false
            }
        };
        if held || cleared {
            self.signal_payments(id, &pair.0);
        }
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
