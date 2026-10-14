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

/// Sol r3 finding 1: a removal whose app.sqlite delete fails (`fail`, a
/// trigger named `fail_delete`), after the wallet left the manager, still
/// fences a journal write begun before it. An import of the wallet deletes
/// the rows the removal left, or is refused while it cannot
/// (`refused_first`), so the reimported wallet gets neither those rows nor
/// the late write.
fn a_failed_removal_leaves_nothing_to_a_reimport(fail: &str, refused_first: bool) {
    let dir = dw_testutil::private_tempdir();
    let dest = dir.path().join("me.dwbackup");
    let f = Fixture::new(true);
    f.give_me();
    (f.engine)
        .block_on(f.s.backup_wallet(f.wallet, dest.clone(), backup_pw()))
        .unwrap();
    // A row the backup does not have, for the delete to fail on.
    f.dave_asks();
    let live = f.s.live().unwrap();
    let pause = Arc::new(TestPause::default());
    *live.tap.pause_journal.lock().unwrap() = Some(Arc::clone(&pause));
    let _release = ReleaseOnDrop(Arc::clone(&pause));
    let (store, rt, wallet) = (Arc::clone(&live.store), f.s.rt.clone(), f.wallet);
    let writer = std::thread::spawn(move || {
        let _rt = rt.enter();
        let cs = contacts(incoming(ME, 3, NEW_MS));
        platform_wallet::changeset::PlatformWalletPersistence::store(&*store, wallet.0, cs)
            .unwrap();
    });
    wait_for("the held store", || pause.reached.load(Ordering::SeqCst));
    let sqlite = rusqlite::Connection::open(f.s.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap();
    sqlite.execute_batch(fail).unwrap();
    assert!(f.engine.block_on(f.removing()).unwrap().is_err());
    if refused_first {
        let refused = (f.engine).block_on(f.s.restore_backup(dest.clone(), backup_pw()));
        assert!(
            matches!(&refused, Err(crate::EngineError::Storage(m)) if m.contains("could not be finished")),
            "{refused:?}"
        );
    }
    sqlite.execute_batch("DROP TRIGGER fail_delete;").unwrap();
    f.reimport(&dest);
    let news = f.news().len();
    pause.release.notify_one();
    writer.join().unwrap();
    let rows = (live.appdb)
        .journal_page(&f.wallet.to_string(), &b58(ME), None, 10, &[])
        .unwrap()
        .rows;
    assert!(
        rows.is_empty(),
        "a row of the removed wallet landed: {rows:?}"
    );
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(f.news().len(), news, "{:?}", f.news());
}

/// The delete of the wallet's journal rows fails.
const FAIL_ROW_DELETE: &str = "CREATE TRIGGER fail_delete BEFORE DELETE ON dp_events \
     BEGIN SELECT RAISE(FAIL, 'injected delete failure'); END;";

/// ... the import finishes the removal.
#[test]
fn a_failed_removal_is_finished_by_the_reimport() {
    a_failed_removal_leaves_nothing_to_a_reimport(FAIL_ROW_DELETE, false);
}

/// ... an import while the delete still fails is refused.
#[test]
fn a_reimport_is_refused_while_a_failed_removal_cannot_finish() {
    a_failed_removal_leaves_nothing_to_a_reimport(FAIL_ROW_DELETE, true);
}

/// ... the removal could not even record the wallet as removed (Opus
/// review of fix round 3): the import still finishes it.
#[test]
fn a_removal_that_could_not_record_the_wallet_is_finished_by_the_reimport() {
    a_failed_removal_leaves_nothing_to_a_reimport(
        "CREATE TRIGGER fail_delete BEFORE INSERT ON removed_wallets \
         BEGIN SELECT RAISE(FAIL, 'injected record failure'); END;",
        false,
    );
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

/// The bell's contract vector (U-07's engine-vector mechanism): each case
/// drives a session whose journal holds the same seeded rows through
/// `events`, `unread_count`, `mark_read`, `ignore` and `unignore`, and
/// records every result as dw-app's IPC shape in
/// `apps/desktop/src/fixture/vectors/notifications.json`. The renderer's
/// fixture backend replays the cases (`fixture/notifications.test.ts`);
/// dw-app checks the hand-written shapes (`notification_vector_is_dw_apps`).
/// Fails when the committed file drifts.
mod vectors {
    use serde_json::{Map, Value, json};

    use super::*;
    use dw_appdb::JournalEntry;

    const REGENERATE: &str = "DW_UPDATE_VECTORS=1 cargo test -p dw-engine notification_vectors";
    /// The seeded rows' times (UNIX s).
    const T0: u64 = 1_790_000_000;
    /// An identity id the wallet lacks, and one that is not Base58.
    const STRANGER: u8 = 9;
    const NOT_BASE58: &str = "0OIl";
    /// Base58 of 32 to 44 characters that is no identity id, which only
    /// decoding shows: 33 bytes (no zero byte, one, all zero) and 23.
    const WRONG_LENGTH: [&str; 4] = [
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        "1zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        "111111111111111111111111111111111",
        "22222222222222222222222222222222",
    ];
    const OTHER_WALLET: WalletId = WalletId([0x5A; 32]);
    /// Rows past one page's 200, so a larger limit shows the cap.
    const PAYMENTS: u64 = 205;
    /// The requests waiting besides Dave's, by sender and `$createdAt`
    /// (ms): one newer than his and two of one time, which their ids
    /// order.
    const REQUESTERS: [(u8, u64); 3] = [
        (5, NEW_MS + 3_000_000),
        (6, NEW_MS + 1_000_000),
        (7, NEW_MS + 1_000_000),
    ];

    fn path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/desktop/src/fixture/vectors/notifications.json")
    }

    /// dw-app's `EventKindName` of the journal's kind name.
    fn kind_name(kind: &str) -> &'static str {
        match kind {
            "username_registered" => "usernameRegistered",
            "contest_won" => "contestWon",
            "contest_lost" => "contestLost",
            "contest_locked" => "contestLocked",
            "request_received" => "requestReceived",
            "request_accepted" => "requestAccepted",
            "contact_established" => "contactEstablished",
            "payment_received" => "paymentReceived",
            other => panic!("no vector name for {other}: add one"),
        }
    }

    fn kind_of(kind: EventKind) -> &'static str {
        let snake = serde_json::to_value(kind).unwrap();
        kind_name(snake.as_str().unwrap())
    }

    /// The vector's name for an identity id: `me`, `dave`, `stranger`, or
    /// `c<n>` for the seeded senders.
    fn label(id: &str) -> String {
        if id == b58(ME) {
            "me".into()
        } else if id == b58(DAVE) {
            "dave".into()
        } else if id == b58(STRANGER) {
            "stranger".into()
        } else {
            (1..=u8::MAX)
                .find(|&n| b58(n) == id)
                .map(|n| format!("c{n}"))
                .unwrap_or_else(|| panic!("no label for {id}"))
        }
    }

    fn id_of(label: &str) -> String {
        match label {
            "me" => b58(ME),
            "dave" => b58(DAVE),
            "stranger" => b58(STRANGER),
            other => other
                .strip_prefix('c')
                .and_then(|n| n.parse().ok())
                .map(b58)
                .unwrap_or_else(|| other.to_owned()),
        }
    }

    /// A time the run stamped (a `mark_read`, an `ignore`) is `"now"`: the
    /// fixture's clock stands in for it.
    fn time(t: u64) -> Value {
        if t >= T0 + 1_000_000 {
            json!("now")
        } else {
            json!(t)
        }
    }

    /// dw-app's `DashPayEventInfo`, by hand.
    fn event(e: &DashPayEvent) -> Value {
        json!({
            "id": e.id,
            "kind": kind_of(e.kind),
            "contact": e.contact.as_deref().map(label),
            "reference": e.reference,
            "at": e.at,
            "readAt": e.read_at.map(time),
        })
    }

    /// dw-app's `DashPayEventPage`, the pending requests by contact.
    fn page(p: &EventPage) -> Value {
        json!({
            "pending": p.pending.iter().map(|c| label(&c.contact)).collect::<Vec<_>>(),
            "new": p.new.iter().map(event).collect::<Vec<_>>(),
            "earlier": p.earlier.iter().map(event).collect::<Vec<_>>(),
            "nextCursor": p.next_cursor,
        })
    }

    /// dw-app's error: these calls fail with no data.
    fn result<T>(r: Result<T, impl Coded>, ok: impl FnOnce(T) -> Value) -> Value {
        match r {
            Ok(v) => json!({ "ok": ok(v) }),
            Err(e) => json!({ "error": { "code": e.code(), "data": null } }),
        }
    }

    trait Coded {
        fn code(&self) -> &'static str;
    }
    impl Coded for PlatformError {
        fn code(&self) -> &'static str {
            PlatformError::code(self)
        }
    }
    impl Coded for super::super::errors::ContactError {
        fn code(&self) -> &'static str {
            super::super::errors::ContactError::code(self)
        }
    }

    /// One seeded row: identity, kind, contact, raw reference, read time.
    type Seed = (u8, &'static str, String, String, Option<u64>);

    /// The rows every case starts from, oldest first, after Dave's request
    /// (row 1, which the tap journals): every kind, a name kind's
    /// `@<stamp>`, an empty contact and an empty reference, read and
    /// unread rows, and more payments than a page holds.
    fn seeds() -> Vec<(u64, Vec<Seed>)> {
        let none = String::new;
        let s = |n: u8| b58(n);
        let mut out = vec![
            (
                T0 + 100,
                vec![
                    (
                        ME,
                        "username_registered",
                        none(),
                        "alice@1789990000".into(),
                        Some(T0 + 150),
                    ),
                    (ME, "contest_won", none(), "alicia@1789990001".into(), None),
                ],
            ),
            (
                T0 + 200,
                vec![
                    (ME, "request_accepted", s(20), "2100000".into(), None),
                    (
                        ME,
                        "contact_established",
                        s(21),
                        "2200000".into(),
                        Some(T0 + 250),
                    ),
                    (ME, "contest_lost", none(), "alfie@1789990002".into(), None),
                    (ME, "contest_locked", none(), none(), None),
                    // Another identity's row: never on ME's pages.
                    (STRANGER, "request_received", s(22), "2300000".into(), None),
                ],
            ),
        ];
        for i in 0..PAYMENTS {
            let read = (i % 3 == 0).then_some(T0 + 1_000 + i);
            let txid = format!("{:064x}", 0xd00d_0000_u64 + i);
            out.push((
                T0 + 300 + i,
                vec![(ME, "payment_received", s(30 + (i % 10) as u8), txid, read)],
            ));
        }
        out
    }

    /// The requests waiting, in the order they come: Dave's, then
    /// `REQUESTERS`' (the sender's name, id and `since`, UNIX s).
    fn requests() -> Value {
        let all = [(DAVE, NEW_MS)].into_iter().chain(REQUESTERS);
        (all.map(|(n, ms)| json!({ "contact": label(&b58(n)), "id": b58(n), "since": ms / 1000 })))
            .collect()
    }

    /// A session at the seeded state: Dave's request is pending (row 1),
    /// then `REQUESTERS`' (rows 2–4).
    fn seeded() -> Fixture {
        let f = Fixture::new(true);
        f.give_me();
        f.dave_asks();
        for (n, ms) in REQUESTERS {
            let before = f.news().len();
            f.with_me(move |m, p| {
                m.add_incoming_contact_request(request(n, ME, ms), p)
                    .unwrap();
            });
            wait_for("the request's news", || f.news().len() > before);
        }
        let live = f.s.live().unwrap();
        let wallet = f.wallet.to_string();
        // The tap stamped the requests' rows with the clock: a fixed time
        // instead.
        let sqlite =
            rusqlite::Connection::open(f.s.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap();
        let stamped = sqlite.execute("UPDATE dp_events SET at = ?1", [(T0 + 10) as i64]);
        assert_eq!(stamped.unwrap(), 1 + REQUESTERS.len());
        for (now, rows) in seeds() {
            let entries: Vec<JournalEntry> = (rows.into_iter())
                .map(
                    |(identity, kind, contact, reference, read_at)| JournalEntry {
                        identity: b58(identity),
                        kind,
                        contact,
                        reference,
                        read_at,
                        unless_any: &[],
                    },
                )
                .collect();
            let added = live.appdb.record_events(&wallet, &entries, now).unwrap();
            assert_eq!(added.len(), entries.len());
        }
        live.tap.unread_moved(f.wallet);
        settle_count(&f, &b58(ME));
        f
    }

    /// Waits until `identity`'s count is its unread rows: the tap counts
    /// what a write moved after it.
    fn settle_count(f: &Fixture, identity: &str) {
        let live = f.s.live().unwrap();
        let unread = (live.appdb.journal(&f.wallet.to_string()).unwrap().iter())
            .filter(|r| r.identity == identity && r.read_at.is_none())
            .count() as u32;
        wait_for("the count", || {
            f.dp().unread_count(identity.to_owned()).unwrap() == unread
        });
    }

    /// The seeded journal as the fixture world takes it: raw rows.
    fn rows(f: &Fixture) -> Value {
        let rows =
            f.s.live()
                .unwrap()
                .appdb
                .journal(&f.wallet.to_string())
                .unwrap();
        (rows.iter())
            .map(|r| {
                json!({
                    "id": r.id,
                    "identity": label(&r.identity),
                    "kind": r.kind,
                    "contact": (!r.contact.is_empty()).then(|| label(&r.contact)),
                    "ref": r.reference,
                    "at": r.at,
                    "readAt": r.read_at.map(time),
                })
            })
            .collect()
    }

    /// One step: what the fixture test calls, by the vector's names.
    enum Step {
        Events(&'static str, Option<u64>, u32),
        Unread(&'static str),
        MarkRead(&'static str, u64),
        Ignore(&'static str),
        Unignore(&'static str),
        /// The next requests sweep: an unignored sender's request comes
        /// back (platform-wallet rewinds its cursor; the fixture's world is
        /// always past it).
        Sweep,
        OtherWallet(Box<Step>),
    }

    fn args(step: &Step) -> Value {
        match step {
            Step::Events(identity, cursor, limit) => {
                json!({ "events": { "identity": identity, "cursor": cursor, "limit": limit } })
            }
            Step::Unread(identity) => json!({ "unreadCount": { "identity": identity } }),
            Step::MarkRead(identity, up_to) => {
                json!({ "markRead": { "identity": identity, "upTo": up_to } })
            }
            Step::Ignore(contact) => json!({ "ignore": { "contact": contact } }),
            Step::Unignore(contact) => json!({ "unignore": { "contact": contact } }),
            Step::Sweep => json!({ "sweep": null }),
            Step::OtherWallet(step) => json!({ "otherWallet": args(step) }),
        }
    }

    fn run(f: &Fixture, dp: &super::super::dashpay::DashPay, step: &Step) -> Value {
        match step {
            Step::Events(identity, cursor, limit) => result(
                f.engine
                    .block_on(dp.events(id_of(identity), *cursor, *limit)),
                |p| page(&p),
            ),
            Step::Unread(identity) => result(dp.unread_count(id_of(identity)), |n| json!(n)),
            Step::MarkRead(identity, up_to) => result(
                f.engine.block_on(dp.mark_read(id_of(identity), *up_to)),
                |()| Value::Null,
            ),
            Step::Ignore(contact) => result(
                f.engine.block_on(dp.ignore(b58(ME), id_of(contact))),
                |()| Value::Null,
            ),
            Step::Unignore(contact) => result(
                f.engine.block_on(dp.unignore(b58(ME), id_of(contact))),
                |()| Value::Null,
            ),
            Step::Sweep => {
                f.with_me(|m, p| {
                    m.add_incoming_contact_request(request(DAVE, ME, NEW_MS), p)
                        .unwrap();
                });
                Value::Null
            }
            Step::OtherWallet(step) => run(f, &f.s.dashpay(OTHER_WALLET), step),
        }
    }

    fn cases() -> Vec<(&'static str, Vec<Step>)> {
        use Step::*;
        let wrong_length = WRONG_LENGTH
            .into_iter()
            .flat_map(|id| [Events(id, None, 5), Unread(id), MarkRead(id, 1_000_000)]);
        vec![
            (
                "pages",
                vec![
                    Events("me", None, 1_000),
                    Events("me", Some(14), 200),
                    Events("me", None, 0),
                    Events("me", None, 7),
                    Events("me", Some(207), 7),
                    Events("me", Some(200), 7),
                    Events("me", Some(1_000_000), 3),
                    Events("me", Some(4), 5),
                    Events("me", Some(1), 5),
                    Events("me", Some(0), 5),
                    Unread("me"),
                ],
            ),
            (
                "markRead",
                vec![
                    MarkRead("me", 0),
                    Unread("me"),
                    MarkRead("me", 50),
                    Unread("me"),
                    Events("me", Some(56), 12),
                    MarkRead("me", 50),
                    MarkRead("me", 1_000_000),
                    Unread("me"),
                    Events("me", None, 4),
                ],
            ),
            (
                "ignore",
                vec![
                    Unread("me"),
                    Ignore("dave"),
                    Unread("me"),
                    Events("me", None, 3),
                    Events("me", Some(3), 5),
                    // Only Dave's row is below the last: no next page.
                    Events("me", Some(3), 1),
                    Events("me", Some(2), 5),
                    Unignore("dave"),
                    Sweep,
                    Events("me", None, 3),
                    Events("me", Some(3), 5),
                    Unread("me"),
                ],
            ),
            (
                "refusals",
                vec![
                    Events(NOT_BASE58, None, 5),
                    Events("stranger", None, 5),
                    Unread(NOT_BASE58),
                    Unread("stranger"),
                    MarkRead(NOT_BASE58, 1),
                    MarkRead("stranger", 1_000_000),
                    Unread("stranger"),
                    Unread("me"),
                    OtherWallet(Box::new(Events("me", None, 5))),
                    OtherWallet(Box::new(Unread("me"))),
                    OtherWallet(Box::new(MarkRead("me", 1))),
                ]
                .into_iter()
                .chain(wrong_length)
                .collect(),
            ),
        ]
    }

    fn build() -> Value {
        let mut out = Vec::new();
        let mut seed_rows = None;
        for (name, steps) in cases() {
            let f = seeded();
            let rows = rows(&f);
            assert!(seed_rows.get_or_insert_with(|| rows.clone()) == &rows);
            let dp = f.dp();
            let mut recorded = Vec::new();
            for step in &steps {
                // Settle the count a write moved before the next read.
                if let Step::Unread(identity) = step
                    && *identity != NOT_BASE58
                    && !WRONG_LENGTH.contains(identity)
                {
                    settle_count(&f, &id_of(identity));
                }
                let mut s = Map::new();
                s.insert("call".into(), args(step));
                s.insert("result".into(), run(&f, &dp, step));
                recorded.push(Value::Object(s));
            }
            out.push(json!({ "name": name, "steps": recorded }));
            f.engine.block_on(f.engine.shutdown()).unwrap();
        }
        let kinds: Map<String, Value> = [
            EventKind::UsernameRegistered,
            EventKind::ContestWon,
            EventKind::ContestLost,
            EventKind::ContestLocked,
            EventKind::RequestReceived,
            EventKind::RequestAccepted,
            EventKind::ContactEstablished,
            EventKind::PaymentReceived,
        ]
        .into_iter()
        .map(|k| {
            let snake = serde_json::to_value(k)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned();
            (snake, json!(kind_of(k)))
        })
        .collect();
        json!({
            "comment": format!(
                "Generated by dw-engine (rust/crates/dw-engine/src/platform/notifications_tests.rs, `vectors`) from the bell over a session; do not edit. Regenerate with {REGENERATE}."
            ),
            "notBase58": NOT_BASE58,
            "wrongLength": WRONG_LENGTH,
            "kinds": kinds,
            "requests": requests(),
            "rows": seed_rows.unwrap(),
            "cases": out,
        })
    }

    #[test]
    fn notification_vectors() {
        let text = serde_json::to_string_pretty(&build()).unwrap() + "\n";
        let path = path();
        if std::env::var_os("DW_UPDATE_VECTORS").is_some() {
            std::fs::write(&path, &text).unwrap();
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            committed == text,
            "apps/desktop/src/fixture/vectors/notifications.json is stale; run {REGENERATE}"
        );
    }
}
