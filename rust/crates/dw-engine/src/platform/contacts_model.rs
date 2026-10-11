//! The contacts read model (DP2-01; DASHPAY §3.1 `contacts.rs`, F10, F13).
//! Not part of the facade: `contacts.rs` holds the records and the calls.
//!
//! - **Source.** platform-wallet's memory, the state its DashPay sync pass
//!   keeps: per owned identity the incoming and sent requests, the
//!   established contacts with their private details, the ignored senders,
//!   the cached contact profiles, the deferred contact-crypto queue and the
//!   DashPay accounts of the Core wallet. Each read takes it under `try_read`
//!   and keeps it as the wallet's snapshot; while a writer holds the lock
//!   (a pass applying its results, SPV processing a block) the last snapshot
//!   answers and the page says `updating`, and once the writer is done the
//!   snapshot is rebuilt and `Platform{Contacts}` signalled. Hosts also
//!   re-query on the changeset tap's `Platform{Contacts}`, `Identities` and
//!   `Payments` signals (E0-06).
//! - **States.** One relation per contact and owner, by precedence:
//!   established, then ignored, then incoming, then outgoing (the library
//!   establishes a pair as soon as both requests exist, and ignoring a sender
//!   drops its request, so overlaps are transient; an ignored contact stays
//!   a contact). Established contacts with `hidden` set, and ignored
//!   senders, are the Hidden section.
//! - **Times** are authoritative only (DEC-114): a request's `$createdAt`,
//!   a payment's block time (an unconfirmed one has none yet). A restore
//!   that replays the same contacts sorts them the same way, and nothing
//!   here writes the journal or makes news (catch-up silence is the tap's,
//!   DEC-125/135/139).
//! - **Provenance** is positive (DEC-125, DEC-183): a row is `unverified`
//!   when any Platform datum it shows or relies on, as the library holds it
//!   now, has no verification record of that content once the trusted
//!   fallback was used. The classes are the contact's profile and username
//!   as shown (the label's digest binds the spelling and the contact), and
//!   each contact request the relation rests on. A contact's identity has no
//!   class: its verified request binds the keys and the xpub the channel
//!   uses (m4 §5). E0-10b's re-fetch writes the records; the library keeps
//!   no contact usernames, so each one shown is reported to it
//!   (`Provenance::show_label`).
//! - **Rotation.** A contact's `since` is the later of its two requests, and
//!   the library replaces a request in place when either side rotates its
//!   key: the contact then dates from the rotation (declared residual).
//! - **Usernames** are not kept by the library: `contact_names.rs` fetches
//!   them in the background and signals `Contacts` when one lands.

use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use dashcore::Txid;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use key_wallet::account::account_collection::DashpayAccountKey;
use key_wallet::transaction_checking::TransactionContext;
use platform_wallet::changeset::PendingContactCryptoKind;
use platform_wallet::wallet::identity::PaymentStatus;
use platform_wallet::{ContactRequest, ManagedIdentity, PlatformWalletInfo};
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use super::contact_names::ContactNames;
use super::contacts::{
    ChannelState, ContactDetail, ContactQuery, ContactSection, ContactSectionPage, ContactSort,
    ContactSummary, ContactsPage, PrivateDetails, PublishState, Relation,
};
use super::errors::{ContactError, IdentityError, PlatformError};
use super::profile::Profile;
use super::provenance::Entity;
use super::recovery::profile_of;
use super::reverify::{Held, ShownProfile};
use super::runtime::guard;
use crate::{EngineError, NetworkSession, PlatformChange, WalletId};

/// The sections in the order a page lists them when the query names none.
const ALL_SECTIONS: [ContactSection; 4] = [
    ContactSection::Requests,
    ContactSection::Contacts,
    ContactSection::Pending,
    ContactSection::Hidden,
];

/// The deferred ops that build a contact's payment accounts: the "unlock to
/// finish setting up" work (the library's own banner count,
/// `count_account_build_ops`, adds `AutoAccept`, which holds a request).
const ACCOUNT_OPS: [PendingContactCryptoKind; 2] = [
    PendingContactCryptoKind::RegisterReceiving,
    PendingContactCryptoKind::RegisterExternal,
];

/// How much of an id a search needs before it matches ids by their start.
const ID_PREFIX_MIN: usize = 4;

/// One contact request as the read model keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Request {
    pub(super) sender: Identifier,
    pub(super) recipient: Identifier,
    /// `$createdAt`, UNIX ms.
    pub(super) created_at: u64,
}

impl From<&ContactRequest> for Request {
    fn from(r: &ContactRequest) -> Self {
        Self {
            sender: r.sender_id,
            recipient: r.recipient_id,
            created_at: r.created_at,
        }
    }
}

/// One contact of one owned identity, before the joins of a read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) contact: Identifier,
    pub(super) relation: Relation,
    pub(super) channel: ChannelState,
    /// Waiting for an unlock: its channel is `SetupPending`, or an
    /// auto-accept of its request is queued.
    pub(super) setup_pending: bool,
    pub(super) outgoing: Option<Request>,
    pub(super) incoming: Option<Request>,
    pub(super) private: PrivateDetails,
    pub(super) profile: Option<Profile>,
    /// What the row shows or rests on besides the username, as provenance
    /// sees it: the shown profile, and each contact request the relation
    /// rests on (none for an ignored sender).
    pub(super) entities: Vec<Entity>,
    /// Its DashPay payments (not failed), either way; none for an ignored
    /// sender.
    pub(super) payments: Vec<Txid>,
}

/// One owned identity's contacts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct OwnerContacts {
    pub(super) entries: Vec<Entry>,
}

impl OwnerContacts {
    fn established(&self) -> usize {
        (self.entries.iter())
            .filter(|e| e.relation == Relation::Contact)
            .count()
    }

    fn get(&self, contact: &Identifier) -> Option<&Entry> {
        self.entries.iter().find(|e| e.contact == *contact)
    }
}

/// A wallet's contacts, by owned identity (every one, with or without
/// contacts).
pub(super) type WalletContacts = BTreeMap<Identifier, OwnerContacts>;

/// The wallet's contacts from the library's state.
pub(super) fn wallet_contacts(info: &PlatformWalletInfo, id: &WalletId) -> WalletContacts {
    let accounts = &info.core_wallet.accounts.dashpay_external_accounts;
    (info.identity_manager.wallet_managed_identities(&id.0))
        .map(|m| {
            let has_external = |contact: &Identifier| {
                accounts.contains_key(&DashpayAccountKey {
                    index: 0,
                    user_identity_id: m.identity.id().to_buffer(),
                    friend_identity_id: contact.to_buffer(),
                })
            };
            (m.identity.id(), owner_contacts(m, &has_external))
        })
        .collect()
}

/// One owned identity's contacts. `has_external(contact)`: the Core wallet
/// holds the DashPay account that pays them (the one the library builds).
pub(super) fn owner_contacts(
    m: &ManagedIdentity,
    has_external: &dyn Fn(&Identifier) -> bool,
) -> OwnerContacts {
    let dp = m.dashpay();
    let queued = |contact: &Identifier, kinds: &[PendingContactCryptoKind]| {
        (dp.pending_contact_crypto.iter())
            .any(|p| p.contact_id == *contact && kinds.contains(&p.op.kind()))
    };
    let entry = |contact: Identifier, relation: Relation, requests: &[&ContactRequest]| {
        let cached = (dp.contact_profiles.get(&contact)).and_then(|e| e.profile.as_ref());
        let profile = cached.map(|p| Held::Profile {
            id: contact,
            shown: ShownProfile::of(p),
        });
        let requests = requests.iter().map(|r| Held::ContactRequest((*r).clone()));
        Entry {
            contact,
            relation,
            channel: ChannelState::NotEstablished,
            setup_pending: false,
            outgoing: None,
            incoming: None,
            private: PrivateDetails::default(),
            profile: cached.map(profile_of),
            entities: profile
                .into_iter()
                .chain(requests)
                .map(|h| h.entity())
                .collect(),
            payments: Vec::new(),
        }
    };

    // Lowest precedence first: a later state of the same contact replaces it.
    let mut entries: BTreeMap<Identifier, Entry> = BTreeMap::new();
    for (recipient, request) in dp.sent_contact_requests() {
        let e = Entry {
            outgoing: Some(request.into()),
            ..entry(*recipient, Relation::PendingOutgoing, &[request])
        };
        entries.insert(*recipient, e);
    }
    for (sender, request) in dp.incoming_contact_requests() {
        let e = Entry {
            incoming: Some(request.into()),
            setup_pending: queued(sender, &[PendingContactCryptoKind::AutoAccept]),
            ..entry(*sender, Relation::PendingIncoming, &[request])
        };
        entries.insert(*sender, e);
    }
    for sender in dp.ignored_senders() {
        entries.insert(*sender, entry(*sender, Relation::Ignored, &[]));
    }
    for (contact, c) in dp.established_contacts() {
        // As the library sees it: it builds the paying account when it is
        // missing and rebuilds it when built from an older request
        // (`external_account_needs_rebuild`), both with the signer.
        let channel = if c.payment_channel_broken {
            ChannelState::Broken
        } else if queued(contact, &ACCOUNT_OPS)
            || !has_external(contact)
            || c.external_account_reference != Some(c.incoming_request.account_reference)
        {
            ChannelState::SetupPending
        } else {
            ChannelState::Ready
        };
        let e = Entry {
            channel,
            setup_pending: channel == ChannelState::SetupPending,
            outgoing: Some((&c.outgoing_request).into()),
            incoming: Some((&c.incoming_request).into()),
            private: PrivateDetails {
                alias: c.alias.clone(),
                note: c.note.clone(),
                hidden: c.is_hidden,
            },
            ..entry(
                *contact,
                Relation::Contact,
                &[&c.outgoing_request, &c.incoming_request],
            )
        };
        entries.insert(*contact, e);
    }

    for (txid, payment) in &dp.payments {
        let Ok(txid) = Txid::from_str(txid) else {
            continue;
        };
        if payment.status != PaymentStatus::Failed
            && let Some(e) = entries.get_mut(&payment.counterparty_id)
            && e.relation != Relation::Ignored
        {
            e.payments.push(txid);
        }
    }
    OwnerContacts {
        entries: entries.into_values().collect(),
    }
}

/// What a read joins to the library's state.
pub(super) struct Join<'a> {
    /// The contact's username, when fetched.
    pub(super) username: &'a dyn Fn(&Identifier) -> Option<String>,
    /// `Provenance::is_unverified`.
    pub(super) unverified: &'a dyn Fn(&Entity) -> bool,
    /// When a transaction happened, UNIX seconds.
    pub(super) tx_time: &'a dyn Fn(&Txid) -> Option<u64>,
}

fn b58(id: &Identifier) -> String {
    id.to_string(Encoding::Base58)
}

impl Entry {
    /// When the relation began, UNIX ms: the request it rests on, or for a
    /// contact the later of the two (when the second one made it).
    fn since_ms(&self) -> Option<u64> {
        match self.relation {
            Relation::Contact => Some(self.outgoing?.created_at.max(self.incoming?.created_at)),
            Relation::PendingIncoming => Some(self.incoming?.created_at),
            Relation::PendingOutgoing => Some(self.outgoing?.created_at),
            Relation::Ignored | Relation::Stranger | Relation::IsSelf => None,
        }
    }

    /// Whether anything the row shows (`username` included) or rests on is
    /// unverified.
    fn unverified(&self, username: Option<&str>, join: &Join) -> bool {
        let label = username.map(|name| Held::label(name, self.contact).entity());
        (self.entities.iter())
            .chain(&label)
            .any(|e| (join.unverified)(e))
    }

    fn summary(&self, join: &Join) -> ContactSummary {
        let username = (join.username)(&self.contact);
        let since = self.since_ms().map(|ms| ms / 1000);
        let paid = (self.payments.iter())
            .filter_map(|t| (join.tx_time)(t))
            .max();
        ContactSummary {
            contact: b58(&self.contact),
            unverified: self.unverified(username.as_deref(), join),
            username,
            display_name: self.profile.as_ref().and_then(|p| p.display_name.clone()),
            alias: self.private.alias.clone(),
            relation: self.relation,
            hidden: self.private.hidden,
            channel: self.channel,
            since,
            last_activity_at: paid.max(since),
        }
    }

    fn section(&self) -> ContactSection {
        match self.relation {
            Relation::PendingIncoming => ContactSection::Requests,
            Relation::PendingOutgoing => ContactSection::Pending,
            Relation::Ignored => ContactSection::Hidden,
            _ if self.private.hidden => ContactSection::Hidden,
            _ => ContactSection::Contacts,
        }
    }
}

/// The name a row shows, folded for comparison: alias, display name,
/// username (the first that is not blank), else the id.
fn shown_name(s: &ContactSummary) -> String {
    [&s.alias, &s.display_name, &s.username]
        .into_iter()
        .flatten()
        .find(|v| !v.trim().is_empty())
        .unwrap_or(&s.contact)
        .to_lowercase()
}

/// Known values first, in `T`'s order; unknown ones last.
fn known_first<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (a, b) => b.is_some().cmp(&a.is_some()),
    }
}

fn compare(sort: ContactSort, a: &ContactSummary, b: &ContactSummary) -> Ordering {
    let by_name = || shown_name(a).cmp(&shown_name(b));
    let username = |s: &ContactSummary| s.username.as_deref().map(str::to_lowercase);
    match sort {
        ContactSort::DisplayName => by_name(),
        ContactSort::Username => known_first(username(a), username(b)),
        // Newest first.
        ContactSort::DateAdded => known_first(a.since.map(Reverse), b.since.map(Reverse)),
        ContactSort::LastActivity => known_first(
            a.last_activity_at.map(Reverse),
            b.last_activity_at.map(Reverse),
        ),
    }
    .then_with(|| a.contact.cmp(&b.contact))
}

/// `text` folded, against the username, display name and alias, or as the
/// start of the contact's id once it is `ID_PREFIX_MIN` characters long.
fn matches(s: &ContactSummary, text: &str) -> bool {
    let folded = text.to_lowercase();
    (text.chars().count() >= ID_PREFIX_MIN && s.contact.starts_with(text))
        || [&s.username, &s.display_name, &s.alias]
            .into_iter()
            .flatten()
            .any(|v| v.to_lowercase().contains(&folded))
}

/// The page for `q`: the sections it names (all when none, each once), in
/// its order. Requests and Pending are newest first whatever the sort;
/// Contacts and Hidden follow `q.sort`.
pub(super) fn page(
    owner: &OwnerContacts,
    q: &ContactQuery,
    join: &Join,
    updating: bool,
) -> ContactsPage {
    let text = q.text.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let asked = if q.sections.is_empty() {
        &ALL_SECTIONS[..]
    } else {
        &q.sections
    };
    let mut wanted: Vec<ContactSection> = Vec::new();
    for s in asked {
        if !wanted.contains(s) {
            wanted.push(*s);
        }
    }
    let sections = wanted
        .into_iter()
        .map(|section| {
            let sort = match section {
                ContactSection::Requests | ContactSection::Pending => ContactSort::DateAdded,
                ContactSection::Contacts | ContactSection::Hidden => q.sort,
            };
            let mut contacts: Vec<ContactSummary> = (owner.entries.iter())
                .filter(|e| e.section() == section)
                .map(|e| e.summary(join))
                .filter(|s| text.is_none_or(|t| matches(s, t)))
                .collect();
            contacts.sort_by(|a, b| compare(sort, a, b));
            ContactSectionPage { section, contacts }
        })
        .collect();
    ContactsPage { sections, updating }
}

/// One contact in full.
pub(super) fn detail(
    owner: &OwnerContacts,
    contact: &Identifier,
    join: &Join,
) -> Option<ContactDetail> {
    let e = owner.get(contact)?;
    let at = |r: Option<Request>| r.map(|r| r.created_at / 1000);
    Some(ContactDetail {
        summary: e.summary(join),
        profile: e.profile.clone(),
        private_details: e.private.clone(),
        // Whether a `contactInfo` was published is DP2-04's to record.
        publish_state: if e.relation == Relation::Contact && owner.established() < 2 {
            PublishState::DeferredUntilTwoContacts
        } else {
            PublishState::Local
        },
        request_sent_at: at(e.outgoing),
        request_received_at: at(e.incoming),
        // DP3-01's `dp_payment_lock`.
        payment_lock: None,
    })
}

/// The session's contacts state: each wallet's last snapshot, the wallets
/// waiting for a writer to finish, the usernames, and the background tasks
/// (refreshes, username batches) that close ends.
#[derive(Default)]
pub(crate) struct ContactsRuntime {
    snapshots: Arc<Mutex<BTreeMap<WalletId, Arc<WalletContacts>>>>,
    refreshing: Arc<Mutex<BTreeSet<WalletId>>>,
    pub(super) names: ContactNames,
    tasks: Mutex<Tasks>,
}

#[derive(Default)]
struct Tasks {
    running: Vec<JoinHandle<()>>,
    closed: bool,
}

impl ContactsRuntime {
    pub(crate) fn forget(&self, id: &WalletId) {
        guard(&self.snapshots).remove(id);
        guard(&self.refreshing).remove(id);
        self.names.forget(id);
    }

    /// Runs `task` on `rt` until it ends or the session closes; once closed
    /// it never runs (a read racing the close).
    pub(super) fn spawn(&self, rt: &Handle, task: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = guard(&self.tasks);
        if !tasks.closed {
            tasks.running.retain(|t| !t.is_finished());
            tasks.running.push(rt.spawn(task));
        }
    }

    /// Session close (DASHPAY §3.2: nothing runs while closed): ends every
    /// background task and waits for it, so no fetch lands afterwards, and
    /// refuses new ones. A username batch cut short lands what it owed as
    /// failed.
    pub(crate) async fn close(&self) {
        let running = {
            let mut tasks = guard(&self.tasks);
            tasks.closed = true;
            std::mem::take(&mut tasks.running)
        };
        for task in running {
            task.abort();
            if let Err(e) = task.await
                && !e.is_cancelled()
            {
                tracing::warn!(error = %e, "a contacts task ended abnormally");
            }
        }
    }
}

pub(super) fn identifier(text: &str) -> Result<Identifier, ContactError> {
    Identifier::from_string(text, Encoding::Base58).map_err(|_| {
        PlatformError::InvalidArgument {
            detail: "not a Base58 identity id".into(),
        }
        .into()
    })
}

impl NetworkSession {
    /// The wallet's contacts, live; else, while a writer holds the
    /// wallet's state, the last snapshot if any (`true`). A stale read
    /// queues one wait per wallet for the writer to finish, which rebuilds
    /// the snapshot and signals `Platform{Contacts}` for each of the
    /// wallet's identities (`Identities` when it has none), so the host's
    /// re-query is live.
    fn read_contacts(
        &self,
        id: WalletId,
    ) -> Result<(Option<Arc<WalletContacts>>, bool), EngineError> {
        let manager = self.manager()?;
        let wm = manager.wallet_manager_arc();
        let state = &self.platform.contacts;
        if let Ok(wm) = wm.try_read() {
            let contacts = Arc::new(
                (wm.get_wallet_info(&id.0))
                    .map(|info| wallet_contacts(info, &id))
                    .unwrap_or_default(),
            );
            // Under the read guard: no writer commits in between, so an
            // older read cannot replace a newer snapshot.
            guard(&state.snapshots).insert(id, Arc::clone(&contacts));
            return Ok((Some(contacts), false));
        }
        if guard(&state.refreshing).insert(id) {
            let (snapshots, refreshing) =
                (Arc::clone(&state.snapshots), Arc::clone(&state.refreshing));
            let hub = Arc::clone(&self.hub);
            state.spawn(&self.rt, async move {
                let identities: Vec<String> = {
                    let wm = wm.read().await;
                    match wm.get_wallet_info(&id.0) {
                        Some(info) => {
                            let contacts = wallet_contacts(info, &id);
                            let identities = contacts.keys().map(b58).collect();
                            guard(&snapshots).insert(id, Arc::new(contacts));
                            identities
                        }
                        // Removed meanwhile.
                        None => Vec::new(),
                    }
                };
                guard(&refreshing).remove(&id);
                // A wallet without identities still wakes the wallet-wide
                // reader (`pending_setup_count`).
                let changes: Vec<PlatformChange> = if identities.is_empty() {
                    vec![PlatformChange::Identities]
                } else {
                    (identities.into_iter())
                        .map(|identity| PlatformChange::Contacts { identity })
                        .collect()
                };
                hub.pump.mark_platform(id, changes);
            });
        }
        Ok((guard(&state.snapshots).get(&id).cloned(), true))
    }

    /// Runs `f` over `identity`'s contacts with the read's joins. A live
    /// read that lacks the identity is `identity.not_found`; a stale one
    /// passes `None`.
    fn with_contacts<R>(
        &self,
        id: WalletId,
        identity: &str,
        f: impl FnOnce(Option<&OwnerContacts>, &Join, bool) -> Result<R, ContactError>,
    ) -> Result<R, ContactError> {
        let _op = self.try_enter().map_err(PlatformError::from)?;
        self.require_wallet(&id).map_err(PlatformError::from)?;
        let owner = identifier(identity)?;
        let (contacts, updating) = self.read_contacts(id).map_err(PlatformError::from)?;
        let owned = contacts.as_ref().and_then(|c| c.get(&owner));
        if owned.is_none() && !updating {
            return Err(PlatformError::from(IdentityError::NotFound).into());
        }
        let provenance = self.live().map_err(PlatformError::from)?.provenance;
        let names = &self.platform.contacts.names;
        let unverified = |e: &Entity| provenance.is_unverified(e);
        // Block times only (DEC-114): an unconfirmed payment has none yet.
        let tx_time = |txid: &Txid| match self.hub.history.get(&id, txid)?.context {
            TransactionContext::InBlock(b) | TransactionContext::InChainLockedBlock(b) => {
                Some(u64::from(b.timestamp()))
            }
            _ => None,
        };
        let username = |c: &Identifier| names.label(c);
        let join = Join {
            username: &username,
            unverified: &unverified,
            tx_time: &tx_time,
        };
        let result = f(owned, &join, updating);
        // After the read: it shows the names it had, whenever a fetch lands.
        if let Some(owned) = owned {
            self.fetch_contact_names(id, identity, owned.entries.iter().map(|e| e.contact));
        }
        result
    }

    /// `DashPay::contacts`.
    pub(super) fn contacts_page(
        &self,
        id: WalletId,
        identity: String,
        q: ContactQuery,
    ) -> Result<ContactsPage, ContactError> {
        self.with_contacts(id, &identity, |owned, join, updating| {
            let none = OwnerContacts::default();
            Ok(page(owned.unwrap_or(&none), &q, join, updating))
        })
    }

    /// `DashPay::contact`. Stale without the identity: `platform.unavailable`
    /// (not "unknown"), until the refresh's signal.
    pub(super) fn contact_detail(
        &self,
        id: WalletId,
        identity: String,
        contact: String,
    ) -> Result<Option<ContactDetail>, ContactError> {
        self.with_contacts(id, &identity, |owned, join, _| {
            let contact = identifier(&contact)?;
            let owned = owned.ok_or(PlatformError::Unavailable)?;
            Ok(detail(owned, &contact, join))
        })
    }

    /// `DashPay::pending_setup_count`: over every identity of the wallet.
    /// Stale with no snapshot: `platform.unavailable`.
    pub(super) fn pending_setup(&self, id: WalletId) -> Result<u32, ContactError> {
        let _op = self.try_enter().map_err(PlatformError::from)?;
        self.require_wallet(&id).map_err(PlatformError::from)?;
        let (contacts, _) = self.read_contacts(id).map_err(PlatformError::from)?;
        let contacts = contacts.ok_or(PlatformError::Unavailable)?;
        let pending = (contacts.values())
            .flat_map(|o| &o.entries)
            .filter(|e| e.setup_pending)
            .count();
        Ok(u32::try_from(pending).unwrap_or(u32::MAX))
    }

    /// How each of `users` relates to the wallet's main identity (DP2-03:
    /// search, lookup and scans): `IsSelf` for it, else its relation in the
    /// last contacts snapshot, else `Stranger`, as for every user while the
    /// wallet has no identity or no snapshot yet.
    pub(super) fn relations_to_main(
        &self,
        id: WalletId,
        users: &[Identifier],
    ) -> Result<Vec<Relation>, PlatformError> {
        let main = (self.identity_summaries(id)?.into_iter())
            .find(|s| s.is_main)
            .and_then(|s| Identifier::from_string(&s.identity, Encoding::Base58).ok());
        let (contacts, _) = self.read_contacts(id)?;
        let owner = main.and_then(|m| contacts.as_ref()?.get(&m));
        Ok((users.iter())
            .map(|user| match owner.and_then(|o| o.get(user)) {
                _ if Some(*user) == main => Relation::IsSelf,
                Some(entry) => entry.relation,
                None => Relation::Stranger,
            })
            .collect())
    }
}
