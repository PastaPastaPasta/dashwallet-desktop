//! The per-contact payment lock (DP3-01, `dp_payment_lock`; DASHPAY §2.3
//! step 8): a contact payment whose broadcast outcome is unknown locks its
//! (identity, contact) pair until positive evidence settles it. Held in
//! memory for the sync `payment_lock` read (m4 rule 3), mirrored to
//! app.sqlite, loaded with the wallet's identity choices. A payment's row
//! is written before its first hand-off (D23), so the row, not memory, is
//! the record of every payment that may have gone out. Also the pair's
//! first broadcast under way (D20) and its reserved address no broadcast
//! handed out yet (D21), both for the session.
//!
//! A contact payment from the Shielded or Platform balance (C-funding
//! D-C5) is a `dp_contact_payout` row, written before its source record;
//! while `pending` it is the pair's lock, as a `dp_payment_lock` row is.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use dashcore::{Address, OutPoint, ScriptBuf, Txid};
use dpp::prelude::Identifier;
use dw_appdb::{DpContactPayoutRow, DpPaymentLockRow, PairHolder};
use key_wallet::transaction_checking::TransactionContext;

use super::contacts_model::identifier;
use super::errors::{ContactError, PlatformError};
use super::flows::DispatchState;
use super::lease::ArtifactId;
use super::money_gate::Subject;
use super::payments::{
    ContactPayOutcome, ContactPayQuote, LockResolution, PaymentLock, PaymentSource, PayoutState,
};
use super::registration_flow::b58;
use super::runtime::guard;
use crate::send::check_payee;
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
    /// Each pair's reserved address that no broadcast handed out (D21),
    /// and whether rule 5's re-check passed when it was kept (DP3-04 §4).
    reserved: Mutex<HashMap<(WalletId, Pair), (Address, bool)>>,
    /// Each pair's address its last broadcast took off `reserved`, with its
    /// flag there, for [`Self::give_back`].
    taken: Mutex<HashMap<(WalletId, Pair), (Address, bool)>>,
    /// Each wallet's payout rows, for contact activity (P10–P12).
    payouts: Mutex<HashMap<WalletId, Vec<Payout>>>,
    /// Orders the lock changes (memory and row together): no clear's
    /// delete lands before an earlier write, nor a load between them
    /// (D23).
    rows: tokio::sync::Mutex<()>,
    /// Tests: holds the next contact settlement before it changes a lock.
    #[cfg(test)]
    pause_settle: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: the next payout stops after its row is written.
    #[cfg(test)]
    pub(crate) stop_after_row: std::sync::atomic::AtomicBool,
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
        guard(&self.taken).retain(|(w, _), _| w != id);
        guard(&self.payouts).remove(id);
    }

    /// The wallet's payout rows as last loaded.
    pub(crate) fn payouts(&self, id: &WalletId) -> Vec<Payout> {
        guard(&self.payouts).get(id).cloned().unwrap_or_default()
    }

    /// A restart: everything the session held in memory goes.
    #[cfg(test)]
    pub(crate) fn restart(&self) {
        guard(&self.locks).clear();
        guard(&self.inputs).clear();
        guard(&self.in_flight).clear();
        guard(&self.reserved).clear();
        guard(&self.taken).clear();
        guard(&self.payouts).clear();
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

    /// The pair's reserved address, if no broadcast handed it out, and
    /// whether rule 5's re-check passed when it was kept.
    pub(crate) fn reserved_checked(&self, id: WalletId, pair: Pair) -> Option<(Address, bool)> {
        guard(&self.reserved).get(&(id, pair)).cloned()
    }

    /// Keeps `address` as the pair's reserved one, unless it has one;
    /// `checked`: rule 5's re-check passed for it.
    pub(crate) fn keep(&self, id: WalletId, pair: Pair, address: Address, checked: bool) {
        guard(&self.reserved)
            .entry((id, pair))
            .or_insert((address, checked));
    }

    /// A broadcast that never went out gives `address` back to the pair,
    /// checked if rule 5's re-check had passed for it when the broadcast
    /// took it (H-09: a retry under rule 5 pays it again, rather than
    /// walking the contact's addresses).
    pub(crate) fn give_back(&self, id: WalletId, pair: Pair, address: Address) {
        let taken = guard(&self.taken).remove(&(id, pair));
        let checked = taken.is_some_and(|(a, checked)| a == address && checked);
        self.keep(id, pair, address, checked);
    }

    /// Orders a payout's lock change with every other (memory and row
    /// together, D23).
    pub(crate) async fn rows(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.rows.lock().await
    }

    /// `payout` as contact activity shows it, replacing the one with its
    /// reference.
    pub(crate) fn put_payout(&self, id: WalletId, payout: Payout) {
        let mut payouts = guard(&self.payouts);
        let rows = payouts.entry(id).or_default();
        match rows.iter_mut().find(|p| p.reference == payout.reference) {
            Some(held) => *held = payout,
            None => rows.push(payout),
        }
    }

    /// The state of payout `reference` as last loaded or written.
    pub(crate) fn payout_state(&self, id: &WalletId, reference: &str) -> Option<PayoutState> {
        (guard(&self.payouts).get(id)?.iter())
            .find(|p| p.reference == reference)
            .map(|p| p.state)
    }

    /// Payout `reference` settled as `state`; its lock goes. The pair it
    /// held, if memory had it.
    pub(crate) fn settle_payout(
        &self,
        id: WalletId,
        reference: &str,
        state: PayoutState,
    ) -> Option<Pair> {
        let mut pair = None;
        if let Some(p) = (guard(&self.payouts).get_mut(&id))
            .and_then(|rows| rows.iter_mut().find(|p| p.reference == reference))
        {
            p.state = state;
            pair = Some(p.pair);
        }
        if let Some(locks) = guard(&self.locks).get_mut(&id) {
            locks.retain(|held, lock| {
                let mine = lock.reference == reference && lock.source != PaymentSource::Core;
                if mine {
                    pair = Some(*held);
                }
                !mine
            });
        }
        pair
    }

    /// Locks `pair` behind payout `lock`, unless another lock holds it.
    pub(crate) fn lock_payout(&self, id: WalletId, pair: Pair, lock: PaymentLock) {
        (guard(&self.locks).entry(id).or_default())
            .entry(pair)
            .or_insert(lock);
    }

    /// Drops `address` as the pair's reserved one (DP3-04: it was taken
    /// from payee data that did not verify), so no later `prepare` pays it.
    pub(crate) fn drop_reserved(&self, id: WalletId, pair: Pair, address: &Address) {
        let mut reserved = guard(&self.reserved);
        if reserved.get(&(id, pair)).is_some_and(|(a, _)| a == address) {
            reserved.remove(&(id, pair));
        }
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

/// A `dp_contact_payout` row as contact activity reads it.
#[derive(Debug, Clone)]
pub(crate) struct Payout {
    pub(crate) pair: Pair,
    pub(crate) source: PaymentSource,
    pub(crate) reference: String,
    /// The paid address's script; `None` if it is not one of this network.
    pub(crate) script: Option<ScriptBuf>,
    pub(crate) expected_duffs: u64,
    pub(crate) state: PayoutState,
    pub(crate) since: u64,
}

/// A `dp_contact_payout.source` value.
fn row_source(source: &str) -> Option<PaymentSource> {
    match source {
        "shielded" => Some(PaymentSource::Shielded),
        "platform" => Some(PaymentSource::Platform),
        _ => None,
    }
}

impl Payout {
    fn of(row: &DpContactPayoutRow, network: dashcore::Network) -> Option<Self> {
        let source = row_source(&row.source)?;
        let state = match row.state.as_str() {
            "pending" => PayoutState::Pending,
            "sent" => PayoutState::Sent,
            "not_sent" => PayoutState::NotSent,
            _ => return None,
        };
        let script = (row.address.parse::<Address<_>>().ok())
            .and_then(|a| a.require_network(network).ok())
            .map(|a| a.script_pubkey());
        Some(Self {
            pair: pair_of(&row.identity, &row.contact).ok()?,
            source,
            reference: row.reference.clone(),
            script,
            expected_duffs: row.expected_duffs,
            state,
            since: row.since,
        })
    }
}

/// The settings key of the Platform move record `reference` names (P6).
pub(crate) fn move_key(reference: &str) -> String {
    format!("{}{reference}", super::platform_moves::MOVE_PREFIX)
}

/// `DashPay::contact_pay`'s terms.
pub(crate) struct PayCall {
    pub(crate) identity: String,
    pub(crate) contact: String,
    pub(crate) credits: u64,
    pub(crate) source: PaymentSource,
    pub(crate) note: Option<String>,
    pub(crate) grant: String,
    #[expect(dead_code, reason = "the shielded source's gate (C5a)")]
    pub(crate) confirm_unrecorded: bool,
}

/// The refusal of a payout's source before its slice lands (C5a, C5b);
/// `core`'s payment is the `TxDraft`'s.
pub(crate) fn payout_source(source: PaymentSource) -> PlatformError {
    let feature = match source {
        PaymentSource::Core => {
            return PlatformError::InvalidArgument {
                detail: "a Core payment to a contact is a TxDraft".into(),
            };
        }
        PaymentSource::Shielded => "shielded_contact_payment",
        PaymentSource::Platform => "platform_contact_payment",
    };
    PlatformError::FeatureOff {
        feature: feature.into(),
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
                    txid: row.txid.clone(),
                    since: row.since,
                    source: PaymentSource::Core,
                    reference: row.txid,
                });
                loaded.push((pair, txid));
            }
        }
        for (pair, txid) in loaded {
            if self.lock_evidence(id, &txid) != LockResolution::Unknown {
                self.clear_held(id, pair, &txid.to_string()).await;
            }
        }
        self.load_payouts(id).await;
        drop(_rows);
        super::contact_payout::settle_recorded(self, id).await;
    }

    /// [`Self::load_payment_locks`]' payouts (P6), under its `rows` guard:
    /// a `pending` row is the pair's lock while its source record exists or
    /// cannot be read; one with no source record was never signed, so it
    /// settles `not_sent` and its address goes back to the pair. A pair
    /// with a payment under way here is left to it.
    async fn load_payouts(&self, id: WalletId) {
        let wallet = id.to_string();
        let rows = match self
            .appdb_op(move |db| db.dp_contact_payouts(&wallet))
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(wallet_id = %id, error = %e, "could not read the contact payouts");
                return;
            }
        };
        let network = self.network.core_network();
        let payments = &self.platform.payments;
        let mut payouts = Vec::with_capacity(rows.len());
        for row in rows {
            let Some(mut payout) = Payout::of(&row, network) else {
                continue;
            };
            let pair = payout.pair;
            let under_way = guard(&payments.in_flight).contains_key(&(id, pair));
            if payout.state == PayoutState::Pending && !under_way {
                if self.payout_recorded(id, &row).await == Some(false) {
                    let (wallet, reference) = (id.to_string(), row.reference.clone());
                    let settled = self.appdb_op(move |db| {
                        db.settle_dp_contact_payout(&wallet, &reference, "not_sent")
                    });
                    match settled.await {
                        Ok(true) => {
                            payout.state = PayoutState::NotSent;
                            if let Ok(a) = row.address.parse::<Address<_>>()
                                && let Ok(address) = a.require_network(network)
                            {
                                payments.keep(id, pair, address, false);
                            }
                            self.signal_payments(id, &pair.0);
                        }
                        Ok(false) => {}
                        Err(e) => {
                            tracing::warn!(wallet_id = %id, error = %e, "could not settle a payout");
                        }
                    }
                }
                if payout.state == PayoutState::Pending {
                    (guard(&payments.locks).entry(id).or_default())
                        .entry(pair)
                        .or_insert(PaymentLock {
                            txid: row.reference.clone(),
                            since: row.since,
                            source: payout.source,
                            reference: row.reference,
                        });
                }
            }
            payouts.push(payout);
        }
        guard(&payments.payouts).insert(id, payouts);
    }

    /// Whether `row`'s source record exists (P6): a Platform move record
    /// with its reference, or a dispatch artifact with its `st_hash`.
    /// `None` when that cannot be read, and for a row this install did not
    /// write (no [`dw_appdb::dp_payout_marker`]: a restored backup carries
    /// the row but not its source record).
    async fn payout_recorded(&self, id: WalletId, row: &DpContactPayoutRow) -> Option<bool> {
        let scope = dw_appdb::local_scope(&id.to_string());
        let marker = dw_appdb::dp_payout_marker(&row.reference);
        let record = (row.source == "platform").then(|| move_key(&row.reference));
        let read = self.appdb_op(move |db| {
            let here = db.setting(&scope, &marker)?.is_some();
            let moved = match record {
                Some(key) => Some(db.setting(&scope, &key)?.is_some()),
                None => None,
            };
            Ok((here, moved))
        });
        let (here, moved) = read.await.ok()?;
        if !here {
            return None;
        }
        match row.source.as_str() {
            "platform" => moved,
            "shielded" => {
                let Some(hash) = (row.st_hash.as_deref())
                    .and_then(|h| <[u8; 32]>::try_from(hex::decode(h).ok()?).ok())
                else {
                    return Some(false);
                };
                self.leases.has_shielded_artifact(id, hash).await.ok()
            }
            _ => None,
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
            if reserved.get(&key).is_none_or(|(a, _)| a != address) {
                return Err(ClaimRefused::Spent);
            }
            if let Some(taken) = reserved.remove(&key) {
                guard(&payments.taken).insert(key, taken);
            }
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
                match db.set_dp_payment_lock_durable(&wallet, &row)? {
                    None => Ok(None),
                    Some(PairHolder::Core(txid)) => {
                        let rows = db.dp_payment_locks(&wallet)?;
                        let held = (rows.into_iter()).find(|r| r.txid == txid);
                        let (since, inputs) = held.map_or((row.since, Vec::new()), |h| {
                            (h.since, decode_inputs(&h.inputs))
                        });
                        Ok(Some((txid, since, PaymentSource::Core, inputs)))
                    }
                    // A pending payout holds the pair (P6).
                    Some(PairHolder::Payout {
                        reference,
                        source,
                        since,
                    }) => {
                        let source = row_source(&source).ok_or_else(|| {
                            dw_appdb::AppDbError::Corrupt("dp_contact_payout.source".into())
                        })?;
                        Ok(Some((reference, since, source, Vec::new())))
                    }
                }
            })
            .await?;
        let Some((txid, since, source, inputs)) = held else {
            return Ok(());
        };
        if let Ok(parsed) = txid.parse::<Txid>()
            && source == PaymentSource::Core
        {
            (guard(&self.platform.payments.inputs).entry(parsed)).or_insert(inputs);
        }
        guard(&self.platform.payments.locks)
            .entry(id)
            .or_default()
            .entry(pair)
            .or_insert(PaymentLock {
                txid: txid.clone(),
                since,
                source,
                reference: txid.clone(),
            });
        self.signal_payments(id, &pair.0);
        Err(ContactError::PaymentLocked { txid }.into())
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
            source: PaymentSource::Core,
            reference: txid.to_string(),
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

    pub(crate) fn signal_payments(&self, id: WalletId, owner: &Identifier) {
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
        // A payout settles on its source's evidence (C5a, C5b).
        if lock.source == PaymentSource::Platform {
            return super::contact_payout::resolve(self, id, pair, &lock.reference).await;
        }
        if lock.source != PaymentSource::Core {
            return Ok(LockResolution::Unknown);
        }
        let txid: Txid = lock.txid.parse().map_err(|_| PlatformError::Internal {
            detail: format!("a payment lock names no txid: {}", lock.txid),
        })?;
        let resolution = self.lock_evidence(id, &txid);
        if resolution != LockResolution::Unknown {
            self.clear_payment_lock(id, pair, &lock.txid).await;
        }
        Ok(resolution)
    }

    /// A payout's wallet, pair and amount.
    fn payout_terms(
        &self,
        id: WalletId,
        identity: &str,
        contact: &str,
        credits: u64,
        source: PaymentSource,
    ) -> Result<Pair, ContactError> {
        self.require_wallet(&id).map_err(PlatformError::from)?;
        if source == PaymentSource::Core {
            return Err(payout_source(source).into());
        }
        let pair = pair_of(identity, contact)?;
        if credits == 0 {
            return Err(PlatformError::InvalidArgument {
                detail: "the amount is zero".into(),
            }
            .into());
        }
        Ok(pair)
    }

    /// `DashPay::contact_pay_quote`: the payee and the pair's lock, then the
    /// source (C-funding P1).
    pub(crate) async fn contact_pay_quote_of(
        &self,
        id: WalletId,
        identity: String,
        contact: String,
        credits: u64,
        source: PaymentSource,
    ) -> Result<ContactPayQuote, ContactError> {
        let _op = self.enter().await.map_err(PlatformError::from)?;
        let pair = self.payout_terms(id, &identity, &contact, credits, source)?;
        check_payee(self.payee_state(id, &pair)?, true)?;
        self.check_payment_lock(id, &pair, None)?;
        if source == PaymentSource::Platform
            && super::contact_payout::refuse_on_mainnet(self).is_ok()
        {
            return super::contact_payout::quote_live(self, id, credits).await;
        }
        Err(payout_source(source).into())
    }

    /// `DashPay::contact_pay` (C-funding P2) up to the source's gates: the
    /// live payee, the pair's lock and rule 5 on the payee, before the grant
    /// is redeemed or an address taken.
    pub(crate) async fn contact_pay_of(
        self: &Arc<Self>,
        id: WalletId,
        call: PayCall,
    ) -> Result<ContactPayOutcome, ContactError> {
        let _op = self.enter().await.map_err(PlatformError::from)?;
        if call.source == PaymentSource::Platform
            && super::contact_payout::refuse_on_mainnet(self).is_ok()
        {
            return super::contact_payout::pay_live(self, id, call).await;
        }
        self.pay_gates(id, &call).await?;
        Err(payout_source(call.source).into())
    }

    /// [`Self::contact_pay_of`]'s gates before any source's (C-funding P2):
    /// the arguments, the live payee, the pair's lock and rule 5 on the
    /// payee. Nothing is taken: a refusal keeps the grant and every address.
    pub(crate) async fn pay_gates(
        &self,
        id: WalletId,
        call: &PayCall,
    ) -> Result<Pair, ContactError> {
        let pair =
            self.payout_terms(id, &call.identity, &call.contact, call.credits, call.source)?;
        if call.grant.trim().is_empty() {
            return Err(PlatformError::InvalidArgument {
                detail: "grant is empty".into(),
            }
            .into());
        }
        check_payee(self.payee_state_live(id, &pair).await?, false)?;
        self.check_payment_lock(id, &pair, None)?;
        (self.money_gate(Subject::Payee(id, pair)).await)
            .map_err(|refusal| ContactError::Platform(refusal.platform()))?;
        Ok(pair)
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
