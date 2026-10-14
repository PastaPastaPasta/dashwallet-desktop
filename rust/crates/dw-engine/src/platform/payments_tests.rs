//! Contact payments (DP3-02): history attribution both ways, the library's
//! word for a send past the resident pool, what stays unattributed,
//! contact activity (order, amounts, filters, pages, cursors) and the
//! frequent strip's ranking; then the facade over a session (history rows
//! and details, activity, the strip, errors, a busy wallet) and a replayed
//! restore.

use std::collections::{BTreeMap, HashMap};

use dashcore::address::Payload;
use dashcore::hashes::Hash;
use dashcore::{
    Address, BlockHash, Network, OutPoint, PubkeyHash, ScriptBuf, Transaction, TxIn, TxOut, Txid,
    Witness,
};
use dpp::prelude::Identifier;
use key_wallet::KeySource;
use key_wallet::account::AccountType;
use key_wallet::account::account_collection::DashpayAccountKey;
use key_wallet::bip32::{ExtendedPrivKey, ExtendedPubKey};
use key_wallet::managed_account::ManagedCoreFundsAccount;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
use platform_wallet::wallet::identity::{
    ContactProfileEntry, DashPayProfile, PaymentDirection, PaymentEntry, PaymentStatus,
};

use super::attribution::{ACTIVITY_PAGE, Payees, Score, Side, activity, frequent, scores};
use super::contacts::{ChannelState, ContactSummary, Relation};
use super::contacts_tests::{BOB, CAROL, DAVE, ERIN, Fixture, ME, b58, established_contact, id};
use super::payments::{ActivityDirection, ActivityFilter, ActivityItem, Counterparty};
use super::runtime::guard;
use crate::history::{
    AddressChain, ChainView, Labels, Owned, TxEntry, TxRecord, TxType, records_of,
};
use crate::{HistoryFilter, HistoryQuery, HistorySort, WalletId};

const NET: Network = Network::Regtest;
const DAY: u64 = 24 * 60 * 60;
/// "Now" for the frequent strip.
const NOW: u64 = 1_800_000_000;

fn addr(n: u8) -> Address {
    Address::new(
        NET,
        Payload::PubkeyHash(PubkeyHash::from_byte_array([n; 20])),
    )
}

fn out(value: u64, script: ScriptBuf) -> TxOut {
    TxOut {
        value,
        script_pubkey: script,
    }
}

fn input(txid: Txid, vout: u32) -> TxIn {
    TxIn {
        previous_output: OutPoint { txid, vout },
        script_sig: ScriptBuf::new(),
        sequence: u32::MAX,
        witness: Witness::default(),
    }
}

/// A transaction made unique by `n` (its lock time).
fn tx(n: u32, inputs: Vec<TxIn>, outputs: Vec<TxOut>) -> Transaction {
    Transaction {
        version: 2,
        lock_time: n,
        input: inputs,
        output: outputs,
        special_transaction_payload: None,
    }
}

fn mined(height: u32, time: u32) -> TransactionContext {
    TransactionContext::InBlock(BlockInfo::new(height, BlockHash::all_zeros(), time))
}

fn entry(t: Transaction, context: TransactionContext) -> TxEntry {
    TxEntry {
        tx: t,
        context,
        instant_locked: false,
        first_seen: None,
        own_inputs: BTreeMap::new(),
        abandoned: false,
    }
}

/// Funded by the wallet: input 0 is ours, worth `value`.
fn ours(mut e: TxEntry, value: u64) -> TxEntry {
    e.own_inputs.insert(0, (value, addr(1)));
    e
}

fn side(owner: u8, contact: u8, direction: ActivityDirection) -> Side {
    Side {
        owner: id(owner),
        contact: id(contact),
        direction,
    }
}

/// The wallet's own scripts (1, 2 receiving, 9 change, and each receival
/// address), its history and its payees: receival addresses 20+n (Bob
/// 22, Carol 23, …) and external addresses 30+n (Bob 32, Carol 33, …).
struct Pure {
    owned: HashMap<ScriptBuf, Owned>,
    history: HashMap<Txid, TxEntry>,
    payees: Payees,
}

fn receival(contact: u8) -> ScriptBuf {
    addr(20 + contact).script_pubkey()
}

fn external(contact: u8) -> ScriptBuf {
    addr(30 + contact).script_pubkey()
}

impl Pure {
    fn new() -> Self {
        let mut p = Self {
            owned: HashMap::new(),
            history: HashMap::new(),
            payees: Payees::default(),
        };
        for (n, chain) in [
            (1, AddressChain::Receiving),
            (2, AddressChain::Receiving),
            (9, AddressChain::Change),
        ] {
            p.own(addr(n), chain);
        }
        for contact in [BOB, CAROL, DAVE, ERIN] {
            p.own(addr(20 + contact), AddressChain::Receiving);
            (p.payees).add_script(
                receival(contact),
                side(ME, contact, ActivityDirection::Received),
            );
            (p.payees).add_script(
                external(contact),
                side(ME, contact, ActivityDirection::Sent),
            );
        }
        p
    }

    fn own(&mut self, a: Address, chain: AddressChain) {
        let owned = Owned {
            address: a.clone(),
            chain,
            coinjoin: false,
        };
        self.owned.insert(a.script_pubkey(), owned);
    }

    fn view(&self) -> ChainView<'_> {
        ChainView {
            owned: &self.owned,
            history: &self.history,
            tip: 1_000,
            chainlock_height: None,
            network: NET,
            payees: &self.payees,
        }
    }

    fn add(&mut self, e: TxEntry) -> Txid {
        let txid = e.tx.txid();
        self.history.insert(txid, e);
        txid
    }

    /// `contact` pays the wallet `value` (a foreign input), mined at `time`.
    fn received(&mut self, n: u32, contact: u8, value: u64, time: u32) -> Txid {
        let t = tx(
            n,
            vec![input(Txid::from_byte_array([0xF0; 32]), n)],
            vec![out(value, receival(contact))],
        );
        self.add(entry(t, mined(10, time)))
    }

    /// The wallet pays `contact` `value` with change and a 1,000-duff fee,
    /// mined at `time`.
    fn sent(&mut self, n: u32, contact: u8, value: u64, time: u32) -> Txid {
        let t = tx(
            n,
            vec![input(Txid::from_byte_array([0xF1; 32]), n)],
            vec![
                out(value, external(contact)),
                out(5_000, addr(9).script_pubkey()),
            ],
        );
        self.add(ours(entry(t, mined(10, time)), value + 6_000))
    }

    fn records(&self, txid: &Txid) -> Vec<TxRecord> {
        records_of(txid, &self.history[txid], &self.view(), &Labels::default()).0
    }

    fn page(
        &self,
        contact: u8,
        f: ActivityFilter,
        after: Option<&str>,
    ) -> (Vec<ActivityItem>, Option<String>) {
        let p = activity(&self.view(), &id(ME), &id(contact), f, after).unwrap();
        (p.items, p.next_cursor)
    }
}

fn counterparty(r: &TxRecord) -> Option<&str> {
    r.counterparty.as_ref().map(|c| c.identity.as_str())
}

/// An output to a receival address is a payment from that account's
/// friend; the wallet's other outputs of the same transaction are not.
#[test]
fn a_payment_from_a_contact_is_theirs() {
    let mut p = Pure::new();
    let t = tx(
        1,
        vec![input(Txid::from_byte_array([7; 32]), 0)],
        vec![
            out(70_000, receival(BOB)),
            out(5, addr(60).script_pubkey()),
            out(30_000, addr(2).script_pubkey()),
        ],
    );
    let txid = p.add(entry(t, mined(10, 1_000)));
    let records = p.records(&txid);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].tx_type, TxType::RecvWithAddress);
    assert_eq!(counterparty(&records[0]), Some(b58(BOB).as_str()));
    assert_eq!(counterparty(&records[1]), None);
}

/// An output to an external address in a transaction the wallet funded is
/// a payment to that account's friend; change and other payees are not.
#[test]
fn a_payment_to_a_contact_is_theirs() {
    let mut p = Pure::new();
    let t = tx(
        1,
        vec![input(Txid::from_byte_array([7; 32]), 0)],
        vec![
            out(5_000, addr(9).script_pubkey()),
            out(40_000, external(CAROL)),
            out(20_000, addr(61).script_pubkey()),
        ],
    );
    let txid = p.add(ours(entry(t, mined(10, 1_000)), 66_000));
    let records = p.records(&txid);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].tx_type, TxType::SendToAddress);
    assert_eq!(records[0].amount, -41_000, "the fee is on the first record");
    assert_eq!(counterparty(&records[0]), Some(b58(CAROL).as_str()));
    assert_eq!(counterparty(&records[1]), None);
}

/// A send to an address the resident pool lacks (past the gap after a
/// restore) is the contact's when the library's `Sent` entries name one
/// contact and the transaction has one payee; anything ambiguous stays
/// unattributed.
#[test]
fn a_send_past_the_pool_takes_the_librarys_word() {
    let mut p = Pure::new();
    let far = |n: u32, payees: &[u8]| {
        let mut outputs: Vec<TxOut> = (payees.iter())
            .map(|a| out(10_000, addr(*a).script_pubkey()))
            .collect();
        outputs.push(out(5_000, addr(9).script_pubkey()));
        let t = tx(n, vec![input(Txid::from_byte_array([7; 32]), n)], outputs);
        ours(entry(t, mined(10, 1_000)), 100_000)
    };
    let one = p.add(far(1, &[70]));
    let two = p.add(far(2, &[71, 72]));
    let split = p.add(far(3, &[73]));
    let no_entry = p.add(far(4, &[74]));
    // The wallet pays its own other identity (Bob's receival address is
    // ours, so change) and a stranger; the library's entry names Bob for
    // the output it matched, which is not the payee.
    let own = tx(
        5,
        vec![input(Txid::from_byte_array([7; 32]), 5)],
        vec![
            out(10_000, receival(BOB)),
            out(10_000, addr(75).script_pubkey()),
            out(5_000, addr(9).script_pubkey()),
        ],
    );
    let own = p.add(ours(entry(own, mined(10, 1_000)), 100_000));
    for txid in [one, two, split, own] {
        p.payees.add_sent(txid, id(ME), id(BOB));
    }
    p.payees.add_sent(split, id(ME), id(CAROL));

    let names = |txid: &Txid| {
        p.records(txid)
            .iter()
            .map(|r| counterparty(r).map(str::to_owned))
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&one), vec![Some(b58(BOB))]);
    assert_eq!(names(&two), vec![None, None], "two payees");
    assert_eq!(names(&split), vec![None], "two contacts");
    assert_eq!(names(&no_entry), vec![None], "no entry");
    assert_eq!(names(&own), vec![None], "a known DashPay output");
}

/// Not contact payments: an internal transfer to the wallet's own
/// receival address, a contact's external address in a transaction the
/// wallet did not fund, and a mixed debit.
#[test]
fn what_stays_unattributed() {
    let mut p = Pure::new();
    let internal = tx(
        1,
        vec![input(Txid::from_byte_array([7; 32]), 0)],
        vec![out(99_000, receival(BOB))],
    );
    let internal = p.add(ours(entry(internal, mined(10, 1_000)), 100_000));
    assert_eq!(p.records(&internal)[0].tx_type, TxType::SendToSelf);

    let foreign = tx(
        2,
        vec![input(Txid::from_byte_array([8; 32]), 0)],
        vec![
            out(10_000, external(BOB)),
            out(20_000, addr(1).script_pubkey()),
        ],
    );
    let foreign = p.add(entry(foreign, mined(10, 1_000)));

    let mixed = tx(
        3,
        vec![
            input(Txid::from_byte_array([9; 32]), 0),
            input(Txid::from_byte_array([9; 32]), 1),
        ],
        vec![out(10_000, external(BOB))],
    );
    let mixed = p.add(ours(entry(mixed, mined(10, 1_000)), 50_000));
    assert_eq!(p.records(&mixed)[0].tx_type, TxType::Other);

    for txid in [internal, foreign, mixed] {
        assert!(p.records(&txid).iter().all(|r| r.counterparty.is_none()));
    }
}

/// Activity is the pair's payments, both ways, newest first by block time
/// with unmined ones first; a sent amount is what the contact got (no
/// fee); abandoned transactions are left out; filters and other pairs.
#[test]
fn activity_is_the_pairs_payments_newest_first() {
    let mut p = Pure::new();
    let old_in = p.received(1, BOB, 70_000, 1_000);
    let out_ = p.sent(2, BOB, 40_000, 2_000);
    let new_in = p.received(3, BOB, 30_000, 3_000);
    p.received(4, CAROL, 1, 4_000);
    let pending = tx(
        5,
        vec![input(Txid::from_byte_array([0xF1; 32]), 5)],
        vec![out(90_000, external(BOB))],
    );
    let mut pending = ours(entry(pending, TransactionContext::Mempool), 100_000);
    pending.first_seen = Some(5_000);
    let pending = p.add(pending);
    let abandoned = tx(
        6,
        vec![input(Txid::from_byte_array([0xF1; 32]), 6)],
        vec![out(80_000, external(BOB))],
    );
    let mut abandoned = ours(entry(abandoned, TransactionContext::Mempool), 100_000);
    abandoned.abandoned = true;
    p.add(abandoned);

    let item = |txid: Txid, direction, amount, time: Option<u64>, mined: bool| ActivityItem {
        txid: txid.to_string(),
        direction,
        amount,
        height: mined.then_some(10),
        timestamp: time,
    };
    let (sent, received) = (ActivityDirection::Sent, ActivityDirection::Received);
    let all = vec![
        item(pending, sent, 90_000, Some(5_000), false),
        item(new_in, received, 30_000, Some(3_000), true),
        item(out_, sent, 40_000, Some(2_000), true),
        item(old_in, received, 70_000, Some(1_000), true),
    ];
    assert_eq!(p.page(BOB, ActivityFilter::All, None), (all.clone(), None));
    let only = |d| {
        all.iter()
            .filter(|i| i.direction == d)
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(p.page(BOB, ActivityFilter::Sent, None).0, only(sent));
    assert_eq!(
        p.page(BOB, ActivityFilter::Received, None).0,
        only(received)
    );
    assert_eq!(p.page(CAROL, ActivityFilter::All, None).0.len(), 1);
    assert!(p.page(DAVE, ActivityFilter::All, None).0.is_empty());
    let other_owner = activity(&p.view(), &id(CAROL), &id(BOB), ActivityFilter::All, None);
    assert!(other_owner.unwrap().items.is_empty());
}

/// Pages of [`ACTIVITY_PAGE`] items; each cursor is 63 characters of
/// `[0-9a-f]` plus the direction; pages never repeat or skip an item, and a
/// payment that lands between pages does not move later pages. A
/// malformed cursor is `invalid_argument`.
#[test]
fn activity_pages_by_cursor() {
    let mut p = Pure::new();
    let total = ACTIVITY_PAGE + 7;
    for n in 0..total as u32 {
        // Pairs in one block: the txid breaks the tie.
        p.received(n, BOB, 1_000 + u64::from(n), 1_000 + n / 2);
    }
    let (first, cursor) = p.page(BOB, ActivityFilter::All, None);
    assert_eq!(first.len(), ACTIVITY_PAGE);
    let cursor = cursor.unwrap();
    assert_eq!(cursor.len(), 63);
    assert!(
        cursor
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
    );
    // A newer payment lands between pages.
    p.received(9_999, BOB, 5, 99_999);
    let (second, end) = p.page(BOB, ActivityFilter::All, Some(&cursor));
    assert_eq!(end, None);
    assert_eq!(second.len(), 7);
    let mut seen: Vec<String> = first
        .iter()
        .chain(&second)
        .map(|i| i.txid.clone())
        .collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), total);
    assert!(first.windows(2).all(|w| w[0].timestamp >= w[1].timestamp));

    for bad in [
        "",
        "x",
        &cursor[..62],
        &format!("{}q", &cursor[..62]),
        &cursor.to_uppercase(),
    ] {
        let e = activity(&p.view(), &id(ME), &id(BOB), ActivityFilter::All, Some(bad));
        assert_eq!(e.unwrap_err().code(), "invalid_argument", "{bad:?}");
    }
}

fn summary(n: u8) -> (Identifier, ContactSummary) {
    (
        id(n),
        ContactSummary {
            contact: b58(n),
            username: None,
            display_name: None,
            alias: None,
            relation: Relation::Contact,
            hidden: false,
            channel: ChannelState::Ready,
            since: None,
            last_activity_at: None,
            unverified: false,
        },
    )
}

/// The strip: most often paid first, then most often paying to fill the
/// limit; only payments in the last 90 days (an unmined one counts as
/// now); ties by the latest payment, then the id; no contact without one.
#[test]
fn the_frequent_strip_ranks_sent_then_received() {
    let mut p = Pure::new();
    let at = |days_ago: u64| u32::try_from(NOW - days_ago * DAY).unwrap();
    let mut n = 0;
    let mut next = || {
        n += 1;
        n
    };
    // Bob: paid twice. Carol: paid once, paid us five times. Dave: paid
    // us three times. Erin: paid once, 91 days ago.
    p.sent(next(), BOB, 1_000, at(1));
    p.sent(next(), BOB, 1_000, at(80));
    p.sent(next(), CAROL, 1_000, at(2));
    for _ in 0..5 {
        p.received(next(), CAROL, 1_000, at(3));
    }
    for _ in 0..3 {
        p.received(next(), DAVE, 1_000, at(4));
    }
    p.sent(next(), ERIN, 1_000, at(91));

    let s = scores(&p.view(), &id(ME), NOW);
    assert_eq!(
        s.get(&id(CAROL)),
        Some(&Score {
            sent: 1,
            last_sent: NOW - 2 * DAY,
            received: 5,
            last_received: NOW - 3 * DAY,
        })
    );
    assert_eq!(s.get(&id(ERIN)), None, "outside the window");
    assert!(scores(&p.view(), &id(BOB), NOW).is_empty(), "another owner");

    let everyone = || [BOB, CAROL, DAVE, ERIN, 6].map(summary).to_vec();
    let strip = |limit| {
        (frequent(everyone(), &s, limit).into_iter())
            .map(|c| c.contact)
            .collect::<Vec<_>>()
    };
    let names = |ns: &[u8]| ns.iter().map(|n| b58(*n)).collect::<Vec<_>>();
    assert_eq!(strip(4), names(&[BOB, CAROL, DAVE]));
    assert_eq!(strip(2), names(&[BOB, CAROL]));
    assert_eq!(strip(1), names(&[BOB]));

    // An unmined payment counts as now; a tie breaks on the latest.
    let unmined = tx(
        99,
        vec![input(Txid::from_byte_array([0xF1; 32]), 99)],
        vec![out(1_000, external(DAVE))],
    );
    p.add(ours(entry(unmined, TransactionContext::Mempool), 2_000));
    p.sent(next(), DAVE, 1_000, at(85));
    let s = scores(&p.view(), &id(ME), NOW);
    assert_eq!(s[&id(DAVE)].last_sent, NOW);
    let strip: Vec<String> = (frequent(everyone(), &s, 4).into_iter())
        .map(|c| c.contact)
        .collect();
    assert_eq!(strip, names(&[DAVE, BOB, CAROL]));

    // Equal counts and times: the id decides.
    let tie = BTreeMap::from([
        (
            id(CAROL),
            Score {
                sent: 1,
                last_sent: 5,
                ..Default::default()
            },
        ),
        (
            id(BOB),
            Score {
                sent: 1,
                last_sent: 5,
                ..Default::default()
            },
        ),
    ]);
    let strip: Vec<String> = (frequent(everyone(), &tie, 4).into_iter())
        .map(|c| c.contact)
        .collect();
    let mut expected = names(&[BOB, CAROL]);
    expected.sort();
    assert_eq!(strip, expected);
    assert!(frequent(everyone(), &BTreeMap::new(), 4).is_empty());
}

// ---------------------------------------------------------------------------
// Through a session
// ---------------------------------------------------------------------------

/// The DashPay account pair of our identity and `contact`, as the library
/// builds it: a receival account from our seed (here a test key) and the
/// external one from the contact's request.
fn account_xpub(seed: u8) -> ExtendedPubKey {
    let master = ExtendedPrivKey::new_master(NET, &[seed; 32]).unwrap();
    ExtendedPubKey::from_priv(&master)
}

fn dashpay_account(account_type: AccountType, xpub: ExtendedPubKey) -> ManagedCoreFundsAccount {
    ManagedCoreFundsAccount::from_account(&key_wallet::Account {
        parent_wallet_id: None,
        account_type,
        network: NET,
        account_xpub: xpub,
        is_watch_only: true,
    })
}

/// What a session test needs of its wallet's DashPay accounts.
struct Accounts {
    /// Receival address scripts of each contact, by index.
    receival: BTreeMap<u8, Vec<ScriptBuf>>,
    /// External address scripts.
    external: BTreeMap<u8, Vec<ScriptBuf>>,
}

impl Fixture {
    /// Adds our DashPay account pair with each of `contacts`, the external
    /// pools grown by `extra` addresses past what a fresh account holds.
    fn accounts(&self, contacts: &[u8], extra: u32) -> Accounts {
        let manager = self.s.manager().unwrap();
        let wallet = self.wallet;
        let contacts = contacts.to_vec();
        self.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let info = wm.get_wallet_info_mut(&wallet.0).unwrap();
            let mut accounts = Accounts {
                receival: BTreeMap::new(),
                external: BTreeMap::new(),
            };
            for c in contacts {
                let key = |user: u8, friend: u8| DashpayAccountKey {
                    index: 0,
                    user_identity_id: [user; 32],
                    friend_identity_id: [friend; 32],
                };
                let k = key(ME, c);
                let receive = dashpay_account(
                    AccountType::DashpayReceivingFunds {
                        index: 0,
                        user_identity_id: k.user_identity_id,
                        friend_identity_id: k.friend_identity_id,
                    },
                    account_xpub(c),
                );
                let xpub = account_xpub(100 + c);
                let mut pay = dashpay_account(
                    AccountType::DashpayExternalAccount {
                        index: 0,
                        user_identity_id: k.user_identity_id,
                        friend_identity_id: k.friend_identity_id,
                    },
                    xpub,
                );
                for pool in pay.managed_account_type_mut().address_pools_mut() {
                    pool.generate_addresses(extra, &KeySource::Public(xpub), true)
                        .unwrap();
                }
                let scripts = |a: &ManagedCoreFundsAccount| {
                    let pool = a.managed_account_type().address_pools()[0];
                    let mut by_index: Vec<(u32, ScriptBuf)> = (pool.addresses.iter())
                        .map(|(i, info)| (*i, info.script_pubkey.clone()))
                        .collect();
                    by_index.sort();
                    by_index.into_iter().map(|(_, s)| s).collect::<Vec<_>>()
                };
                accounts.receival.insert(c, scripts(&receive));
                accounts.external.insert(c, scripts(&pay));
                let collection = &mut info.core_wallet.accounts;
                collection.insert_funds_bearing_account(receive).unwrap();
                collection.insert_funds_bearing_account(pay).unwrap();
            }
            accounts
        })
    }

    /// Our own BIP44 receiving address `n`.
    fn own_script(&self, n: u32) -> ScriptBuf {
        let manager = self.s.manager().unwrap();
        let wallet = self.wallet;
        self.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let info = wm.get_wallet_info(&wallet.0).unwrap();
            let account = &info.core_wallet.accounts.standard_bip44_accounts[&0];
            let pool = account.managed_account_type().address_pools()[0];
            pool.addresses[&n].script_pubkey.clone()
        })
    }

    fn put(&self, e: TxEntry) -> Txid {
        let txid = e.tx.txid();
        self.s.hub.history.with_wallet(self.wallet, |h| {
            h.txs.insert(txid, e);
        });
        txid
    }

    fn history(&self) -> Vec<TxRecord> {
        let q = HistoryQuery {
            filter: HistoryFilter::default(),
            sort: HistorySort::NewestFirst,
            cursor: None,
            limit: 500,
        };
        (self.engine.block_on(self.s.history_page(self.wallet, q)))
            .unwrap()
            .records
    }

    fn library_payment(
        &self,
        txid: Txid,
        contact: u8,
        direction: PaymentDirection,
        status: PaymentStatus,
        amount: u64,
    ) {
        self.with_me(move |m, _| {
            (m.dashpay_payments_mut()).insert(
                txid.to_string(),
                PaymentEntry {
                    counterparty_id: id(contact),
                    amount_duffs: amount,
                    memo: None,
                    direction,
                    status,
                },
            );
        });
    }
}

/// Our identity with Bob and Carol as established contacts, Bob's profile
/// cached.
fn with_contacts() -> Fixture {
    let f = Fixture::new(true);
    f.give_me();
    f.with_me(|m, _| {
        m.apply_established_contact(established_contact(BOB, 1_000_000, 2_000_000));
        m.apply_established_contact(established_contact(CAROL, 1_000_000, 2_000_000));
        (m.dashpay_contact_profiles_mut()).insert(
            id(BOB),
            ContactProfileEntry {
                profile: Some(DashPayProfile {
                    display_name: Some("Bob".into()),
                    avatar_url: Some("https://example.com/bob.png".into()),
                    ..Default::default()
                }),
                checked_at_ms: 0,
            },
        );
    });
    f
}

/// The two payments of the session tests: Bob pays our receival address 0,
/// and we pay Bob's external address `index` from a coin of our own.
fn pay_both_ways(f: &Fixture, a: &Accounts, index: usize, first_seen: Option<u64>) -> (Txid, Txid) {
    let from_bob = tx(
        1,
        vec![input(Txid::from_byte_array([0xB0; 32]), 0)],
        vec![out(70_000, a.receival[&BOB][0].clone())],
    );
    let mut from_bob = entry(from_bob, mined(5, 1_000));
    from_bob.first_seen = first_seen;
    let to_bob = tx(
        2,
        vec![input(Txid::from_byte_array([0xA0; 32]), 0)],
        vec![
            out(40_000, a.external[&BOB][index].clone()),
            out(5_000, f.own_script(1)),
        ],
    );
    let mut to_bob = entry(to_bob, mined(6, 2_000));
    to_bob.own_inputs.insert(
        0,
        (46_000, Address::from_script(&f.own_script(0), NET).unwrap()),
    );
    to_bob.first_seen = first_seen;
    (f.put(from_bob), f.put(to_bob))
}

fn bob() -> Counterparty {
    Counterparty {
        identity: b58(BOB),
        username: None,
        display_name: Some("Bob".into()),
        avatar_url: Some("https://example.com/bob.png".into()),
    }
}

/// History rows and the transaction detail carry the counterparty both
/// ways; activity and the strip read the same attribution.
#[test]
fn the_facade_attributes_payments_both_ways() {
    let f = with_contacts();
    let a = f.accounts(&[BOB, CAROL], 0);
    let (from_bob, to_bob) = pay_both_ways(&f, &a, 3, Some(500));

    let rows = f.history();
    let row = |txid: Txid| rows.iter().find(|r| r.txid == txid.to_string()).unwrap();
    assert_eq!(row(from_bob).counterparty, Some(bob()));
    assert_eq!(row(from_bob).amount, 70_000);
    assert_eq!(row(to_bob).counterparty, Some(bob()));
    assert_eq!(row(to_bob).tx_type, TxType::SendToAddress);
    for txid in [from_bob, to_bob] {
        let detail = (f.engine.block_on(f.s.tx_detail(f.wallet, txid.to_string()))).unwrap();
        assert_eq!(detail.records[0].counterparty, Some(bob()));
    }

    let dp = f.dp();
    let page =
        (f.engine
            .block_on(dp.contact_activity(b58(ME), b58(BOB), None, ActivityFilter::All)))
        .unwrap();
    let got: Vec<(String, ActivityDirection, u64)> = (page.items.iter())
        .map(|i| (i.txid.clone(), i.direction, i.amount))
        .collect();
    assert_eq!(
        got,
        vec![
            (to_bob.to_string(), ActivityDirection::Sent, 40_000),
            (from_bob.to_string(), ActivityDirection::Received, 70_000),
        ]
    );
    assert_eq!(
        page.items[0].timestamp,
        Some(2_000),
        "block time, not first seen"
    );
    assert_eq!(page.next_cursor, None);
    let carol =
        f.engine
            .block_on(dp.contact_activity(b58(ME), b58(CAROL), None, ActivityFilter::All));
    assert!(carol.unwrap().items.is_empty());

    // Block times of 1970: outside the window, so no strip until a
    // payment is recent.
    assert!(dp.frequent_contacts(b58(ME), 4).unwrap().is_empty());
    let recent = tx(
        3,
        vec![input(Txid::from_byte_array([0xC0; 32]), 0)],
        vec![out(1_000, a.receival[&CAROL][0].clone())],
    );
    f.put(entry(recent, TransactionContext::Mempool));
    let strip = dp.frequent_contacts(b58(ME), 4).unwrap();
    assert_eq!(
        strip.iter().map(|c| c.contact.clone()).collect::<Vec<_>>(),
        vec![b58(CAROL)]
    );
    assert_eq!(strip[0].relation, Relation::Contact);
}

/// A restore replays the same payments: history only from the chain (no
/// first-seen times), a fresh external pool that no longer reaches the
/// address we paid, and the library's entries as its DashPay pass rebuilds
/// them (`Sent` from the transaction history; no `Received` for a spent
/// coin). Attribution, activity and the strip read the same as before.
#[test]
fn a_replayed_restore_attributes_the_same() {
    use PaymentDirection::{Received, Sent};
    use PaymentStatus::{Confirmed, Failed};

    // Live: the external pool grew to the paid address as we paid it.
    let live = with_contacts();
    let la = live.accounts(&[BOB, CAROL], 30);
    let fresh = la.external[&BOB].len() - 30;
    let (l_from, l_to) = pay_both_ways(&live, &la, fresh + 20, Some(500));
    live.library_payment(l_to, BOB, Sent, Confirmed, 40_000);
    live.library_payment(l_from, BOB, Received, Confirmed, 70_000);

    let restored = with_contacts();
    let ra = restored.accounts(&[BOB, CAROL], 0);
    assert_eq!(ra.external[&BOB].len(), fresh);
    assert_eq!(ra.external[&BOB][..], la.external[&BOB][..fresh]);
    let mut ra_far = ra;
    ra_far.external.insert(BOB, la.external[&BOB].clone());
    let (r_from, r_to) = pay_both_ways(&restored, &ra_far, fresh + 20, None);
    assert_eq!((r_from, r_to), (l_from, l_to));
    // Before the library's pass: the send is not attributed yet.
    let row = |f: &Fixture, txid: Txid| {
        (f.history().into_iter())
            .find(|r| r.txid == txid.to_string())
            .unwrap()
    };
    assert_eq!(row(&restored, r_to).counterparty, None);
    assert_eq!(row(&restored, r_from).counterparty, Some(bob()));
    // Only a `Sent` entry that did not fail counts.
    for (direction, status) in [(Sent, Failed), (Received, Confirmed)] {
        restored.library_payment(r_to, BOB, direction, status, 40_000);
        assert_eq!(
            row(&restored, r_to).counterparty,
            None,
            "{direction:?} {status:?}"
        );
    }
    restored.library_payment(r_to, BOB, Sent, Confirmed, 40_000);

    let strip_of = |f: &Fixture| {
        (f.history().into_iter())
            .map(|r| ((r.txid, r.record_index), r.counterparty))
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(strip_of(&restored), strip_of(&live));
    let activity_of = |f: &Fixture| {
        let dp = f.dp();
        (f.engine
            .block_on(dp.contact_activity(b58(ME), b58(BOB), None, ActivityFilter::All)))
        .unwrap()
    };
    assert_eq!(activity_of(&restored), activity_of(&live));
    assert_eq!(activity_of(&live).items.len(), 2);
}

/// What the calls refuse.
#[test]
fn the_calls_refuse_what_they_cannot_read() {
    let f = with_contacts();
    let dp = f.dp();
    let act = |identity: String, contact: String, cursor: Option<&str>| {
        let cursor = cursor.map(str::to_owned);
        (f.engine
            .block_on(dp.contact_activity(identity, contact, cursor, ActivityFilter::All)))
        .map(|_| ())
        .map_err(|e| e.code())
    };
    assert_eq!(act(b58(ME), b58(BOB), None), Ok(()));
    assert_eq!(
        act("not base58!".into(), b58(BOB), None),
        Err("invalid_argument")
    );
    assert_eq!(
        act(b58(ME), "not base58!".into(), None),
        Err("invalid_argument")
    );
    assert_eq!(act(b58(ME), b58(ME), None), Err("contact.self"));
    assert_eq!(act(b58(9), b58(BOB), None), Err("identity.not_found"));
    assert_eq!(act(b58(ME), b58(BOB), Some("zz")), Err("invalid_argument"));
    // A stranger is no error: nothing was paid.
    assert_eq!(act(b58(ME), b58(9), None), Ok(()));

    let strip = |identity: String| dp.frequent_contacts(identity, 4).map_err(|e| e.code());
    assert_eq!(strip(b58(ME)), Ok(Vec::new()));
    assert_eq!(strip("not base58!".into()), Err("invalid_argument"));
    assert_eq!(strip(b58(9)), Err("identity.not_found"));

    let stranger = f.s.dashpay(WalletId([9; 32]));
    assert_eq!(
        (f.engine.block_on(stranger.contact_activity(
            b58(ME),
            b58(BOB),
            None,
            ActivityFilter::All
        )))
        .unwrap_err()
        .code(),
        "wallet_not_found"
    );
    assert_eq!(
        stranger.frequent_contacts(b58(ME), 4).unwrap_err().code(),
        "wallet_not_found"
    );
}

/// While a writer holds the library's state the strip answers from the
/// last view it read; with none yet, empty.
#[test]
fn a_busy_wallet_strips_from_its_last_view() {
    let f = with_contacts();
    let a = f.accounts(&[BOB], 0);
    let recent = tx(
        1,
        vec![input(Txid::from_byte_array([0xC0; 32]), 0)],
        vec![out(1_000, a.receival[&BOB][0].clone())],
    );
    f.put(entry(recent, TransactionContext::Mempool));
    let dp = f.dp();
    let live = dp.frequent_contacts(b58(ME), 4).unwrap();
    assert_eq!(live.len(), 1);

    let manager = f.s.manager().unwrap();
    let wm = manager.wallet_manager_arc();
    let held = f.engine.block_on(wm.write());
    assert_eq!(dp.frequent_contacts(b58(ME), 4).unwrap(), live);
    // The contacts snapshot stays; the view is gone.
    guard(&f.s.platform.contacts.views).remove(&f.wallet);
    assert!(dp.frequent_contacts(b58(ME), 4).unwrap().is_empty());
    drop(held);
    assert_eq!(dp.frequent_contacts(b58(ME), 4).unwrap(), live);
}

// ---------------------------------------------------------------------------
// T2: public testnet
// ---------------------------------------------------------------------------

/// T2 on the testnet fixture (`DW_DP302_TESTNET_FIXTURE`, wallets A and B,
/// established contacts): both wallets restored from their phrases.
/// - Received: B's history attributes the fixture's A→B payment (DP1-05's,
///   to B's receival address 0) to A, and so do B's activity and strip.
/// - Sent: the first run pays B's DashPay address 0 from A's Core funds
///   (100,000 duffs) and records the txid in `DP3-02.json`; A sees it sent
///   to B and B received from A. Later runs send nothing and check the
///   recorded payment after the restore, both ways.
///
/// Address 0 is inside the pool a restore rebuilds, so the sent side is
/// attributed by its address; the library-entry fallback for sends past
/// the pool is T0 only (`a_replayed_restore_attributes_the_same`).
#[test]
#[ignore = "public testnet; set DW_DP302_TESTNET_FIXTURE"]
fn t2_testnet_contact_payments_after_a_restore() {
    use std::time::{Duration, Instant};

    use dw_vault::{Credential, GrantPurpose};
    use zeroize::Zeroizing;

    use super::recovery_tests::engine as testnet_engine;
    use crate::{DashNetwork, EngineError, ImportOptions, Recipient, SendFailure, SessionOptions};

    const PAY: u64 = 100_000;
    let fixture = std::path::PathBuf::from(
        std::env::var("DW_DP302_TESTNET_FIXTURE").expect("DW_DP302_TESTNET_FIXTURE"),
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    let birth = meta["funding"]["birth_height_hint"].as_u64().unwrap() as u32;
    let identity = |w: &str| meta[w]["identity_id"].as_str().unwrap().to_owned();
    let (a_id, b_id) = (identity("A"), identity("B"));
    let received = meta["contact_payments"][0]["txid"]
        .as_str()
        .unwrap()
        .to_owned();
    let record = fixture.join("DP3-02.json");

    let dir = dw_testutil::private_tempdir();
    let engine = testnet_engine(dir.path());
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
    let wait = |what: &str, secs: u64, done: &mut dyn FnMut() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_secs(1));
        }
        eprintln!("{what} after {:?}", t0.elapsed());
    };
    wait("the wallets at the tip", 3600, &mut || {
        let snap = s.sync_snapshot().unwrap();
        snap.caught_up
            && snap.tip_height.is_some_and(|tip| {
                [a, b]
                    .iter()
                    .all(|w| s.wallet_scan_height(w).is_some_and(|h| h >= tip))
            })
    });
    let row = |w: WalletId, txid: &str| {
        let q = HistoryQuery {
            filter: HistoryFilter::default(),
            sort: HistorySort::NewestFirst,
            cursor: None,
            limit: 500,
        };
        let page = engine.block_on(s.history_page(w, q)).unwrap();
        page.records.into_iter().find(|r| r.txid == txid)
    };
    let from = |r: &Option<TxRecord>| {
        (r.as_ref()).and_then(|r| r.counterparty.as_ref().map(|c| c.identity.clone()))
    };

    // Received after a restore: the fixture's payment, A to B.
    let mut got = None;
    wait("B's payment from A", 600, &mut || {
        got = row(b, &received);
        from(&got) == Some(a_id.clone())
    });
    let got = got.unwrap();
    eprintln!("B's row: {got:?}");
    assert_eq!(got.tx_type, TxType::RecvWithAddress);
    let activity = |w: WalletId, owner: &str, contact: &str| {
        let dp = s.dashpay(w);
        let page = engine.block_on(dp.contact_activity(
            owner.into(),
            contact.into(),
            None,
            ActivityFilter::All,
        ));
        page.unwrap().items
    };
    let b_activity = activity(b, &b_id, &a_id);
    assert!(
        (b_activity.iter())
            .any(|i| i.txid == received && i.direction == ActivityDirection::Received)
    );

    // Sent: A pays B's DashPay address once; every run checks it.
    let sent = match std::fs::read(&record) {
        Ok(bytes) => {
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let txid = json["sent_txid"].as_str().unwrap().to_owned();
            let broadcast = json["broadcast"].as_str().unwrap_or_default();
            assert!(
                !broadcast.starts_with("failed"),
                "{txid} was refused ({broadcast}); delete {} to pay again",
                record.display()
            );
            txid
        }
        Err(_) => {
            let manager = s.manager().unwrap();
            let (user, friend) = (id_bytes(&a_id), id_bytes(&b_id));
            let address = engine.block_on(async {
                let wm = manager.wallet_manager_arc();
                let wm = wm.read().await;
                let info = wm.get_wallet_info(&a.0).unwrap();
                let accounts = &info.core_wallet.accounts.dashpay_external_accounts;
                let (_, account) = (accounts.iter())
                    .find(|(k, _)| k.user_identity_id == user && k.friend_identity_id == friend)
                    .expect("A's DashPay account paying B");
                let pool = account.managed_account_type().address_pools()[0];
                pool.addresses[&0].address.to_string()
            });
            eprintln!("A pays B's DashPay address {address}");
            let draft = s.new_tx_draft(a).unwrap();
            draft
                .set_recipients(vec![Recipient {
                    address,
                    amount: PAY,
                    subtract_fee_from_amount: false,
                    label: None,
                    message: None,
                }])
                .unwrap();
            let grant = (s.vault())
                .authorize(
                    GrantPurpose::Spend {
                        max_duffs: 10 * PAY,
                    },
                    Some(&a.0),
                    Credential::None,
                )
                .unwrap()
                .id;
            let prepared = engine.block_on(draft.prepare(grant)).unwrap();
            let raw = prepared.raw().unwrap();
            let tx: dashcore::Transaction = dashcore::consensus::deserialize(&raw).unwrap();
            let txid = tx.txid().to_string();
            // Recorded first, and only by one run: a rerun after an unknown
            // outcome never pays twice.
            let write = |broadcast: &str, new: bool| {
                let json = serde_json::json!({
                    "sent_txid": txid, "duffs": PAY, "from": "A", "to": "B", "broadcast": broadcast,
                });
                let mut file = (std::fs::OpenOptions::new().write(true).truncate(true))
                    .create_new(new)
                    .open(&record)
                    .unwrap();
                std::io::Write::write_all(&mut file, &serde_json::to_vec_pretty(&json).unwrap())
                    .unwrap();
            };
            write("pending", true);
            match engine.block_on(draft.broadcast(prepared)) {
                Ok(_) => write("accepted", false),
                Err(EngineError::Send(SendFailure::BroadcastUnknown { reason })) => {
                    write(&format!("unknown: {reason}"), false)
                }
                Err(e) => {
                    write(&format!("failed: {e}"), false);
                    panic!("the payment was refused: {e}");
                }
            }
            eprintln!("broadcast {txid}");
            txid
        }
    };
    let mut got = None;
    wait("A's payment to B", 1800, &mut || {
        got = row(a, &sent);
        from(&got) == Some(b_id.clone())
    });
    let got = got.unwrap();
    eprintln!("A's row: {got:?}");
    assert_eq!(got.tx_type, TxType::SendToAddress);
    assert_eq!(
        got.amount,
        -i64::try_from(PAY).unwrap() - i64::try_from(got.fee.unwrap()).unwrap()
    );
    wait("B's payment from A, sent by A", 1800, &mut || {
        from(&row(b, &sent)) == Some(a_id.clone())
    });
    let a_activity = activity(a, &a_id, &b_id);
    let item = a_activity
        .iter()
        .find(|i| i.txid == sent)
        .expect("A's activity");
    assert_eq!(
        (item.direction, item.amount),
        (ActivityDirection::Sent, PAY)
    );
    assert!(activity(b, &b_id, &a_id).iter().any(|i| i.txid == sent));

    // The strips: B was paid by A in the last 90 days, and A paid B.
    let strip = |w: WalletId, owner: &str| {
        let list = s.dashpay(w).frequent_contacts(owner.into(), 5).unwrap();
        list.into_iter().map(|c| c.contact).collect::<Vec<_>>()
    };
    assert_eq!(strip(a, &a_id).first(), Some(&b_id));
    assert!(strip(b, &b_id).contains(&a_id));
    engine.block_on(engine.shutdown()).unwrap();
}

fn id_bytes(b58: &str) -> [u8; 32] {
    use dpp::platform_value::string_encoding::Encoding;
    Identifier::from_string(b58, Encoding::Base58)
        .unwrap()
        .to_buffer()
}
