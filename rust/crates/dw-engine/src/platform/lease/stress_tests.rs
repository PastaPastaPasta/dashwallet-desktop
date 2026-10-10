//! The stress harness and its checker (E0-04 §12 "Stress"), on a fake
//! library: the table and the fence log every J step and every transport
//! call in J order; the checker replays the log against I1–I6.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use tokio::time::Instant;

use super::fence::{HandOff, JournalBackend};
use super::lock::Scope;
use super::table::LeaseTable;
use super::tests::{FakeJournal, H, W, W2, begin, core, status, table, transition};
use super::{
    AdmitRequest, ArtifactId, ArtifactKind, DispatchScope, Lease, LeaseError, LeaseId, Origin,
    Outcome, UnleasedKind, Verdict,
};
use crate::WalletId;
use crate::platform::flows::RevokeCause;

/// How an attempt was granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GrantKind {
    /// A leased First: the lease's authority, its charge taken.
    First,
    /// An unleased First (a library rebroadcast).
    Unleased,
    /// A Resend: a copy of an admitted, marked or durable artifact.
    Resend,
}

/// One J step of the log.
#[derive(Debug, Clone)]
pub(crate) enum LogEvent {
    /// A lease was inserted.
    Begin {
        lease: LeaseId,
        wallet: WalletId,
    },
    /// A lease was ended (`end`, or its last owning handle dropped).
    End {
        lease: LeaseId,
    },
    /// An epoch change other than a lock: every live lease needs a grant,
    /// and still admits Firsts (§4.3).
    EpochChanged,
    /// A freeze (`wallet`: a removal's or close's, else a lock's): the
    /// leases it revoked and the permits in flight.
    Freeze {
        lock: u64,
        wallet: Option<WalletId>,
        at: Instant,
        revoked: Vec<LeaseId>,
        in_flight: Vec<(u64, Instant)>,
    },
    /// The drain of freeze `lock` ended (its barrier share released).
    LockReturned {
        lock: u64,
        at: Instant,
    },
    /// A permit or guard granted; `charge`: what a First's entry holds.
    Grant {
        attempt: u64,
        lease: Option<LeaseId>,
        artifact: ArtifactId,
        deadline: Option<Instant>,
        kind: GrantKind,
        charge: u64,
    },
    /// A step marker's write began (`Committing`).
    Marking {
        artifact: ArtifactId,
        step: String,
    },
    /// A `Dispatching` record (`step: None`) or a step marker is durable.
    Durable {
        artifact: ArtifactId,
        step: Option<String>,
    },
    /// A row-less artifact settled definitely unsent: its `NotSent` row
    /// durable, `refund` returned to its lease.
    Resolved {
        artifact: ArtifactId,
        refund: u64,
    },
    /// A `NotSent` row's write began: from here on it may have committed,
    /// superseding the artifact's markers on disk (DEC-154).
    Superseding {
        artifact: ArtifactId,
    },
    /// The artifact's `Sent` row is durable: its markers stand for good,
    /// also those a `NotSent` row superseded (DEC-154).
    Evidence {
        artifact: ArtifactId,
    },
    /// The fake library handed the artifact off (bytes may be out);
    /// `step`: the scope's step of the copy that did.
    Transport {
        artifact: ArtifactId,
        attempt: u64,
        at: Instant,
        step: Option<String>,
    },
    Refused {
        artifact: ArtifactId,
        cleanup: bool,
    },
    /// An attempt ended with `outcome`.
    Finish {
        attempt: u64,
        outcome: Outcome,
    },
    /// A `register` journal write began.
    Registering {
        wallet: WalletId,
        artifact: ArtifactId,
    },
    /// That write ended.
    Registered {
        wallet: WalletId,
        artifact: ArtifactId,
    },
    /// A removal's erase of the wallet's rows began or ended.
    Erase {
        wallet: WalletId,
        done: bool,
    },
    /// That erase compacted the artifact's rows, and the engine forgot it
    /// (DEC-160).
    Erased {
        wallet: WalletId,
        artifact: ArtifactId,
    },
}

/// Builds the stress must catch (§10.4, review P2a r1 F7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mutation {
    /// The lease is checked in one J step, the permit granted in another.
    CheckThenAct,
    /// Leases are revoked when the drain ends, not at the freeze.
    RevokeInDrain,
    /// A registered artifact's transport runs before its record is durable.
    TransportFirst,
    /// The drain returns with permits unexpired and running.
    EarlyReturn,
    /// F1: a copy goes out whatever the state of its artifact's markers.
    CopyBeforeMarker,
    /// F2: a definite resolution settles in memory and keeps its markers,
    /// which a later copy then reads as a hand-off.
    MarkerOutlives,
    /// F3: the hand-off ignores the permit's deadline.
    LateEntry,
    /// F4: a duplicate `register` writes too.
    DuplicateRegister,
    /// Sol r2 R2-F1: a copy does not wait for its artifact's `Sent` row.
    SkipEvidence,
    /// DEC-154 (3): a failed `NotSent` write is taken as uncommitted, so
    /// copies read the markers it may have superseded as standing.
    FailedUncommitted,
    /// Sol r2 R2-F1: a sighting trusts memory and appends no `Sent` row.
    NoSentRow,
    /// Review r2 H1: a superseded marker comes back durable whatever its
    /// write did. Its trace (a failed own-step write, a settlement, a
    /// sighting, a copy of that step) is too rare for the stress; the
    /// `r2_h1` regression catches it.
    StashDurable,
    /// Sol r3 R3-F1: a removal deletes all of the wallet's `step_log` rows,
    /// as before DEC-160.
    EraseAll,
    /// DEC-160: the removal's erase neither excludes the wallet's copies
    /// nor joins its running writes. A `Sent` row racing the delete is too
    /// rare for the stress; the `r3_f1` regressions catch it.
    NoEraseWindow,
}

impl Mutation {
    /// `DW_LEASE_MUTATION=<variant>` starts every table of a test build
    /// with that mutation: a fix's regression must then fail (review P2a
    /// r1). Unset in normal runs.
    pub(crate) fn from_env() -> Option<Self> {
        let name = std::env::var("DW_LEASE_MUTATION").ok()?;
        [
            Self::CheckThenAct,
            Self::RevokeInDrain,
            Self::TransportFirst,
            Self::EarlyReturn,
            Self::CopyBeforeMarker,
            Self::MarkerOutlives,
            Self::LateEntry,
            Self::DuplicateRegister,
            Self::SkipEvidence,
            Self::FailedUncommitted,
            Self::NoSentRow,
            Self::StashDurable,
            Self::EraseAll,
            Self::NoEraseWindow,
        ]
        .into_iter()
        .find(|m| format!("{m:?}") == name)
    }
}

/// The slack §12 allows a lock's return over its bound.
const SLACK: Duration = Duration::from_millis(250);

/// Replays `log` against design §9.1's invariants, by their labels, and
/// the obligations they rest on:
/// - I1: a registered artifact is handed off only after its durable
///   `Dispatching` record;
/// - I2: every leased First commits under a live lease: not ended, and
///   before every freeze of its wallet after it began (an epoch change
///   leaves it `NeedsGrant`, which still admits Firsts, §4.3);
/// - I3: no registered artifact is both durable and cleaned up;
/// - I4: no refused artifact is handed off before its refusal, and none
///   whose refusal cleaned it up is ever handed off;
/// - I5: a lock returns only once every permit of its freeze finished or
///   expired; H3: and by `max(freeze + H, its last deadline)` + 250 ms;
/// - I6: each artifact is cleaned up at most once;
/// - I10: a definite resolution comes only once every attempt since the
///   last one finished `NotSent`, and refunds exactly what they charged;
/// - I11: a copy is handed off only when every marker its artifact had
///   when the copy was admitted, and (row-less) its own step's, is
///   durable. A marker begun later is a later copy's obligation: the
///   earlier one may already be out;
/// - I16: after a definite resolution, an artifact goes out again only
///   through a new First, never a Resend of its old evidence;
/// - L5: only a live attempt hands off, its own artifact, before its
///   deadline;
/// - DEC-134: one `register` write per artifact at a time, none running
///   when a removal's erase begins, and none begun during it.
pub(crate) fn check(log: &[LogEvent]) -> Result<(), String> {
    struct Lease {
        began: usize,
        wallet: WalletId,
        ended: bool,
    }
    let mut leases: HashMap<LeaseId, Lease> = HashMap::new();
    let mut freezes: Vec<(usize, Option<WalletId>)> = Vec::new();
    let mut revoked: HashSet<LeaseId> = HashSet::new();
    let mut locks: HashMap<u64, (Instant, Vec<(u64, Instant)>)> = HashMap::new();
    // attempt → (artifact, deadline); finished ones move to `finished`.
    let mut live: HashMap<u64, (ArtifactId, Option<Instant>)> = HashMap::new();
    // attempt → the markers of its artifact when it was admitted.
    let mut owed: HashMap<u64, Vec<String>> = HashMap::new();
    let mut finished: HashMap<u64, Outcome> = HashMap::new();
    // Per artifact since its last definite resolution: its attempts, its
    // outstanding charge, its markers (durable or not).
    let mut since: HashMap<ArtifactId, Vec<u64>> = HashMap::new();
    let mut charged: HashMap<ArtifactId, u64> = HashMap::new();
    let mut markers: HashMap<ArtifactId, HashMap<String, bool>> = HashMap::new();
    // DEC-154: markers a `NotSent` row superseded, and artifacts whose
    // `Sent` row is durable.
    let mut superseded: HashMap<ArtifactId, Vec<String>> = HashMap::new();
    let mut evidence: HashSet<ArtifactId> = HashSet::new();
    // Markers whose write landed at some point: only those a `Sent` row
    // makes stand (review r2 H1, M1).
    let mut written: HashMap<ArtifactId, HashSet<String>> = HashMap::new();
    let mut resolved: HashSet<ArtifactId> = HashSet::new();
    let mut registered: HashSet<ArtifactId> = HashSet::new();
    let mut recorded: HashSet<ArtifactId> = HashSet::new();
    let mut sent: HashSet<ArtifactId> = HashSet::new();
    let mut cleaned: HashSet<ArtifactId> = HashSet::new();
    let mut writing: HashMap<(WalletId, ArtifactId), u32> = HashMap::new();
    let mut erasing: HashSet<WalletId> = HashSet::new();
    // When each artifact's `Sent` row was last logged durable, and each
    // wallet's erase last began.
    let mut evidence_at: HashMap<ArtifactId, usize> = HashMap::new();
    let mut erase_began: HashMap<WalletId, usize> = HashMap::new();
    for (n, e) in log.iter().enumerate() {
        match e {
            LogEvent::Begin { lease, wallet } => {
                leases.insert(
                    *lease,
                    Lease {
                        began: n,
                        wallet: *wallet,
                        ended: false,
                    },
                );
            }
            LogEvent::End { lease } => {
                if let Some(l) = leases.get_mut(lease) {
                    l.ended = true;
                }
            }
            LogEvent::EpochChanged => {}
            LogEvent::Freeze {
                lock,
                wallet,
                at,
                revoked: r,
                in_flight,
            } => {
                freezes.push((n, *wallet));
                revoked.extend(r.iter().copied());
                locks.insert(*lock, (*at, in_flight.clone()));
            }
            LogEvent::LockReturned { lock, at } => {
                let Some((frozen, in_flight)) = locks.get(lock) else {
                    return Err(format!("I5: event {n}: lock {lock} returned, never frozen"));
                };
                if let Some((attempt, d)) = in_flight
                    .iter()
                    .find(|(a, d)| live.contains_key(a) && *d > *at)
                {
                    return Err(format!(
                        "I5: event {n}: lock {lock} returned with attempt {attempt} running, \
                         {:?} before its deadline",
                        *d - *at
                    ));
                }
                let last = in_flight.iter().map(|(_, d)| *d).max();
                let bound = last.map_or(*frozen + H, |d| d.max(*frozen + H)) + SLACK;
                if *at > bound {
                    return Err(format!("H3: lock {lock} returned {:?} late", *at - bound));
                }
            }
            LogEvent::Grant {
                attempt,
                lease,
                artifact,
                deadline,
                kind,
                charge,
            } => {
                match kind {
                    GrantKind::First => {
                        let Some((id, l)) = lease.and_then(|id| Some((id, leases.get(&id)?)))
                        else {
                            return Err(format!("I2: event {n}: {artifact} committed, no lease"));
                        };
                        let frozen = revoked.contains(&id)
                            || freezes
                                .iter()
                                .any(|(f, w)| *f > l.began && w.is_none_or(|w| l.wallet == w));
                        if l.ended || frozen {
                            return Err(format!(
                                "I2: event {n}: {artifact} committed under a lease that was not \
                                 live (ended {}, frozen {frozen})",
                                l.ended
                            ));
                        }
                        resolved.remove(artifact);
                    }
                    GrantKind::Unleased => {
                        resolved.remove(artifact);
                    }
                    GrantKind::Resend => {
                        if resolved.contains(artifact) {
                            return Err(format!(
                                "I16: event {n}: {artifact} resent after its definite \
                                 resolution, with no new charge"
                            ));
                        }
                    }
                }
                *charged.entry(*artifact).or_default() += charge;
                since.entry(*artifact).or_default().push(*attempt);
                owed.insert(
                    *attempt,
                    markers
                        .get(artifact)
                        .map(|m| m.keys().cloned().collect())
                        .unwrap_or_default(),
                );
                live.insert(*attempt, (*artifact, *deadline));
            }
            LogEvent::Marking { artifact, step } => {
                markers
                    .entry(*artifact)
                    .or_default()
                    .insert(step.clone(), false);
            }
            LogEvent::Durable {
                artifact,
                step: None,
            } => {
                if cleaned.contains(artifact) {
                    return Err(format!(
                        "I3: event {n}: {artifact} recorded durable after its cleanup"
                    ));
                }
                recorded.insert(*artifact);
            }
            LogEvent::Durable {
                artifact,
                step: Some(s),
            } => {
                let Some(m) = markers.get_mut(artifact).and_then(|m| m.get_mut(s)) else {
                    return Err(format!(
                        "I11: event {n}: {artifact}'s marker for {s} durable, never begun"
                    ));
                };
                *m = true;
                written.entry(*artifact).or_default().insert(s.clone());
            }
            LogEvent::Resolved { artifact, refund } => {
                let attempts = since.remove(artifact).unwrap_or_default();
                if let Some(a) = attempts
                    .iter()
                    .find(|a| finished.get(a) != Some(&Outcome::NotSent))
                {
                    return Err(format!(
                        "I10: event {n}: {artifact} resolved unsent while attempt {a} is \
                         {:?}",
                        finished
                            .get(a)
                            .map_or("running".to_owned(), |o| format!("{o:?}"))
                    ));
                }
                let owed = charged.remove(artifact).unwrap_or(0);
                if *refund != owed {
                    return Err(format!(
                        "I10: event {n}: {artifact} refunded {refund}, charged {owed}"
                    ));
                }
                if let Some(m) = markers.remove(artifact) {
                    superseded
                        .entry(*artifact)
                        .or_default()
                        .extend(m.into_keys());
                }
                resolved.insert(*artifact);
            }
            LogEvent::Superseding { artifact } => {
                if !evidence.contains(artifact)
                    && let Some(m) = markers.get_mut(artifact)
                {
                    m.values_mut().for_each(|d| *d = false);
                }
            }
            LogEvent::Evidence { artifact } => {
                evidence.insert(*artifact);
                evidence_at.insert(*artifact, n);
                let m = markers.entry(*artifact).or_default();
                for s in superseded.remove(artifact).unwrap_or_default() {
                    m.entry(s).or_insert(false);
                }
                let on_disk = written.get(artifact);
                for (s, d) in m.iter_mut() {
                    *d = on_disk.is_some_and(|w| w.contains(s));
                }
                // A seen-sent artifact may be resent with no new charge.
                resolved.remove(artifact);
            }
            LogEvent::Transport {
                artifact,
                attempt,
                at,
                step,
            } => {
                let Some((granted, deadline)) = live.get(attempt) else {
                    return Err(format!(
                        "L5: event {n}: transport by attempt {attempt}, which {}",
                        if finished.contains_key(attempt) {
                            "finished"
                        } else {
                            "was never granted"
                        }
                    ));
                };
                if granted != artifact {
                    return Err(format!(
                        "L5: event {n}: attempt {attempt} is not {artifact}'s"
                    ));
                }
                if deadline.is_some_and(|d| *at >= d) {
                    return Err(format!(
                        "L5: event {n}: {artifact} handed off after its deadline"
                    ));
                }
                if registered.contains(artifact) && !recorded.contains(artifact) {
                    return Err(format!(
                        "I1: event {n}: {artifact} handed off before its record"
                    ));
                }
                let marks = markers.get(artifact);
                let durable = |s: &String| marks.is_some_and(|m| m.get(s) == Some(&true));
                if let Some(s) = owed
                    .get(attempt)
                    .and_then(|o| o.iter().find(|s| !durable(s)))
                {
                    return Err(format!(
                        "I11: event {n}: {artifact} handed off before its marker for {s} \
                         is durable"
                    ));
                }
                // A registered artifact's evidence is its record (I1):
                // markers are for row-less steps (§7.6).
                if let Some(s) = step
                    && !registered.contains(artifact)
                    && !durable(s)
                {
                    return Err(format!(
                        "I11: event {n}: {artifact} handed off for {s} with no durable \
                         marker of it"
                    ));
                }
                if cleaned.contains(artifact) {
                    return Err(format!(
                        "I4: event {n}: {artifact} handed off after its cleanup"
                    ));
                }
                sent.insert(*artifact);
            }
            LogEvent::Refused { artifact, cleanup } => {
                if sent.contains(artifact) {
                    return Err(format!(
                        "I4: event {n}: {artifact} refused after a hand-off"
                    ));
                }
                if *cleanup {
                    if registered.contains(artifact) && recorded.contains(artifact) {
                        return Err(format!(
                            "I3: event {n}: {artifact} cleaned up after its durable record"
                        ));
                    }
                    if !cleaned.insert(*artifact) {
                        return Err(format!("I6: event {n}: {artifact} cleaned up twice"));
                    }
                }
            }
            LogEvent::Finish { attempt, outcome } => {
                if live.remove(attempt).is_none() {
                    return Err(format!(
                        "L5: event {n}: attempt {attempt} finished, not running"
                    ));
                }
                finished.insert(*attempt, *outcome);
            }
            LogEvent::Registering { wallet, artifact } => {
                if erasing.contains(wallet) {
                    return Err(format!(
                        "DEC-134: event {n}: {artifact}'s register began during its \
                         wallet's erase"
                    ));
                }
                let w = writing.entry((*wallet, *artifact)).or_default();
                if *w > 0 {
                    return Err(format!(
                        "DEC-134: event {n}: a second register write of {artifact}"
                    ));
                }
                *w += 1;
                registered.insert(*artifact);
            }
            LogEvent::Registered { wallet, artifact } => {
                let w = writing.entry((*wallet, *artifact)).or_default();
                *w = w.saturating_sub(1);
            }
            LogEvent::Erase {
                wallet,
                done: false,
            } => {
                if let Some(((_, a), _)) = writing.iter().find(|((w, _), k)| w == wallet && **k > 0)
                {
                    return Err(format!(
                        "DEC-134: event {n}: the erase began with {a}'s register running"
                    ));
                }
                erasing.insert(*wallet);
                erase_began.insert(*wallet, n);
            }
            LogEvent::Erase { wallet, done: true } => {
                erasing.remove(wallet);
            }
            LogEvent::Erased { wallet, artifact } => {
                if !erasing.contains(wallet) {
                    return Err(format!(
                        "DEC-160: event {n}: {artifact} erased outside its wallet's erase"
                    ));
                }
                if evidence_at
                    .get(artifact)
                    .is_some_and(|e| erase_began.get(wallet).is_some_and(|b| e < b))
                {
                    return Err(format!(
                        "I11: event {n}: {artifact} erased with its Sent row durable"
                    ));
                }
                // Otherwise a seen-sent artifact's markers stand only on a
                // `Sent` row landing during the erase, which only the disk
                // orders.
                if !evidence.contains(artifact)
                    && let Some((s, _)) = markers
                        .get(artifact)
                        .and_then(|m| m.iter().find(|(_, durable)| **durable))
                {
                    return Err(format!(
                        "I11: event {n}: {artifact} erased with its marker for {s} standing"
                    ));
                }
                // The engine starts it afresh, as after a restart's sweep.
                markers.remove(artifact);
                superseded.remove(artifact);
                evidence.remove(artifact);
                evidence_at.remove(artifact);
                written.remove(artifact);
                resolved.remove(artifact);
                charged.remove(artifact);
                since.remove(artifact);
            }
        }
    }
    Ok(())
}

tokio::task_local! {
    /// Each task's own random stream, so a seed fixes every task's draws
    /// whatever the interleaving (the journal's blocking writes still run
    /// in real time).
    static RNG: RefCell<StdRng>;
}

/// The shared state of one stress run.
struct Run {
    t: Arc<LeaseTable>,
    j: Arc<FakeJournal>,
    stop: AtomicBool,
    next: AtomicU32,
    /// Every artifact a flow admitted, with its kind and step, for the
    /// resume tasks.
    done: Mutex<Vec<(ArtifactId, ArtifactKind, Option<String>)>>,
    registered: Mutex<HashSet<ArtifactId>>,
    /// Owning handles kept past their flow, until a lock or epoch change.
    kept: Mutex<Vec<Lease>>,
    salt: u8,
    /// The first step copy handed off with its marker not standing in the
    /// journal itself (review r2 M1): an oracle that does not depend on
    /// the order of the logged events.
    disk: Mutex<Option<String>>,
}

impl Run {
    fn below(&self, n: u64) -> u64 {
        RNG.with(|r| r.borrow_mut().gen_range(0..n.max(1)))
    }

    fn pause(&self, max_ms: u64) -> tokio::time::Sleep {
        tokio::time::sleep(Duration::from_millis(self.below(max_ms)))
    }

    /// Spawns `f` with its own random stream, drawn from this task's.
    fn spawn<F: Future + Send + 'static>(&self, f: F) -> tokio::task::JoinHandle<F::Output>
    where
        F::Output: Send + 'static,
    {
        let seed = RNG.with(|r| r.borrow_mut().r#gen::<u64>());
        tokio::spawn(RNG.scope(RefCell::new(StdRng::seed_from_u64(seed)), f))
    }

    /// A fresh artifact id: the run's salt and a counter.
    fn artifact(&self) -> ArtifactId {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        let mut a = ArtifactId([self.salt; 32]);
        a.0[..4].copy_from_slice(&n.to_le_bytes());
        a
    }

    /// The fake library's hand-off under a verdict, through the engine's
    /// deadline discipline (`LeaseTable::hand_off`, which `dispatch`
    /// uses): a random wait before the library (it may outlive the
    /// permit), then the library, which may stall past the deadline, with
    /// a random outcome. Only bytes that may have gone out are logged.
    async fn hand_off(&self, a: ArtifactId, step: Option<String>, v: Verdict) {
        let (id, deadline) = match &v {
            Verdict::First(p) => (p.id(), Some(p.deadline())),
            Verdict::Resend(g) | Verdict::FirstUnleased(g) => (g.id(), None),
            Verdict::Refused { .. } | Verdict::Deferred => return,
        };
        // No wait at all a quarter of the time: a hand-off right after
        // `admit` returned.
        let before = match self.below(8) {
            0 | 1 => 0,
            2 => self.below(2 * H.as_millis() as u64),
            _ => self.below(50),
        };
        let within = match self.below(8) {
            0 => self.below(2 * H.as_millis() as u64),
            _ => self.below(50),
        };
        let outcome = match self.below(4) {
            0 => Outcome::NotSent,
            1 => Outcome::MaybeSent,
            _ => Outcome::Sent,
        };
        let ended = self
            .t
            .hand_off(
                deadline,
                async {
                    tokio::time::sleep(Duration::from_millis(before)).await;
                    Ok::<_, ()>(())
                },
                |()| async move {
                    if outcome != Outcome::NotSent {
                        // A registered artifact's record fences it instead.
                        if let Some(s) = &step
                            && !self.registered.lock().unwrap().contains(&a)
                            && !self.j.stands(&W, &a, s)
                        {
                            self.disk.lock().unwrap().get_or_insert(format!(
                                "I11: {a} handed off for {s} with its marker not standing \
                                 in the journal"
                            ));
                        }
                        self.t.log_transport(a, id, step);
                    }
                    tokio::time::sleep(Duration::from_millis(within)).await;
                    outcome
                },
            )
            .await
            .unwrap();
        let outcome = match ended {
            HandOff::Done(o) => o,
            HandOff::NotEntered => Outcome::NotSent,
            HandOff::Cut => Outcome::MaybeSent,
        };
        // Sol r2 R2-F1: the artifact seen sent while its definite
        // resolution is written, so the finish runs on a blocking thread.
        let sight = outcome == Outcome::NotSent && self.below(3) == 0;
        if sight {
            let t = Arc::clone(&self.t);
            self.spawn(async move {
                tokio::task::yield_now().await;
                t.note_seen(W, a);
            });
        } else if outcome == Outcome::NotSent && self.below(4) == 0 {
            // Review r2 H1: seen sent after it settled, when its superseded
            // markers come back.
            let (t, wait) = (Arc::clone(&self.t), self.below(100));
            self.spawn(async move {
                tokio::time::sleep(Duration::from_millis(wait)).await;
                t.note_seen(W, a);
            });
        }
        let finish = move || match v {
            Verdict::First(p) => {
                p.finish(outcome);
            }
            Verdict::Resend(g) | Verdict::FirstUnleased(g) => {
                g.finish(outcome);
            }
            Verdict::Refused { .. } | Verdict::Deferred => {}
        };
        if sight {
            tokio::task::spawn_blocking(finish).await.unwrap();
        } else {
            finish();
        }
    }

    /// A copy of `a` (kind `kind`) scoped by a random way: its own step, a
    /// step of its own, no step, or as an unleased rebroadcast; under a
    /// new lease unless unleased. Admitted, then handed off.
    async fn copy(self: Arc<Self>, a: ArtifactId, kind: ArtifactKind, step: Option<String>) {
        let tracked = self.registered.lock().unwrap().contains(&a);
        let req = |scope: Option<DispatchScope>| AdmitRequest {
            wallet: W,
            artifact: a,
            kind,
            tracked_row: tracked,
            scope,
        };
        let step = match self.below(4) {
            0 | 1 => step,
            2 => Some(format!("copy/{}", self.below(4))),
            _ => None,
        };
        let (v, step) = if self.below(4) == 0 {
            let t = Arc::clone(&self.t);
            let v = DispatchScope::unleased(W, UnleasedKind::Rebroadcast, async move {
                t.admit(req(DispatchScope::current())).await
            })
            .await;
            (v, None)
        } else {
            let Ok(lease) = begin(&self.t, W, 1_000_000, 1_000_000, 0).await else {
                return;
            };
            let scope = DispatchScope {
                wallet: W,
                origin: Origin::Lease(lease.id()),
                step: step.clone(),
            };
            (self.t.admit(req(Some(scope))).await, step)
        };
        self.hand_off(a, step, v).await;
    }

    /// One of the 16 flows: a lease, one artifact of a random kind (a Core
    /// send, a state transition with or without a resumable step, a
    /// registered asset lock), a random pause before `admit`, the
    /// hand-off, sometimes a concurrent copy and a later repeat.
    async fn flow(self: Arc<Self>) {
        while !self.stop.load(Ordering::SeqCst) {
            let lease = match begin(&self.t, W, 1_000_000, 1_000_000, 1_000_000).await {
                Ok(l) => l,
                Err(LeaseError::Locked) => continue,
                Err(e) => panic!("begin: {e:?}"),
            };
            let a = self.artifact();
            let mut step = None;
            let mut req = match self.below(4) {
                0 => {
                    let Ok(charge) = lease.charge_spend(10) else {
                        continue;
                    };
                    self.t.bind_spend(lease.id(), W, a, charge);
                    core(&lease, a)
                }
                1 => transition(&lease, a, 1),
                2 => {
                    step = Some(format!("step/{}", self.below(6)));
                    transition(&lease, a, 1)
                }
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
            if let Some(scope) = req.scope.as_mut() {
                scope.step = step.clone();
            }
            self.pause(100).await;
            let kind = req.kind;
            // A copy racing the First, often inside its marker write.
            let racing = (self.below(3) == 0).then(|| {
                let run = Arc::clone(&self);
                let step = step.clone();
                self.spawn(async move {
                    if run.below(2) == 0 {
                        tokio::task::yield_now().await;
                    } else {
                        run.pause(5).await;
                    }
                    run.copy(a, kind, step).await
                })
            });
            let v = self.t.admit(req.clone()).await;
            self.done.lock().unwrap().push((a, kind, step.clone()));
            self.hand_off(a, step.clone(), v).await;
            if let Some(racing) = racing {
                racing.await.unwrap();
            }
            if self.below(3) == 0 {
                self.pause(100).await;
                let v = self.t.admit(req).await;
                self.hand_off(a, step, v).await;
            }
            match self.below(3) {
                0 => lease.end(),
                1 => self.kept.lock().unwrap().push(lease),
                // Its last owning handle drops here, which ends it (F6).
                _ => {}
            }
        }
    }

    /// One of the 4 resume tasks: an earlier artifact admitted again, as
    /// any copy.
    async fn resume(self: Arc<Self>) {
        while !self.stop.load(Ordering::SeqCst) {
            self.pause(150).await;
            let pick = {
                let done = self.done.lock().unwrap();
                if done.is_empty() {
                    continue;
                }
                done[self.below(done.len() as u64) as usize].clone()
            };
            let (a, kind, step) = pick;
            Arc::clone(&self).copy(a, kind, step).await;
        }
    }

    /// The registrar: registrations of a second wallet, often two of the
    /// same artifact at once (one concurrent duplicate, one from another
    /// lease), racing that wallet's removals.
    async fn registrar(self: Arc<Self>) {
        while !self.stop.load(Ordering::SeqCst) {
            self.pause(40).await;
            let Ok(lease) = begin(&self.t, W2, 1_000_000, 0, 0).await else {
                continue;
            };
            let a = self.artifact();
            let mut writes = Vec::new();
            for _ in 0..1 + self.below(3) {
                let other = self.below(4) == 0;
                let lease = if other {
                    match begin(&self.t, W2, 1_000_000, 0, 0).await {
                        Ok(l) => l,
                        Err(_) => continue,
                    }
                } else {
                    lease.clone()
                };
                let t = Arc::clone(&self.t);
                writes.push(self.spawn(async move {
                    lease
                        .scope(async move { t.register(W2, a, 10, Vec::new()).await })
                        .await
                }));
            }
            for w in writes {
                let _ = w.await.unwrap();
            }
        }
    }

    /// The lock driver: `cycles` lock (async or sync), epoch change,
    /// removal of either wallet, and journal fault cycles at random
    /// intervals.
    async fn locks(self: Arc<Self>, cycles: usize) {
        for _ in 0..cycles {
            self.pause(120).await;
            self.j
                .dispatch_fault
                .store([0, 0, 0, 1, 2][self.below(5) as usize], Ordering::SeqCst);
            self.j.fail_step.store(self.below(6) == 0, Ordering::SeqCst);
            self.j
                .fail_resolve
                .store(self.below(6) == 0, Ordering::SeqCst);
            self.j
                .resolve_durable_err
                .store(self.below(6) == 0, Ordering::SeqCst);
            self.j.fail_sent.store(self.below(6) == 0, Ordering::SeqCst);
            self.j
                .sent_durable_err
                .store(self.below(6) == 0, Ordering::SeqCst);
            match self.below(5) {
                0 => {
                    self.t.lock_sync(RevokeCause::Lock, status);
                }
                1 => {
                    // An unlock or scope change: a new vault epoch.
                    let epoch = self.t.inspect(|i| i.observed_epoch) + 1;
                    self.t.epoch_changed(epoch);
                    self.kept.lock().unwrap().clear();
                }
                2 => {
                    // Sol r3 R3-F1: the flows' own wallet too, whose
                    // row-less step copies run across its removal.
                    let w = if self.below(2) == 0 { W } else { W2 };
                    let mut removal =
                        self.t
                            .freeze(RevokeCause::WalletRemoved, Scope::Wallet(w), false);
                    removal.drained().await;
                    self.t.erase_wallet_rows(w).await;
                    if w == W {
                        self.kept.lock().unwrap().clear();
                    }
                }
                _ => {
                    self.t.lock(RevokeCause::Lock, status).await;
                    self.kept.lock().unwrap().clear();
                }
            }
        }
        self.j.dispatch_fault.store(0, Ordering::SeqCst);
        self.j.fail_step.store(false, Ordering::SeqCst);
        self.j.fail_resolve.store(false, Ordering::SeqCst);
        self.j.resolve_durable_err.store(false, Ordering::SeqCst);
        self.j.fail_sent.store(false, Ordering::SeqCst);
        self.j.sent_durable_err.store(false, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// One run: 16 flows, 4 resume tasks, the registrar, `cycles` lock
/// cycles; returns the checker's verdict.
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
        stop: AtomicBool::new(false),
        next: AtomicU32::new(0),
        done: Mutex::new(Vec::new()),
        registered: Mutex::new(HashSet::new()),
        kept: Mutex::new(Vec::new()),
        salt: seed as u8,
        disk: Mutex::new(None),
    });
    let rng = |n: u64| RefCell::new(StdRng::seed_from_u64(seed << 8 | n));
    let mut tasks = Vec::new();
    for n in 0..16 {
        tasks.push(tokio::spawn(RNG.scope(rng(n), Arc::clone(&run).flow())));
    }
    for n in 16..20 {
        tasks.push(tokio::spawn(RNG.scope(rng(n), Arc::clone(&run).resume())));
    }
    tasks.push(tokio::spawn(
        RNG.scope(rng(20), Arc::clone(&run).registrar()),
    ));
    tasks.push(tokio::spawn(
        RNG.scope(rng(21), Arc::clone(&run).locks(cycles)),
    ));
    for task in tasks {
        task.await.unwrap();
    }
    run.kept.lock().unwrap().clear();
    // Let every spawned drain finish.
    tokio::time::sleep(2 * H).await;
    let log = t.inspect(|i| std::mem::take(&mut i.log));
    let count = |f: fn(&LogEvent) -> bool| log.iter().filter(|e| f(e)).count();
    let coverage = [
        (
            "hand-offs",
            count(|e| matches!(e, LogEvent::Transport { .. })),
            50,
        ),
        (
            "step hand-offs",
            count(|e| matches!(e, LogEvent::Transport { step: Some(_), .. })),
            10,
        ),
        (
            "resolutions",
            count(|e| matches!(e, LogEvent::Resolved { .. })),
            10,
        ),
        (
            "marked resolutions",
            {
                let mut marked = HashSet::new();
                log.iter()
                    .filter(|e| match e {
                        LogEvent::Marking { artifact, .. } => {
                            marked.insert(*artifact);
                            false
                        }
                        LogEvent::Resolved { artifact, .. } => marked.contains(artifact),
                        _ => false,
                    })
                    .count()
            },
            3,
        ),
        (
            "Sent rows over a NotSent write",
            {
                let mut superseding = HashSet::new();
                log.iter()
                    .filter(|e| match e {
                        LogEvent::Superseding { artifact } => {
                            superseding.insert(*artifact);
                            false
                        }
                        LogEvent::Evidence { artifact } => superseding.contains(artifact),
                        _ => false,
                    })
                    .count()
            },
            5,
        ),
        (
            "Sent rows after a settlement",
            {
                let mut settled = HashSet::new();
                log.iter()
                    .filter(|e| match e {
                        LogEvent::Resolved { artifact, .. } => {
                            settled.insert(*artifact);
                            false
                        }
                        LogEvent::Evidence { artifact } => settled.contains(artifact),
                        _ => false,
                    })
                    .count()
            },
            3,
        ),
        (
            "registers",
            count(|e| matches!(e, LogEvent::Registering { .. })),
            20,
        ),
        (
            "erases",
            count(|e| matches!(e, LogEvent::Erase { done: true, .. })),
            3,
        ),
        (
            "erases of the flows' wallet",
            count(|e| matches!(e, LogEvent::Erase { wallet, done: true } if *wallet == W)),
            1,
        ),
        (
            "epoch changes",
            count(|e| matches!(e, LogEvent::EpochChanged)),
            3,
        ),
    ];
    // A mutation may stop the run doing some of it; the checker decides.
    if mutation.is_none() {
        for (what, seen, floor) in coverage {
            assert!(seen >= floor, "seed {seed}: {seen} {what}, under {floor}");
        }
        eprintln!(
            "seed {seed}: {}",
            coverage
                .map(|(what, seen, _)| format!("{seen} {what}"))
                .join(", ")
        );
    }
    check(&log)?;
    if let Some(e) = run.disk.lock().unwrap().take() {
        return Err(e);
    }
    match run.j.bare_final_row() {
        Some(a) => Err(format!(
            "I11: {a}'s Sent row is on disk with its markers erased"
        )),
        None => Ok(()),
    }
}

/// The seeds to run: `DW_LEASE_STRESS_SEEDS` of them (3 by default; the
/// gate runs 10).
fn seeds() -> std::ops::RangeInclusive<u64> {
    let n = std::env::var("DW_LEASE_STRESS_SEEDS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(3);
    1..=n
}

#[tokio::test(start_paused = true)]
async fn the_stress_upholds_the_invariants() {
    for seed in seeds() {
        stress(seed, None, 300)
            .await
            .unwrap_or_else(|e| panic!("seed {seed}: {e}"));
    }
}

/// Each mutation build must fail the checker (§12 "Mutation runs"),
/// breaking the invariant it targets, not just any.
#[tokio::test(start_paused = true)]
async fn the_stress_catches_every_mutation() {
    for (mutation, invariant) in [
        (Mutation::CheckThenAct, "I2"),
        (Mutation::RevokeInDrain, "I2"),
        (Mutation::TransportFirst, "I1"),
        (Mutation::EarlyReturn, "I5"),
        (Mutation::CopyBeforeMarker, "I11"),
        (Mutation::MarkerOutlives, "I16"),
        (Mutation::LateEntry, "L5"),
        (Mutation::DuplicateRegister, "DEC-134"),
        (Mutation::SkipEvidence, "I11"),
        (Mutation::FailedUncommitted, "I11"),
        (Mutation::NoSentRow, "I11"),
        (Mutation::EraseAll, "I11"),
    ] {
        let mut seen = Vec::new();
        for seed in 1..=5 {
            if let Err(e) = stress(seed, Some(mutation), 300).await {
                let hit = e.starts_with(&format!("{invariant}:"));
                seen.push(e);
                if hit {
                    break;
                }
            }
        }
        let e = seen
            .iter()
            .find(|e| e.starts_with(&format!("{invariant}:")))
            .unwrap_or_else(|| panic!("{mutation:?} did not break {invariant}: {seen:?}"));
        eprintln!("{mutation:?}: {e}");
    }
}

/// A bad trace per rule, Sol's two blind spots among them, and the good
/// traces next to them.
#[test]
fn the_checker_flags_each_invariant() {
    let at = Instant::now();
    let l = [7u8; 16];
    let a = ArtifactId([1; 32]);
    let begin = LogEvent::Begin {
        lease: l,
        wallet: W,
    };
    let grant = |attempt, lease: Option<LeaseId>, kind, charge| LogEvent::Grant {
        attempt,
        lease,
        artifact: a,
        deadline: lease.map(|_| at + H),
        kind,
        charge,
    };
    let first = |attempt| grant(attempt, Some(l), GrantKind::First, 0);
    let resend = |attempt| grant(attempt, None, GrantKind::Resend, 0);
    let transport = |attempt, step: Option<&str>| LogEvent::Transport {
        artifact: a,
        attempt,
        at,
        step: step.map(str::to_owned),
    };
    let finish = |attempt, outcome| LogEvent::Finish { attempt, outcome };
    let freeze = |in_flight| LogEvent::Freeze {
        lock: 1,
        wallet: None,
        at,
        revoked: vec![l],
        in_flight,
    };
    let returned = |after| LogEvent::LockReturned {
        lock: 1,
        at: at + after,
    };
    let marking = |s: &str| LogEvent::Marking {
        artifact: a,
        step: s.into(),
    };
    let durable = |s: Option<&str>| LogEvent::Durable {
        artifact: a,
        step: s.map(str::to_owned),
    };
    let resolved = |refund| LogEvent::Resolved {
        artifact: a,
        refund,
    };
    let register = LogEvent::Registering {
        wallet: W2,
        artifact: a,
    };
    let registered = LogEvent::Registered {
        wallet: W2,
        artifact: a,
    };
    let erase = |done| LogEvent::Erase { wallet: W2, done };
    let refused = |cleanup| LogEvent::Refused {
        artifact: a,
        cleanup,
    };
    let fails = |log: &[LogEvent], label: &str| {
        let e = check(log).expect_err(label);
        assert!(e.starts_with(&format!("{label}:")), "{label}: {e}");
    };
    // I1: a registered artifact before its record.
    let reg = [
        begin.clone(),
        register.clone(),
        registered.clone(),
        first(1),
    ];
    fails(&[&reg[..], &[transport(1, None)]].concat(), "I1");
    check(&[&reg[..], &[durable(None), transport(1, None)]].concat()).unwrap();
    // I2: a First under a lease a freeze revoked, or that ended; one
    // after an epoch change (`NeedsGrant`) is fine.
    fails(&[begin.clone(), freeze(vec![]), first(1)], "I2");
    fails(&[begin.clone(), LogEvent::End { lease: l }, first(1)], "I2");
    check(&[begin.clone(), LogEvent::EpochChanged, first(1)]).unwrap();
    let other = LogEvent::Freeze {
        lock: 1,
        wallet: Some(W2),
        at,
        revoked: vec![],
        in_flight: vec![],
    };
    check(&[begin.clone(), other, first(1)]).unwrap();
    // I3: a durable registered artifact cleaned up.
    fails(&[&reg[..], &[durable(None), refused(true)]].concat(), "I3");
    // I4: a refusal after a hand-off; a hand-off after a cleanup.
    fails(
        &[begin.clone(), first(1), transport(1, None), refused(false)],
        "I4",
    );
    fails(
        &[begin.clone(), refused(true), first(1), transport(1, None)],
        "I4",
    );
    // I5 (Sol's first blind spot): a lock returning with an unexpired,
    // unfinished permit; finished or expired, it may.
    let held = [begin.clone(), first(1), freeze(vec![(1, at + H)])];
    fails(&[&held[..], &[returned(Duration::ZERO)]].concat(), "I5");
    check(
        &[
            &held[..],
            &[finish(1, Outcome::Sent), returned(Duration::ZERO)],
        ]
        .concat(),
    )
    .unwrap();
    check(&[&held[..], &[returned(H)]].concat()).unwrap();
    // H3: a late return.
    fails(
        &[
            freeze(vec![]),
            returned(H + SLACK + Duration::from_millis(1)),
        ],
        "H3",
    );
    // I6: two cleanups.
    fails(&[refused(true), refused(true)], "I6");
    // I11 under DEC-154 (Sol r2 R2-F1): a NotSent write that began may
    // have superseded the markers, so a copy needs the Sent row (or a new
    // marker) first; a Sent row also brings back markers a NotSent row
    // superseded.
    let superseding = LogEvent::Superseding { artifact: a };
    let evidence = LogEvent::Evidence { artifact: a };
    let marked = [
        begin.clone(),
        marking("s"),
        durable(Some("s")),
        first(1),
        transport(1, Some("s")),
        finish(1, Outcome::NotSent),
    ];
    fails(
        &[
            &marked[..],
            &[superseding.clone(), resend(2), transport(2, Some("s"))],
        ]
        .concat(),
        "I11",
    );
    check(
        &[
            &marked[..],
            &[
                superseding.clone(),
                evidence.clone(),
                resend(2),
                transport(2, Some("s")),
            ],
        ]
        .concat(),
    )
    .unwrap();
    let settled = [&marked[..], &[superseding.clone(), resolved(0)]].concat();
    fails(
        &[&settled[..], &[resend(2), transport(2, Some("s"))]].concat(),
        "I16",
    );
    check(
        &[
            &settled[..],
            &[evidence.clone(), resend(2), transport(2, Some("s"))],
        ]
        .concat(),
    )
    .unwrap();
    // I10: a resolution with an attempt running or possibly out, or
    // refunding other than it charged.
    let charged = [begin.clone(), grant(1, Some(l), GrantKind::First, 5)];
    fails(&[&charged[..], &[resolved(5)]].concat(), "I10");
    fails(
        &[&charged[..], &[finish(1, Outcome::MaybeSent), resolved(5)]].concat(),
        "I10",
    );
    fails(
        &[&charged[..], &[finish(1, Outcome::NotSent), resolved(10)]].concat(),
        "I10",
    );
    check(&[&charged[..], &[finish(1, Outcome::NotSent), resolved(5)]].concat()).unwrap();
    // I11: a copy before its artifact's markers, or its own, are durable.
    let marked = [begin.clone(), marking("s"), first(1), resend(2)];
    fails(&[&marked[..], &[transport(2, None)]].concat(), "I11");
    fails(&[&marked[..], &[transport(2, Some("t"))]].concat(), "I11");
    check(
        &[
            &marked[..],
            &[
                durable(Some("s")),
                transport(2, None),
                transport(1, Some("s")),
            ],
        ]
        .concat(),
    )
    .unwrap();
    // A marker begun after a copy's admission is not its obligation.
    check(&[begin.clone(), first(1), marking("s"), transport(1, None)]).unwrap();
    // A registered artifact's step scope needs no marker, only its record.
    check(&[&reg[..], &[durable(None), transport(1, Some("t"))]].concat()).unwrap();
    // I16: a Resend of old evidence after a definite resolution; a new
    // First is fine.
    let settled = [
        begin.clone(),
        marking("s"),
        durable(Some("s")),
        first(1),
        finish(1, Outcome::NotSent),
        resolved(0),
    ];
    fails(&[&settled[..], &[resend(2)]].concat(), "I16");
    check(&[&settled[..], &[first(2), resend(3)]].concat()).unwrap();
    // L5 (Sol's second blind spot): a transport by a finished guard; at
    // the deadline; without a grant; by another artifact's attempt.
    fails(
        &[resend(1), finish(1, Outcome::Sent), transport(1, None)],
        "L5",
    );
    fails(
        &[
            begin.clone(),
            first(1),
            LogEvent::Transport {
                artifact: a,
                attempt: 1,
                at: at + H,
                step: None,
            },
        ],
        "L5",
    );
    fails(&[transport(1, None)], "L5");
    fails(
        &[
            resend(1),
            LogEvent::Transport {
                artifact: ArtifactId([2; 32]),
                attempt: 1,
                at,
                step: None,
            },
        ],
        "L5",
    );
    // DEC-134: a second register write; an erase with one running; one
    // begun during the erase.
    fails(&[register.clone(), register.clone()], "DEC-134");
    fails(&[register.clone(), erase(false)], "DEC-134");
    fails(&[erase(false), register.clone()], "DEC-134");
    check(&[
        register.clone(),
        registered.clone(),
        erase(false),
        erase(true),
        register,
    ])
    .unwrap();
}
