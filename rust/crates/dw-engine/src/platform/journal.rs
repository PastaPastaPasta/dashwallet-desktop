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
//! - Catch-up silence: an event is stored read when it is history, in two
//!   ways. While a restore's passes run ([`ChangesetTap::catch_up`]), every
//!   event of the wallet is. And whatever happened before this installation
//!   first brought the wallet up ([`CATCH_UP_BEFORE_KEY`], persisted, so a
//!   restore cut off by a budget, a lock or a kill stays silent) is history
//!   whenever it arrives: a request by its `$createdAt`, a payment by its
//!   transaction's time, a name the library found with no acquisition time
//!   (discovery's) or one acquired before then.
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
use platform_wallet::changeset::{Merge, PlatformWalletChangeSet};
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

/// How old what an event reports is, for catch-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Age {
    /// The source's own time, UNIX ms.
    At(u64),
    /// Known to be history (a name discovery found, with no time).
    History,
    /// Not known yet (a payment whose transaction is not in view).
    Unknown,
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
    /// The DPNS names of the wallet's own identities, by identity (Base58).
    names: BTreeMap<String, Vec<DpnsNameInfo>>,
    /// Block times (s) of the transactions in the changeset.
    tx_times: HashMap<Txid, u32>,
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
                c.names.insert(b58(id), entry.dpns_names.clone());
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
        // The transactions often ride the same round (the event bridge).
        if let Some(core) = &cs.core {
            c.tx_times.extend(
                core.records
                    .iter()
                    .filter_map(|r| Some((r.txid, r.context.block_info()?.timestamp()))),
            );
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
    /// [`CATCH_UP_BEFORE_KEY`] per wallet, UNIX seconds, once known.
    catch_up_before: Mutex<HashMap<WalletId, u64>>,
    /// The trusted-quorum fallback is in use (E0-10b sets it).
    trust_fallback: AtomicBool,
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
        guard(&self.catch_up).contains_key(id)
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

    /// The age of a payment: its transaction's block time, else its time in
    /// the history (`None` while unconfirmed and unseen: news).
    fn payment_age(&self, id: &WalletId, c: &Classified, txid: &str) -> Age {
        let Ok(txid) = Txid::from_str(txid) else {
            return Age::Unknown;
        };
        let secs = match c.tx_times.get(&txid) {
            Some(t) => Some(u64::from(*t)),
            None => self
                .hub
                .history
                .get(id, &txid)
                .and_then(|e| crate::history::timestamp_of(&e)),
        };
        secs.map_or(Age::Unknown, |s| Age::At(s.saturating_mul(1000)))
    }

    /// Finishes and records a changeset the persister accepted.
    pub(crate) fn record(&self, id: WalletId, mut c: Classified) {
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
            e.age = self.payment_age(&id, &c, &e.reference);
        }
        events.extend(self.name_events(&wallet, std::mem::take(&mut c.names)));

        let silent = self.in_catch_up(&id);
        let before_ms = (!silent && !events.is_empty())
            .then(|| self.catch_up_before(id))
            .flatten()
            .map(|s| s.saturating_mul(1000));
        let history = |age: Age| match age {
            Age::At(at) => before_ms.is_some_and(|before| at < before),
            Age::History => true,
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
    fn name_events(&self, wallet: &str, names: BTreeMap<String, Vec<DpnsNameInfo>>) -> Vec<Event> {
        let mut out = Vec::new();
        for (identity, names) in names {
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
                // Discovery stores the names it finds with no time.
                let mut age = name.acquired_at.map_or(Age::History, Age::At);
                if let Some((_, ends_at)) = won {
                    out.push(event(
                        EventKind::ContestWon,
                        name_ref(&name.label, Some(*ends_at)),
                        age,
                    ));
                    age = Age::History;
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
    /// Holds `id` in catch-up while a restore pass runs: `None` after close.
    pub(super) fn catch_up(&self, id: WalletId) -> Option<CatchUp> {
        Some(self.live().ok()?.tap.catch_up(id))
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
