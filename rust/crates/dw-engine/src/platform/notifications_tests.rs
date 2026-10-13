//! The bell over a session (DP2-05): a restore's history makes no news,
//! one request is one news event, the feed pages, `mark_read` and the
//! counts, an ignored sender goes quiet, and the errors.

use std::time::{Duration, Instant};

use std::sync::Arc;
use std::sync::atomic::Ordering;

use dw_vault::{Credential, GrantPurpose};
use zeroize::Zeroizing;

use super::SpvState;
use super::contacts::{ContactQuery, ContactSection, ContactSort, Relation};
use super::contacts_tests::{Fixture, wait_for as wait_long};
use super::errors::PlatformError;
use super::journal::ChangesetTap;
use super::journal_tests::{self, Recorder, b58, contacts, incoming, request};
use super::notifications::{DashPayEvent, EventKind, EventPage};
use crate::coins::TestPause;
use crate::held_writes::ReleaseOnDrop;
use crate::{
    DashNetwork, EngineEvent, EventSink, ImportOptions, PlatformChange, SessionOptions, WalletId,
};

const ME: u8 = 1;
const DAVE: u8 = 4;
/// The wallet's catch-up boundary (UNIX s) and request times either side
/// of it (ms).
const BEFORE: u64 = 1_000;
const OLD_MS: u64 = 500_000;
const NEW_MS: u64 = 2_000_000;
/// Historic senders: 40 of them.
const SENDERS: std::ops::Range<u8> = 10..50;

fn wait_for(what: &str, done: impl FnMut() -> bool) {
    wait_long(what, Duration::from_secs(20), done);
}

impl Fixture {
    /// Every `DashPayNews` event delivered so far, flattened.
    fn news(&self) -> Vec<DashPayEvent> {
        (self.sink.0.lock().unwrap().iter())
            .filter_map(|e| match e {
                EngineEvent::DashPayNews {
                    wallet_id,
                    identity,
                    events,
                    more,
                    ..
                } => {
                    assert_eq!(
                        (*wallet_id, identity.as_str(), *more),
                        (self.wallet, b58(ME).as_str(), 0)
                    );
                    Some(events.clone())
                }
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn feed_signals(&self) -> usize {
        let of = PlatformChange::Events { identity: b58(ME) };
        (self.sink.0.lock().unwrap().iter())
            .filter(|e| matches!(e, EngineEvent::Platform { change, .. } if *change == of))
            .count()
    }

    fn events(&self, cursor: Option<u64>, limit: u32) -> EventPage {
        (self.engine)
            .block_on(self.dp().events(b58(ME), cursor, limit))
            .unwrap()
    }

    fn unread(&self) -> u32 {
        self.dp().unread_count(b58(ME)).unwrap()
    }

    /// A wallet with an identity whose history predates its boundary: 40
    /// incoming requests, as a restore's first pass brings them back.
    fn restored() -> Self {
        let f = Fixture::new(true);
        f.give_me();
        let tap = f.s.live().unwrap().tap;
        assert!(tap.move_boundary_to(f.wallet, BEFORE, false));
        f.with_me(|m, p| {
            for n in SENDERS {
                m.add_incoming_contact_request(request(n, ME, OLD_MS + u64::from(n)), p)
                    .unwrap();
            }
        });
        f
    }

    /// A request from Dave after the boundary, once its news is out.
    fn dave_asks(&self) -> DashPayEvent {
        let before = self.news().len();
        self.with_me(|m, p| {
            m.add_incoming_contact_request(request(DAVE, ME, NEW_MS), p)
                .unwrap();
        });
        wait_for("Dave's news", || self.news().len() > before);
        self.news().pop().unwrap()
    }
}

fn contacts_in(events: &[DashPayEvent]) -> Vec<String> {
    events.iter().filter_map(|e| e.contact.clone()).collect()
}

/// No notification storm after a restore, and one event per request
/// received: 40 historic requests make no news and count nothing unread,
/// but are all in the bell (Pending and Earlier); the first request after
/// the boundary is one news event, one unread row, and a replay of it is
/// none.
#[test]
fn a_restore_makes_no_news_and_a_new_request_one() {
    let f = Fixture::restored();
    // The journal's feed signal is out once the rows are written.
    wait_for("the feed signal", || f.feed_signals() > 0);
    assert_eq!(f.unread(), 0, "before the first count");
    wait_for("the first count", || {
        f.s.live().unwrap().tap.unread(f.wallet, &b58(ME)).is_some()
    });
    assert_eq!(f.unread(), 0);
    let page = f.events(None, 200);
    assert_eq!(page.pending.len(), SENDERS.len());
    assert!(page.new.is_empty());
    assert_eq!(page.earlier.len(), SENDERS.len());
    assert!(
        page.earlier
            .iter()
            .all(|e| e.kind == EventKind::RequestReceived)
    );
    assert_eq!(page.next_cursor, None);
    std::thread::sleep(Duration::from_millis(400));
    assert!(f.news().is_empty(), "a storm: {:?}", f.news());

    let news = f.dave_asks();
    assert_eq!(news.kind, EventKind::RequestReceived);
    assert_eq!(news.contact, Some(b58(DAVE)));
    assert_eq!(news.reference, Some(NEW_MS.to_string()));
    wait_for("Dave counted", || f.unread() == 1);
    let page = f.events(None, 200);
    assert_eq!(
        page.new,
        [DashPayEvent {
            at: page.new[0].at,
            ..news.clone()
        }]
    );
    assert_eq!(page.pending.len(), SENDERS.len() + 1);

    // The library's changeset replayed: no row, no news.
    f.store(contacts(incoming(ME, DAVE, NEW_MS)));
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(f.news().len(), 1);
    assert_eq!(f.journal().len(), SENDERS.len() + 1);

    // Read: the count drops and the row moves to Earlier.
    let signals = f.feed_signals();
    (f.engine)
        .block_on(f.dp().mark_read(b58(ME), news.id))
        .unwrap();
    assert_eq!(f.unread(), 0);
    wait_for("the feed signal", || f.feed_signals() > signals);
    let page = f.events(None, 200);
    assert!(page.new.is_empty());
    assert_eq!(page.earlier.len(), SENDERS.len() + 1);
    assert_eq!(page.earlier[0].id, news.id);
    assert!(page.earlier[0].read_at.is_some());
}

/// The feed pages newest first below its cursor, every row once, Pending
/// on the first page only.
#[test]
fn the_feed_pages_newest_first() {
    let f = Fixture::restored();
    let mut seen = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = f.events(cursor, 15);
        assert_eq!(page.pending.is_empty(), pages > 0);
        assert!(page.new.len() + page.earlier.len() <= 15);
        seen.extend(contacts_in(&page.earlier));
        pages += 1;
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    let newest_first: Vec<String> = SENDERS.rev().map(b58).collect();
    assert_eq!(seen, newest_first);
    // A limit of 0 is 1.
    assert_eq!(f.events(None, 0).earlier.len(), 1);
}

/// Ignoring a sender is a local mute: their unread rows are read, and the
/// feed leaves them out until they are un-ignored.
#[test]
fn an_ignored_sender_goes_quiet() {
    let f = Fixture::restored();
    let news = f.dave_asks();
    wait_for("Dave counted", || f.unread() == 1);
    let dp = f.dp();
    f.engine.block_on(dp.ignore(b58(ME), b58(DAVE))).unwrap();
    assert_eq!(f.unread(), 0);
    let page = f.events(None, 200);
    assert!(!contacts_in(&page.earlier).contains(&b58(DAVE)));
    assert!(page.new.is_empty());
    assert_eq!(page.pending.len(), SENDERS.len());

    f.engine.block_on(dp.unignore(b58(ME), b58(DAVE))).unwrap();
    let page = f.events(None, 200);
    assert_eq!(page.earlier[0].id, news.id);
    assert!(page.earlier[0].read_at.is_some());
    assert_eq!(f.unread(), 0);
}

/// The calls' refusals: an id that is not Base58, a wallet the session
/// lacks, an identity the wallet lacks.
#[test]
fn the_bell_refuses_bad_arguments() {
    let f = Fixture::new(true);
    f.give_me();
    let dp = f.dp();
    assert_eq!(
        dp.unread_count("0OIl".into()).unwrap_err().code(),
        "invalid_argument"
    );
    let code = |r: Result<(), super::errors::PlatformError>| r.unwrap_err().code();
    assert_eq!(
        code(f.engine.block_on(dp.mark_read("0OIl".into(), 1))),
        "invalid_argument"
    );
    assert_eq!(
        f.engine
            .block_on(dp.events(b58(9), None, 10))
            .unwrap_err()
            .code(),
        "identity.not_found"
    );
    let other = f.s.dashpay(WalletId([0x5A; 32]));
    assert_eq!(
        other.unread_count(b58(ME)).unwrap_err().code(),
        "wallet_not_found"
    );
    assert_eq!(
        f.engine
            .block_on(other.events(b58(ME), None, 10))
            .unwrap_err()
            .code(),
        "wallet_not_found"
    );
    assert_eq!(
        code(f.engine.block_on(other.mark_read(b58(ME), 1))),
        "wallet_not_found"
    );
}

/// A `.dwbackup` restore brings the bell's rows with their read state, and
/// the import recounts them: the badge is right with no write after it
/// (and no first count from `unread_count`).
#[test]
fn a_restored_bundles_unread_rows_count() {
    let f = Fixture::new(true);
    f.give_me();
    f.dave_asks();
    let dir = dw_testutil::private_tempdir();
    let dest = dir.path().join("me.dwbackup");
    let pw = || Some(Zeroizing::new(b"bk".to_vec()));
    (f.engine)
        .block_on(f.s.backup_wallet(f.wallet, dest.clone(), pw()))
        .unwrap();

    let engine = journal_tests::engine(
        &dir.path().join("b"),
        Arc::new(Recorder::default()) as Arc<dyn EventSink>,
    );
    let s = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                no_platform: true,
                ..Default::default()
            },
        ))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let ids = engine.block_on(s.restore_backup(dest, pw())).unwrap();
    assert_eq!(ids, [f.wallet]);
    let tap = s.live().unwrap().tap;
    assert_eq!(tap.unread(f.wallet, &b58(ME)), Some(1));
    engine.block_on(engine.shutdown()).unwrap();
}

/// Sol r1 finding 3: an `events` call the gate admitted reads the
/// contacts inside that admission, so a close that starts meanwhile (and
/// waits for it) does not fail it with `network_not_open`.
#[test]
fn an_admitted_events_call_outlives_a_starting_close() {
    let f = Fixture::new(true);
    f.give_me();
    let (pause, _release) = f.pause_at("events");
    let dp = f.dp();
    let events =
        f.s.rt
            .spawn(async move { dp.events(b58(ME), None, 10).await });
    wait_for("the admitted call", || pause.reached.load(Ordering::SeqCst));
    let s = Arc::clone(&f.s);
    let closing = f.s.rt.spawn(async move { s.close().await });
    wait_for("the close to start", || !f.s.is_open());
    pause.release.notify_one();
    let page = f.engine.block_on(events).unwrap();
    assert!(page.is_ok(), "{page:?}");
    f.engine.block_on(closing).unwrap();
}

/// A wallet with Dave's request unread, backed up to `dest` as it is.
fn dave_unread_backed_up(dest: &std::path::Path) -> Fixture {
    let f = Fixture::new(true);
    f.give_me();
    f.dave_asks();
    (f.engine)
        .block_on(f.s.backup_wallet(f.wallet, dest.to_owned(), backup_pw()))
        .unwrap();
    f
}

fn backup_pw() -> Option<Zeroizing<Vec<u8>>> {
    Some(Zeroizing::new(b"bk".to_vec()))
}

impl Fixture {
    /// Removes the wallet, as `remove_wallet` with a wipe grant, in the
    /// background.
    fn removing(&self) -> tokio::task::JoinHandle<Result<(), crate::EngineError>> {
        let (s, wallet) = (Arc::clone(&self.s), self.wallet);
        let wipe = (s
            .vault()
            .authorize(GrantPurpose::Wipe, Some(&wallet.0), Credential::None))
        .unwrap()
        .id;
        self.s
            .rt
            .spawn(async move { s.remove_wallet(wallet, wipe).await })
    }

    /// The same wallet imported again from `dest`: its rows come back with
    /// the read state they were backed up with.
    fn reimport(&self, dest: &std::path::Path) {
        let ids = (self.engine)
            .block_on(self.s.restore_backup(dest.to_owned(), backup_pw()))
            .unwrap();
        assert_eq!(ids, [self.wallet]);
    }

    /// Dave's restored request is unread, in the journal and in the count
    /// (the reimported wallet's identity is not in memory: `give_me` made
    /// it there only, so the feed's contacts read would not find it).
    fn dave_still_unread(&self) {
        let live = self.s.live().unwrap();
        let journal = (live.appdb)
            .journal_page(&self.wallet.to_string(), &b58(ME), None, 10, &[])
            .unwrap();
        let unread: Vec<_> = (journal.rows.iter())
            .filter(|r| r.read_at.is_none())
            .map(|r| (r.kind.as_str(), r.contact.clone()))
            .collect();
        assert_eq!(unread, [("request_received", b58(DAVE))], "{journal:?}");
        let tap = live.tap;
        assert_eq!(tap.unread(self.wallet, &b58(ME)), Some(1));
    }

    /// Holds the next call that reaches `at`; released at the latest when
    /// the second value drops, so a failed assert leaves nothing held.
    fn pause_at(&self, at: &'static str) -> (Arc<TestPause>, ReleaseOnDrop) {
        let pause = Arc::new(TestPause::default());
        *self.s.platform.contacts.job_pause.lock().unwrap() = Some((at, Arc::clone(&pause)));
        let release = ReleaseOnDrop(Arc::clone(&pause));
        (pause, release)
    }
}

/// Sol r1 finding 1: a `mark_read(u64::MAX)` whose app.sqlite job is held
/// holds the wallet's write fence, so a removal waits for it; the same
/// wallet imported afterwards keeps its restored rows unread.
#[test]
fn a_held_mark_read_finishes_before_a_removal_and_reimport() {
    let dir = dw_testutil::private_tempdir();
    let dest = dir.path().join("me.dwbackup");
    let f = dave_unread_backed_up(&dest);
    let (pause, _release) = f.pause_at("mark_read");
    let dp = f.dp();
    let marking =
        f.s.rt
            .spawn(async move { dp.mark_read(b58(ME), u64::MAX).await });
    wait_for("the held job", || pause.reached.load(Ordering::SeqCst));
    let fence = f.s.platform.contacts.fences.of(f.wallet);
    assert!(fence.try_write().is_err(), "the job holds no fence");
    let removing = f.removing();
    // A writer waits at the fence: no read gets in.
    wait_long("the removal at the fence", Duration::from_secs(60), || {
        fence.try_read().is_err() || removing.is_finished()
    });
    assert!(!removing.is_finished(), "the removal did not wait");
    pause.release.notify_one();
    f.engine.block_on(marking).unwrap().unwrap();
    f.engine.block_on(removing).unwrap().unwrap();
    f.reimport(&dest);
    f.dave_still_unread();
}

/// Sol r1 finding 1: a `mark_read(u64::MAX)` admitted for the wallet but
/// delayed past its removal and a same-seed reimport marks nothing of the
/// reimported wallet: its restored rows stay unread.
#[test]
fn a_mark_read_admitted_before_a_removal_and_reimport_marks_nothing() {
    let dir = dw_testutil::private_tempdir();
    let dest = dir.path().join("me.dwbackup");
    let f = dave_unread_backed_up(&dest);
    let (pause, _release) = f.pause_at("mark_read admitted");
    let dp = f.dp();
    let marking =
        f.s.rt
            .spawn(async move { dp.mark_read(b58(ME), u64::MAX).await });
    wait_for("the admitted call", || pause.reached.load(Ordering::SeqCst));
    f.engine.block_on(f.removing()).unwrap().unwrap();
    f.reimport(&dest);
    pause.release.notify_one();
    let marked = f.engine.block_on(marking).unwrap();
    assert!(
        matches!(marked, Err(PlatformError::WalletNotFound)),
        "{marked:?}"
    );
    f.dave_still_unread();
}

/// Sol r2 new 2: an `unread_count` that checked the wallet before its
/// removal finished starts no count after it: the removed wallet's counts
/// stay gone.
#[test]
fn an_unread_count_checked_before_a_removal_starts_no_count() {
    let f = Fixture::new(true);
    f.give_me();
    let (pause, _release) = f.pause_at("unread_count checked");
    let dp = f.dp();
    let count = std::thread::spawn(move || dp.unread_count(b58(ME)));
    wait_for("the checked call", || pause.reached.load(Ordering::SeqCst));
    f.engine.block_on(f.removing()).unwrap().unwrap();
    pause.release.notify_one();
    assert_eq!(count.join().unwrap().unwrap(), 0);
    let tap = f.s.live().unwrap().tap;
    assert_eq!(
        tap.unread(f.wallet, &b58(ME)),
        None,
        "the removed wallet counted"
    );
}

/// Sol r2 (E0-06's journal writer): a library changeset held at `at` (a
/// test pause of the tap) across the wallet's removal and a same-seed
/// reimport writes no row into the reimported wallet and makes no news.
fn a_store_held_across_a_removal_and_reimport_journals_nothing(
    at: fn(&ChangesetTap) -> &std::sync::Mutex<Option<Arc<TestPause>>>,
) {
    let dir = dw_testutil::private_tempdir();
    let dest = dir.path().join("me.dwbackup");
    let f = Fixture::new(true);
    f.give_me();
    (f.engine)
        .block_on(f.s.backup_wallet(f.wallet, dest.clone(), backup_pw()))
        .unwrap();
    let live = f.s.live().unwrap();
    let pause = Arc::new(TestPause::default());
    *at(&live.tap).lock().unwrap() = Some(Arc::clone(&pause));
    let _release = ReleaseOnDrop(Arc::clone(&pause));
    let (store, rt, wallet) = (Arc::clone(&live.store), f.s.rt.clone(), f.wallet);
    let writer = std::thread::spawn(move || {
        let _rt = rt.enter();
        let cs = contacts(incoming(ME, DAVE, NEW_MS));
        platform_wallet::changeset::PlatformWalletPersistence::store(&*store, wallet.0, cs)
            .unwrap();
    });
    wait_for("the held store", || pause.reached.load(Ordering::SeqCst));
    f.engine.block_on(f.removing()).unwrap().unwrap();
    f.reimport(&dest);
    pause.release.notify_one();
    writer.join().unwrap();
    let rows = (live.appdb)
        .journal_page(&f.wallet.to_string(), &b58(ME), None, 10, &[])
        .unwrap()
        .rows;
    assert!(rows.is_empty(), "the late write landed: {rows:?}");
    std::thread::sleep(Duration::from_millis(400));
    assert!(f.news().is_empty(), "{:?}", f.news());
}

/// ... held after the library's commit, before the tap's record began:
/// the wallet is the one the store call began with.
#[test]
fn a_record_held_across_a_removal_and_reimport_journals_nothing() {
    a_store_held_across_a_removal_and_reimport_journals_nothing(|tap| &tap.pause_record);
}

/// ... held in the journal write, before app.sqlite.
#[test]
fn a_journal_write_held_across_a_removal_and_reimport_writes_nothing() {
    a_store_held_across_a_removal_and_reimport_journals_nothing(|tap| &tap.pause_journal);
}

// ---------------------------------------------------------------------------
// Live
// ---------------------------------------------------------------------------

/// T2, read-only: fixture wallet A restored into a fresh engine on public
/// testnet brings back its contacts (B at least) with no news at all:
/// every contact row is read, each contact has one "accepted" or "is now
/// your contact" event, and only names (DEC-114 (4): their time may be
/// unknown) can count unread. A later DashPay pass (the loop's own, a
/// minute on) adds no row and no news.
#[test]
#[ignore = "public testnet; set DW_DP205_TESTNET_FIXTURE"]
fn t2_a_testnet_restore_makes_no_news() {
    let fixture = std::path::PathBuf::from(
        std::env::var("DW_DP205_TESTNET_FIXTURE").expect("DW_DP205_TESTNET_FIXTURE"),
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    let birth = meta["funding"]["birth_height_hint"].as_u64().unwrap() as u32;
    let me = meta["A"]["identity_id"].as_str().unwrap().to_owned();
    let b = meta["B"]["identity_id"].as_str().unwrap().to_owned();

    let dir = dw_testutil::private_tempdir();
    let sink = Arc::new(Recorder::default());
    let engine = journal_tests::engine(dir.path(), Arc::clone(&sink) as Arc<dyn EventSink>);
    let s = engine
        .block_on(engine.open_network(DashNetwork::Testnet, SessionOptions::default()))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let phrase = std::fs::read_to_string(fixture.join("A.mnemonic")).unwrap();
    let wallet = engine
        .block_on(s.import_wallet(
            Zeroizing::new(phrase.trim().as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(birth),
                ..ImportOptions::default()
            },
        ))
        .unwrap();
    let t0 = Instant::now();
    engine.block_on(s.start_spv()).unwrap();
    wait_long("SPV", Duration::from_secs(60), || {
        s.spv_state().unwrap() == SpvState::Running
    });
    let dp = s.dashpay(wallet);
    let q = ContactQuery {
        sections: vec![ContactSection::Contacts],
        sort: ContactSort::DisplayName,
        text: None,
    };
    let mut others = Vec::new();
    wait_long("A's contact B", Duration::from_secs(240), || {
        dp.contacts(me.clone(), q.clone()).is_ok_and(|p| {
            others = (p.sections.iter())
                .flat_map(|s| &s.contacts)
                .filter(|c| c.relation == Relation::Contact)
                .map(|c| c.contact.clone())
                .collect();
            !p.updating && others.contains(&b)
        })
    });
    let journal = || {
        engine
            .block_on(s.appdb_op(move |db| db.journal(&wallet.to_string())))
            .unwrap()
    };
    let news = || {
        (sink.0.lock().unwrap().iter())
            .filter(|e| matches!(e, EngineEvent::DashPayNews { .. }))
            .count()
    };
    let rows = journal();
    eprintln!("A after {:?}: contacts {others:?}, {rows:#?}", t0.elapsed());
    let unread = rows.iter().filter(|r| r.read_at.is_none());
    assert!(
        unread.clone().all(|r| r.kind == "username_registered"),
        "unread after a restore: {rows:?}"
    );
    let unread = unread.count() as u32;
    for other in &others {
        let made = (rows.iter())
            .filter(|r| r.contact == *other)
            .filter(|r| ["request_accepted", "contact_established"].contains(&r.kind.as_str()))
            .count();
        assert_eq!(made, 1, "{other}: {rows:?}");
    }
    wait_for("the unread count", || {
        dp.unread_count(me.clone()).unwrap() == unread
    });
    let page = engine.block_on(dp.events(me.clone(), None, 50)).unwrap();
    assert_eq!(page.new.len() as u32, unread);
    assert_eq!(page.new.len() + page.earlier.len(), rows.len());

    // The loop's next passes (every 15 s while visible, 60 s hidden).
    std::thread::sleep(Duration::from_secs(75));
    assert_eq!(format!("{:?}", journal()), format!("{rows:?}"));
    assert_eq!(news(), 0, "news after a restore");
    engine.block_on(engine.shutdown()).unwrap();
}
