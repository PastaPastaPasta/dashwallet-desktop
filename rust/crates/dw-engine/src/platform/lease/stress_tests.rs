//! The stress harness and its checker (E0-04 §12 "Stress"), on a fake
//! library: the table and the fence log every J step and every transport
//! call in J order; the checker replays the log against I1–I6.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use tokio::time::Instant;

use super::fence::JournalBackend;
use super::table::LeaseTable;
use super::tests::{FakeJournal, H, W, begin, core, status, table, transition};
use super::{
    AdmitRequest, ArtifactId, ArtifactKind, DispatchScope, LeaseError, LeaseId, Outcome,
    UnleasedKind, Verdict,
};
use crate::platform::flows::RevokeCause;

/// One J step of the log.
#[derive(Debug, Clone)]
pub(crate) enum LogEvent {
    /// A lease was inserted.
    Begin {
        lease: LeaseId,
    },
    /// A freeze: the leases it revoked and the permits in flight.
    Freeze {
        lock: u64,
        at: Instant,
        revoked: Vec<LeaseId>,
        in_flight: Vec<(u64, Instant)>,
    },
    /// The drain of lock `lock` ended (its barrier share released).
    LockReturned {
        lock: u64,
        at: Instant,
    },
    /// A permit or guard granted: the commit of a First.
    Grant {
        attempt: u64,
        lease: Option<LeaseId>,
        artifact: ArtifactId,
        deadline: Option<Instant>,
    },
    /// A `Dispatching` record or step marker is durable.
    Durable {
        artifact: ArtifactId,
    },
    /// The fake transport handed the artifact off.
    Transport {
        artifact: ArtifactId,
        attempt: u64,
        at: Instant,
    },
    Refused {
        artifact: ArtifactId,
        cleanup: bool,
    },
    Finish {
        attempt: u64,
    },
}

/// Builds the stress must catch (§10.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mutation {
    /// The lease is checked in one J step, the permit granted in another.
    CheckThenAct,
    /// Leases are revoked when the drain ends, not at the freeze.
    RevokeInDrain,
    /// A registered artifact's transport runs before its record is durable.
    TransportFirst,
}

/// The slack §12 allows a lock's return over its bound.
const SLACK: Duration = Duration::from_millis(250);

/// Replays `log` against the invariants:
/// - I1: every leased commit (a Grant) precedes every freeze after its
///   lease began: no lock lets a lease it should have revoked commit;
/// - I2: no refused artifact is handed off before its refusal, and none
///   whose refusal cleaned it up is ever handed off;
/// - I3: each lock returns by `max(freeze + H, its last in-flight
///   deadline)` + 250 ms;
/// - I4: a registered artifact is handed off only after a durable record;
/// - I5: each artifact is cleaned up (its reservation released) at most once;
/// - I6: no permit's transport starts at or after its deadline, and every
///   transport has a grant for its own artifact.
pub(crate) fn check(log: &[LogEvent], registered: &HashSet<ArtifactId>) -> Result<(), String> {
    let mut begun: HashMap<LeaseId, usize> = HashMap::new();
    let mut freezes: Vec<usize> = Vec::new();
    let mut revoked: HashSet<LeaseId> = HashSet::new();
    let mut locks: HashMap<u64, (Instant, Option<Instant>)> = HashMap::new();
    let mut attempts: HashMap<u64, (ArtifactId, Option<Instant>)> = HashMap::new();
    let mut durable: HashSet<ArtifactId> = HashSet::new();
    let mut sent: HashSet<ArtifactId> = HashSet::new();
    let mut cleaned: HashSet<ArtifactId> = HashSet::new();
    for (n, e) in log.iter().enumerate() {
        match e {
            LogEvent::Begin { lease } => {
                begun.insert(*lease, n);
            }
            LogEvent::Freeze {
                lock,
                at,
                revoked: r,
                in_flight,
            } => {
                freezes.push(n);
                revoked.extend(r.iter().copied());
                locks.insert(*lock, (*at, in_flight.iter().map(|(_, d)| *d).max()));
            }
            LogEvent::LockReturned { lock, at } => {
                let (frozen, last) = locks[lock];
                let bound = last.map_or(frozen + H, |d| d.max(frozen + H)) + SLACK;
                if *at > bound {
                    return Err(format!("I3: lock {lock} returned {:?} late", *at - bound));
                }
            }
            LogEvent::Grant {
                attempt,
                lease,
                artifact,
                deadline,
            } => {
                if let Some(l) = lease {
                    let began = begun.get(l).copied().unwrap_or(0);
                    if revoked.contains(l) || freezes.iter().any(|f| *f > began) {
                        return Err(format!(
                            "I1: event {n}: {artifact} committed under a lease a lock revoked"
                        ));
                    }
                }
                attempts.insert(*attempt, (*artifact, *deadline));
            }
            LogEvent::Durable { artifact } => {
                durable.insert(*artifact);
            }
            LogEvent::Transport {
                artifact,
                attempt,
                at,
            } => {
                let Some((granted, deadline)) = attempts.get(attempt) else {
                    return Err(format!("I6: event {n}: transport without a grant"));
                };
                if granted != artifact {
                    return Err(format!(
                        "I6: event {n}: attempt {attempt} is not {artifact}'s"
                    ));
                }
                if deadline.is_some_and(|d| *at >= d) {
                    return Err(format!(
                        "I6: event {n}: {artifact} handed off after its deadline"
                    ));
                }
                if registered.contains(artifact) && !durable.contains(artifact) {
                    return Err(format!(
                        "I4: event {n}: {artifact} handed off before its record"
                    ));
                }
                if cleaned.contains(artifact) {
                    return Err(format!(
                        "I2: event {n}: {artifact} handed off after its cleanup"
                    ));
                }
                sent.insert(*artifact);
            }
            LogEvent::Refused { artifact, cleanup } => {
                if sent.contains(artifact) {
                    return Err(format!(
                        "I2: event {n}: {artifact} refused after a hand-off"
                    ));
                }
                if *cleanup && !cleaned.insert(*artifact) {
                    return Err(format!("I5: event {n}: {artifact} cleaned up twice"));
                }
            }
            LogEvent::Finish { attempt } => {
                if !attempts.contains_key(attempt) {
                    return Err(format!(
                        "I6: event {n}: attempt {attempt} finished, never granted"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The shared state of one stress run.
struct Run {
    t: Arc<LeaseTable>,
    j: Arc<FakeJournal>,
    rng: Mutex<StdRng>,
    stop: AtomicBool,
    next: AtomicU32,
    /// Every artifact a flow admitted, for the resume tasks.
    done: Mutex<Vec<(ArtifactId, ArtifactKind)>>,
    registered: Mutex<HashSet<ArtifactId>>,
    salt: u8,
}

impl Run {
    fn below(&self, n: u64) -> u64 {
        self.rng.lock().unwrap().gen_range(0..n.max(1))
    }

    fn pause(&self, max_ms: u64) -> tokio::time::Sleep {
        tokio::time::sleep(Duration::from_millis(self.below(max_ms)))
    }

    /// A fresh artifact id: the run's salt and a counter.
    fn artifact(&self) -> ArtifactId {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        let mut a = ArtifactId([self.salt; 32]);
        a.0[..4].copy_from_slice(&n.to_le_bytes());
        a
    }

    /// The fake library's hand-off under a verdict: a random pause (it may
    /// outlive the permit), then the transport unless the deadline passed,
    /// with a random outcome. Only bytes that may have gone out are logged.
    async fn hand_off(&self, a: ArtifactId, v: Verdict) {
        let outcome = |r: u64| match r {
            0 => Outcome::NotSent,
            1 => Outcome::MaybeSent,
            _ => Outcome::Sent,
        };
        match v {
            Verdict::First(p) => {
                // No pause at all a quarter of the time: a hand-off right
                // after `admit` returned.
                match self.below(8) {
                    0 | 1 => {}
                    2 => self.pause(2 * H.as_millis() as u64).await,
                    _ => self.pause(50).await,
                }
                if Instant::now() < p.deadline() {
                    let o = outcome(self.below(4));
                    if o != Outcome::NotSent {
                        self.t.log_transport(a, p.id());
                    }
                    p.finish(o);
                } else {
                    p.finish(Outcome::MaybeSent);
                }
            }
            Verdict::Resend(g) | Verdict::FirstUnleased(g) => {
                self.pause(50).await;
                let o = outcome(self.below(4));
                if o != Outcome::NotSent {
                    self.t.log_transport(a, g.id());
                }
                g.finish(o);
            }
            Verdict::Refused { .. } | Verdict::Deferred => {}
        }
    }

    /// One of the 16 flows: a lease, one artifact of a random kind, a
    /// random pause before `admit`, the hand-off, sometimes a repeat.
    async fn flow(self: Arc<Self>) {
        while !self.stop.load(Ordering::SeqCst) {
            let lease = match begin(&self.t, W, 1_000_000, 1_000_000, 1_000_000).await {
                Ok(l) => l,
                Err(LeaseError::Locked) => continue,
                Err(e) => panic!("begin: {e:?}"),
            };
            let a = self.artifact();
            let req = match self.below(3) {
                0 => {
                    let Ok(charge) = lease.charge_spend(10) else {
                        continue;
                    };
                    self.t.bind_spend(lease.id(), W, a, charge);
                    core(&lease, a)
                }
                1 => transition(&lease, a, 1),
                _ => {
                    let t = Arc::clone(&self.t);
                    let registered = lease
                        .scope(async move { t.register(W, a, 10, Vec::new()).await })
                        .await;
                    if registered.is_err() {
                        continue;
                    }
                    self.registered.lock().unwrap().insert(a);
                    AdmitRequest {
                        kind: ArtifactKind::CoreTx { asset_lock: true },
                        tracked_row: true,
                        ..core(&lease, a)
                    }
                }
            };
            self.pause(100).await;
            let kind = req.kind;
            let v = self.t.admit(req.clone()).await;
            self.done.lock().unwrap().push((a, kind));
            self.hand_off(a, v).await;
            if self.below(3) == 0 {
                self.pause(100).await;
                let v = self.t.admit(req).await;
                self.hand_off(a, v).await;
            }
            if self.below(2) == 0 {
                lease.end();
            }
        }
    }

    /// One of the 4 resume tasks: an earlier artifact admitted again under
    /// a new lease or as an unleased rebroadcast.
    async fn resume(self: Arc<Self>) {
        while !self.stop.load(Ordering::SeqCst) {
            self.pause(150).await;
            let pick = {
                let done = self.done.lock().unwrap();
                if done.is_empty() {
                    continue;
                }
                done[self.below(done.len() as u64) as usize]
            };
            let (a, kind) = pick;
            let tracked = self.registered.lock().unwrap().contains(&a);
            let req = |scope| AdmitRequest {
                wallet: W,
                artifact: a,
                kind,
                tracked_row: tracked,
                scope,
            };
            let v = if self.below(2) == 0 {
                let t = Arc::clone(&self.t);
                DispatchScope::unleased(W, UnleasedKind::Rebroadcast, async move {
                    t.admit(req(DispatchScope::current())).await
                })
                .await
            } else {
                let Ok(lease) = begin(&self.t, W, 1_000_000, 1_000_000, 0).await else {
                    continue;
                };
                let t = Arc::clone(&self.t);
                lease
                    .scope(async move { t.admit(req(DispatchScope::current())).await })
                    .await
            };
            self.hand_off(a, v).await;
        }
    }

    /// The lock driver: `cycles` lock (async or sync), unlock and journal
    /// fault cycles at random intervals.
    async fn locks(self: Arc<Self>, cycles: usize) {
        for _ in 0..cycles {
            self.pause(120).await;
            self.j
                .dispatch_fault
                .store([0, 0, 0, 1, 2][self.below(5) as usize], Ordering::SeqCst);
            match self.below(3) {
                0 => {
                    self.t.lock_sync(RevokeCause::Lock, status);
                }
                1 => {
                    self.t.wait_gates().await;
                }
                _ => {
                    self.t.lock(RevokeCause::Lock, status).await;
                }
            }
        }
        self.j.dispatch_fault.store(0, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// One run: 16 flows, 4 resume tasks, `cycles` lock cycles; returns the
/// checker's verdict.
async fn stress(seed: u64, mutation: Option<Mutation>, cycles: usize) -> Result<(), String> {
    let (t, _) = table();
    let j = Arc::new(FakeJournal::default());
    j.stall_ms.store(1, Ordering::SeqCst);
    t.load_journal(
        Some(j.clone() as Arc<dyn JournalBackend>),
        Vec::new(),
        Vec::new(),
    );
    t.inspect(|i| i.mutation = mutation);
    let run = Arc::new(Run {
        t: Arc::clone(&t),
        j,
        rng: Mutex::new(StdRng::seed_from_u64(seed)),
        stop: AtomicBool::new(false),
        next: AtomicU32::new(0),
        done: Mutex::new(Vec::new()),
        registered: Mutex::new(HashSet::new()),
        salt: seed as u8,
    });
    let mut tasks = Vec::new();
    for _ in 0..16 {
        tasks.push(tokio::spawn(Arc::clone(&run).flow()));
    }
    for _ in 0..4 {
        tasks.push(tokio::spawn(Arc::clone(&run).resume()));
    }
    tasks.push(tokio::spawn(Arc::clone(&run).locks(cycles)));
    for task in tasks {
        task.await.unwrap();
    }
    // Let every spawned drain finish.
    tokio::time::sleep(2 * H).await;
    let log = t.inspect(|i| std::mem::take(&mut i.log));
    assert!(
        log.iter()
            .filter(|e| matches!(e, LogEvent::Transport { .. }))
            .count()
            > 50,
        "the run handed off too little to mean anything"
    );
    check(&log, &run.registered.lock().unwrap())
}

#[tokio::test(start_paused = true)]
async fn the_stress_upholds_i1_to_i6() {
    for seed in 1..=3 {
        stress(seed, None, 300)
            .await
            .unwrap_or_else(|e| panic!("seed {seed}: {e}"));
    }
}

/// Each mutation build must fail the checker (§12 "Mutation runs").
#[tokio::test(start_paused = true)]
async fn the_stress_catches_every_mutation() {
    // Each mutation must break the invariant it targets, not just any.
    for (mutation, invariant) in [
        (Mutation::CheckThenAct, "I1"),
        (Mutation::RevokeInDrain, "I1"),
        (Mutation::TransportFirst, "I4"),
    ] {
        let mut seen = Vec::new();
        for seed in 1..=5 {
            if let Err(e) = stress(seed, Some(mutation), 300).await {
                seen.push(e);
                if seen.last().is_some_and(|e| e.starts_with(invariant)) {
                    break;
                }
            }
        }
        let e = seen
            .iter()
            .find(|e| e.starts_with(invariant))
            .unwrap_or_else(|| panic!("{mutation:?} did not break {invariant}: {seen:?}"));
        eprintln!("{mutation:?}: {e}");
    }
}

#[test]
fn the_checker_flags_each_invariant() {
    let at = Instant::now();
    let l = [7u8; 16];
    let a = ArtifactId([1; 32]);
    let grant = |attempt, lease| LogEvent::Grant {
        attempt,
        lease,
        artifact: a,
        deadline: Some(at + H),
    };
    let none = HashSet::new();
    let freeze = LogEvent::Freeze {
        lock: 1,
        at,
        revoked: vec![],
        in_flight: vec![],
    };
    // I1: a commit under a lease after a freeze that should have revoked it.
    let log = [
        LogEvent::Begin { lease: l },
        freeze.clone(),
        grant(1, Some(l)),
    ];
    assert!(check(&log, &none).unwrap_err().starts_with("I1"));
    // I2: a refusal after a hand-off.
    let transport = LogEvent::Transport {
        artifact: a,
        attempt: 1,
        at,
    };
    let log = [
        grant(1, None),
        transport.clone(),
        LogEvent::Refused {
            artifact: a,
            cleanup: false,
        },
    ];
    assert!(check(&log, &none).unwrap_err().starts_with("I2"));
    // I3: a late return.
    let log = [
        freeze,
        LogEvent::LockReturned {
            lock: 1,
            at: at + H + SLACK + Duration::from_millis(1),
        },
    ];
    assert!(check(&log, &none).unwrap_err().starts_with("I3"));
    // I4: a registered artifact before its record.
    let log = [grant(1, None), transport.clone()];
    assert!(
        check(&log, &HashSet::from([a]))
            .unwrap_err()
            .starts_with("I4")
    );
    assert!(check(&log, &none).is_ok());
    // I5: two cleanups.
    let refused = LogEvent::Refused {
        artifact: a,
        cleanup: true,
    };
    assert!(
        check(&[refused.clone(), refused], &none)
            .unwrap_err()
            .starts_with("I5")
    );
    // I6: a transport at the deadline, or without a grant.
    let late = LogEvent::Transport {
        artifact: a,
        attempt: 1,
        at: at + H,
    };
    assert!(
        check(&[grant(1, Some(l)), late], &none)
            .unwrap_err()
            .starts_with("I6")
    );
    assert!(check(&[transport], &none).unwrap_err().starts_with("I6"));
}
