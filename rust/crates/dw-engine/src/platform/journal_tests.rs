//! The changeset tap (ROADMAP E0-06): one test per row of DASHPAY §3.5, then
//! idempotence, catch-up silence, the trust fallback, and the wiring through
//! a session's `WalletStore` and bring-up.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashcore::hashes::Hash;
use dashcore::{BlockHash, OutPoint, ScriptBuf, Transaction, TxOut, Txid};
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::util::strings::convert_to_homograph_safe_chars;
use dw_appdb::{AppDb, JournalRow, SqlValue, TableRows};
use key_wallet::account::{AccountType, StandardAccountType};
use key_wallet::bip32::{ExtendedPrivKey, ExtendedPubKey};
use key_wallet::managed_account::transaction_record::{TransactionDirection, TransactionRecord};
use key_wallet::transaction_checking::{BlockInfo, TransactionContext, TransactionType};
use platform_wallet::changeset::{
    AccountRegistrationEntry, AssetLockChangeSet, ContactChangeSet, ContactRequestEntry,
    DpnsNameSaleStatus, DpnsNameStateChangeSet, DpnsNameStateEntry, IdentityChangeSet,
    IdentityEntry, Merge, PlatformWalletChangeSet, PlatformWalletPersistence,
    ReceivedContactRequestKey, SentContactRequestKey,
};
use platform_wallet::wallet::identity::{PaymentDirection, PaymentEntry, PaymentStatus};
use platform_wallet::{ContactRequest, DpnsNameInfo, EstablishedContact, IdentityStatus};
use zeroize::Zeroizing;

use super::journal::{CATCH_UP_BEFORE_KEY, ChangesetTap, Times, classify, kind_name};
use super::notifications::EventKind;
use crate::events::SessionHub;
use crate::{
    DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, ImportOptions, NetworkSession,
    PlatformChange, SessionOptions, WalletId,
};

const W: WalletId = WalletId([0xAA; 32]);
/// Our identity and two other people.
const ME: u8 = 1;
const BOB: u8 = 2;
const CAROL: u8 = 3;
/// When this installation first brought `W` up, in the tests that set it
/// (UNIX seconds); the ms times below are either side of it.
const BEFORE: u64 = 1_000;
const OLD_MS: u64 = 500_000;
const NEW_MS: u64 = 2_000_000;

fn id(n: u8) -> Identifier {
    Identifier::from([n; 32])
}

fn b58(n: u8) -> String {
    id(n).to_string(Encoding::Base58)
}

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

fn identity(n: u8, index: Option<u32>, names: &[(&str, Option<u64>)]) -> IdentityEntry {
    IdentityEntry {
        id: id(n),
        balance: 0,
        revision: 0,
        identity_index: index,
        last_updated_balance_block_time: None,
        last_synced_keys_block_time: None,
        dpns_names: names
            .iter()
            .map(|(label, acquired_at)| DpnsNameInfo {
                label: label.to_string(),
                acquired_at: *acquired_at,
            })
            .collect(),
        contested_dpns_names: Vec::new(),
        status: IdentityStatus::Active,
        wallet_id: None,
        dashpay_profile: None,
        dashpay_payments: BTreeMap::new(),
        contact_profiles: BTreeMap::new(),
        ignored_senders: BTreeSet::new(),
    }
}

fn identities(entries: Vec<IdentityEntry>) -> PlatformWalletChangeSet {
    PlatformWalletChangeSet {
        identities: Some(IdentityChangeSet {
            identities: entries.into_iter().map(|e| (e.id, e)).collect(),
            removed: BTreeSet::new(),
        }),
        ..Default::default()
    }
}

fn contacts(cs: ContactChangeSet) -> PlatformWalletChangeSet {
    PlatformWalletChangeSet {
        contacts: Some(cs),
        ..Default::default()
    }
}

fn incoming(owner: u8, sender: u8, created_at: u64) -> ContactChangeSet {
    let mut cs = ContactChangeSet::default();
    cs.incoming_requests.insert(
        ReceivedContactRequestKey {
            owner_id: id(owner),
            sender_id: id(sender),
        },
        ContactRequestEntry {
            request: request(sender, owner, created_at),
        },
    );
    cs
}

/// `owner` and `contact` established: our request at `ours`, theirs at
/// `theirs` (ms).
fn established(owner: u8, contact: u8, ours: u64, theirs: u64) -> ContactChangeSet {
    let mut cs = ContactChangeSet::default();
    cs.established.insert(
        SentContactRequestKey {
            owner_id: id(owner),
            recipient_id: id(contact),
        },
        EstablishedContact::new(
            id(contact),
            request(owner, contact, ours),
            request(contact, owner, theirs),
        ),
    );
    cs
}

fn payment(
    identity: u8,
    txid: &str,
    from: u8,
    direction: PaymentDirection,
) -> PlatformWalletChangeSet {
    let entry = PaymentEntry {
        counterparty_id: id(from),
        amount_duffs: 1_000,
        memo: None,
        direction,
        status: PaymentStatus::Confirmed,
    };
    PlatformWalletChangeSet {
        dashpay_payments_overlay: Some(BTreeMap::from([(
            id(identity),
            BTreeMap::from([(txid.to_string(), entry)]),
        )])),
        ..Default::default()
    }
}

/// A received transaction, in a block of time `block_time` (s) or in the
/// mempool.
fn tx_record(n: u32, block_time: Option<u32>) -> TransactionRecord {
    let tx = Transaction {
        version: 2,
        lock_time: n,
        input: Vec::new(),
        output: vec![TxOut {
            value: 1_000,
            script_pubkey: ScriptBuf::new(),
        }],
        special_transaction_payload: None,
    };
    let context = match block_time {
        Some(t) => TransactionContext::InBlock(BlockInfo::new(10, BlockHash::all_zeros(), t)),
        None => TransactionContext::Mempool,
    };
    TransactionRecord::new(
        tx,
        AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        },
        context,
        TransactionType::Standard,
        TransactionDirection::Incoming,
        Vec::new(),
        Vec::new(),
        1_000,
    )
}

fn outpoint(n: u8, vout: u32) -> OutPoint {
    OutPoint::new(Txid::from_byte_array([n; 32]), vout)
}

fn contacts_of(n: u8) -> BTreeSet<PlatformChange> {
    BTreeSet::from([PlatformChange::Contacts { identity: b58(n) }])
}

#[derive(Default)]
struct Recorder(Mutex<Vec<EngineEvent>>);

impl EventSink for Recorder {
    fn emit(&self, event: EngineEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// The persister's authoritative times, as a test sets them.
#[derive(Default)]
struct FakeTimes {
    blocks: Mutex<HashMap<Txid, u32>>,
    names: Mutex<HashMap<(Identifier, String), u64>>,
}

impl FakeTimes {
    /// The record as the persister holds it: confirmed in its block, or not.
    fn confirm(&self, record: &TransactionRecord) {
        if let Some(block) = record.context.block_info() {
            self.blocks
                .lock()
                .unwrap()
                .insert(record.txid, block.timestamp());
        }
    }

    /// The marketplace row of `label` for identity `n`, created at `ms`.
    fn name(&self, n: u8, label: &str, ms: u64) {
        let key = (id(n), convert_to_homograph_safe_chars(label));
        self.names.lock().unwrap().insert(key, ms);
    }
}

impl Times for FakeTimes {
    fn block_time(&self, _: WalletId, txid: &Txid) -> Option<u32> {
        self.blocks.lock().unwrap().get(txid).copied()
    }

    fn name_time(&self, _: WalletId, identity: &Identifier, normalized: &str) -> Option<u64> {
        let key = (*identity, normalized.to_string());
        self.names.lock().unwrap().get(&key).copied()
    }
}

struct Harness {
    hub: Arc<SessionHub>,
    appdb: Arc<AppDb>,
    tap: Arc<ChangesetTap>,
    times: FakeTimes,
}

impl Harness {
    fn new() -> Self {
        let hub = Arc::new(SessionHub::new(
            DashNetwork::Regtest,
            Arc::new(Recorder::default()),
        ));
        let appdb = Arc::new(AppDb::open_in_memory().unwrap());
        let tap = Arc::new(ChangesetTap::new(Arc::clone(&hub), Arc::clone(&appdb)));
        Self {
            hub,
            appdb,
            tap,
            times: FakeTimes::default(),
        }
    }

    /// A tap whose wallet `W` was first brought up here at [`BEFORE`].
    fn restored() -> Self {
        let h = Self::new();
        let scope = dw_appdb::local_scope(&W.to_string());
        h.appdb
            .set_setting(&scope, CATCH_UP_BEFORE_KEY, Some(&BEFORE.to_string()))
            .unwrap();
        h
    }

    fn feed(&self, cs: PlatformWalletChangeSet) {
        self.feed_to(W, cs);
    }

    /// What `WalletStore::store` does around an accepting persister.
    fn feed_to(&self, wallet: WalletId, cs: PlatformWalletChangeSet) {
        if let Some(c) = classify(&cs) {
            self.tap.record(wallet, c, &self.times);
        }
    }

    /// The signals marked since the last call.
    fn changes(&self) -> BTreeSet<PlatformChange> {
        let mut pending = self.hub.pump.take_platform();
        let mine = pending.remove(&W).unwrap_or_default();
        assert!(pending.is_empty(), "another wallet signalled: {pending:?}");
        mine
    }

    fn journal(&self) -> Vec<JournalRow> {
        self.appdb.journal(&W.to_string()).unwrap()
    }

    /// The journal as (kind, contact, ref, read).
    fn rows(&self) -> Vec<(String, String, String, bool)> {
        self.journal()
            .into_iter()
            .map(|r| (r.kind, r.contact, r.reference, r.read_at.is_some()))
            .collect()
    }
}

fn row(
    kind: EventKind,
    contact: &str,
    reference: &str,
    read: bool,
) -> (String, String, String, bool) {
    (
        kind_name(kind).to_string(),
        contact.to_string(),
        reference.to_string(),
        read,
    )
}

#[test]
fn kind_names_are_the_facade_serde_names() {
    for kind in [
        EventKind::UsernameRegistered,
        EventKind::ContestWon,
        EventKind::ContestLost,
        EventKind::ContestLocked,
        EventKind::RequestReceived,
        EventKind::RequestAccepted,
        EventKind::ContactEstablished,
        EventKind::PaymentReceived,
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), kind_name(kind));
    }
}

#[test]
fn core_only_changesets_are_not_classified() {
    assert!(classify(&PlatformWalletChangeSet::default()).is_none());
    let empty_dashpay = PlatformWalletChangeSet {
        identities: Some(IdentityChangeSet::default()),
        contacts: Some(ContactChangeSet::default()),
        dashpay_payments_overlay: Some(BTreeMap::new()),
        ..Default::default()
    };
    assert!(classify(&empty_dashpay).is_none());
}

/// §3.5 row `identities`: `Identities`; "username registered", once per
/// holding of a name, and only for the wallet's own identities.
#[test]
fn row_identities_signal_and_journal_usernames() {
    let h = Harness::new();
    h.feed(identities(vec![
        identity(ME, Some(0), &[("alice", Some(NEW_MS))]),
        // An identity the wallet only watches: no news.
        identity(BOB, None, &[("bob", Some(NEW_MS))]),
    ]));
    assert_eq!(h.changes(), BTreeSet::from([PlatformChange::Identities]));
    let alice = format!("alice@{NEW_MS}");
    assert_eq!(
        h.rows(),
        vec![row(EventKind::UsernameRegistered, "", &alice, false)]
    );
    assert_eq!(h.journal()[0].identity, b58(ME));

    // The next snapshot carries the same name: nothing new. A second name,
    // and the first one held again after it left, are new events. A name
    // with no marketplace row has no authoritative time: news (DEC-114).
    h.feed(identities(vec![identity(
        ME,
        Some(0),
        &[("alice", Some(NEW_MS)), ("al1ce2", Some(NEW_MS + 1))],
    )]));
    h.feed(identities(vec![identity(
        ME,
        Some(0),
        &[("alice", Some(NEW_MS + 2)), ("found", None)],
    )]));
    assert_eq!(
        h.rows(),
        vec![
            row(EventKind::UsernameRegistered, "", &alice, false),
            row(
                EventKind::UsernameRegistered,
                "",
                &format!("al1ce2@{}", NEW_MS + 1),
                false
            ),
            row(
                EventKind::UsernameRegistered,
                "",
                &format!("alice@{}", NEW_MS + 2),
                false
            ),
            row(EventKind::UsernameRegistered, "", "found", false),
        ]
    );

    // A removal signals too.
    h.changes();
    let mut removal = IdentityChangeSet::default();
    removal.removed.insert(id(ME));
    h.feed(PlatformWalletChangeSet {
        identities: Some(removal),
        ..Default::default()
    });
    assert_eq!(h.changes(), BTreeSet::from([PlatformChange::Identities]));
}

/// §3.5 row `identities`, contest outcome: a watched label that becomes an
/// owned name is `ContestWon`, with its `UsernameRegistered` stored read.
#[test]
fn row_identities_journal_a_won_contest_once() {
    let h = Harness::new();
    h.appdb
        .watch_contest(&W.to_string(), &b58(ME), "Alice", 7_000)
        .unwrap();
    let mut entry = identity(ME, Some(0), &[]);
    entry.contested_dpns_names = vec!["Alice".into()];
    h.feed(identities(vec![entry]));
    assert!(h.rows().is_empty(), "still contested");

    // Won: DPNS compares labels homograph-normalized.
    let won = || identities(vec![identity(ME, Some(0), &[("a1ice", Some(NEW_MS))])]);
    h.feed(won());
    h.feed(won());
    assert_eq!(
        h.rows(),
        vec![
            row(EventKind::ContestWon, "", "a1ice@7000", false),
            row(
                EventKind::UsernameRegistered,
                "",
                &format!("a1ice@{NEW_MS}"),
                true
            ),
        ]
    );
}

/// §3.5 row `contacts`, received: `Contacts{identity}`; "@bob sent you a
/// contact request", once per contact.
#[test]
fn row_contacts_received_journals_the_request() {
    let h = Harness::new();
    h.feed(contacts(incoming(ME, BOB, NEW_MS)));
    assert_eq!(h.changes(), contacts_of(ME));
    assert_eq!(
        h.rows(),
        vec![row(
            EventKind::RequestReceived,
            &b58(BOB),
            &NEW_MS.to_string(),
            false
        )]
    );
    // A pending request rotated (a newer `$createdAt`) is no news.
    h.feed(contacts(incoming(ME, BOB, NEW_MS + 1)));
    assert_eq!(h.journal().len(), 1);
}

/// §3.5 row `contacts`, established: "@bob accepted your request" when ours
/// came first, "is now your contact" when we accepted theirs.
#[test]
fn row_contacts_established_journals_who_accepted() {
    let h = Harness::new();
    h.feed(contacts(established(ME, BOB, NEW_MS, NEW_MS + 10)));
    h.feed(contacts(established(ME, CAROL, NEW_MS + 30, NEW_MS + 20)));
    assert_eq!(h.changes(), contacts_of(ME));
    assert_eq!(
        h.rows(),
        vec![
            row(
                EventKind::RequestAccepted,
                &b58(BOB),
                &(NEW_MS + 10).to_string(),
                false
            ),
            row(
                EventKind::ContactEstablished,
                &b58(CAROL),
                &(NEW_MS + 20).to_string(),
                false
            ),
        ]
    );
}

/// A contact's key rotation re-emits the relationship with a newer request
/// on either side, and a stale incoming request may follow: none of it is
/// news (review C2).
#[test]
fn rotated_requests_of_a_contact_are_not_news() {
    let h = Harness::new();
    h.feed(contacts(incoming(ME, CAROL, NEW_MS)));
    // We accept Carol's request.
    h.feed(contacts(established(ME, CAROL, NEW_MS + 10, NEW_MS)));
    let before = h.rows();
    assert_eq!(
        before.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        ["request_received", "contact_established"]
    );
    // They rotate: their newer request makes ours the older one.
    h.feed(contacts(established(ME, CAROL, NEW_MS + 10, NEW_MS + 50)));
    // We rotate: ours is newer again.
    h.feed(contacts(established(ME, CAROL, NEW_MS + 90, NEW_MS + 50)));
    // An incoming request of the established contact.
    h.feed(contacts(incoming(ME, CAROL, NEW_MS + 50)));
    assert_eq!(h.rows(), before);
}

/// §3.5 row alias, note, hidden, `ignored_senders`: `Contacts{identity}`,
/// no journal entry.
#[test]
fn row_contact_edits_signal_without_journal() {
    let h = Harness::new();
    h.feed(contacts(established(ME, BOB, NEW_MS, NEW_MS + 10)));
    h.changes();
    let before = h.journal();

    // Alias, note and hidden ride `established`.
    let mut edit = established(ME, BOB, NEW_MS, NEW_MS + 10);
    for contact in edit.established.values_mut() {
        contact.alias = Some("Bobby".into());
        contact.note = Some("from work".into());
        contact.is_hidden = true;
    }
    h.feed(contacts(edit));
    assert_eq!(h.changes(), contacts_of(ME));

    let mut ignore = ContactChangeSet::default();
    ignore.ignored.insert((id(ME), id(CAROL)));
    h.feed(contacts(ignore));
    assert_eq!(h.changes(), contacts_of(ME));
    let mut unignore = ContactChangeSet::default();
    unignore.unignored.insert((id(ME), id(CAROL)));
    h.feed(contacts(unignore));
    assert_eq!(h.changes(), contacts_of(ME));
    assert_eq!(h.journal(), before);
}

/// §3.5 row `asset_locks`: `Registration{draft}`, the registration whose
/// lock it is; no journal entry.
#[test]
fn row_asset_locks_signal_their_registration() {
    let h = Harness::new();
    let lock = outpoint(7, 1);
    // DP1-02 writes these rows; a `.dwbackup` import stands in for it.
    let text = |s: &str| SqlValue::Text(s.into());
    let columns = [
        "wallet_id",
        "label",
        "funding",
        "asset_lock_outpoint",
        "phase",
        "created_at",
        "updated_at",
    ];
    h.appdb
        .import_wallet_rows(
            &W.to_string(),
            &[TableRows {
                table: "dp_registration".into(),
                columns: columns.iter().map(|c| c.to_string()).collect(),
                rows: vec![vec![
                    text(&W.to_string()),
                    text("alice"),
                    text("{}"),
                    text(&lock.to_string()),
                    text("funding_sent"),
                    SqlValue::Integer(1),
                    SqlValue::Integer(1),
                ]],
            }],
        )
        .unwrap();
    let draft = h
        .appdb
        .registration_for_lock(&W.to_string(), &lock.to_string())
        .unwrap()
        .unwrap();
    let mut locks = AssetLockChangeSet::default();
    locks.removed.insert(lock);
    // A lock no registration names (a top-up).
    locks.removed.insert(outpoint(8, 0));
    h.feed(PlatformWalletChangeSet {
        asset_locks: Some(locks),
        ..Default::default()
    });
    assert_eq!(
        h.changes(),
        BTreeSet::from([
            PlatformChange::Registration {
                draft: Some(draft.to_string())
            },
            PlatformChange::Registration { draft: None },
        ])
    );
    assert!(h.journal().is_empty());
}

/// §3.5 row `dashpay_payments_overlay`: `Payments{identity}`; "Received …
/// from @alice" for a received payment. Its age is the block time of its
/// confirmed transaction, as the persister has it (DEC-114): a payment the
/// library reconstructs after a restore's passes, from a transaction older
/// than the wallet's first bring-up here, is stored read; an unconfirmed
/// one, or one with no transaction on file, is news.
#[test]
fn row_payments_journal_received_payments() {
    let h = Harness::restored();
    let old = tx_record(1, Some(500));
    let new = tx_record(2, Some(2_000));
    let mempool = tx_record(3, None);
    for record in [&old, &new, &mempool] {
        h.times.confirm(record);
    }
    for txid in [old.txid, new.txid, mempool.txid] {
        h.feed(payment(
            ME,
            &txid.to_string(),
            BOB,
            PaymentDirection::Received,
        ));
    }
    h.feed(payment(ME, "bb22", CAROL, PaymentDirection::Sent));
    assert_eq!(
        h.changes(),
        BTreeSet::from([PlatformChange::Payments { identity: b58(ME) }])
    );
    let bob = b58(BOB);
    let received =
        |txid: Txid, read| row(EventKind::PaymentReceived, &bob, &txid.to_string(), read);
    assert_eq!(
        h.rows(),
        vec![
            received(old.txid, true),
            received(new.txid, false),
            received(mempool.txid, false),
        ]
    );
}

/// §3.5 row `account_registrations` (contact accounts):
/// `Contacts{identity}`, no journal entry; other accounts are not DashPay.
#[test]
fn row_contact_accounts_signal_contacts() {
    let h = Harness::new();
    let secp = dashcore::secp256k1::Secp256k1::new();
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[7; 32]).unwrap();
    let xpub = ExtendedPubKey::from_priv(&secp, &master);
    let account = |account_type| AccountRegistrationEntry {
        account_type,
        account_xpub: xpub,
    };
    h.feed(PlatformWalletChangeSet {
        account_registrations: vec![
            account(AccountType::DashpayReceivingFunds {
                index: 0,
                user_identity_id: [ME; 32],
                friend_identity_id: [BOB; 32],
            }),
            account(AccountType::DashpayExternalAccount {
                index: 0,
                user_identity_id: [CAROL; 32],
                friend_identity_id: [BOB; 32],
            }),
        ],
        ..Default::default()
    });
    let mut expected = contacts_of(ME);
    expected.extend(contacts_of(CAROL));
    assert_eq!(h.changes(), expected);
    assert!(h.journal().is_empty());

    let standard = PlatformWalletChangeSet {
        account_registrations: vec![account(AccountType::CoinJoin { index: 0 })],
        ..Default::default()
    };
    assert!(classify(&standard).is_none());
}

fn name_state(document: u8, label: &str) -> DpnsNameStateEntry {
    DpnsNameStateEntry {
        document_id: id(document),
        wallet_identity_id: id(ME),
        label: label.into(),
        normalized_label: convert_to_homograph_safe_chars(label),
        normalized_parent_domain_name: "dash".into(),
        price: Some(5),
        status: DpnsNameSaleStatus::Owned,
        created_at_ms: None,
        updated_at_ms: None,
        transferred_at_ms: None,
        last_synced_at_ms: 1,
    }
}

/// §3.5 row `dpns_name_states`: `Names`, no journal entry.
#[test]
fn row_dpns_name_states_signal_names() {
    let h = Harness::new();
    let mut states = DpnsNameStateChangeSet::default();
    states.names.insert(id(9), name_state(9, "Alice"));
    h.feed(PlatformWalletChangeSet {
        dpns_name_states: Some(states),
        ..Default::default()
    });
    assert_eq!(h.changes(), BTreeSet::from([PlatformChange::Names]));
    assert!(h.journal().is_empty());
}

/// Journal writes are idempotent per (kind, contact, txid or request id),
/// and a replay keeps the row's read state.
#[test]
fn replayed_changesets_journal_each_event_once() {
    let h = Harness::new();
    let everything = || {
        let mut cs = contacts(incoming(ME, BOB, NEW_MS));
        Merge::merge(
            cs.contacts.as_mut().unwrap(),
            established(ME, CAROL, NEW_MS, NEW_MS + 1),
        );
        cs.identities =
            identities(vec![identity(ME, Some(0), &[("alice", Some(NEW_MS))])]).identities;
        cs.dashpay_payments_overlay =
            payment(ME, "aa11", BOB, PaymentDirection::Received).dashpay_payments_overlay;
        cs
    };
    h.feed(everything());
    let first = h.journal();
    assert_eq!(first.len(), 4);
    for _ in 0..3 {
        h.feed(everything());
    }
    assert_eq!(h.journal(), first);
}

/// Catch-up (DEC-125): a recovery phase moves the wallet's boundary to now,
/// never back. Events Platform dates before it are stored read, later ones
/// and undated ones are news; another wallet is not affected, and signals
/// are never silenced.
#[test]
fn a_recovery_phase_moves_the_boundary_forward_only() {
    let h = Harness::new();
    let other = WalletId([0xBB; 32]);
    let later_ms = (crate::events::unix_now() + 3_600) * 1_000;
    h.feed(contacts(incoming(ME, BOB, NEW_MS)));
    assert!(h.journal()[0].read_at.is_none(), "no boundary yet");

    h.tap.advance_catch_up(W);
    h.feed(contacts(incoming(ME, CAROL, NEW_MS)));
    h.feed(identities(vec![identity(
        ME,
        Some(0),
        &[("alice", Some(NEW_MS))],
    )]));
    h.times.name(ME, "dated", NEW_MS);
    h.feed(identities(vec![identity(ME, Some(0), &[("dated", None)])]));
    h.feed(payment(ME, "aa11", BOB, PaymentDirection::Received));
    h.feed(contacts(incoming(ME, 7, later_ms)));
    h.feed_to(other, contacts(incoming(ME, CAROL, NEW_MS)));
    assert_eq!(
        h.rows()[1..],
        [
            row(
                EventKind::RequestReceived,
                &b58(CAROL),
                &NEW_MS.to_string(),
                true
            ),
            // Undated: news (the declared residual).
            row(
                EventKind::UsernameRegistered,
                "",
                &format!("alice@{NEW_MS}"),
                false
            ),
            row(EventKind::UsernameRegistered, "", "dated", true),
            row(EventKind::PaymentReceived, &b58(BOB), "aa11", false),
            row(
                EventKind::RequestReceived,
                &b58(7),
                &later_ms.to_string(),
                false
            ),
        ]
    );
    let other_rows = h.appdb.journal(&other.to_string()).unwrap();
    assert!(other_rows[0].read_at.is_none());
    let pending = h.hub.pump.take_platform();
    assert!(pending.contains_key(&W) && pending.contains_key(&other));

    // A boundary ahead of now (a clock stepped back) stays; so does any
    // boundary under a first bring-up.
    let scope = dw_appdb::local_scope(&W.to_string());
    let ahead = (later_ms / 1_000 + 3_600).to_string();
    h.appdb
        .set_setting(&scope, CATCH_UP_BEFORE_KEY, Some(&ahead))
        .unwrap();
    h.tap.forget(&W);
    h.tap.advance_catch_up(W);
    h.tap.note_first_bring_up(W);
    assert_eq!(
        h.appdb.setting(&scope, CATCH_UP_BEFORE_KEY).unwrap(),
        Some(ahead)
    );
    h.feed(contacts(incoming(ME, 8, later_ms)));
    assert!(h.journal().last().unwrap().read_at.is_some());
}

/// What happened before the wallet's first bring-up here stays history
/// when it arrives after the recovery phases (a pass cut off by its
/// budget, a lock or a kill; review C1), by authoritative time only
/// (DEC-114): requests by `$createdAt`, relationships by their later
/// request, names by their marketplace row. A name's `acquired_at` (a
/// refresh stamps the fetch's wall clock) is never its age.
#[test]
fn history_arriving_after_the_restore_passes_is_stored_read() {
    let h = Harness::restored();
    h.feed(contacts(incoming(ME, BOB, OLD_MS)));
    h.feed(contacts(established(ME, CAROL, OLD_MS, OLD_MS + 1)));
    h.times.name(ME, "old", OLD_MS);
    h.times.name(ME, "new", NEW_MS);
    h.times.name(ME, "refreshed", OLD_MS);
    h.feed(identities(vec![identity(
        ME,
        Some(0),
        &[
            ("old", Some(OLD_MS)),
            ("new", Some(NEW_MS)),
            // Fetched now, created on Platform long ago.
            ("refreshed", Some(NEW_MS + 5)),
            // An old stamp, but no row: no authoritative time.
            ("unrowed", Some(OLD_MS)),
        ],
    )]));
    // Accepted after the restore, although the first request is older.
    h.feed(contacts(established(ME, 9, OLD_MS, NEW_MS)));
    h.feed(contacts(incoming(ME, 7, NEW_MS)));
    let read: Vec<(String, bool)> = h.rows().into_iter().map(|r| (r.0, r.3)).collect();
    assert_eq!(
        read,
        [
            ("request_received", true),
            ("request_accepted", true),
            ("username_registered", true),
            ("username_registered", false),
            ("username_registered", true),
            ("username_registered", false),
            ("request_accepted", false),
            ("request_received", false),
        ]
        .map(|(k, r)| (k.to_string(), r))
    );
}

/// The first bring-up's time is kept, persisted, and read back by a later
/// session's tap.
#[test]
fn the_first_bring_up_time_is_kept() {
    let h = Harness::restored();
    h.tap.note_first_bring_up(W);
    let scope = dw_appdb::local_scope(&W.to_string());
    let stored = || h.appdb.setting(&scope, CATCH_UP_BEFORE_KEY).unwrap();
    assert_eq!(stored(), Some(BEFORE.to_string()));
    let other = WalletId([0xBB; 32]);
    h.tap.note_first_bring_up(other);
    let scope = dw_appdb::local_scope(&other.to_string());
    let first = h.appdb.setting(&scope, CATCH_UP_BEFORE_KEY).unwrap();
    assert!(first.is_some());
    // A later session over the same database uses it.
    let later = ChangesetTap::new(Arc::clone(&h.hub), Arc::clone(&h.appdb));
    if let Some(c) = classify(&contacts(incoming(ME, BOB, OLD_MS))) {
        later.record(W, c, &h.times);
    }
    assert!(h.journal()[0].read_at.is_some());
}

/// A removed wallet imported again is a new first bring-up; a stored time
/// that cannot be read is never overwritten.
#[test]
fn the_first_bring_up_restarts_after_removal_and_survives_bad_reads() {
    let h = Harness::restored();
    h.tap.note_first_bring_up(W);
    let scope = dw_appdb::local_scope(&W.to_string());
    h.appdb.delete_wallet(&W.to_string()).unwrap();
    h.tap.forget(&W);
    h.tap.note_first_bring_up(W);
    let stored = h
        .appdb
        .setting(&scope, CATCH_UP_BEFORE_KEY)
        .unwrap()
        .unwrap();
    assert!(stored.parse::<u64>().unwrap() > BEFORE, "{stored}");

    let other = WalletId([0xBB; 32]);
    let scope = dw_appdb::local_scope(&other.to_string());
    h.appdb
        .set_setting(&scope, CATCH_UP_BEFORE_KEY, Some("garbled"))
        .unwrap();
    h.tap.note_first_bring_up(other);
    assert_eq!(
        h.appdb
            .setting(&scope, CATCH_UP_BEFORE_KEY)
            .unwrap()
            .as_deref(),
        Some("garbled")
    );
}

const ABANDON_12: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn engine(dir: &std::path::Path, sink: Arc<dyn EventSink>) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: dir.join("data"),
            worker_threads: Some(2),
            vault: dw_vault::VaultConfig {
                kdf: dw_vault::KdfPolicy::Fixed(dw_vault::KdfParams::TEST),
                os_store: Arc::new(dw_vault::MemoryOsStore::new()),
                ..dw_vault::VaultConfig::default()
            },
        },
        sink,
    )
    .unwrap()
}

/// An offline session (DAPI and peers unreachable) with an imported wallet.
fn session(engine: &Engine, no_platform: bool) -> (Arc<NetworkSession>, WalletId) {
    let s = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                no_platform,
                ..Default::default()
            },
        ))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let wallet = engine
        .block_on(s.import_wallet(
            Zeroizing::new(ABANDON_12.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    (s, wallet)
}

/// The wiring: a changeset the library stores through the session's
/// `WalletStore` reaches the journal and an `EngineEvent::Platform`; one the
/// SQLite persister refuses leaves no trace.
#[test]
fn the_session_store_taps_what_the_persister_accepts() {
    let dir = dw_testutil::private_tempdir();
    let sink = Arc::new(Recorder::default());
    let engine = engine(dir.path(), Arc::clone(&sink) as Arc<dyn EventSink>);
    let (s, wallet) = session(&engine, true);
    let store = s.live().unwrap().store;
    let names = || {
        let mut states = DpnsNameStateChangeSet::default();
        states.names.insert(id(9), name_state(9, "Alice"));
        // Payments need their identity's row (a foreign key).
        PlatformWalletChangeSet {
            dpns_name_states: Some(states),
            dashpay_payments_overlay: payment(ME, "aa11", BOB, PaymentDirection::Received)
                .dashpay_payments_overlay,
            ..identities(vec![identity(ME, Some(0), &[])])
        }
    };

    // Not a registered wallet: the persister refuses it.
    let stranger = WalletId([0x5A; 32]);
    assert!(store.store(stranger.0, names()).is_err());
    store.store(wallet.0, names()).unwrap();

    let journal = |w: WalletId| {
        engine
            .block_on(s.appdb_op(move |db| db.journal(&w.to_string())))
            .unwrap()
    };
    assert!(journal(stranger).is_empty());
    let rows = journal(wallet);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].kind, "payment_received");
    assert!(rows[0].read_at.is_none());

    let platform = |events: &[EngineEvent]| -> Vec<(WalletId, PlatformChange)> {
        events
            .iter()
            .filter_map(|e| match e {
                EngineEvent::Platform {
                    wallet_id, change, ..
                } => Some((*wallet_id, change.clone())),
                _ => None,
            })
            .collect()
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let seen = loop {
        let seen = platform(&sink.0.lock().unwrap());
        if seen.len() >= 3 || Instant::now() > deadline {
            break seen;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        seen,
        vec![
            (wallet, PlatformChange::Identities),
            (wallet, PlatformChange::Payments { identity: b58(ME) }),
            (wallet, PlatformChange::Names),
        ]
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// The bring-up records the wallet's first bring-up here (bringup.rs
/// notes it right after admission, before the first pass), and the setting
/// is local: a `.dwbackup` does not carry it.
#[test]
fn the_bring_up_records_the_first_bring_up() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine(dir.path(), Arc::new(Recorder::default()));
    let (s, wallet) = session(&engine, false);
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let stored = || {
        let scope = scope.clone();
        engine
            .block_on(s.appdb_op(move |db| db.setting(&scope, CATCH_UP_BEFORE_KEY)))
            .unwrap()
    };
    assert_eq!(stored(), None);
    engine.block_on(s.start_spv()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while stored().is_none() {
        assert!(Instant::now() < deadline, "no first bring-up recorded");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Not exported: a restore elsewhere starts its own.
    let exported = engine
        .block_on(s.appdb_op(move |db| db.export_wallet_rows(&wallet.to_string())))
        .unwrap();
    assert!(!format!("{exported:?}").contains(CATCH_UP_BEFORE_KEY));
    engine.block_on(engine.shutdown()).unwrap();
}
