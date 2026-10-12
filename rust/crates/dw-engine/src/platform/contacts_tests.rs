//! The contacts read model (DP2-01): every state transition through the
//! library's own calls, sections, sorts and search, each provenance class,
//! a replayed restore, then the facade over a session (errors, the busy
//! snapshot, usernames, catch-up silence) and a live read of the testnet
//! fixture (ignored).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashcore::Txid;
use dashcore::hashes::Hash;
use dpp::identity::Identity;
use dpp::identity::v0::IdentityV0;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_appdb::AppDb;
use platform_wallet::changeset::{
    PendingContactCrypto, PendingContactCryptoOp, PlatformWalletChangeSet,
    PlatformWalletPersistence,
};
use platform_wallet::events::WalletEvent;
use platform_wallet::wallet::identity::{
    ContactProfileEntry, DashPayProfile, PaymentDirection, PaymentEntry, PaymentStatus,
};
use platform_wallet::wallet::persister::{NoPlatformPersistence, WalletPersister};
use platform_wallet::{ContactRequest, EstablishedContact, ManagedIdentity};
use zeroize::Zeroizing;

use super::contact_names::{BoxedFuture, NameSource};
use super::contacts::{
    ChannelState, ContactDetail, ContactQuery, ContactSection, ContactSort, ContactsPage,
    PublishState, Relation,
};
use super::contacts_model::{Join, OwnerContacts, detail, owner_contacts, page};
use super::dashpay::DashPay;
use super::errors::PlatformError;
use super::journal_tests::{Recorder, engine, session, tx_record};
use super::provenance::{Entity, Provenance};
use super::reverify::{Held, ShownProfile, held_unverified, record_step};
use super::runtime::guard;
use super::{SpvState, recovery_tests};
use crate::{
    DashNetwork, Engine, EngineEvent, EventSink, ImportOptions, NetworkSession, PlatformChange,
    SessionOptions, WalletId,
};

const W: WalletId = WalletId([0xAA; 32]);
/// Our identity and the people around it.
const ME: u8 = 1;
const BOB: u8 = 2;
const CAROL: u8 = 3;
const DAVE: u8 = 4;
const ERIN: u8 = 5;

fn id(n: u8) -> Identifier {
    Identifier::from([n; 32])
}

fn b58(n: u8) -> String {
    id(n).to_string(Encoding::Base58)
}

/// `sender` to `recipient`, made at `created_at` (UNIX ms).
fn request(sender: u8, recipient: u8, created_at: u64) -> ContactRequest {
    ContactRequest::new(
        id(sender),
        id(recipient),
        0,
        0,
        0,
        vec![1; 96],
        1,
        created_at,
    )
}

fn identity(n: u8) -> Identity {
    Identity::V0(IdentityV0 {
        id: id(n),
        public_keys: Default::default(),
        balance: 0,
        revision: 0,
    })
}

/// The pair `me` and `n` as the library restores it.
fn established_contact(n: u8, ours: u64, theirs: u64) -> EstablishedContact {
    EstablishedContact::new(id(n), request(ME, n, ours), request(n, ME, theirs))
}

/// A crypto write queued for our identity and contact `n`.
fn crypto(n: u8, op: PendingContactCryptoOp) -> PendingContactCrypto {
    PendingContactCrypto {
        owner_identity_id: id(ME),
        contact_id: id(n),
        op,
        enqueued_at_ms: 0,
    }
}

fn txid(n: u8) -> Txid {
    Txid::from_byte_array([n; 32])
}

/// Our identity in the library's memory, changed through the library's own
/// calls, with the contacts whose paying DashPay account the Core wallet
/// holds.
struct Me {
    m: ManagedIdentity,
    p: WalletPersister,
    built: BTreeSet<Identifier>,
}

impl Me {
    fn new() -> Self {
        Self {
            m: ManagedIdentity::new(identity(ME), 0),
            p: WalletPersister::new(W.0, Arc::new(NoPlatformPersistence)),
            built: BTreeSet::new(),
        }
    }

    fn contacts(&self) -> OwnerContacts {
        owner_contacts(&self.m, &|c| self.built.contains(c))
    }

    fn sent(&mut self, to: u8, created_at: u64) {
        (self.m)
            .add_sent_contact_request(request(ME, to, created_at), &self.p)
            .unwrap();
    }

    fn received(&mut self, from: u8, created_at: u64) {
        (self.m)
            .add_incoming_contact_request(request(from, ME, created_at), &self.p)
            .unwrap();
    }

    /// Both requests, and the pair's accounts built.
    fn established(&mut self, n: u8, ours: u64, theirs: u64) {
        self.sent(n, ours);
        self.received(n, theirs);
        self.build(n);
    }

    /// The paying account built, from the contact's current request.
    fn build(&mut self, n: u8) {
        self.built.insert(id(n));
        let c = self.contact(n);
        c.external_account_reference = Some(c.incoming_request.account_reference);
    }

    fn contact(&mut self, n: u8) -> &mut EstablishedContact {
        self.m.established_contact_mut(&id(n)).unwrap()
    }

    fn queue(&mut self, n: u8, op: PendingContactCryptoOp) {
        (self.m.dashpay_pending_contact_crypto_mut()).push(crypto(n, op));
    }

    /// The library's cache of `n`'s profile; `None`: confirmed absent.
    fn cache_profile(&mut self, n: u8, display_name: Option<&str>) {
        let profile = display_name.map(|d| DashPayProfile {
            display_name: Some(d.into()),
            ..Default::default()
        });
        (self.m.dashpay_contact_profiles_mut()).insert(
            id(n),
            ContactProfileEntry {
                profile,
                checked_at_ms: 0,
            },
        );
    }

    fn pay(&mut self, tx: u8, to: u8, status: PaymentStatus) {
        (self.m.dashpay_payments_mut()).insert(
            txid(tx).to_string(),
            PaymentEntry {
                counterparty_id: id(to),
                amount_duffs: 1_000,
                memo: None,
                direction: PaymentDirection::Sent,
                status,
            },
        );
    }
}

/// What a read joins, as a test sets it.
#[derive(Default)]
struct Joins {
    names: BTreeMap<Identifier, String>,
    times: BTreeMap<Txid, u64>,
    provenance: Option<Provenance>,
    /// `dp_contact_info` digests by contact.
    published: BTreeMap<Identifier, [u8; 32]>,
}

impl Joins {
    fn run<R>(&self, f: impl FnOnce(&Join) -> R) -> R {
        let username = |c: &Identifier| self.names.get(c).cloned();
        let unverified =
            |e: &Entity| (self.provenance.as_ref()).is_some_and(|p| p.is_unverified(e));
        let tx_time = |t: &Txid| self.times.get(t).copied();
        let published = |c: &Identifier| self.published.get(c).copied();
        f(&Join {
            username: &username,
            unverified: &unverified,
            tx_time: &tx_time,
            published: &published,
        })
    }

    fn page(&self, owner: &OwnerContacts, q: &ContactQuery) -> ContactsPage {
        self.run(|j| page(owner, q, j, false))
    }

    /// The sections of `me`'s page, all of them, by display name.
    fn listed(&self, me: &Me) -> Vec<(ContactSection, Vec<String>)> {
        layout(&self.page(&me.contacts(), &every(ContactSort::DisplayName)))
    }

    fn detail(&self, owner: &OwnerContacts, n: u8) -> Option<ContactDetail> {
        self.run(|j| detail(owner, &id(n), j))
    }

    /// Each contact's `unverified`, over every section.
    fn unverified(&self, owner: &OwnerContacts) -> BTreeMap<String, bool> {
        let all = self.page(owner, &query(&[], ContactSort::DisplayName, None));
        (all.sections.into_iter())
            .flat_map(|s| s.contacts)
            .map(|c| (c.contact, c.unverified))
            .collect()
    }
}

fn query(sections: &[ContactSection], sort: ContactSort, text: Option<&str>) -> ContactQuery {
    ContactQuery {
        sections: sections.to_vec(),
        sort,
        text: text.map(str::to_owned),
    }
}

fn every(sort: ContactSort) -> ContactQuery {
    query(&[], sort, None)
}

/// The page as sections of contact ids.
fn layout(page: &ContactsPage) -> Vec<(ContactSection, Vec<String>)> {
    (page.sections.iter())
        .map(|s| {
            let ids = s.contacts.iter().map(|c| c.contact.clone()).collect();
            (s.section, ids)
        })
        .collect()
}

/// The ids in `section` of `page`.
fn ids_in(page: &ContactsPage, section: ContactSection) -> Vec<String> {
    (layout(page).into_iter())
        .find(|(s, _)| *s == section)
        .map(|(_, ids)| ids)
        .unwrap_or_default()
}

fn names(ns: &[u8]) -> Vec<String> {
    ns.iter().map(|n| b58(*n)).collect()
}

/// Every section, with these contacts in it.
fn sections(
    requests: &[u8],
    contacts: &[u8],
    pending: &[u8],
    hidden: &[u8],
) -> Vec<(ContactSection, Vec<String>)> {
    vec![
        (ContactSection::Requests, names(requests)),
        (ContactSection::Contacts, names(contacts)),
        (ContactSection::Pending, names(pending)),
        (ContactSection::Hidden, names(hidden)),
    ]
}

// ---------------------------------------------------------------------------
// State transitions
// ---------------------------------------------------------------------------

/// Sending a request makes a pending row; the reply establishes the pair,
/// whose channel waits for its accounts, then is ready.
#[test]
fn an_outgoing_request_waits_until_the_reply_establishes_the_contact() {
    let (mut me, j) = (Me::new(), Joins::default());
    me.sent(BOB, 1_000_000);
    assert_eq!(j.listed(&me), sections(&[], &[], &[BOB], &[]));
    let d = j.detail(&me.contacts(), BOB).unwrap();
    assert_eq!(d.summary.relation, Relation::PendingOutgoing);
    assert_eq!(d.summary.channel, ChannelState::NotEstablished);
    assert_eq!(d.summary.since, Some(1_000));
    assert_eq!(
        (d.request_sent_at, d.request_received_at),
        (Some(1_000), None)
    );

    // Their reply: a contact, its channel not set up yet.
    me.received(BOB, 3_000_000);
    let d = j.detail(&me.contacts(), BOB).unwrap();
    assert_eq!(d.summary.relation, Relation::Contact);
    assert_eq!(d.summary.channel, ChannelState::SetupPending);
    assert!(me.contacts().entries[0].setup_pending);
    assert_eq!(j.listed(&me), sections(&[], &[BOB], &[], &[]));

    // Its accounts built: ready. The contact dates from the reply.
    me.build(BOB);
    let d = j.detail(&me.contacts(), BOB).unwrap();
    assert_eq!(d.summary.channel, ChannelState::Ready);
    assert!(!me.contacts().entries[0].setup_pending);
    assert_eq!(d.summary.since, Some(3_000));
    assert_eq!(
        (d.request_sent_at, d.request_received_at),
        (Some(1_000), Some(3_000))
    );
}

/// An incoming request is a request until we send ours (the accept).
#[test]
fn an_incoming_request_is_a_request_until_accepted() {
    let (mut me, j) = (Me::new(), Joins::default());
    me.received(BOB, 2_000_000);
    assert_eq!(j.listed(&me), sections(&[BOB], &[], &[], &[]));
    let d = j.detail(&me.contacts(), BOB).unwrap();
    assert_eq!(d.summary.relation, Relation::PendingIncoming);
    assert_eq!(d.summary.channel, ChannelState::NotEstablished);
    assert_eq!(d.summary.since, Some(2_000));
    assert_eq!(
        (d.request_sent_at, d.request_received_at),
        (None, Some(2_000))
    );

    me.sent(BOB, 5_000_000);
    me.build(BOB);
    let d = j.detail(&me.contacts(), BOB).unwrap();
    assert_eq!(d.summary.relation, Relation::Contact);
    assert_eq!(d.summary.channel, ChannelState::Ready);
    assert_eq!(d.summary.since, Some(5_000));
    assert_eq!(
        (d.request_sent_at, d.request_received_at),
        (Some(5_000), Some(2_000))
    );
}

/// Ignoring drops the sender's request and hides them, over a request of
/// ours too; un-ignoring forgets them until they ask again.
#[test]
fn ignoring_a_sender_hides_them_until_unignored() {
    let (mut me, j) = (Me::new(), Joins::default());
    me.received(BOB, 2_000_000);
    me.sent(CAROL, 1_000_000);
    me.m.ignore_sender(&id(BOB));
    me.m.ignore_sender(&id(CAROL));
    assert_eq!(
        j.listed(&me),
        vec![
            (ContactSection::Requests, vec![]),
            (ContactSection::Contacts, vec![]),
            (ContactSection::Pending, vec![]),
            (ContactSection::Hidden, {
                let mut ids = names(&[BOB, CAROL]);
                ids.sort();
                ids
            }),
        ]
    );
    let d = j.detail(&me.contacts(), BOB).unwrap();
    assert_eq!(d.summary.relation, Relation::Ignored);
    assert_eq!(d.summary.channel, ChannelState::NotEstablished);
    assert_eq!((d.summary.since, d.request_received_at), (None, None));
    assert_eq!(d.publish_state, PublishState::Local);

    me.m.unignore_sender(&id(BOB));
    assert!(j.detail(&me.contacts(), BOB).is_none());
    me.received(BOB, 4_000_000);
    assert_eq!(
        j.detail(&me.contacts(), BOB).unwrap().summary.relation,
        Relation::PendingIncoming
    );
}

/// A withdrawn request, either way, leaves no row.
#[test]
fn withdrawn_requests_leave_no_row() {
    let mut me = Me::new();
    me.sent(BOB, 1_000_000);
    me.received(CAROL, 2_000_000);
    assert_eq!(me.contacts().entries.len(), 2);
    me.m.remove_sent_contact_request(&id(BOB));
    me.m.remove_incoming_contact_request(&id(CAROL));
    assert_eq!(me.contacts(), OwnerContacts::default());
}

/// Hiding a contact moves it to Hidden, still a contact; unhiding moves it
/// back.
#[test]
fn hiding_a_contact_moves_it_to_hidden_and_back() {
    let (mut me, j) = (Me::new(), Joins::default());
    me.established(BOB, 1_000_000, 2_000_000);
    me.contact(BOB).is_hidden = true;
    assert_eq!(j.listed(&me), sections(&[], &[], &[], &[BOB]));
    let s = j.detail(&me.contacts(), BOB).unwrap().summary;
    assert_eq!(
        (s.relation, s.hidden, s.channel),
        (Relation::Contact, true, ChannelState::Ready)
    );

    me.contact(BOB).is_hidden = false;
    assert_eq!(j.listed(&me), sections(&[], &[BOB], &[], &[]));
}

/// The channel: broken as the library flags it; waiting for setup while an
/// account op is queued, even with the accounts there; a queued contactInfo
/// decrypt is no setup. A queued auto-accept makes a request wait too.
#[test]
fn channel_states_follow_the_library() {
    let mut me = Me::new();
    for (n, ours, theirs) in [(BOB, 1, 2), (CAROL, 3, 4), (DAVE, 5, 6)] {
        me.established(n, ours * 1_000, theirs * 1_000);
    }
    me.contact(BOB).payment_channel_broken = true;
    me.queue(CAROL, PendingContactCryptoOp::RegisterReceiving);
    me.queue(DAVE, PendingContactCryptoOp::ContactInfoDecrypt);
    me.received(ERIN, 9_000);
    me.queue(ERIN, PendingContactCryptoOp::AutoAccept);
    let state = |me: &Me, n: u8| {
        let c = me.contacts();
        let e = c.entries.iter().find(|e| e.contact == id(n)).unwrap();
        (e.channel, e.setup_pending)
    };
    assert_eq!(state(&me, BOB), (ChannelState::Broken, false));
    assert_eq!(state(&me, CAROL), (ChannelState::SetupPending, true));
    assert_eq!(state(&me, DAVE), (ChannelState::Ready, false));
    assert_eq!(state(&me, ERIN), (ChannelState::NotEstablished, true));

    // The external account's op too, and without its op a missing account.
    let mut me = Me::new();
    me.established(BOB, 1_000, 2_000);
    me.queue(
        BOB,
        PendingContactCryptoOp::RegisterExternal {
            encrypted_public_key: vec![0; 96],
            our_decryption_key_index: 0,
            contact_encryption_key_index: 0,
        },
    );
    assert_eq!(state(&me, BOB), (ChannelState::SetupPending, true));
    let mut me = Me::new();
    me.established(BOB, 1_000, 2_000);
    me.built.clear();
    assert_eq!(state(&me, BOB), (ChannelState::SetupPending, true));
    // Built from an older request (or not known to be current, after a cold
    // start): the library rebuilds it.
    let mut me = Me::new();
    me.established(BOB, 1_000, 2_000);
    me.contact(BOB).external_account_reference = None;
    assert_eq!(state(&me, BOB), (ChannelState::SetupPending, true));
    me.contact(BOB).external_account_reference = Some(77);
    assert_eq!(state(&me, BOB), (ChannelState::SetupPending, true));
    me.build(BOB);
    assert_eq!(state(&me, BOB), (ChannelState::Ready, false));

    // A broken channel heals when the library clears the flag; a drained
    // auto-accept no longer waits.
    me.contact(BOB).payment_channel_broken = true;
    assert_eq!(state(&me, BOB), (ChannelState::Broken, false));
    me.contact(BOB).payment_channel_broken = false;
    assert_eq!(state(&me, BOB), (ChannelState::Ready, false));
    me.received(ERIN, 9_000);
    me.queue(ERIN, PendingContactCryptoOp::AutoAccept);
    assert_eq!(state(&me, ERIN), (ChannelState::NotEstablished, true));
    me.m.dashpay_pending_contact_crypto_mut().clear();
    assert_eq!(state(&me, ERIN), (ChannelState::NotEstablished, false));
}

/// Ignoring a contact's identity leaves the contact (the library keeps it
/// established); its payments still count. An ignored sender's payment
/// does not date them.
#[test]
fn an_ignored_contact_stays_a_contact() {
    let (mut me, mut j) = (Me::new(), Joins::default());
    me.established(BOB, 1_000_000, 2_000_000);
    me.m.ignore_sender(&id(BOB));
    me.received(CAROL, 3_000_000);
    me.m.ignore_sender(&id(CAROL));
    me.pay(1, BOB, PaymentStatus::Confirmed);
    me.pay(2, CAROL, PaymentStatus::Confirmed);
    j.times.insert(txid(1), 5_000);
    j.times.insert(txid(2), 6_000);
    assert_eq!(j.listed(&me), sections(&[], &[BOB], &[], &[CAROL]));
    let bob = j.detail(&me.contacts(), BOB).unwrap().summary;
    assert_eq!(
        (bob.relation, bob.last_activity_at),
        (Relation::Contact, Some(5_000))
    );
    let carol = j.detail(&me.contacts(), CAROL).unwrap().summary;
    assert_eq!((carol.since, carol.last_activity_at), (None, None));
}

/// Declared residual: the library replaces a contact's request in place when
/// either side rotates its key, so the contact dates from the rotation.
#[test]
fn a_rotated_request_moves_the_contact_date() {
    let (mut me, j) = (Me::new(), Joins::default());
    me.established(BOB, 1_000_000, 2_000_000);
    me.contact(BOB).incoming_request = request(BOB, ME, 9_000_000);
    let d = j.detail(&me.contacts(), BOB).unwrap();
    assert_eq!(
        (d.summary.since, d.request_received_at),
        (Some(9_000), Some(9_000))
    );
    assert_eq!(d.request_sent_at, Some(1_000));
}

/// The private details as the library holds them; publishing waits for a
/// second contact, then the details read as published while their digest is
/// the one recorded (DP2-04); the payment lock is not this task's.
#[test]
fn private_details_and_the_publish_state() {
    let (mut me, mut j) = (Me::new(), Joins::default());
    me.established(BOB, 1_000, 2_000);
    me.contact(BOB).alias = Some("Bobby".into());
    me.contact(BOB).note = Some("met at the meetup".into());
    me.received(DAVE, 3_000);
    let d = j.detail(&me.contacts(), BOB).unwrap();
    assert_eq!(d.private_details.alias.as_deref(), Some("Bobby"));
    assert_eq!(d.private_details.note.as_deref(), Some("met at the meetup"));
    assert!(!d.private_details.hidden);
    assert_eq!(d.summary.alias.as_deref(), Some("Bobby"));
    assert_eq!(d.publish_state, PublishState::DeferredUntilTwoContacts);
    assert_eq!(d.payment_lock, None);
    assert_eq!(
        j.detail(&me.contacts(), DAVE).unwrap().publish_state,
        PublishState::Local
    );

    me.established(CAROL, 4_000, 5_000);
    assert_eq!(
        j.detail(&me.contacts(), BOB).unwrap().publish_state,
        PublishState::Local
    );

    let shown = j.detail(&me.contacts(), BOB).unwrap().private_details;
    j.published
        .insert(id(BOB), super::private_details::digest(&shown));
    assert_eq!(
        j.detail(&me.contacts(), BOB).unwrap().publish_state,
        PublishState::Published
    );
    // A hidden-only change is a change.
    me.contact(BOB).is_hidden = true;
    assert_eq!(
        j.detail(&me.contacts(), BOB).unwrap().publish_state,
        PublishState::Local
    );
    // A record names one contact.
    j.published
        .insert(id(CAROL), super::private_details::digest(&shown));
    assert_eq!(
        j.detail(&me.contacts(), CAROL).unwrap().publish_state,
        PublishState::Local
    );
}

/// Profiles come from the library's contact-profile cache; a
/// confirmed-absent one shows none. Usernames come from the read's join.
#[test]
fn profiles_join_from_the_cache() {
    let (mut me, mut j) = (Me::new(), Joins::default());
    me.established(BOB, 1_000, 2_000);
    me.established(CAROL, 3_000, 4_000);
    me.received(DAVE, 5_000);
    me.cache_profile(BOB, Some("Bob Builder"));
    me.cache_profile(CAROL, None);
    me.cache_profile(DAVE, Some("Dave"));
    j.names.insert(id(BOB), "bob".into());

    let bob = j.detail(&me.contacts(), BOB).unwrap();
    assert_eq!(bob.summary.display_name.as_deref(), Some("Bob Builder"));
    assert_eq!(bob.summary.username.as_deref(), Some("bob"));
    assert_eq!(
        bob.profile.and_then(|p| p.display_name).as_deref(),
        Some("Bob Builder")
    );
    let carol = j.detail(&me.contacts(), CAROL).unwrap();
    assert_eq!((carol.profile, carol.summary.username), (None, None));
    let dave = j.detail(&me.contacts(), DAVE).unwrap();
    assert_eq!(dave.summary.display_name.as_deref(), Some("Dave"));
}

// ---------------------------------------------------------------------------
// Sections, sorts, search
// ---------------------------------------------------------------------------

/// Three contacts named three ways, two requests and an outgoing one.
fn crowd() -> (Me, Joins) {
    let (mut me, mut j) = (Me::new(), Joins::default());
    // Bob: an alias over his username; since 3 000 s.
    me.established(BOB, 1_000_000, 3_000_000);
    me.contact(BOB).alias = Some("Zed".into());
    j.names.insert(id(BOB), "bob".into());
    // Carol: a display name only; since 1 000 s, paid at 9 000 s.
    me.established(CAROL, 500_000, 1_000_000);
    me.cache_profile(CAROL, Some("Carol"));
    me.pay(1, CAROL, PaymentStatus::Confirmed);
    j.times.insert(txid(1), 9_000);
    // Dave: a username only; since 2 000 s; a failed payment does not count.
    me.established(DAVE, 2_000_000, 1_500_000);
    j.names.insert(id(DAVE), "dave".into());
    me.pay(2, DAVE, PaymentStatus::Failed);
    j.times.insert(txid(2), 10_000);
    // Requests: Erin's after Frank's; an outgoing one to 7.
    me.received(6, 4_000_000);
    me.received(ERIN, 5_000_000);
    me.sent(7, 6_000_000);
    (me, j)
}

/// Each sort over the Contacts section; requests stay newest first.
#[test]
fn the_four_sorts() {
    let (me, j) = crowd();
    let contacts = me.contacts();
    let order = |sort| ids_in(&j.page(&contacts, &every(sort)), ContactSection::Contacts);
    // Shown names: "zed" (alias), "carol" (display name), "dave" (username).
    assert_eq!(order(ContactSort::DisplayName), names(&[CAROL, DAVE, BOB]));
    // Known usernames first.
    assert_eq!(order(ContactSort::Username), names(&[BOB, DAVE, CAROL]));
    // Newest first.
    assert_eq!(order(ContactSort::DateAdded), names(&[BOB, DAVE, CAROL]));
    // Carol's payment is the latest activity; the others date from their
    // requests.
    assert_eq!(order(ContactSort::LastActivity), names(&[CAROL, BOB, DAVE]));
    let page = j.page(&contacts, &every(ContactSort::DisplayName));
    assert_eq!(ids_in(&page, ContactSection::Requests), names(&[ERIN, 6]));
    let carol = j.detail(&contacts, CAROL).unwrap().summary;
    assert_eq!(
        (carol.since, carol.last_activity_at),
        (Some(1_000), Some(9_000))
    );
    let dave = j.detail(&contacts, DAVE).unwrap().summary;
    assert_eq!(
        (dave.since, dave.last_activity_at),
        (Some(2_000), Some(2_000))
    );
}

/// A blank alias or display name does not hide the next name.
#[test]
fn blank_names_fall_through() {
    let (mut me, mut j) = crowd();
    me.contact(BOB).alias = Some("  ".into());
    j.names.insert(id(BOB), "aaron".into());
    let order = ids_in(
        &j.page(&me.contacts(), &every(ContactSort::DisplayName)),
        ContactSection::Contacts,
    );
    assert_eq!(order, names(&[BOB, CAROL, DAVE]));
}

/// Contacts whose usernames are unknown tie on the username sort and fall
/// to the common tie-break, the id; their aliases do not order them.
#[test]
fn unknown_usernames_tie_on_the_id() {
    let (mut me, j) = (Me::new(), Joins::default());
    let mut ids = [CAROL, DAVE];
    ids.sort_by_key(|&n| b58(n));
    for (n, alias) in ids.into_iter().zip(["Zed", "Abe"]) {
        me.established(n, 1_000, 2_000);
        me.contact(n).alias = Some(alias.into());
    }
    let order = ids_in(
        &j.page(&me.contacts(), &every(ContactSort::Username)),
        ContactSection::Contacts,
    );
    assert_eq!(order, names(&ids));
}

/// Sections as asked (each once, in the query's order); the search folds
/// case over username, display name and alias, or takes an id's start;
/// blank text filters nothing.
#[test]
fn sections_and_search() {
    let (me, j) = crowd();
    let contacts = me.contacts();
    let sort = ContactSort::DisplayName;
    let asked = j.page(
        &contacts,
        &query(
            &[
                ContactSection::Pending,
                ContactSection::Requests,
                ContactSection::Pending,
            ],
            sort,
            None,
        ),
    );
    assert_eq!(
        layout(&asked),
        vec![
            (ContactSection::Pending, names(&[7])),
            (ContactSection::Requests, names(&[ERIN, 6])),
        ]
    );
    let found = |text: &str| {
        let page = j.page(&contacts, &query(&[], sort, Some(text)));
        (page.sections.into_iter())
            .flat_map(|s| s.contacts)
            .map(|c| c.contact)
            .collect::<Vec<_>>()
    };
    assert_eq!(found("zE"), names(&[BOB]));
    assert_eq!(found("BOB"), names(&[BOB]));
    assert_eq!(found("caro"), names(&[CAROL]));
    assert_eq!(found("dav"), names(&[DAVE]));
    assert_eq!(found(&b58(ERIN)[..6]), names(&[ERIN]));
    // Under four characters, an id prefix matches nothing by itself.
    let short = &b58(ERIN)[..3];
    assert!(!found(short).contains(&b58(ERIN)), "{short}");
    assert_eq!(found("   ").len(), 6);
    assert!(found("nobody").is_empty());
    // A search that finds nothing still lists the sections.
    let none = j.page(&contacts, &query(&[], sort, Some("nobody")));
    assert_eq!(layout(&none), sections(&[], &[], &[], &[]));
}

// ---------------------------------------------------------------------------
// Provenance: one test per class
// ---------------------------------------------------------------------------

/// Joins over a fresh trust state with the fallback used: absence of a
/// record now means unverified.
fn trusting() -> Joins {
    let p = Provenance::open(Arc::new(AppDb::open_in_memory().unwrap()));
    p.fallback_in_use().unwrap();
    Joins {
        provenance: Some(p),
        ..Default::default()
    }
}

fn record(j: &Joins, h: Held) {
    (j.provenance.as_ref().unwrap())
        .record_verified(&h.entity())
        .unwrap();
}

fn record_request(j: &Joins, sender: u8, recipient: u8, created_at: u64) {
    record(
        j,
        Held::ContactRequest(request(sender, recipient, created_at)),
    );
}

/// The profile shown as `display_name`, as provenance holds it.
fn shown_profile(n: u8, display_name: &str) -> Held {
    Held::Profile {
        id: id(n),
        shown: ShownProfile {
            display_name: Some(display_name.into()),
            ..Default::default()
        },
    }
}

fn flags(pairs: &[(u8, bool)]) -> BTreeMap<String, bool> {
    pairs.iter().map(|(n, f)| (b58(*n), *f)).collect()
}

/// Before the fallback is ever used nothing is unverified.
#[test]
fn nothing_is_unverified_before_the_fallback_is_used() {
    let mut me = Me::new();
    me.established(BOB, 1_000, 2_000);
    me.received(CAROL, 3_000);
    let j = Joins {
        provenance: Some(Provenance::open(Arc::new(AppDb::open_in_memory().unwrap()))),
        ..Default::default()
    };
    assert_eq!(
        j.unverified(&me.contacts()),
        flags(&[(BOB, false), (CAROL, false)])
    );
}

/// A contact's identity has no class of its own (m4 §5): its verified
/// requests bind the keys and the xpub the channel uses.
#[test]
fn a_contact_needs_no_identity_record() {
    let (mut me, j) = (Me::new(), trusting());
    me.established(BOB, 1_000, 2_000);
    me.sent(CAROL, 3_000);
    for (s, r, at) in [(ME, BOB, 1_000), (BOB, ME, 2_000), (ME, CAROL, 3_000)] {
        record_request(&j, s, r, at);
    }
    assert_eq!(
        j.unverified(&me.contacts()),
        flags(&[(BOB, false), (CAROL, false)])
    );
}

/// `profile`: only a profile shown needs its record, and only of the
/// content shown (DEC-183): one the library replaced is unverified again.
#[test]
fn provenance_class_profile() {
    let (mut me, j) = (Me::new(), trusting());
    me.received(BOB, 1_000);
    me.received(CAROL, 2_000);
    me.received(DAVE, 3_000);
    record_request(&j, BOB, ME, 1_000);
    record_request(&j, CAROL, ME, 2_000);
    record_request(&j, DAVE, ME, 3_000);
    me.cache_profile(BOB, Some("Bob"));
    me.cache_profile(CAROL, None);
    assert_eq!(
        j.unverified(&me.contacts()),
        flags(&[(BOB, true), (CAROL, false), (DAVE, false)])
    );
    record(&j, shown_profile(BOB, "Bob"));
    assert_eq!(
        j.unverified(&me.contacts()),
        flags(&[(BOB, false), (CAROL, false), (DAVE, false)])
    );
    me.cache_profile(BOB, Some("Mallory"));
    assert!(j.unverified(&me.contacts())[&b58(BOB)], "replaced");
}

/// `dpns_label`: a username shown needs a record of that spelling as this
/// contact's (v2: the digest binds both). Another spelling of the same
/// label, or the same spelling as someone else's, does not verify it.
#[test]
fn provenance_class_dpns_label() {
    let (mut me, mut j) = (Me::new(), trusting());
    me.received(BOB, 1_000);
    me.received(CAROL, 2_000);
    record_request(&j, BOB, ME, 1_000);
    record_request(&j, CAROL, ME, 2_000);
    j.names.insert(id(BOB), "B0b-Smith".into());
    let unverified = |j: &Joins| j.unverified(&me.contacts());
    assert_eq!(unverified(&j), flags(&[(BOB, true), (CAROL, false)]));
    // One record per normalized label: each replaces the one before.
    record(&j, Held::label("Bob-Smith", id(BOB)));
    assert!(unverified(&j)[&b58(BOB)], "another spelling");
    record(&j, Held::label("B0b-Smith", id(CAROL)));
    assert!(unverified(&j)[&b58(BOB)], "another owner");
    record(&j, Held::label("B0b-Smith", id(BOB)));
    assert_eq!(unverified(&j), flags(&[(BOB, false), (CAROL, false)]));
}

/// `contact_request`: every request the relation rests on, both for a
/// contact, as the library holds it; an ignored sender rests on none.
#[test]
fn provenance_class_contact_request() {
    let (mut me, j) = (Me::new(), trusting());
    me.established(BOB, 1_000, 2_000);
    me.received(CAROL, 3_000);
    me.m.ignore_sender(&id(CAROL));
    me.received(DAVE, 4_000);
    assert_eq!(
        j.unverified(&me.contacts()),
        flags(&[(BOB, true), (CAROL, false), (DAVE, true)])
    );
    record_request(&j, ME, BOB, 1_000);
    record_request(&j, DAVE, ME, 4_000);
    assert_eq!(
        j.unverified(&me.contacts()),
        flags(&[(BOB, true), (CAROL, false), (DAVE, false)])
    );
    // Another request's time is another request.
    record_request(&j, BOB, ME, 2_001);
    assert!(j.unverified(&me.contacts())[&b58(BOB)]);
    record_request(&j, BOB, ME, 2_000);
    assert_eq!(
        j.unverified(&me.contacts()),
        flags(&[(BOB, false), (CAROL, false), (DAVE, false)])
    );
    // Content, not just the key (DEC-183): the request rotated in place.
    me.contact(BOB).incoming_request.sender_key_index += 1;
    assert!(j.unverified(&me.contacts())[&b58(BOB)], "rotated");
}

// ---------------------------------------------------------------------------
// Restore
// ---------------------------------------------------------------------------

/// A restore replays the contacts through the library's restore calls, in
/// whatever order: the read model is the same, since its times are the
/// requests' own.
#[test]
fn a_replayed_restore_reads_the_same() {
    let mut live = Me::new();
    live.established(BOB, 1_000_000, 3_000_000);
    live.established(CAROL, 500_000, 1_000_000);
    live.contact(CAROL).alias = Some("Caz".into());
    live.received(DAVE, 4_000_000);
    live.sent(ERIN, 2_000_000);

    let mut restored = Me::new();
    restored
        .m
        .apply_sent_contact_request(request(ME, ERIN, 2_000_000));
    restored
        .m
        .apply_incoming_contact_request(request(DAVE, ME, 4_000_000));
    let mut carol = established_contact(CAROL, 500_000, 1_000_000);
    carol.alias = Some("Caz".into());
    restored.m.apply_established_contact(carol);
    (restored.m).apply_established_contact(established_contact(BOB, 1_000_000, 3_000_000));
    restored.build(BOB);
    restored.build(CAROL);

    assert_eq!(restored.contacts(), live.contacts());
    let j = Joins::default();
    for sort in [
        ContactSort::DisplayName,
        ContactSort::Username,
        ContactSort::DateAdded,
        ContactSort::LastActivity,
    ] {
        assert_eq!(
            j.page(&restored.contacts(), &every(sort)),
            j.page(&live.contacts(), &every(sort))
        );
    }
}

// ---------------------------------------------------------------------------
// Through a session
// ---------------------------------------------------------------------------

/// An offline session with an imported wallet, and our identity in it.
struct Fixture {
    engine: Engine,
    s: Arc<NetworkSession>,
    wallet: WalletId,
    sink: Arc<Recorder>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new(no_platform: bool) -> Self {
        let dir = dw_testutil::private_tempdir();
        let sink = Arc::new(Recorder::default());
        let engine = engine(dir.path(), Arc::clone(&sink) as Arc<dyn EventSink>);
        let (s, wallet) = session(&engine, no_platform);
        Self {
            engine,
            s,
            wallet,
            sink,
            _dir: dir,
        }
    }

    fn give_me(&self) {
        let manager = self.s.manager().unwrap();
        let wallet_id = self.wallet;
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&wallet_id.0).await.unwrap();
            (wallet.state_mut().await.identity_manager)
                .add_identity(identity(ME), 0, wallet_id.0, wallet.persister())
                .unwrap();
        });
    }

    fn with_me(&self, f: impl FnOnce(&mut ManagedIdentity, &WalletPersister) + Send + 'static) {
        let manager = self.s.manager().unwrap();
        let wallet_id = self.wallet;
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&wallet_id.0).await.unwrap();
            let mut state = wallet.state_mut().await;
            let managed = (state.identity_manager)
                .managed_identity_mut(&id(ME))
                .unwrap();
            f(managed, wallet.persister());
        });
    }

    fn dp(&self) -> Arc<DashPay> {
        self.s.dashpay(self.wallet)
    }

    fn page(&self) -> ContactsPage {
        self.dp()
            .contacts(b58(ME), every(ContactSort::DisplayName))
            .unwrap()
    }

    /// Stores `cs` as the library does, through the session's
    /// `WalletStore` and its tap.
    fn store(&self, cs: PlatformWalletChangeSet) {
        let store = self.s.live().unwrap().store;
        store.store(self.wallet.0, cs).unwrap();
    }

    fn signals(&self, of: &PlatformChange) -> usize {
        (self.sink.0.lock().unwrap().iter())
            .filter(|e| matches!(e, EngineEvent::Platform { change, .. } if change == of))
            .count()
    }

    fn contacts_signals(&self) -> usize {
        self.signals(&PlatformChange::Contacts { identity: b58(ME) })
    }

    fn journal(&self) -> Vec<dw_appdb::JournalRow> {
        let w = self.wallet;
        (self.engine)
            .block_on(self.s.appdb_op(move |db| db.journal(&w.to_string())))
            .unwrap()
    }
}

fn wait_for(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The facade reads the wallet's contacts as the library's own calls left
/// them, and refuses ids it cannot use.
#[test]
fn the_facade_reads_a_wallets_contacts() {
    let f = Fixture::new(true);
    let dp = f.dp();
    let q = || every(ContactSort::DisplayName);
    assert_eq!(
        dp.contacts(b58(ME), q()).unwrap_err().code(),
        "identity.not_found"
    );
    assert_eq!(dp.pending_setup_count().unwrap(), 0);

    f.give_me();
    assert_eq!(layout(&f.page()), sections(&[], &[], &[], &[]));
    f.with_me(|m, p| {
        m.add_incoming_contact_request(request(BOB, ME, 2_000_000), p)
            .unwrap();
        m.add_sent_contact_request(request(ME, CAROL, 3_000_000), p)
            .unwrap();
        m.add_sent_contact_request(request(ME, DAVE, 1_000_000), p)
            .unwrap();
        m.add_incoming_contact_request(request(DAVE, ME, 4_000_000), p)
            .unwrap();
    });
    let page = f.page();
    assert!(!page.updating);
    assert_eq!(layout(&page), sections(&[BOB], &[DAVE], &[CAROL], &[]));
    // This wallet holds no DashPay accounts: Dave's channel waits.
    let dave = dp.contact(b58(ME), b58(DAVE)).unwrap().unwrap();
    assert_eq!(dave.summary.channel, ChannelState::SetupPending);
    assert_eq!(dave.summary.since, Some(4_000));
    assert!(!dave.summary.unverified);
    assert_eq!(dp.pending_setup_count().unwrap(), 1);
    assert_eq!(dp.contact(b58(ME), b58(9)).unwrap(), None);

    assert_eq!(
        dp.contacts(b58(9), q()).unwrap_err().code(),
        "identity.not_found"
    );
    assert_eq!(
        dp.contacts("not base58!".into(), q()).unwrap_err().code(),
        "invalid_argument"
    );
    assert_eq!(
        dp.contact(b58(ME), "not base58!".into())
            .unwrap_err()
            .code(),
        "invalid_argument"
    );
    let stranger = f.s.dashpay(WalletId([9; 32]));
    assert_eq!(
        stranger.contacts(b58(ME), q()).unwrap_err().code(),
        "wallet_not_found"
    );
    assert_eq!(
        stranger.pending_setup_count().unwrap_err().code(),
        "wallet_not_found"
    );
}

/// While a writer holds the wallet's state, a read answers with the last
/// snapshot and says `updating`; with none yet the page is empty (not "no
/// such identity") and a contact or the count is `platform.unavailable`.
/// Once the writer is done the snapshot is rebuilt and `Contacts` signalled,
/// so the re-query is live.
#[test]
fn a_busy_wallet_answers_with_its_last_snapshot_then_signals() {
    let f = Fixture::new(true);
    f.give_me();
    // Through the restore calls: no changesets, so no tap signals.
    f.with_me(|m, _| m.apply_incoming_contact_request(request(BOB, ME, 2_000_000)));
    let dp = f.dp();
    let live = f.page();
    let manager = f.s.manager().unwrap();
    let wm = manager.wallet_manager_arc();
    let held = f.engine.block_on(wm.write());

    let busy = f.page();
    assert!(busy.updating);
    assert_eq!(busy.sections, live.sections);
    assert_eq!(dp.pending_setup_count().unwrap(), 0);
    assert!(dp.contact(b58(ME), b58(BOB)).unwrap().is_some());

    f.s.platform.contacts.forget(&f.wallet);
    let empty = f.page();
    assert!(empty.updating);
    assert_eq!(layout(&empty), sections(&[], &[], &[], &[]));
    let q = every(ContactSort::DisplayName);
    assert!(dp.contacts(b58(9), q).unwrap().updating);
    let unavailable = "platform.unavailable";
    assert_eq!(
        dp.contact(b58(ME), b58(BOB)).unwrap_err().code(),
        unavailable
    );
    assert_eq!(dp.pending_setup_count().unwrap_err().code(), unavailable);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(f.contacts_signals(), 0);

    drop(held);
    wait_for("the refresh's signal", Duration::from_secs(10), || {
        f.contacts_signals() > 0
    });
    assert_eq!(f.page(), live);
}

/// A busy wallet without identities, never read before: the count is
/// `platform.unavailable`, and once the writer is done the refresh wakes
/// the wallet-wide reader with `Identities`.
#[test]
fn a_busy_wallet_without_identities_signals_identities() {
    let f = Fixture::new(true);
    let dp = f.dp();
    let identities = || f.signals(&PlatformChange::Identities);
    let before = identities();
    let manager = f.s.manager().unwrap();
    let wm = manager.wallet_manager_arc();
    let held = f.engine.block_on(wm.write());
    assert_eq!(
        dp.pending_setup_count().unwrap_err().code(),
        "platform.unavailable"
    );
    drop(held);
    wait_for("the refresh's signal", Duration::from_secs(10), || {
        identities() > before
    });
    assert_eq!(dp.pending_setup_count().unwrap(), 0);
}

/// Queue-only crypto writes (DASHPAY §3.5) invalidate the read model: an
/// addition alone, a removal alone and an auto-accept's expiry each signal
/// `Contacts` for the owner, and none writes a journal row.
#[test]
fn queued_setup_alone_signals_contacts() {
    let f = Fixture::new(true);
    f.give_me();
    // Through the restore calls: no changesets, so no tap signals. Carol's
    // channel waits (this wallet holds no DashPay accounts); her build is
    // queued.
    f.with_me(|m, _| {
        m.apply_incoming_contact_request(request(BOB, ME, 1_000));
        m.apply_established_contact(established_contact(CAROL, 1_000, 2_000));
        (m.dashpay_pending_contact_crypto_mut())
            .push(crypto(CAROL, PendingContactCryptoOp::RegisterReceiving));
    });
    let dp = f.dp();
    assert_eq!(dp.pending_setup_count().unwrap(), 1);
    let journal = f.journal();
    let signalled = |n: usize| {
        wait_for("a Contacts signal", Duration::from_secs(10), || {
            f.contacts_signals() >= n
        });
        assert_eq!(f.contacts_signals(), n);
    };
    let dequeue = |n: u8, op: PendingContactCryptoOp| {
        let key = crypto(n, op).key();
        f.with_me(move |m, _| (m.dashpay_pending_contact_crypto_mut()).retain(|p| p.key() != key));
        f.store(PlatformWalletChangeSet {
            pending_contact_crypto_cleared: vec![key],
            ..Default::default()
        });
    };

    // An addition only: Bob's request is queued for auto-accept.
    let auto_accept = crypto(BOB, PendingContactCryptoOp::AutoAccept);
    let queued = auto_accept.clone();
    f.with_me(move |m, _| m.dashpay_pending_contact_crypto_mut().push(queued));
    f.store(PlatformWalletChangeSet {
        pending_contact_crypto_added: vec![auto_accept],
        ..Default::default()
    });
    signalled(1);
    assert_eq!(dp.pending_setup_count().unwrap(), 2);

    // A removal only: Carol's build leaves the queue (her channel still
    // waits on the account).
    dequeue(CAROL, PendingContactCryptoOp::RegisterReceiving);
    signalled(2);
    assert_eq!(dp.pending_setup_count().unwrap(), 2);

    // The auto-accept expires: Bob's request stays, no longer setting up.
    dequeue(BOB, PendingContactCryptoOp::AutoAccept);
    signalled(3);
    assert_eq!(dp.pending_setup_count().unwrap(), 1);
    assert_eq!(ids_in(&f.page(), ContactSection::Requests), names(&[BOB]));
    assert_eq!(format!("{:?}", f.journal()), format!("{journal:?}"));
}

/// A payment the library already holds as Confirmed (InstantSend) that a
/// block then mines: the library writes no payment change and the tap
/// signals nothing, but the block time moves the contact's last activity.
/// `HistoryChanged` for the wallet is the re-query trigger (m4 §2.4), and
/// the re-query shows it.
#[test]
fn a_mined_payment_moves_last_activity_on_history_changed() {
    let f = Fixture::new(true);
    f.give_me();
    let (mempool, mined) = (tx_record(1, None), tx_record(1, Some(9_000)));
    let txid = mempool.txid;
    f.with_me(move |m, _| {
        m.apply_established_contact(established_contact(BOB, 1_000_000, 2_000_000));
        m.apply_established_contact(established_contact(CAROL, 3_000_000, 4_000_000));
        (m.dashpay_payments_mut()).insert(
            txid.to_string(),
            PaymentEntry {
                counterparty_id: id(BOB),
                amount_duffs: 1_000,
                memo: None,
                direction: PaymentDirection::Sent,
                status: PaymentStatus::Confirmed,
            },
        );
    });
    let history_changed = || {
        (f.sink.0.lock().unwrap().iter())
            .filter(|e| {
                matches!(e, EngineEvent::HistoryChanged { wallet_id, txids, .. }
                    if *wallet_id == f.wallet && txids.contains(&txid))
            })
            .count()
    };
    let dp = f.dp();
    let activity = || {
        let page = (dp.contacts(b58(ME), every(ContactSort::LastActivity))).unwrap();
        let bob = dp.contact(b58(ME), b58(BOB)).unwrap().unwrap();
        (
            ids_in(&page, ContactSection::Contacts),
            bob.summary.last_activity_at,
        )
    };

    (f.s.hub).apply_wallet_event(&WalletEvent::TransactionDetected {
        wallet_id: f.wallet.0,
        record: Box::new(mempool),
        balance: Default::default(),
        account_balances: BTreeMap::new(),
        addresses_derived: Vec::new(),
    });
    wait_for(
        "the mempool HistoryChanged",
        Duration::from_secs(10),
        || history_changed() == 1,
    );
    // Unmined: no block time, so Bob's activity is his request's.
    assert_eq!(activity(), (names(&[CAROL, BOB]), Some(2_000)));

    (f.s.hub).apply_wallet_event(&WalletEvent::BlockProcessed {
        wallet_id: f.wallet.0,
        height: 10,
        chain_lock: None,
        inserted: Vec::new(),
        updated: vec![mined],
        matured: Vec::new(),
        balance: Default::default(),
        account_balances: BTreeMap::new(),
        addresses_derived: Vec::new(),
    });
    wait_for(
        "the block's HistoryChanged",
        Duration::from_secs(10),
        || history_changed() == 2,
    );
    assert_eq!(activity(), (names(&[BOB, CAROL]), Some(9_000)));
    assert_eq!(f.contacts_signals(), 0);
}

/// The session's reads use its trust state: once the fallback is used, a
/// contact is unverified until its requests are recorded as verified.
#[test]
fn reads_use_the_sessions_provenance() {
    let f = Fixture::new(true);
    f.give_me();
    f.with_me(|m, _| m.apply_established_contact(established_contact(BOB, 1_000, 2_000)));
    let unverified = || {
        let d = f.dp().contact(b58(ME), b58(BOB)).unwrap().unwrap();
        d.summary.unverified
    };
    assert!(!unverified());
    let provenance = f.s.live().unwrap().provenance;
    provenance.fallback_in_use().unwrap();
    assert!(unverified());
    for (s, r, at) in [(ME, BOB, 1_000), (BOB, ME, 2_000)] {
        let held = Held::ContactRequest(request(s, r, at));
        provenance.record_verified(&held.entity()).unwrap();
    }
    assert!(!unverified());
}

/// Usernames as a test scripts them: the names it knows, an error for the
/// others; every identity asked for, and how many answers it gave. With
/// `hold`, each answer waits for a permit.
#[derive(Default)]
struct FakeNames {
    names: BTreeMap<Identifier, Option<&'static str>>,
    hold: Option<tokio::sync::Semaphore>,
    asked: Mutex<Vec<Identifier>>,
    answered: AtomicUsize,
}

impl NameSource for FakeNames {
    fn username(
        &self,
        identity: Identifier,
    ) -> BoxedFuture<'_, Result<Option<String>, PlatformError>> {
        guard(&self.asked).push(identity);
        let answer = self.names.get(&identity).copied();
        Box::pin(async move {
            if let Some(hold) = &self.hold {
                hold.acquire().await.unwrap().forget();
            }
            self.answered.fetch_add(1, Ordering::SeqCst);
            answer
                .map(|n| n.map(str::to_owned))
                .ok_or(PlatformError::Unavailable)
        })
    }
}

/// A read shows the usernames it has and queues the rest; once one lands
/// the owner gets `Platform{Contacts}` and the next read shows it. A fresh
/// name and a recently failed one are not asked again.
#[test]
fn usernames_land_in_the_background_and_signal_contacts() {
    let f = Fixture::new(false);
    f.give_me();
    // Through the restore calls: no changesets, so no tap signals.
    f.with_me(|m, _| {
        m.apply_established_contact(established_contact(BOB, 1_000, 2_000));
        m.apply_incoming_contact_request(request(CAROL, ME, 3_000));
    });
    let fake = Arc::new(FakeNames {
        names: BTreeMap::from([(id(BOB), Some("bob"))]),
        ..Default::default()
    });
    *guard(&f.s.platform.contacts.names.source) = Some(Arc::clone(&fake) as Arc<dyn NameSource>);
    let usernames = |page: &ContactsPage| -> BTreeMap<String, Option<String>> {
        (page.sections.iter())
            .flat_map(|s| &s.contacts)
            .map(|c| (c.contact.clone(), c.username.clone()))
            .collect()
    };

    assert_eq!(f.contacts_signals(), 0);
    let first = f.page();
    assert_eq!(
        usernames(&first),
        BTreeMap::from([(b58(BOB), None), (b58(CAROL), None)])
    );
    wait_for("a Contacts signal", Duration::from_secs(10), || {
        f.contacts_signals() > 0
    });
    assert_eq!(
        usernames(&f.page()),
        BTreeMap::from([(b58(BOB), Some("bob".to_owned())), (b58(CAROL), None)])
    );
    let mut asked = guard(&fake.asked).clone();
    asked.sort();
    assert_eq!(asked, [id(BOB), id(CAROL)]);

    // Bob's is fresh and Carol's failed a moment ago: nothing to ask.
    f.page();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(guard(&fake.asked).len(), 2);
    assert_eq!(f.contacts_signals(), 1);
}

/// Session close cancels a name fetch in flight: it never answers or lands,
/// and the name is no longer owed.
#[test]
fn closing_the_session_cancels_name_fetches() {
    let f = Fixture::new(false);
    f.give_me();
    f.with_me(|m, _| m.apply_incoming_contact_request(request(BOB, ME, 1_000)));
    let fake = Arc::new(FakeNames {
        names: BTreeMap::from([(id(BOB), Some("bob"))]),
        hold: Some(tokio::sync::Semaphore::new(0)),
        ..Default::default()
    });
    *guard(&f.s.platform.contacts.names.source) = Some(Arc::clone(&fake) as Arc<dyn NameSource>);
    f.page();
    wait_for("the fetch", Duration::from_secs(10), || {
        !guard(&fake.asked).is_empty()
    });
    let names = f.s.platform.contacts.names.clone();
    assert!(names.in_flight(&id(BOB)));

    (f.engine)
        .block_on(f.engine.close_network(DashNetwork::Regtest))
        .unwrap();
    (fake.hold.as_ref().unwrap()).add_permits(1);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(fake.answered.load(Ordering::SeqCst), 0);
    assert!(!names.in_flight(&id(BOB)));
    assert_eq!(names.label(&id(BOB)), None);
    assert_eq!(f.contacts_signals(), 0);
}

/// The library keeps no contact usernames, so each one a read shows is
/// reported to provenance (`show_label`) as it lands, and E0-10b's re-fetch
/// walks it: once its fetch verifies, the record step writes it and the row
/// is verified. A name that goes away is withdrawn; the record of the old
/// spelling then verifies no other spelling of the same label.
#[test]
fn shown_usernames_reach_the_refetch() {
    let f = Fixture::new(false);
    f.give_me();
    f.with_me(|m, _| m.apply_established_contact(established_contact(BOB, 1_000, 2_000)));
    let provenance = f.s.live().unwrap().provenance;
    provenance.fallback_in_use().unwrap();
    for (s, r, at) in [(ME, BOB, 1_000), (BOB, ME, 2_000)] {
        let held = Held::ContactRequest(request(s, r, at));
        provenance.record_verified(&held.entity()).unwrap();
    }
    let names = &f.s.platform.contacts.names;
    // Bob's DPNS name from now on is `name`, and due again.
    let answer = |name: Option<&'static str>| {
        let fake = FakeNames {
            names: BTreeMap::from([(id(BOB), name)]),
            ..Default::default()
        };
        *guard(&names.source) = Some(Arc::new(fake) as Arc<dyn NameSource>);
        names.expire(&id(BOB));
    };
    let bob = || {
        let d = f.dp().contact(b58(ME), b58(BOB)).unwrap().unwrap();
        (d.summary.username, d.summary.unverified)
    };
    // A read queues the fetch; the `n`th change signals, and the next read
    // shows it.
    let landed = |n: usize| {
        bob();
        wait_for("the name to land", Duration::from_secs(10), || {
            f.contacts_signals() == n
        });
        bob()
    };
    let manager = f.s.manager().unwrap();
    let wm = manager.wallet_manager_arc();
    let held = || {
        let wm = f.engine.block_on(wm.read());
        held_unverified(&wm, &provenance)
    };
    let label = |spelling: &str| Held::label(spelling, id(BOB));

    answer(Some("bob"));
    assert_eq!(landed(1), (Some("bob".into()), true));
    assert!(held().contains(&label("bob")));
    // The re-fetch's record step, once the fetch of Bob's label verified.
    let recorded = f.engine.block_on(async {
        let wm = wm.read().await;
        record_step(&wm, &provenance, &[&label("bob")])
            .await
            .unwrap()
    });
    assert_eq!(recorded.written, 1);
    assert_eq!(bob(), (Some("bob".into()), false));
    assert!(!held().contains(&label("bob")));

    answer(None);
    assert_eq!(landed(2), (None, false));
    assert!(provenance.shown_labels().is_empty());

    // `B0b` and `bob` share a normalized label; the record is `bob`'s.
    answer(Some("B0b"));
    assert_eq!(landed(3), (Some("B0b".into()), true));
    assert!(held().contains(&label("B0b")));
    assert_eq!(provenance.shown_labels(), [(id(BOB), "B0b".to_owned())]);
}

/// Without Platform (`no_platform`) a read never fetches.
#[test]
fn a_session_without_platform_fetches_no_usernames() {
    let f = Fixture::new(true);
    f.give_me();
    f.with_me(|m, _| m.apply_incoming_contact_request(request(BOB, ME, 1_000)));
    let fake = Arc::new(FakeNames::default());
    *guard(&f.s.platform.contacts.names.source) = Some(Arc::clone(&fake) as Arc<dyn NameSource>);
    f.page();
    std::thread::sleep(Duration::from_millis(300));
    assert!(guard(&fake.asked).is_empty());
}

/// When this installation first brought the wallet up (UNIX s), and request
/// times either side of it (ms).
const BEFORE: u64 = 1_000;
const OLD_MS: u64 = 500_000;
const NEW_MS: u64 = 2_000_000;

/// Catch-up silence (DEC-125/135/139) holds through the read model: contacts
/// a restore brings back from before the first bring-up make no news, the
/// reads write nothing, and only a request after the boundary is news. The
/// rows sort by their requests' times whenever they arrived.
#[test]
fn catch_up_contacts_stay_silent_and_reads_write_nothing() {
    let f = Fixture::new(true);
    f.give_me();
    let tap = f.s.live().unwrap().tap;
    assert!(tap.move_boundary_to(f.wallet, BEFORE, false));
    f.with_me(|m, p| {
        m.add_incoming_contact_request(request(BOB, ME, OLD_MS), p)
            .unwrap();
        m.add_sent_contact_request(request(ME, CAROL, OLD_MS + 1), p)
            .unwrap();
        m.add_incoming_contact_request(request(CAROL, ME, OLD_MS + 2), p)
            .unwrap();
    });
    let restored = f.journal();
    assert!(
        restored.iter().all(|r| r.read_at.is_some()),
        "catch-up made news: {restored:?}"
    );

    let dp = f.dp();
    for _ in 0..3 {
        let page = f.page();
        assert_eq!(layout(&page), sections(&[BOB], &[CAROL], &[], &[]));
        dp.contact(b58(ME), b58(CAROL)).unwrap().unwrap();
        dp.pending_setup_count().unwrap();
    }
    assert_eq!(format!("{:?}", f.journal()), format!("{restored:?}"));
    let bob = dp.contact(b58(ME), b58(BOB)).unwrap().unwrap();
    assert_eq!(bob.summary.since, Some(OLD_MS / 1000));

    // After the boundary: news, and the newest request first.
    f.with_me(|m, p| {
        m.add_incoming_contact_request(request(DAVE, ME, NEW_MS), p)
            .unwrap();
    });
    let now = f.journal();
    assert_eq!(now.len(), restored.len() + 1, "{now:?}");
    assert!(now.iter().any(|r| r.read_at.is_none()), "{now:?}");
    assert_eq!(
        ids_in(&f.page(), ContactSection::Requests),
        names(&[DAVE, BOB])
    );
}

// ---------------------------------------------------------------------------
// Live
// ---------------------------------------------------------------------------

/// The testnet DashPay fixture: A and B restored side by side, each sees the
/// other as an established contact with a ready channel, the other's
/// username and the fixture's request times. Read-only: no transactions.
#[test]
#[ignore = "public testnet; set DW_DP201_TESTNET_FIXTURE"]
fn the_testnet_fixture_reads_its_established_contact() {
    let fixture = std::path::PathBuf::from(
        std::env::var("DW_DP201_TESTNET_FIXTURE").expect("DW_DP201_TESTNET_FIXTURE"),
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    let birth = meta["funding"]["birth_height_hint"].as_u64().unwrap() as u32;
    let text = |v: &serde_json::Value| v.as_str().unwrap().to_owned();
    let request_ms = |owner: &str| {
        (meta["contact_requests"].as_array().unwrap().iter())
            .find(|r| r["owner"] == owner)
            .unwrap()["created_at_ms"]
            .as_u64()
            .unwrap()
    };

    let dir = dw_testutil::private_tempdir();
    let engine = recovery_tests::engine(dir.path());
    let s = engine
        .block_on(engine.open_network(DashNetwork::Testnet, SessionOptions::default()))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let import = |w: &str| {
        let phrase = std::fs::read_to_string(fixture.join(format!("{w}.mnemonic"))).unwrap();
        engine
            .block_on(s.import_wallet(
                Zeroizing::new(phrase.trim().as_bytes().to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions {
                    birth_height: Some(birth),
                    ..ImportOptions::default()
                },
            ))
            .unwrap()
    };
    let t0 = Instant::now();
    let (a, b) = (import("A"), import("B"));
    engine.block_on(s.start_spv()).unwrap();
    wait_for("SPV", Duration::from_secs(60), || {
        s.spv_state().unwrap() == SpvState::Running
    });

    for (w, wallet, other) in [("A", a, "B"), ("B", b, "A")] {
        let me = text(&meta[w]["identity_id"]);
        let them = text(&meta[other]["identity_id"]);
        let their_name = text(&meta[other]["expected_main_name"]);
        let dp = s.dashpay(wallet);
        // A live read: SPV, still syncing, holds the wallet's state at
        // times, and a read then answers from the snapshot (`updating`).
        let read = || {
            let page = dp
                .contacts(me.clone(), every(ContactSort::DisplayName))
                .ok()?;
            let d = dp.contact(me.clone(), them.clone()).ok().flatten()?;
            let done = !page.updating
                && d.summary.relation == Relation::Contact
                && d.summary.channel == ChannelState::Ready
                && d.summary.username.as_deref() == Some(their_name.as_str());
            done.then_some((page, d))
        };
        let mut found = None;
        wait_for(&format!("{w}'s contact"), Duration::from_secs(180), || {
            found = read();
            found.is_some()
        });
        let (page, d) = found.unwrap();
        eprintln!(
            "{w} after {:?}: {:?}; {:?}",
            t0.elapsed(),
            layout(&page),
            d.summary
        );
        assert_eq!(
            layout(&page),
            vec![
                (ContactSection::Requests, vec![]),
                (ContactSection::Contacts, vec![them.clone()]),
                (ContactSection::Pending, vec![]),
                (ContactSection::Hidden, vec![]),
            ]
        );
        let (ours, theirs) = (request_ms(w), request_ms(other));
        assert_eq!(d.request_sent_at, Some(ours / 1000));
        assert_eq!(d.request_received_at, Some(theirs / 1000));
        assert_eq!(d.summary.since, Some(ours.max(theirs) / 1000));
        assert!(!d.summary.hidden);
        assert!(!d.summary.unverified);
        assert_eq!(d.publish_state, PublishState::DeferredUntilTwoContacts);
        assert_eq!(dp.pending_setup_count().unwrap(), 0);
    }
    engine.block_on(engine.shutdown()).unwrap();
}
