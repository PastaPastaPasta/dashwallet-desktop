//! Tests of contest_watch.rs (DP1-04): the verdicts, the cadence, and on a
//! session with Platform scripted, each outcome (won, lost, locked, still
//! voting, ended while the app was closed), catch-up silence, adoption, the
//! request budget, a pass that outlives its wallet, and `contest_status`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dpp::prelude::Identifier;

use dw_appdb::ContestWatch;
use zeroize::Zeroizing;

use super::contacts_tests::{Fixture, wait_for};
use super::contest_watch::*;
use super::errors::PlatformError;
use super::journal_tests::{b58, id};
use super::names::{ContestContender, ContestState, record_contest};
use super::notifications::EventKind;
use super::runtime::guard;
use crate::events::unix_now;
use crate::{DashNetwork, EngineEvent, PlatformChange, SessionOptions};

const ME: u8 = 1;
const BOB: u8 = 2;
const CAROL: u8 = 3;

/// Platform's vote states and end-date index, scripted, cut to a page as
/// Drive cuts them; every request counted.
#[derive(Default)]
pub(crate) struct FakePolls {
    polls: Mutex<BTreeMap<String, Poll>>,
    ends: Mutex<BTreeMap<String, u64>>,
    poll_calls: AtomicU32,
    ends_calls: AtomicU32,
    /// The `limit` of each end-date request.
    end_limits: Mutex<Vec<u16>>,
    /// Holds the next vote-state query until released.
    hold: Mutex<Option<Arc<Held>>>,
}

#[derive(Default)]
struct Held {
    reached: AtomicBool,
    release: tokio::sync::Notify,
}

impl FakePolls {
    pub(super) fn set(&self, normalized: &str, poll: Poll) {
        guard(&self.polls).insert(normalized.into(), poll);
    }

    fn set_end(&self, normalized: &str, ends_at: u64) {
        guard(&self.ends).insert(normalized.into(), ends_at);
    }

    /// The vote-state and end-date queries so far.
    fn calls(&self) -> (u32, u32) {
        (
            self.poll_calls.load(Ordering::SeqCst),
            self.ends_calls.load(Ordering::SeqCst),
        )
    }
}

impl ContestNet for FakePolls {
    async fn page(&self, normalized: &str, limit: u16) -> Result<Poll, PlatformError> {
        self.poll_calls.fetch_add(1, Ordering::SeqCst);
        let held = guard(&self.hold).take();
        if let Some(held) = held {
            held.reached.store(true, Ordering::SeqCst);
            held.release.notified().await;
        }
        let mut poll = (guard(&self.polls).get(normalized).cloned()).unwrap_or_default();
        // Drive keys an open poll's contenders by identity and pages them;
        // a resolved poll answers its stored result, every contender.
        poll.contenders.sort_by_key(|(id, _)| id.to_buffer());
        if poll.winner.is_none() {
            poll.contenders.truncate(usize::from(limit));
        }
        Ok(poll)
    }

    async fn end_page(
        &self,
        from: u64,
        limit: u16,
    ) -> Result<BTreeMap<String, u64>, PlatformError> {
        self.ends_calls.fetch_add(1, Ordering::SeqCst);
        guard(&self.end_limits).push(limit);
        let mut ends: Vec<(u64, String)> = (guard(&self.ends).iter())
            .filter(|&(_, &end)| end >= from)
            .map(|(label, &end)| (end, label.clone()))
            .collect();
        ends.sort();
        ends.truncate(usize::from(limit));
        Ok(ends.into_iter().map(|(end, label)| (label, end)).collect())
    }
}

/// `n` contenders who all sort before [`ME`] (identity order), with `ME`
/// after them when `with_me`.
fn crowd(winner: Option<PollWinner>, at_ms: u64, n: u8, with_me: bool) -> Poll {
    let early = |k: u8| {
        let mut bytes = [0u8; 32];
        bytes[1] = k;
        Identifier::from(bytes)
    };
    let mut contenders: Vec<(Identifier, Option<u32>)> =
        (0..n).map(|k| (early(k), Some(1))).collect();
    if with_me {
        contenders.push((id(ME), Some(0)));
    }
    Poll {
        winner: winner.map(|w| (w, at_ms)),
        contenders,
        lock_votes: Some(0),
        abstain_votes: Some(0),
    }
}

fn first_of_crowd() -> Identifier {
    Identifier::from([0u8; 32])
}

/// A poll with `contenders` (each `n` with `n` votes), resolved as `winner`.
pub(super) fn poll(winner: Option<PollWinner>, at_ms: u64, contenders: &[u8]) -> Poll {
    Poll {
        winner: winner.map(|w| (w, at_ms)),
        contenders: contenders
            .iter()
            .map(|&n| (id(n), Some(n.into())))
            .collect(),
        lock_votes: Some(1),
        abstain_votes: Some(0),
    }
}

fn now_ms() -> u64 {
    unix_now() * 1000
}

fn watch(label: &str, ends_at: u64, last_state: Option<&str>) -> ContestWatch {
    ContestWatch {
        identity: b58(ME),
        label: label.into(),
        ends_at,
        last_state: last_state.map(str::to_string),
    }
}

impl Fixture {
    /// Our identity, contending for `label` as the library records it,
    /// on a wallet past its first bring-up.
    fn contending(&self, label: &'static str) {
        self.give_me();
        self.caught_up();
        self.with_me(move |m, p| record_contest(m, label, p));
    }

    /// The catch-up boundary a first bring-up sets, long ago.
    fn caught_up(&self) {
        let tap = self.s.live().unwrap().tap;
        assert!(tap.move_boundary_to(self.wallet, 1_000, true));
    }

    /// What `register_name` does on `ContestStarted`.
    fn start_watching(&self, label: &'static str, ends_at: Option<u64>) {
        let (s, wallet_id) = (Arc::clone(&self.s), self.wallet);
        self.engine.block_on(async move {
            let manager = s.manager().unwrap();
            let wallet = manager.get_wallet(&wallet_id.0).await.unwrap();
            (s.start_watching(&wallet, wallet_id, &id(ME), label, ends_at)).await;
        });
    }

    fn watch(&self, label: &str, ends_at: u64) {
        let (w, i, label) = (self.wallet.to_string(), b58(ME), label.to_string());
        (self.engine)
            .block_on(
                self.s
                    .appdb_op(move |db| db.watch_contest(&w, &i, &label, ends_at)),
            )
            .unwrap();
    }

    fn set_state(&self, label: &str, state: &str) {
        let (w, i) = (self.wallet.to_string(), b58(ME));
        let (label, state) = (label.to_string(), state.to_string());
        (self.engine)
            .block_on(
                (self.s).appdb_op(move |db| db.set_contest_watch_state(&w, &i, &label, 0, &state)),
            )
            .unwrap();
    }

    fn watches(&self) -> Vec<ContestWatch> {
        let w = self.wallet.to_string();
        (self.engine)
            .block_on(self.s.appdb_op(move |db| db.wallet_contest_watches(&w)))
            .unwrap()
    }

    /// One watch pass against `net`, as the supervisor runs it.
    fn pass(&self, net: &FakePolls) -> Result<(), PlatformError> {
        let manager = self.s.manager().unwrap();
        (self.engine).block_on(self.s.watch_contests(net, &manager, self.wallet))
    }

    /// Every row due again: what the time to the next check does.
    fn later(&self) {
        self.s.platform.contests.forget(&self.wallet);
    }

    /// Whether a `Platform{Names}` went out since the last call (BL-153:
    /// `DashPay::status` shows the end and drops an unplaced contest).
    fn names_signalled(&self) -> bool {
        let mut marks = self.s.hub.pump.platform_marks.lock().unwrap();
        let seen = marks.replace(Vec::new()).unwrap_or_default();
        seen.iter().any(|(_, c)| c.contains(&PlatformChange::Names))
    }

    fn open_contests(&self) -> Vec<String> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.with_me(move |m, _| tx.send(m.contested_dpns_names.clone()).unwrap());
        rx.recv().unwrap()
    }

    /// The library lists `label` as our name, as a marketplace pass does.
    fn list(&self, label: &'static str) {
        self.with_me(move |m, p| {
            let mut names = m.dpns_names.clone();
            names.push(platform_wallet::DpnsNameInfo {
                label: label.into(),
                acquired_at: Some(7),
            });
            m.set_dpns_names(names, p);
        });
    }

    /// The journal's name rows: kind, ref, and whether stored read.
    fn outcomes(&self) -> Vec<(String, String, bool)> {
        (self.journal().into_iter())
            .filter(|r| r.kind.starts_with("contest_") || r.kind == "username_registered")
            .map(|r| (r.kind, r.reference, r.read_at.is_some()))
            .collect()
    }

    /// The kinds of every `DashPayNews` event delivered so far.
    fn news_kinds(&self) -> Vec<EventKind> {
        (self.sink.0.lock().unwrap().iter())
            .filter_map(|e| match e {
                EngineEvent::DashPayNews { events, .. } => Some(events.clone()),
                _ => None,
            })
            .flatten()
            .map(|e| e.kind)
            .collect()
    }

    /// The news once `n` events are out (the Platform clock sends them).
    fn news_after(&self, n: usize) -> Vec<EventKind> {
        wait_for("the news", Duration::from_secs(20), || {
            self.news_kinds().len() >= n
        });
        self.news_kinds()
    }

    /// No news, a few clock ticks on.
    fn no_news(&self) -> bool {
        std::thread::sleep(Duration::from_secs(1));
        self.news_kinds().is_empty()
    }

    fn reopen(&mut self) {
        let engine = &self.engine;
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap();
        self.s = engine
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
    }
}

fn row(kind: &str, reference: &str, read: bool) -> (String, String, bool) {
    (kind.into(), reference.into(), read)
}

// ---------------------------------------------------------------------------
// Verdicts and cadence
// ---------------------------------------------------------------------------

/// D2: the winner decides; no winner is still voting while `me` contends.
#[test]
fn every_vote_state_has_one_verdict() {
    let me = id(ME);
    let at = 5_000;
    let cases = [
        (
            poll(Some(PollWinner::Identity(me)), at, &[ME, BOB]),
            Verdict::Won { at },
        ),
        (
            poll(Some(PollWinner::Identity(id(BOB))), at, &[ME, BOB]),
            Verdict::Lost {
                winner: Some(id(BOB)),
                at,
            },
        ),
        (
            poll(Some(PollWinner::Locked), at, &[ME, BOB]),
            Verdict::Locked { at },
        ),
        (
            poll(Some(PollWinner::NoWinner), at, &[ME]),
            Verdict::Lost { winner: None, at },
        ),
        (poll(None, 0, &[BOB, ME]), Verdict::Open),
        // No poll for the label, or one without us, resolved or not.
        (Poll::default(), Verdict::Unplaced),
        (poll(None, 0, &[BOB, CAROL]), Verdict::Unplaced),
        (
            poll(Some(PollWinner::Identity(id(BOB))), at, &[BOB, CAROL]),
            Verdict::Unplaced,
        ),
        (
            poll(Some(PollWinner::Locked), at, &[BOB]),
            Verdict::Unplaced,
        ),
        (poll(Some(PollWinner::NoWinner), at, &[]), Verdict::Unplaced),
    ];
    for (poll, expected) in cases {
        assert_eq!(verdict(&poll, &me), expected, "{poll:?}");
    }
    // An open poll's full page may not list us: open, never unplaced; one
    // short of full that lacks us is complete. A resolved poll lists every
    // contender, so however many, lacking us is unplaced.
    let full = Poll {
        contenders: (0..100u8).map(|n| (id(n + 10), None)).collect(),
        ..Poll::default()
    };
    assert_eq!(verdict(&full, &me), Verdict::Open);
    let mut short = full.clone();
    short.contenders.pop();
    assert_eq!(verdict(&short, &me), Verdict::Unplaced);
    let resolved = Poll {
        winner: Some((PollWinner::Identity(id(BOB)), at)),
        ..full.clone()
    };
    assert_eq!(verdict(&resolved, &me), Verdict::Unplaced);
    let locked = Poll {
        winner: Some((PollWinner::Locked, at)),
        ..full.clone()
    };
    assert_eq!(verdict(&locked, &me), Verdict::Unplaced);
    let won = Poll {
        winner: Some((PollWinner::Identity(me), at)),
        ..full
    };
    assert_eq!(verdict(&won, &me), Verdict::Won { at });
}

#[test]
fn what_the_watch_saw_round_trips() {
    for seen in [
        Seen::New,
        Seen::Open,
        Seen::Won { at: 42, since: 9 },
        Seen::Unplaced { since: 7 },
    ] {
        assert_eq!(Seen::parse(seen.text().as_deref()), seen);
    }
    assert_eq!(Seen::parse(Some("won:x")), Seen::New);
    assert_eq!(Seen::parse(Some("won:42")), Seen::New);
    assert_eq!(Seen::parse(Some("later")), Seen::New);
}

/// D1: 10 min while the end is unknown or far, 1 min in the hour either
/// side of it, 10 min after; a won row 1 min; never checked: now.
#[test]
fn the_cadence_tightens_around_the_end() {
    let end = 100_000;
    let at = |ends_at, state: Option<&str>, checked| due_at(&watch("a", ends_at, state), checked);
    assert_eq!(at(end, None, None), 0);
    assert_eq!(at(0, Some("open"), Some(50_000)), 50_600);
    // Far: 10 min, but not past the start of the last hour.
    assert_eq!(at(end, Some("open"), Some(10_000)), 10_600);
    assert_eq!(at(end, Some("open"), Some(96_000)), 96_400);
    // Around the end.
    assert_eq!(at(end, Some("open"), Some(96_400)), 96_460);
    assert_eq!(at(end, Some("open"), Some(end + 3_599)), end + 3_659);
    assert_eq!(at(end, Some("open"), Some(end + 3_600)), end + 4_200);
    // Won: the library's list only, every minute.
    assert_eq!(at(0, Some("won:5:6"), Some(50_000)), 50_060);
}

// ---------------------------------------------------------------------------
// Outcomes through a session
// ---------------------------------------------------------------------------

/// Lost: one `ContestLost` feed event, news, with the row's ref; the label
/// leaves the open contests and the watch stops asking.
#[test]
fn a_lost_contest_is_one_news_event_and_ends_the_watch() {
    let f = Fixture::new(true);
    f.contending("alice");
    f.watch("alice", 9_000);
    let net = FakePolls::default();
    net.set(
        "a11ce",
        poll(Some(PollWinner::Identity(id(BOB))), now_ms(), &[ME, BOB]),
    );
    f.pass(&net).unwrap();
    assert_eq!(f.outcomes(), [row("contest_lost", "alice@9000", false)]);
    assert_eq!(f.news_after(1), [EventKind::ContestLost]);
    assert_eq!(f.dp().unread_count(b58(ME)).unwrap(), 1);
    assert!(f.watches().is_empty());
    assert!(f.open_contests().is_empty());

    f.later();
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (1, 0));
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(f.news_kinds().len(), 1);
}

/// Locked: `ContestLocked`; nobody gets the name, the watch stops.
#[test]
fn a_locked_contest_is_news_and_ends_the_watch() {
    let f = Fixture::new(true);
    f.contending("alice");
    f.watch("alice", 9_000);
    let net = FakePolls::default();
    net.set(
        "a11ce",
        poll(Some(PollWinner::Locked), now_ms(), &[ME, BOB]),
    );
    f.pass(&net).unwrap();
    assert_eq!(f.outcomes(), [row("contest_locked", "alice@9000", false)]);
    assert_eq!(f.news_after(1), [EventKind::ContestLocked]);
    assert!(f.watches().is_empty());
    assert!(f.open_contests().is_empty());
}

/// Won: `ContestWon` at once; the row waits, without asking Platform, for
/// the library to list the name, whose snapshot then journals nothing new
/// (its `UsernameRegistered` stored read); then the watch ends.
#[test]
fn a_won_contest_is_one_event_and_waits_for_the_name() {
    let f = Fixture::new(true);
    f.contending("alice");
    f.watch("Alice", 9_000);
    let net = FakePolls::default();
    let at = now_ms();
    net.set(
        "a11ce",
        poll(Some(PollWinner::Identity(id(ME))), at, &[ME, BOB]),
    );
    f.pass(&net).unwrap();
    assert_eq!(f.outcomes(), [row("contest_won", "Alice@9000", false)]);
    let [row_] = &f.watches()[..] else {
        panic!("one row")
    };
    let seen = Seen::parse(row_.last_state.as_deref());
    assert!(
        matches!(seen, Seen::Won { at: a, .. } if a == at),
        "{seen:?}"
    );

    // Not listed yet: no query, no event.
    f.later();
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (1, 0));
    assert_eq!(f.watches().len(), 1);

    f.list("alice");
    f.later();
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (1, 0));
    assert_eq!(
        f.outcomes(),
        [
            row("contest_won", "Alice@9000", false),
            row("username_registered", "alice@7", true),
        ]
    );
    assert_eq!(f.news_after(1), [EventKind::ContestWon]);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(f.news_kinds(), [EventKind::ContestWon], "one news event");
    assert!(f.watches().is_empty());
    assert!(f.open_contests().is_empty());
}

/// A won name the library still does not list after a day: the watch
/// stops waiting (the event is already out).
#[test]
fn a_won_row_waits_a_day_for_the_name_at_most() {
    let f = Fixture::new(true);
    f.contending("alice");
    f.watch("alice", 9_000);
    let at = now_ms();
    f.set_state("alice", &format!("won:{at}:{}", unix_now() - 24 * 3600));
    let net = FakePolls::default();
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (0, 0));
    assert_eq!(f.outcomes(), [row("contest_won", "alice@9000", false)]);
    assert!(f.watches().is_empty());
    assert!(f.open_contests().is_empty());
}

/// Still voting: no event; an adopted contest learns its end from the
/// end-date index once, and the watch keeps it.
#[test]
fn a_contest_still_voting_is_adopted_and_learns_its_end() {
    let f = Fixture::new(true);
    f.contending("bob");
    let net = FakePolls::default();
    net.set("b0b", poll(None, 0, &[ME, CAROL]));
    let end = unix_now() + 3 * 3600;
    net.set_end("b0b", end);
    f.names_signalled();
    f.pass(&net).unwrap();
    assert_eq!(f.watches(), [watch("b0b", end, Some("open"))]);
    assert_eq!(net.calls(), (1, 1));
    assert!(f.outcomes().is_empty());
    assert!(f.names_signalled(), "the end learned");

    // Not due yet; then due, with the end known.
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (1, 1));
    f.later();
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (2, 1));
    assert!(!f.names_signalled(), "the end unchanged");
    assert_eq!(f.open_contests(), ["b0b"]);
}

/// A contest that ended while the app was closed: the first pass after a
/// restart finds the outcome, and it is news (the wallet is no restore).
#[test]
fn an_outcome_reached_while_closed_is_news_after_a_restart() {
    let mut f = Fixture::new(true);
    f.contending("alice");
    let net = FakePolls::default();
    net.set("a11ce", poll(None, 0, &[ME, BOB]));
    f.pass(&net).unwrap();
    assert_eq!(f.watches().len(), 1);

    f.reopen();
    let resolved = now_ms() - 3_600_000;
    net.set(
        "a11ce",
        poll(Some(PollWinner::Identity(id(BOB))), resolved, &[ME, BOB]),
    );
    f.pass(&net).unwrap();
    assert_eq!(f.outcomes(), [row("contest_lost", "a11ce@0", false)]);
    assert_eq!(f.news_after(1), [EventKind::ContestLost]);
    assert!(f.watches().is_empty());
}

/// Catch-up silence: an outcome from before the wallet's catch-up boundary
/// (a restore finding an old result) is in the feed, stored read, no news.
#[test]
fn an_outcome_before_the_catch_up_boundary_is_stored_read() {
    let f = Fixture::new(true);
    f.contending("alice");
    f.watch("alice", 9_000);
    let tap = f.s.live().unwrap().tap;
    assert!(tap.move_boundary_to(f.wallet, unix_now() + 60, false));
    let net = FakePolls::default();
    net.set("a11ce", poll(Some(PollWinner::Locked), now_ms(), &[ME]));
    f.pass(&net).unwrap();
    assert_eq!(f.outcomes(), [row("contest_locked", "alice@9000", true)]);
    assert!(f.no_news());
    assert_eq!(f.dp().unread_count(b58(ME)).unwrap(), 0);
    assert!(f.watches().is_empty());
}

/// A wallet before its first bring-up here (a restore whose pass came
/// first) has no catch-up boundary: the watch leaves it alone, so an old
/// outcome is not news, and arms no timer for it.
#[test]
fn a_wallet_without_a_catch_up_boundary_is_not_watched() {
    let f = Fixture::new(true);
    f.give_me();
    f.with_me(|m, p| record_contest(m, "alice", p));
    f.watch("alice", 9_000);
    let net = FakePolls::default();
    net.set("a11ce", poll(Some(PollWinner::Locked), now_ms(), &[ME]));
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (0, 0));
    assert!(f.outcomes().is_empty());
    let manager = f.s.manager().unwrap();
    assert_eq!(f.engine.block_on(f.s.next_contest_pass(&manager)), None);

    f.caught_up();
    f.pass(&net).unwrap();
    assert_eq!(f.outcomes(), [row("contest_locked", "alice@9000", false)]);
}

/// Adoption and `register_name` race for one contest: one row, in the
/// spelling the identity contended with (its `contested_name` row), with
/// the end the registration knew; a fresh registration forgets an
/// unplaced streak.
#[test]
fn a_contest_is_watched_once_in_the_typed_spelling() {
    let f = Fixture::new(true);
    f.contending("a11ce");
    let (s, wallet) = (Arc::clone(&f.s), f.wallet);
    (f.engine)
        .block_on(s.set_name_pref(wallet, b58(ME), "contested_name", Some("Alice".into())))
        .unwrap();
    let net = FakePolls::default();
    net.set("a11ce", poll(None, 0, &[ME, BOB]));
    f.pass(&net).unwrap();
    assert_eq!(f.watches(), [watch("Alice", 0, Some("open"))]);

    f.set_state("Alice", "unplaced:5");
    f.start_watching("alice", Some(9_000));
    assert_eq!(f.watches(), [watch("Alice", 9_000, None)]);
    f.later();
    f.pass(&net).unwrap();
    assert_eq!(f.watches(), [watch("Alice", 9_000, Some("open"))]);
}

/// D2: a row whose identity Platform does not list stays watched for a
/// day, then goes with no event, and the library's stale open contest
/// with it (no re-adoption).
#[test]
fn an_unplaced_watch_ends_after_a_day_without_news() {
    let f = Fixture::new(true);
    f.contending("alice");
    f.watch("alice", 0);
    let net = FakePolls::default();
    f.pass(&net).unwrap();
    let since = Seen::parse(f.watches()[0].last_state.as_deref());
    assert!(matches!(since, Seen::Unplaced { .. }), "{since:?}");

    f.set_state("alice", &format!("unplaced:{}", unix_now() - 24 * 3600));
    f.later();
    f.names_signalled();
    f.pass(&net).unwrap();
    assert!(f.watches().is_empty());
    assert!(f.names_signalled(), "the watch ended");
    assert!(f.outcomes().is_empty());
    assert!(f.open_contests().is_empty());
    f.later();
    f.pass(&net).unwrap();
    assert!(f.watches().is_empty());
}

/// D1's budget: a pass asks about [`PASS_CAP`] rows at most, the longest
/// unchecked first; the end-date index once.
#[test]
fn a_pass_asks_about_sixteen_rows_at_most() {
    let f = Fixture::new(true);
    f.give_me();
    f.caught_up();
    for n in 0..20 {
        f.watch(&format!("name{n:02}"), 0);
    }
    let net = FakePolls::default();
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (16, 1), "sixteen rows, one end-date page");
    // The other four next pass; then nothing is due.
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (16 + 4, 2));
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (20, 2));
}

/// D1's budget, end dates: one end-date request a pass, of one page of
/// [`ENDS_PAGE`] polls, however many end later; a row whose end is beyond
/// it stays unknown (and on the 10-minute cadence).
#[test]
fn a_pass_reads_one_end_date_page() {
    let f = Fixture::new(true);
    f.contending("alice");
    f.watch("alice", 0);
    f.watch("bob", 0);
    let net = FakePolls::default();
    let soon = unix_now() + 600;
    for n in 0..150u64 {
        net.set_end(&format!("p{n:03}"), soon + n);
    }
    net.set_end("b0b", soon);
    net.set_end("a11ce", soon + 1_000);
    net.set("a11ce", poll(None, 0, &[ME]));
    net.set("b0b", poll(None, 0, &[ME]));
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (2, 1));
    assert_eq!(*guard(&net.end_limits), [100]);
    let ends: Vec<(String, u64)> = (f.watches().into_iter())
        .map(|r| (r.label, r.ends_at))
        .collect();
    assert_eq!(ends, [("alice".into(), 0), ("bob".into(), soon)]);
}

/// D2/D7 (Sol r1 #2): an open poll's full first page without us proves
/// nothing, so the row stays open (no unplaced streak, no outcome) and
/// asks again, one request a time. Resolved, Drive answers every
/// contender: listed, the outcome is ours; not listed, unplaced.
#[test]
fn a_full_open_page_without_us_keeps_the_watch_open() {
    let f = Fixture::new(true);
    f.contending("alice");
    f.watch("alice", 9_000);
    let net = FakePolls::default();
    let at = now_ms();
    // We sort after a hundred others: off the open poll's page.
    net.set("a11ce", crowd(None, 0, 100, true));
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (1, 0));
    assert_eq!(f.watches(), [watch("alice", 9_000, Some("open"))]);
    f.later();
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (2, 0));
    assert_eq!(f.watches(), [watch("alice", 9_000, Some("open"))]);

    // Resolved: all 101 listed, us among them.
    let lost = Some(PollWinner::Identity(first_of_crowd()));
    net.set("a11ce", crowd(lost, at, 100, true));
    f.later();
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (3, 0));
    assert_eq!(f.outcomes(), [row("contest_lost", "alice@9000", false)]);

    // Resolved without us: unplaced, no outcome.
    f.watch("bob", 9_000);
    net.set("b0b", crowd(lost, at, 100, false));
    f.later();
    f.pass(&net).unwrap();
    assert_eq!(net.calls(), (4, 0));
    assert_eq!(f.outcomes().len(), 1);
    let seen = Seen::parse(f.watches()[0].last_state.as_deref());
    assert!(matches!(seen, Seen::Unplaced { .. }), "{seen:?}");
}

/// Sol r1 #1: on a restore, the marketplace lists a won name before its
/// name-state row and before the watch asks. The win is still the watch's
/// alone, aged by the resolving block, so an old win is stored read; the
/// username is history.
#[test]
fn an_old_win_listed_before_the_watch_asks_is_stored_read() {
    let f = Fixture::new(true);
    f.contending("alice");
    f.watch("alice", 9_000);
    let tap = f.s.live().unwrap().tap;
    assert!(tap.move_boundary_to(f.wallet, unix_now() + 60, false));
    f.list("alice");
    assert_eq!(f.outcomes(), [row("username_registered", "alice@7", true)]);

    let net = FakePolls::default();
    let at = now_ms();
    net.set(
        "a11ce",
        poll(Some(PollWinner::Identity(id(ME))), at, &[ME, BOB]),
    );
    f.pass(&net).unwrap();
    assert_eq!(
        f.outcomes(),
        [
            row("username_registered", "alice@7", true),
            row("contest_won", "alice@9000", true),
        ]
    );
    assert!(f.no_news());
    assert!(f.watches().is_empty());
    assert!(f.open_contests().is_empty());
}

/// The same order live (no restore): the name is history and the watch's
/// win is the one news event.
#[test]
fn a_win_listed_before_the_watch_asks_is_one_news_event() {
    let f = Fixture::new(true);
    f.contending("alice");
    f.watch("alice", 9_000);
    f.list("alice");
    let net = FakePolls::default();
    net.set(
        "a11ce",
        poll(Some(PollWinner::Identity(id(ME))), now_ms(), &[ME]),
    );
    f.pass(&net).unwrap();
    assert_eq!(
        f.outcomes(),
        [
            row("username_registered", "alice@7", true),
            row("contest_won", "alice@9000", false),
        ]
    );
    assert_eq!(f.news_after(1), [EventKind::ContestWon]);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(f.news_kinds(), [EventKind::ContestWon]);
    assert!(f.watches().is_empty());
}

/// D4 (DP2-05's rule): a pass that read the wallet before its removal and
/// a same-seed reimport writes nothing into the reimported wallet: no
/// event, and its restored watch row stays.
#[test]
fn a_pass_that_outlives_its_wallet_writes_nothing() {
    let dir = dw_testutil::private_tempdir();
    let dest = dir.path().join("me.dwbackup");
    let f = Fixture::new(true);
    f.contending("alice");
    f.watch("alice", 9_000);
    let pw = || Some(Zeroizing::new(b"bk".to_vec()));
    (f.engine)
        .block_on(f.s.backup_wallet(f.wallet, dest.clone(), pw()))
        .unwrap();

    let net = Arc::new(FakePolls::default());
    net.set(
        "a11ce",
        poll(Some(PollWinner::Identity(id(BOB))), now_ms(), &[ME, BOB]),
    );
    let held = Arc::new(Held::default());
    *guard(&net.hold) = Some(Arc::clone(&held));
    let (s, wallet, n) = (Arc::clone(&f.s), f.wallet, Arc::clone(&net));
    let passing = f.s.rt.spawn(async move {
        let manager = s.manager().unwrap();
        s.watch_contests(&*n, &manager, wallet).await
    });
    wait_for("the held query", Duration::from_secs(20), || {
        held.reached.load(Ordering::SeqCst)
    });

    let wipe = (f.s.vault())
        .authorize(
            dw_vault::GrantPurpose::Wipe,
            Some(&f.wallet.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    (f.engine)
        .block_on(f.s.remove_wallet(f.wallet, wipe))
        .unwrap();
    let ids = (f.engine).block_on(f.s.restore_backup(dest, pw())).unwrap();
    assert_eq!(ids, [f.wallet]);

    held.release.notify_one();
    let passed = f.engine.block_on(passing).unwrap();
    assert!(
        matches!(passed, Err(PlatformError::WalletNotFound)),
        "{passed:?}"
    );
    assert!(f.outcomes().is_empty());
    assert!(f.no_news());
    assert_eq!(f.watches(), [watch("alice", 9_000, None)]);
}

/// `contest_status`: Platform's state now, with the contenders' votes and
/// the end; an outcome for a watched contest is applied as the watch
/// applies it; a label we neither contend for nor watch is refused.
#[test]
fn contest_status_reads_the_poll_and_applies_an_outcome() {
    let f = Fixture::new(true);
    f.contending("alice");
    let net = Arc::new(FakePolls::default());
    *guard(&f.s.platform.contests.net) = Some(Arc::clone(&net));
    let status = |label: &str| {
        let dp = f.dp();
        f.engine.block_on(dp.contest_status(b58(ME), label.into()))
    };

    net.set("a11ce", poll(None, 0, &[BOB, ME]));
    let end = unix_now() + 3_600;
    net.set_end("a11ce", end);
    let open = status("alice").unwrap();
    assert_eq!(open.state, ContestState::Open);
    assert_eq!(open.ends_at, Some(end));
    // In identity order, as Drive pages them.
    assert_eq!(
        open.contenders,
        [
            ContestContender {
                identity: b58(ME),
                votes: Some(1),
                is_self: true,
            },
            ContestContender {
                identity: b58(BOB),
                votes: Some(2),
                is_self: false,
            },
        ]
    );
    assert_eq!((open.lock_votes, open.abstain_votes), (Some(1), Some(0)));
    assert!(f.outcomes().is_empty(), "a read of an unwatched contest");

    let refused = status("carol").unwrap_err();
    assert_eq!(refused.code(), "invalid_argument");
    // Resolved, but between others.
    net.set(
        "a11ce",
        poll(Some(PollWinner::Identity(id(BOB))), now_ms(), &[BOB, CAROL]),
    );
    let refused = status("alice").unwrap_err();
    assert_eq!(refused.code(), "invalid_argument");

    f.watch("alice", 9_000);
    net.set(
        "a11ce",
        poll(Some(PollWinner::Identity(id(BOB))), now_ms(), &[ME, BOB]),
    );
    let lost = status("alice").unwrap();
    assert_eq!(
        lost.state,
        ContestState::Lost {
            winner: Some(b58(BOB))
        }
    );
    assert_eq!(lost.ends_at, Some(9_000));
    assert_eq!(f.outcomes(), [row("contest_lost", "alice@9000", false)]);
    assert!(f.watches().is_empty());
}

/// `registrations()`: a contested row shows the end its watch row holds.
#[test]
fn a_contested_registration_shows_the_watched_end() {
    let f = Fixture::new(true);
    f.give_me();
    let (w, row) = (
        f.wallet.to_string(),
        dw_appdb::NewRegistration {
            identity_index: Some(0),
            identity: Some(b58(ME)),
            label: "alice".into(),
            temp_label: None,
            funding: super::registration_rows::Funding::CoreBalance {
                lock_duffs: 25_000_000,
            }
            .encode(),
            initial_profile: None,
            phase: "contested".into(),
        },
    );
    let draft = (f.engine)
        .block_on(f.s.appdb_op(move |db| db.insert_registration(&w, &row, 1)))
        .unwrap()
        .to_string();
    let ends = || {
        let all = f.engine.block_on(f.dp().registrations()).unwrap();
        let row = all.into_iter().find(|r| r.draft == draft).unwrap();
        row.contest_ends_at
    };
    assert_eq!(ends(), None);
    f.watch("Alice", 0);
    assert_eq!(ends(), None, "end unknown");
    f.watch("Alice", 9_000);
    assert_eq!(ends(), Some(9_000));
}

// ---------------------------------------------------------------------------
// Live
// ---------------------------------------------------------------------------

/// Labels of past testnet contests (the explorer's `contestedResources`
/// dump, 2026-10-13): their vote states are kept for good once resolved.
const TESTNET_LABELS: &[&str] = &[
    "dash1",
    "c0ntest",
    "detv0tetest01",
    "b00",
    "asdv0t1ng",
    "hehe",
    "das",
    "dsa",
    "asdsep1ast",
];

/// T2, read-only (no wallet, no write): real testnet polls through the
/// SDK the watch uses, each contender's verdict printed. Won, lost and
/// locked are each seen; any poll still in the end-date index is open (or
/// ended and awaiting its block) for every contender.
#[test]
#[ignore = "public testnet, read-only"]
fn t2_testnet_polls_give_won_lost_and_locked() {
    let dir = dw_testutil::private_tempdir();
    let engine = super::recovery_tests::engine(dir.path());
    let s = engine
        .block_on(engine.open_network(DashNetwork::Testnet, SessionOptions::default()))
        .unwrap();
    let net = SdkContests(s.manager().unwrap().sdk_arc());
    let ends = engine.block_on(ends_page(&net)).unwrap();
    eprintln!("end-date index: {ends:?}");
    let mut labels: Vec<String> = TESTNET_LABELS.iter().map(|l| l.to_string()).collect();
    labels.extend(ends.keys().cloned());
    let mut seen = BTreeMap::<&str, Vec<String>>::new();
    for label in &labels {
        let poll = engine.block_on(net.page(label, POLL_PAGE)).unwrap();
        eprintln!(
            "{label}: winner {:?}, {} contenders, lock {:?}, abstain {:?}",
            poll.winner,
            poll.contenders.len(),
            poll.lock_votes,
            poll.abstain_votes
        );
        for (who, votes) in &poll.contenders {
            let v = verdict(&poll, who);
            eprintln!(
                "  {} ({votes:?} votes): {v:?}",
                super::registration_flow::b58(who)
            );
            let kind = match v {
                Verdict::Open => "open",
                Verdict::Won { .. } => "won",
                Verdict::Lost { .. } => "lost",
                Verdict::Locked { .. } => "locked",
                Verdict::Unplaced => unreachable!("a listed contender is placed"),
            };
            seen.entry(kind).or_default().push(label.clone());
            if ends.contains_key(label) {
                assert_eq!(v, Verdict::Open, "{label} is still in the end-date index");
            }
        }
        if poll.contenders.is_empty() {
            assert_eq!(verdict(&poll, &id(ME)), Verdict::Unplaced, "{label}");
        }
    }
    eprintln!("seen: {seen:?}");
    for kind in ["won", "lost", "locked"] {
        assert!(seen.contains_key(kind), "no {kind} poll among {labels:?}");
    }
    engine.block_on(engine.shutdown()).unwrap();
}
