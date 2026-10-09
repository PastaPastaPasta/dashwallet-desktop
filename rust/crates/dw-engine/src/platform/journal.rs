//! The changeset tap (DASHPAY §2.7, §3.5; ROADMAP E0-06). Every changeset
//! the library stores goes through [`crate::store::WalletStore`], which hands
//! it here: it becomes debounced `EngineEvent::Platform` signals, `dp_events`
//! journal rows and, while the trusted-quorum fallback is in use (§2.2),
//! `dp_trust_unverified` rows.
//!
//! - Signals go through the event pump's `Platform` domain (≤ 4 Hz per
//!   domain, trailing edge kept). Hosts re-query; rows are never pushed.
//! - Journal writes are `INSERT OR IGNORE` on (identity, kind, contact, ref),
//!   so the library's whole-snapshot changesets, seen again and again, add
//!   each event once. Contact events are once per relationship: a rotated
//!   request (new `$createdAt`) is not news.
//! - Catch-up silence (DEC-114). While a recovery phase runs, every event
//!   of the wallet is stored read: a restore's bring-up, the discovery of
//!   identities, the bring-up queued after it and the names pass
//!   ([`ChangesetTap::catch_up`], and the recovery mark that carries a
//!   recovery from one phase to the next). Outside them, an event is stored
//!   read when its authoritative time predates this installation's first
//!   bring-up of the wallet ([`CATCH_UP_BEFORE_KEY`]): a request's
//!   `$createdAt`, a payment's confirmed block time, a name's marketplace
//!   row time, all read from the persister ([`Times`]). Local fetch or
//!   observation times are never ages. An event with no authoritative time
//!   (an unconfirmed payment, a name with no row) is news; an old one that
//!   first surfaces outside every recovery phase may show unread once
//!   (declared residual, cosmetic).
//! - Trust flags fail closed: while the fallback is in use they are written
//!   before the SQLite persister sees the changeset, and a failure refuses
//!   the store (`Transient` when the database is busy, nothing applied), so
//!   no entity is ever stored unflagged. A store the persister then refuses
//!   leaves a flag behind: over-flagging only costs a re-verification.
//! - The journal and the signals follow the persister's acceptance, so a
//!   refused changeset leaves no event. The journal is display data: a
//!   failure to write it is logged, never returned to the library.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dashcore::{OutPoint, Txid};
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::util::strings::convert_to_homograph_safe_chars;
use dw_appdb::{AppDb, AppDbError, JournalEntry, UnverifiedEntity};
use key_wallet::account::AccountType;
use platform_wallet::changeset::{Merge, PlatformWalletChangeSet, PlatformWalletPersistence};
use platform_wallet::wallet::identity::{PaymentDirection, PaymentStatus};
use platform_wallet::{ContactRequest, DpnsNameInfo};

use super::notifications::EventKind;
use super::runtime::guard;
use crate::events::{SessionHub, unix_now};
use crate::{NetworkSession, PlatformChange, WalletId};

/// The wallet's local setting (not exported to a `.dwbackup`): when this
/// installation first brought the wallet up, UNIX seconds. What happened
/// before is catch-up.
pub(crate) const CATCH_UP_BEFORE_KEY: &str = "dashpay.catch_up_before";

/// `dp_trust_unverified.kind` values (dw-appdb migration `dashpay`).
const UNVERIFIED_IDENTITY: &str = "identity";
const UNVERIFIED_REQUEST: &str = "contact_request";
const UNVERIFIED_LABEL: &str = "dpns_label";

/// The `dp_events.kind` text: `EventKind`'s serde name.
pub(crate) fn kind_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::UsernameRegistered => "username_registered",
        EventKind::ContestWon => "contest_won",
        EventKind::ContestLost => "contest_lost",
        EventKind::ContestLocked => "contest_locked",
        EventKind::RequestReceived => "request_received",
        EventKind::RequestAccepted => "request_accepted",
        EventKind::ContactEstablished => "contact_established",
        EventKind::PaymentReceived => "payment_received",
    }
}

/// Kinds whose row for the same contact makes an event no news: one
/// "sent you a request" per contact and none once established, one
/// "accepted" or "is now your contact" per relationship.
fn exclusive_with(kind: EventKind) -> &'static [&'static str] {
    const ESTABLISHED: &[&str] = &["request_accepted", "contact_established"];
    const ANY_CONTACT: &[&str] = &[
        "request_received",
        "request_accepted",
        "contact_established",
    ];
    match kind {
        EventKind::RequestReceived => ANY_CONTACT,
        EventKind::RequestAccepted | EventKind::ContactEstablished => ESTABLISHED,
        _ => &[],
    }
}

/// The journal `ref` of a name event: the label, then after `@` what tells
/// two holdings of it apart, so a name that leaves and comes back is a new
/// event (DPNS labels have no `@`): when it was acquired (ms) for
/// `UsernameRegistered`, the contest's end for the contest kinds. DP1-04
/// writes contest outcomes with the same text.
pub(crate) fn name_ref(label: &str, stamp: Option<u64>) -> String {
    match stamp {
        Some(stamp) => format!("{label}@{stamp}"),
        None => label.to_string(),
    }
}

/// The journal `ref` of a request event: the request's `$createdAt` (ms),
/// which names it among the contact's requests (the library's
/// `ContactRequest` carries no document id).
fn request_ref(request: &ContactRequest) -> String {
    request.created_at.to_string()
}

fn b58(id: &Identifier) -> String {
    id.to_string(Encoding::Base58)
}

/// How old what an event reports is, for catch-up (DEC-114).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Age {
    /// The source's authoritative time, UNIX ms.
    At(u64),
    /// No authoritative time: news outside a recovery phase.
    Unknown,
    /// The companion of an event that notifies (a won contest's username):
    /// stored read whatever its time.
    Shadowed,
}

/// Where the tap reads authoritative times (DEC-114): the persister, which
/// has accepted the changeset by the time the tap records it.
pub(crate) trait Times {
    /// The block time (s) of the wallet's confirmed transaction.
    fn block_time(&self, wallet: WalletId, txid: &Txid) -> Option<u32>;
    /// When the identity acquired the name on Platform (ms): its marketplace
    /// row's transfer, else creation time.
    fn name_time(&self, wallet: WalletId, identity: &Identifier, normalized: &str) -> Option<u64>;
}

impl<P: PlatformWalletPersistence + ?Sized> Times for P {
    fn block_time(&self, wallet: WalletId, txid: &Txid) -> Option<u32> {
        match self.get_core_tx_record(wallet.0, txid) {
            Ok(record) => Some(record?.context.block_info()?.timestamp()),
            Err(e) => {
                tracing::warn!(wallet_id = %wallet, error = %e, "could not read a transaction's time");
                None
            }
        }
    }

    fn name_time(&self, wallet: WalletId, identity: &Identifier, normalized: &str) -> Option<u64> {
        match self.get_dpns_name_state(wallet.0, identity, normalized) {
            Ok(row) => row.and_then(|r| r.transferred_at_ms.or(r.created_at_ms)),
            Err(e) => {
                tracing::warn!(wallet_id = %wallet, error = %e, "could not read a name's time");
                None
            }
        }
    }
}

/// One journal event before its read state is known.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Event {
    identity: String,
    kind: EventKind,
    contact: String,
    reference: String,
    age: Age,
}

/// What a changeset means for DashPay, before the lookups that finish it
/// (the registration a lock funds, the contests being watched, transaction
/// times).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Classified {
    changes: BTreeSet<PlatformChange>,
    /// Tracked asset locks touched: each signals its registration.
    locks: BTreeSet<OutPoint>,
    /// Contact and payment events.
    events: Vec<Event>,
    /// The DPNS names of the wallet's own identities, by identity.
    names: BTreeMap<Identifier, Vec<DpnsNameInfo>>,
    unverified: BTreeSet<UnverifiedEntity>,
}

fn identity_entity(id: &Identifier) -> UnverifiedEntity {
    UnverifiedEntity {
        kind: UNVERIFIED_IDENTITY,
        key: b58(id),
    }
}

fn label_entity(label: &str) -> UnverifiedEntity {
    UnverifiedEntity {
        kind: UNVERIFIED_LABEL,
        key: convert_to_homograph_safe_chars(label),
    }
}

fn request_entity(request: &ContactRequest) -> UnverifiedEntity {
    UnverifiedEntity {
        kind: UNVERIFIED_REQUEST,
        key: format!(
            "{}:{}:{}",
            b58(&request.sender_id),
            b58(&request.recipient_id),
            request.created_at
        ),
    }
}

/// Classifies a changeset by the §3.5 table. `None` when it touches nothing
/// DashPay (Core-only changesets, the common case). `unverified` lists every
/// Platform entity it carries, used only while the fallback is in use.
pub(crate) fn classify(cs: &PlatformWalletChangeSet) -> Option<Classified> {
    let mut c = Classified::default();

    // `identities`: Identities; username registered, contest outcome.
    if let Some(ids) = cs.identities.as_ref().filter(|i| !i.is_empty()) {
        c.changes.insert(PlatformChange::Identities);
        // A tombstone touches its identity too.
        c.unverified.extend(ids.removed.iter().map(identity_entity));
        for (id, entry) in &ids.identities {
            c.unverified.insert(identity_entity(id));
            let labels = (entry.dpns_names.iter().map(|n| n.label.as_str()))
                .chain(entry.contested_dpns_names.iter().map(String::as_str));
            c.unverified.extend(labels.map(label_entity));
            // Names of identities the wallet only watches are not news.
            if entry.identity_index.is_some() && !entry.dpns_names.is_empty() {
                c.names.insert(*id, entry.dpns_names.clone());
            }
        }
    }
    // Keys and profiles are identity data too.
    if let Some(keys) = cs.identity_keys.as_ref().filter(|k| !k.is_empty()) {
        c.changes.insert(PlatformChange::Identities);
        let touched = keys.upserts.keys().chain(&keys.removed);
        c.unverified
            .extend(touched.map(|(id, _)| identity_entity(id)));
    }
    if let Some(profiles) = cs.dashpay_profiles.as_ref().filter(|p| !p.is_empty()) {
        c.changes.insert(PlatformChange::Identities);
        c.unverified.extend(profiles.keys().map(identity_entity));
    }

    // `contacts`: received, established, and the local edits (alias, note,
    // hidden ride `established`; ignored senders their own sets).
    if let Some(contacts) = cs.contacts.as_ref().filter(|c| !c.is_empty()) {
        let owners: BTreeSet<Identifier> = (contacts.sent_requests.keys().map(|k| k.owner_id))
            .chain(contacts.removed_sent.iter().map(|k| k.owner_id))
            .chain(contacts.incoming_requests.keys().map(|k| k.owner_id))
            .chain(contacts.removed_incoming.iter().map(|k| k.owner_id))
            .chain(contacts.established.keys().map(|k| k.owner_id))
            .chain((contacts.ignored.iter().chain(&contacts.unignored)).map(|(owner, _)| *owner))
            .collect();
        c.changes.extend(
            owners
                .iter()
                .map(|o| PlatformChange::Contacts { identity: b58(o) }),
        );
        c.unverified.extend(
            contacts
                .sent_requests
                .values()
                .map(|e| request_entity(&e.request)),
        );
        for (key, entry) in &contacts.incoming_requests {
            c.unverified.insert(request_entity(&entry.request));
            c.events.push(Event {
                identity: b58(&key.owner_id),
                kind: EventKind::RequestReceived,
                contact: b58(&key.sender_id),
                reference: request_ref(&entry.request),
                age: Age::At(entry.request.created_at),
            });
        }
        for (key, contact) in &contacts.established {
            let (outgoing, incoming) = (&contact.outgoing_request, &contact.incoming_request);
            c.unverified.insert(request_entity(outgoing));
            c.unverified.insert(request_entity(incoming));
            // Ours came first: they accepted it. Theirs came first: we did.
            let kind = if outgoing.created_at < incoming.created_at {
                EventKind::RequestAccepted
            } else {
                EventKind::ContactEstablished
            };
            c.events.push(Event {
                identity: b58(&key.owner_id),
                kind,
                contact: b58(&key.recipient_id),
                reference: request_ref(incoming),
                // When the relationship was made: the later request.
                age: Age::At(outgoing.created_at.max(incoming.created_at)),
            });
        }
    }

    // `asset_locks`: the registration the lock funds.
    if let Some(locks) = cs.asset_locks.as_ref().filter(|l| !l.is_empty()) {
        c.locks.extend(locks.asset_locks.keys().copied());
        c.locks.extend(locks.removed.iter().copied());
    }

    // `dashpay_payments_overlay`: every payment write rides it (the library
    // funnels them through one method), received ones are journaled.
    if let Some(overlay) = &cs.dashpay_payments_overlay {
        for (identity, payments) in overlay.iter().filter(|(_, p)| !p.is_empty()) {
            let identity = b58(identity);
            c.changes.insert(PlatformChange::Payments {
                identity: identity.clone(),
            });
            for (txid, payment) in payments {
                if payment.direction == PaymentDirection::Received
                    && payment.status != PaymentStatus::Failed
                {
                    c.events.push(Event {
                        identity: identity.clone(),
                        kind: EventKind::PaymentReceived,
                        contact: b58(&payment.counterparty_id),
                        reference: txid.clone(),
                        age: Age::Unknown,
                    });
                }
            }
        }
    }

    // `account_registrations`: contact accounts.
    for entry in &cs.account_registrations {
        if let AccountType::DashpayReceivingFunds {
            user_identity_id, ..
        }
        | AccountType::DashpayExternalAccount {
            user_identity_id, ..
        } = entry.account_type
        {
            c.changes.insert(PlatformChange::Contacts {
                identity: b58(&Identifier::from(user_identity_id)),
            });
        }
    }

    // `dpns_name_states`: Names. The rows carry their normalized label.
    if let Some(states) = cs.dpns_name_states.as_ref().filter(|s| !s.is_empty()) {
        c.changes.insert(PlatformChange::Names);
        c.unverified
            .extend(states.names.values().map(|e| UnverifiedEntity {
                kind: UNVERIFIED_LABEL,
                key: e.normalized_label.clone(),
            }));
    }

    (!c.changes.is_empty() || !c.locks.is_empty()).then_some(c)
}

/// The session's changeset tap: where classified changesets go.
pub(crate) struct ChangesetTap {
    hub: Arc<SessionHub>,
    appdb: Arc<AppDb>,
    /// Wallets whose restore passes are running, with how many hold them.
    catch_up: Mutex<HashMap<WalletId, usize>>,
    /// Wallets in a recovery that spans tasks, with the phases it still owes:
    /// from a restore's bring-up or a discovery that found identities until
    /// the bring-up queued after a discovery and the names pass are done.
    recovering: Mutex<HashMap<WalletId, Owed>>,
    /// [`CATCH_UP_BEFORE_KEY`] per wallet, UNIX seconds, once known.
    catch_up_before: Mutex<HashMap<WalletId, u64>>,
    /// The trusted-quorum fallback is in use (E0-10b sets it).
    trust_fallback: AtomicBool,
}

/// A phase a recovery hands on to (DEC-114).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// The bring-up queued after a discovery.
    BringUp,
    /// The names pass (DP1-05 pass 2).
    Names,
}

/// The phases a wallet's recovery still owes.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Owed {
    bring_up: bool,
    names: bool,
}

/// Ends a [`Phase::BringUp`] when dropped, on any path out of a bring-up.
pub(crate) struct BringUpPhase {
    tap: Arc<ChangesetTap>,
    id: WalletId,
}

impl Drop for BringUpPhase {
    fn drop(&mut self) {
        self.tap.end_phase(&self.id, Phase::BringUp);
    }
}

/// Holds a wallet in catch-up until dropped ([`ChangesetTap::catch_up`]).
pub(crate) struct CatchUp {
    tap: Arc<ChangesetTap>,
    id: WalletId,
}

impl Drop for CatchUp {
    fn drop(&mut self) {
        let mut catch_up = guard(&self.tap.catch_up);
        if let Some(n) = catch_up.get_mut(&self.id) {
            *n -= 1;
            if *n == 0 {
                catch_up.remove(&self.id);
            }
        }
    }
}

impl ChangesetTap {
    pub(crate) fn new(hub: Arc<SessionHub>, appdb: Arc<AppDb>) -> Self {
        Self {
            hub,
            appdb,
            catch_up: Mutex::new(HashMap::new()),
            recovering: Mutex::new(HashMap::new()),
            catch_up_before: Mutex::new(HashMap::new()),
            trust_fallback: AtomicBool::new(false),
        }
    }

    /// Stores the wallet's events read, with no OS notification, until the
    /// guard drops (catch-up silence, DASHPAY §2.7). Held across a restore's
    /// passes (`bringup.rs`).
    pub(crate) fn catch_up(self: &Arc<Self>, id: WalletId) -> CatchUp {
        *guard(&self.catch_up).entry(id).or_default() += 1;
        CatchUp {
            tap: Arc::clone(self),
            id,
        }
    }

    fn in_catch_up(&self, id: &WalletId) -> bool {
        guard(&self.catch_up).contains_key(id) || guard(&self.recovering).contains_key(id)
    }

    /// Marks `id` recovering until the phases it hands on are done: its
    /// names pass, and with `bring_up` the bring-up queued after a
    /// discovery. Its events are stored read meanwhile.
    pub(crate) fn begin_recovery(&self, id: WalletId, bring_up: bool) {
        let mut all = guard(&self.recovering);
        let owed = all.entry(id).or_default();
        owed.names = true;
        owed.bring_up |= bring_up;
    }

    /// One owed phase is done; the recovery ends with the last.
    pub(crate) fn end_phase(&self, id: &WalletId, phase: Phase) {
        let mut all = guard(&self.recovering);
        if let Some(owed) = all.get_mut(id) {
            match phase {
                Phase::BringUp => owed.bring_up = false,
                Phase::Names => owed.names = false,
            }
            if !owed.bring_up && !owed.names {
                all.remove(id);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn is_recovering(&self, id: &WalletId) -> bool {
        guard(&self.recovering).contains_key(id)
    }

    /// Ends `id`'s recovery whatever it owes (removal, unload, rollback).
    pub(crate) fn end_recovery(&self, id: &WalletId) {
        guard(&self.recovering).remove(id);
    }

    /// While on, every entity a changeset touches gets a
    /// `dp_trust_unverified` row (§2.2 rule 2).
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "E0-10b's quorum provider sets it")
    )]
    pub(crate) fn set_trust_fallback(&self, on: bool) {
        self.trust_fallback.store(on, Ordering::SeqCst);
    }

    /// The wallet's [`CATCH_UP_BEFORE_KEY`], from the cache or the database;
    /// `None` when unknown (logged when it could not be read).
    fn catch_up_before(&self, id: WalletId) -> Option<u64> {
        self.load_catch_up_before(id).unwrap_or_else(|e| {
            tracing::warn!(wallet_id = %id, error = %e, "could not read the catch-up time");
            None
        })
    }

    /// `Ok(None)` only when nothing is on file.
    fn load_catch_up_before(&self, id: WalletId) -> Result<Option<u64>, String> {
        if let Some(at) = guard(&self.catch_up_before).get(&id) {
            return Ok(Some(*at));
        }
        let scope = dw_appdb::local_scope(&id.to_string());
        let Some(text) = (self.appdb)
            .setting(&scope, CATCH_UP_BEFORE_KEY)
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        let at: u64 = text.parse().map_err(|_| format!("not a time: {text:?}"))?;
        guard(&self.catch_up_before).insert(id, at);
        Ok(Some(at))
    }

    /// Records when this installation first brought `id` up, unless it is
    /// on file: what happened before is catch-up. A value that cannot be
    /// read is left alone (moving the boundary later would silence real
    /// events). Blocking (app.sqlite).
    pub(crate) fn note_first_bring_up(&self, id: WalletId) {
        match self.load_catch_up_before(id) {
            Ok(None) => {}
            Ok(Some(_)) => return,
            Err(e) => {
                tracing::warn!(wallet_id = %id, error = %e, "could not read the catch-up time");
                return;
            }
        }
        let scope = dw_appdb::local_scope(&id.to_string());
        let now = unix_now();
        match self
            .appdb
            .set_setting(&scope, CATCH_UP_BEFORE_KEY, Some(&now.to_string()))
        {
            Ok(()) => {
                guard(&self.catch_up_before).insert(id, now);
            }
            Err(e) => {
                tracing::warn!(wallet_id = %id, error = %e, "could not store the catch-up time");
            }
        }
    }

    /// Forgets a removed wallet's cached catch-up time: the same seed
    /// imported again is a new first bring-up.
    pub(crate) fn forget(&self, id: &WalletId) {
        guard(&self.catch_up_before).remove(id);
        self.end_recovery(id);
    }

    /// Flags what `c` touches while the fallback is in use. Runs before the
    /// persister takes the changeset: an error refuses the store.
    pub(crate) fn flag(&self, id: WalletId, c: &Classified) -> Result<(), AppDbError> {
        if c.unverified.is_empty() || !self.trust_fallback.load(Ordering::SeqCst) {
            return Ok(());
        }
        let unverified: Vec<UnverifiedEntity> = c.unverified.iter().cloned().collect();
        self.appdb
            .flag_unverified(&id.to_string(), &unverified, unix_now())
    }

    /// Finishes and records a changeset the persister accepted, reading
    /// authoritative times from `times`.
    pub(crate) fn record(&self, id: WalletId, mut c: Classified, times: &dyn Times) {
        let wallet = id.to_string();
        let now = unix_now();
        let mut changes = std::mem::take(&mut c.changes);
        for outpoint in &c.locks {
            let draft = self
                .appdb
                .registration_for_lock(&wallet, &outpoint.to_string())
                .unwrap_or_else(|e| {
                    tracing::warn!(wallet_id = %id, error = %e, "could not read the registrations");
                    None
                });
            changes.insert(PlatformChange::Registration {
                draft: draft.map(|d| d.to_string()),
            });
        }

        let mut events = std::mem::take(&mut c.events);
        for e in events
            .iter_mut()
            .filter(|e| e.kind == EventKind::PaymentReceived)
        {
            // The confirmed transaction's block time.
            let block = Txid::from_str(&e.reference)
                .ok()
                .and_then(|txid| times.block_time(id, &txid));
            e.age = block.map_or(Age::Unknown, |s| Age::At(u64::from(s) * 1000));
        }
        events.extend(self.name_events(id, std::mem::take(&mut c.names), times));

        let silent = self.in_catch_up(&id);
        let before_ms = (!silent && !events.is_empty())
            .then(|| self.catch_up_before(id))
            .flatten()
            .map(|s| s.saturating_mul(1000));
        let history = |age: Age| match age {
            Age::At(at) => before_ms.is_some_and(|before| at < before),
            Age::Shadowed => true,
            Age::Unknown => false,
        };
        let entries: Vec<JournalEntry> = events
            .into_iter()
            .map(|e| JournalEntry {
                read_at: (silent || history(e.age)).then_some(now),
                identity: e.identity,
                kind: kind_name(e.kind),
                contact: e.contact,
                reference: e.reference,
                unless_any: exclusive_with(e.kind),
            })
            .collect();

        if !entries.is_empty()
            && let Err(e) = self.appdb.record_events(&wallet, &entries, now)
        {
            tracing::warn!(wallet_id = %id, error = %e, "could not write the DashPay journal");
        }
        if !changes.is_empty() {
            self.hub.pump.mark_platform(id, changes);
        }
    }

    /// The name events. A name won in a watched contest is `ContestWon`,
    /// and its `UsernameRegistered` is history, so the win is one
    /// notification and a later snapshot of the name adds none. Lost and
    /// locked outcomes need Platform's answer: DP1-04's contest watch
    /// writes them, and removes the watch row when the contest resolves (a
    /// row left behind would call a later purchase of the label a win).
    /// A name's age is its marketplace row time, never its `acquired_at`
    /// (a fetch's wall clock on some library paths).
    fn name_events(
        &self,
        id: WalletId,
        names: BTreeMap<Identifier, Vec<DpnsNameInfo>>,
        times: &dyn Times,
    ) -> Vec<Event> {
        let wallet = id.to_string();
        let wallet = wallet.as_str();
        let mut out = Vec::new();
        for (identity_id, names) in names {
            let identity = b58(&identity_id);
            let watches: Vec<(String, u64)> = self
                .appdb
                .contest_watches(wallet, &identity)
                .unwrap_or_else(|e| {
                    tracing::warn!(wallet_id = wallet, error = %e, "could not read the contest watches");
                    Vec::new()
                })
                .into_iter()
                .map(|(label, ends_at)| (convert_to_homograph_safe_chars(&label), ends_at))
                .collect();
            let event = |kind, reference, age| Event {
                identity: identity.clone(),
                kind,
                contact: String::new(),
                reference,
                age,
            };
            for name in names {
                let safe = convert_to_homograph_safe_chars(&name.label);
                let won = watches.iter().find(|(label, _)| *label == safe);
                let mut age = times
                    .name_time(id, &identity_id, &safe)
                    .map_or(Age::Unknown, Age::At);
                if let Some((_, ends_at)) = won {
                    out.push(event(
                        EventKind::ContestWon,
                        name_ref(&name.label, Some(*ends_at)),
                        age,
                    ));
                    age = Age::Shadowed;
                }
                out.push(event(
                    EventKind::UsernameRegistered,
                    name_ref(&name.label, name.acquired_at),
                    age,
                ));
            }
        }
        out
    }
}

impl NetworkSession {
    /// Holds `id` in catch-up while a recovery phase runs: `None` after
    /// close.
    pub(super) fn catch_up(&self, id: WalletId) -> Option<CatchUp> {
        Some(self.live().ok()?.tap.catch_up(id))
    }

    /// [`ChangesetTap::begin_recovery`]: a recovery goes on in later phases.
    pub(super) fn begin_recovery(&self, id: WalletId, bring_up: bool) {
        if let Ok(live) = self.live() {
            live.tap.begin_recovery(id, bring_up);
        }
    }

    /// [`ChangesetTap::end_phase`].
    pub(super) fn end_phase(&self, id: &WalletId, phase: Phase) {
        if let Ok(live) = self.live() {
            live.tap.end_phase(id, phase);
        }
    }

    /// Ends the owed bring-up phase of `id` when the guard drops.
    pub(super) fn bring_up_phase(&self, id: WalletId) -> Option<BringUpPhase> {
        let tap = self.live().ok()?.tap;
        Some(BringUpPhase { tap, id })
    }

    /// [`ChangesetTap::end_recovery`].
    pub(crate) fn end_recovery(&self, id: &WalletId) {
        if let Ok(live) = self.live() {
            live.tap.end_recovery(id);
        }
    }

    /// [`ChangesetTap::note_first_bring_up`] off the caller's thread.
    pub(super) async fn note_first_bring_up(&self, id: WalletId) {
        let Ok(live) = self.live() else {
            return;
        };
        let tap = live.tap;
        if let Err(e) = tokio::task::spawn_blocking(move || tap.note_first_bring_up(id)).await {
            tracing::warn!(wallet_id = %id, error = %e, "could not store the catch-up time");
        }
    }
}
