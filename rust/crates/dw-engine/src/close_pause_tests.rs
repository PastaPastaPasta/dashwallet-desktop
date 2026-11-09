//! An in-app close that must not cut what runs (`Engine::close_network_unless_busy`)
//! against a real session: an operation admitted before the check that
//! becomes busy during the drain refuses the close, and the session goes
//! on unchanged.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::SweepDestination;
use super::sweep_tests::{
    ReleaseOnDrop, hook, move_quote, pause_at, reached, spawn_move, spend_grant, within,
};
use crate::coins::TestPause;
use crate::platform::lease::session_tests::{Fixture, fixture};
use crate::platform::{FlowKind, LeaseStateView};
use crate::{CloseRefused, DashNetwork, NetworkSession};

const COIN: u64 = 100_000_000;
const DRAIN: Duration = Duration::from_secs(20);

/// What the app's check finds: flows holding a live lease, mixing, and
/// `broadcasts` (the app's count of broadcasts not yet returned).
#[derive(Debug, Default, PartialEq)]
struct Busy {
    flows: usize,
    mixing: bool,
    broadcast: bool,
}

fn busy(
    broadcasts: Arc<AtomicUsize>,
) -> impl Fn(&NetworkSession) -> Option<Busy> + Send + Sync + 'static {
    move |s| {
        let flows = (s.leases().unwrap().iter())
            .filter(|l| {
                !matches!(
                    l.state,
                    LeaseStateView::Revoked { .. } | LeaseStateView::Ended
                )
            })
            .count();
        let busy = Busy {
            flows,
            mixing: s.mixing_busy(),
            broadcast: broadcasts.load(Ordering::SeqCst) > 0,
        };
        (busy != Busy::default()).then_some(busy)
    }
}

/// The close, on its own thread, refused or not.
fn close(f: &Fixture, broadcasts: &Arc<AtomicUsize>) -> Result<bool, CloseRefused<Busy>> {
    let busy = busy(Arc::clone(broadcasts));
    f.engine.block_on(
        f.engine
            .close_network_unless_busy(DashNetwork::Regtest, DRAIN, busy),
    )
}

/// Waits until the close is draining.
fn draining(f: &Fixture, drains: usize) {
    for _ in 0..2_000 {
        if f.session.gate.drains() == drains {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the close never drained");
}

/// The session is the open one, admits operations and kept its wallet.
fn unchanged(f: &Fixture) {
    let open = f.engine.block_on(f.engine.session(&DashNetwork::Regtest));
    assert!(open.is_some_and(|s| Arc::ptr_eq(&s, &f.session)));
    assert!(f.session.is_open());
    assert!(f.session.try_enter().is_ok());
    within(f, f.session.wallet(&f.wallet)).unwrap();
}

fn refused_busy(r: Result<bool, CloseRefused<Busy>>, expected: Busy) {
    match r {
        Err(CloseRefused::Busy(b)) => assert_eq!(b, expected),
        other => panic!("{other:?}"),
    }
}

/// Mixing admitted before the check, waiting before it sets its flag: the
/// close waits for it, sees it mixing and is refused.
#[test]
fn a_mixing_start_admitted_before_the_check_refuses_the_close() {
    let f = fixture(false);
    for n in 1..=4 {
        f.credit_coinjoin(n, COIN);
    }
    hook(&f, 500, &[]);
    let read = pause_at(&f, |h, p| h.start_after_read = Some(p));
    let _release = ReleaseOnDrop(vec![Arc::clone(&read)]);
    let starting = {
        let (s, w) = (Arc::clone(&f.session), f.wallet);
        (f.session.rt).spawn(async move { s.start_mixing(w).await })
    };
    reached(&read);
    let broadcasts = Arc::new(AtomicUsize::new(0));
    std::thread::scope(|scope| {
        let closing = scope.spawn(|| close(&f, &broadcasts));
        draining(&f, 1);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!closing.is_finished(), "the close waits for the start");
        read.release.notify_one();
        refused_busy(
            closing.join().unwrap(),
            Busy {
                mixing: true,
                ..Busy::default()
            },
        );
    });
    within(&f, async { starting.await.unwrap() }).unwrap();
    assert!(f.session.coinjoin.is_mixing(&f.wallet));
    unchanged(&f);
}

/// A flow admitted before the check, waiting before its lease: the close
/// waits for it, sees the lease and is refused; the flow keeps its lease.
#[test]
fn a_flow_admitted_before_the_check_refuses_the_close() {
    let f = fixture(false);
    let grant = f.platform_op(0, 1_000);
    let admitted = pause_at(&f, |h, p| h.flow_admitted = Some(p));
    let _release = ReleaseOnDrop(vec![Arc::clone(&admitted)]);
    let flowing = {
        let (s, w) = (Arc::clone(&f.session), f.wallet);
        (f.session.rt)
            .spawn(async move { s.begin_flow(w, FlowKind::ContactRequest, vec![grant]).await })
    };
    reached(&admitted);
    let broadcasts = Arc::new(AtomicUsize::new(0));
    std::thread::scope(|scope| {
        let closing = scope.spawn(|| close(&f, &broadcasts));
        draining(&f, 1);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!closing.is_finished(), "the close waits for the flow");
        admitted.release.notify_one();
        refused_busy(
            closing.join().unwrap(),
            Busy {
                flows: 1,
                ..Busy::default()
            },
        );
    });
    f.engine.block_on(flowing).unwrap().unwrap();
    let leases = f.session.leases().unwrap();
    assert!(
        matches!(leases[..], [ref l] if !matches!(l.state, LeaseStateView::Revoked { .. } | LeaseStateView::Ended))
    );
    unchanged(&f);
}

/// A broadcast started before the check refuses the close at once and
/// returns its result; one asked for while the close drains waits for the
/// decision, refuses it, and then runs on the session it started on.
#[test]
fn a_broadcast_started_before_the_decision_refuses_the_close() {
    let f = fixture(false);
    for n in 1..=4 {
        f.credit_coinjoin(n, COIN);
    }
    let wire = hook(&f, 500, &[]);
    let broadcasts = Arc::new(AtomicUsize::new(0));

    // Started (the app counts it, then the engine admits it) before the check.
    let before = pause_at(&f, |h, p| h.before_broadcast = Some((0, p)));
    let _release = ReleaseOnDrop(vec![Arc::clone(&before)]);
    broadcasts.fetch_add(1, Ordering::SeqCst);
    let moving = spawn_move(&f);
    reached(&before);
    refused_busy(
        close(&f, &broadcasts),
        Busy {
            broadcast: true,
            ..Busy::default()
        },
    );
    assert_eq!(f.session.gate.drains(), 0, "refused by the first check");
    before.release.notify_one();
    let moved = f.engine.block_on(moving).unwrap().unwrap();
    broadcasts.fetch_sub(1, Ordering::SeqCst);
    assert_eq!((moved.txids.len(), wire.txs().len()), (1, 1));
    unchanged(&f);

    // Asked for while the close drains an operation admitted earlier.
    f.credit_coinjoin(9, COIN);
    let wire = hook(&f, 500, &[]);
    // Read before the pause: the plan is an operation of its own.
    let (quote, grant) = (move_quote(&f), spend_grant(&f));
    let held = Arc::new(TestPause::default());
    let _release = ReleaseOnDrop(vec![Arc::clone(&held)]);
    let earlier = {
        let (s, held) = (Arc::clone(&f.session), Arc::clone(&held));
        (f.session.rt).spawn(async move {
            let _op = s.enter().await.unwrap();
            held.reached.store(true, Ordering::SeqCst);
            held.release.notified().await;
        })
    };
    reached(&held);
    std::thread::scope(|scope| {
        let closing = scope.spawn(|| close(&f, &broadcasts));
        draining(&f, 1);
        broadcasts.fetch_add(1, Ordering::SeqCst);
        let moving = {
            let (s, w) = (Arc::clone(&f.session), f.wallet);
            (f.session.rt).spawn(async move {
                s.move_mixed_coins(w, SweepDestination::Wallet, quote, grant)
                    .await
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            wire.txs().is_empty(),
            "the broadcast waits for the decision"
        );
        held.release.notify_one();
        refused_busy(
            closing.join().unwrap(),
            Busy {
                broadcast: true,
                ..Busy::default()
            },
        );
        let moved = f.engine.block_on(moving).unwrap().unwrap();
        broadcasts.fetch_sub(1, Ordering::SeqCst);
        assert_eq!((moved.txids.len(), wire.txs().len()), (1, 1));
    });
    f.engine.block_on(earlier).unwrap();
    unchanged(&f);
}

/// Nothing busy: the close commits, and an operation that waited on the
/// pause is refused.
#[test]
fn an_idle_session_closes_and_refuses_what_waited() {
    let f = fixture(false);
    let held = Arc::new(TestPause::default());
    let _release = ReleaseOnDrop(vec![Arc::clone(&held)]);
    let earlier = {
        let (s, held) = (Arc::clone(&f.session), Arc::clone(&held));
        (f.session.rt).spawn(async move {
            let _op = s.enter().await.unwrap();
            held.reached.store(true, Ordering::SeqCst);
            held.release.notified().await;
        })
    };
    reached(&held);
    let broadcasts = Arc::new(AtomicUsize::new(0));
    std::thread::scope(|scope| {
        let closing = scope.spawn(|| close(&f, &broadcasts));
        draining(&f, 1);
        let late = {
            let s = Arc::clone(&f.session);
            (f.session.rt).spawn(async move { s.enter().await.is_err() })
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(!late.is_finished(), "waits on the pause");
        held.release.notify_one();
        assert!(matches!(closing.join().unwrap(), Ok(true)));
        assert!(
            f.engine.block_on(late).unwrap(),
            "refused once the close committed"
        );
    });
    f.engine.block_on(earlier).unwrap();
    assert!(!f.session.is_open());
    assert!(
        f.engine
            .block_on(f.engine.session(&DashNetwork::Regtest))
            .is_none()
    );
}
