//! The release of a refused first send (`shielded_release`, #444): spend 1
//! of [`spends`] stored under the hook, its first broadcast failed, so the
//! library holds its notes `Unresolved(Cancelled)`. Platform's proved reads
//! come from a script ([`Script`]); every case runs a real pass.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

use platform_wallet::wallet::shielded::ShieldedNote;

use super::*;
use crate::platform::shielded::{ShieldedActivityStatus as Shown, ShieldedRefusal};
use crate::platform::shielded_release::{
    ANCHOR_READ_MARGIN, ANCHOR_RECHECK, Held, NULLIFIER_READ_LIMIT, Proved, ReleaseNet, Statuses,
    Verdict, batches, verdict,
};
use crate::platform::trust::BoxFuture;

/// Platform as the release reads it.
#[derive(Default)]
struct Platform {
    /// The anchors tree and its height; `None`: the read fails.
    anchors: Option<(Vec<[u8; 32]>, u64)>,
    /// The nullifier read's height; `None`: the read fails.
    nullifiers_height: Option<u64>,
    spent: HashSet<[u8; 32]>,
    /// Nullifiers the proof leaves out.
    missing: HashSet<[u8; 32]>,
    /// The anchors read's block time (ms); `None`: [`later_ms`].
    anchors_time_ms: Option<u64>,
    /// The proofs' quorum keys are the trusted service's, not SPV-verified.
    unverified: bool,
    /// How many of the next nullifier reads fail.
    fail_nullifier_reads: usize,
}

#[derive(Default)]
struct Script {
    platform: Mutex<Platform>,
    anchor_reads: AtomicUsize,
    nullifier_reads: AtomicUsize,
}

fn now_ms() -> u64 {
    crate::events::unix_now() * 1000
}

/// Block times past [`ANCHOR_READ_MARGIN`] after any disposition the test
/// writes.
fn later_ms() -> u64 {
    now_ms() + ANCHOR_READ_MARGIN.as_millis() as u64 + 15 * 60_000
}

impl ReleaseNet for Script {
    fn anchors(&self) -> BoxFuture<Option<Proved<Vec<[u8; 32]>>>> {
        self.anchor_reads.fetch_add(1, Ordering::SeqCst);
        let p = self.platform.lock().unwrap();
        let read = (p.anchors.clone()).map(|(value, height)| Proved {
            value,
            height,
            time_ms: p.anchors_time_ms.unwrap_or_else(later_ms),
            // `trust::scoped` sets it from the lookups noted below.
            verified: false,
        });
        let verified = !p.unverified;
        Box::pin(async move {
            crate::platform::trust::note_lookup(verified);
            read
        })
    }

    fn nullifiers(&self, nullifiers: Vec<[u8; 32]>) -> BoxFuture<Option<Statuses>> {
        self.nullifier_reads.fetch_add(1, Ordering::SeqCst);
        let mut p = self.platform.lock().unwrap();
        let failing = p.fail_nullifier_reads > 0;
        p.fail_nullifier_reads = p.fail_nullifier_reads.saturating_sub(1);
        // Platform refuses a longer query outright.
        let answered = !failing && nullifiers.len() <= NULLIFIER_READ_LIMIT;
        let read = (p.nullifiers_height.filter(|_| answered)).map(|height| Proved {
            value: (nullifiers.into_iter())
                .filter(|n| !p.missing.contains(n))
                .map(|n| (n, p.spent.contains(&n)))
                .collect(),
            height,
            time_ms: later_ms(),
            verified: false,
        });
        let verified = !p.unverified;
        Box::pin(async move {
            crate::platform::trust::note_lookup(verified);
            read
        })
    }
}

/// Spend 1's anchor.
fn anchor() -> [u8; 32] {
    spends()[0].anchor
}

/// Platform after spend 1's anchor was pruned: another anchor recorded,
/// both reads at height 100, nothing spent.
fn pruned() -> Platform {
    Platform {
        anchors: Some((vec![[0xbb; 32]], 100)),
        nullifiers_height: Some(100),
        ..Default::default()
    }
}

fn notes_of(nullifiers: &[[u8; 32]]) -> Vec<ShieldedNote> {
    (nullifiers.iter().enumerate())
        .map(|(i, n)| ShieldedNote {
            position: 40 + i as u64,
            cmx: [0x90 + i as u8; 32],
            nullifier: *n,
            block_height: 1,
            is_spent: false,
            value: 1_000_000,
            note_data: Vec::new(),
        })
        .collect()
}

struct Fixture {
    _dir: tempfile::TempDir,
    engine: Engine,
    session: Arc<NetworkSession>,
    wallet: WalletId,
    script: Arc<Script>,
}

impl Fixture {
    /// Spend 1 refused at its first broadcast, the library knowing its
    /// notes unspent; spend 2 still stored. Platform reads as `platform`.
    fn new(platform: Platform) -> Self {
        let f = Self::stuck(platform);
        f.recorded_at(1, 1);
        f.recorded_at(2, 1);
        f
    }

    /// As [`Self::new`], with no height either spend's anchor was proved
    /// recorded at: as an earlier build left them.
    fn stuck(platform: Platform) -> Self {
        let dir = dw_testutil::private_tempdir();
        let engine = engine_at(dir.path());
        let (session, wallet) = first_launch(&engine);
        let f = Self {
            _dir: dir,
            engine,
            session,
            wallet,
            script: Arc::default(),
        };
        f.save_notes(&nullifiers(1));
        store_spends(&f.engine, &f.session, f.wallet);
        f.cancel(1);
        f.reads(platform);
        f
    }

    /// The dispatch hook's read: spend `n`'s anchor proved recorded at
    /// `height`.
    fn recorded_at(&self, n: u8, height: u64) {
        let (s, w) = (Arc::clone(&self.session), self.wallet);
        (self.engine)
            .block_on(async move {
                crate::platform::shielded_release::record_presence(&s, w, st_hash(n), height).await
            })
            .unwrap();
    }

    /// The height spend `n`'s anchor was proved recorded at, as kept.
    fn presence(&self, n: u8) -> Option<u64> {
        let (s, w) = (Arc::clone(&self.session), self.wallet);
        (self.engine)
            .block_on(async move {
                crate::platform::shielded_release::presence(&s, w, st_hash(n)).await
            })
            .unwrap()
    }

    fn reads(&self, platform: Platform) {
        *self.script.platform.lock().unwrap() = platform;
        let net: Arc<dyn ReleaseNet> = self.script.clone();
        *self.session.platform.release_net.lock().unwrap() = Some(net);
    }

    fn platform(&self) -> std::sync::MutexGuard<'_, Platform> {
        self.script.platform.lock().unwrap()
    }

    fn cancel(&self, n: u8) {
        let cancelled = ShieldedSpendDisposition::Unresolved(UnresolvedReason::Cancelled);
        resolve(&self.engine, &self.session, self.wallet, n, cancelled);
    }

    fn coordinator(&self) -> Arc<platform_wallet::wallet::shielded::NetworkShieldedCoordinator> {
        let (s, m) = (Arc::clone(&self.session), self.session.manager().unwrap());
        (self.engine)
            .block_on(async move { pool::coordinator(&s, &m, None).await })
            .unwrap()
    }

    fn save_notes(&self, nullifiers: &[[u8; 32]]) {
        let (coordinator, wallet) = (self.coordinator(), self.wallet);
        let notes = notes_of(nullifiers);
        self.engine.block_on(async move {
            let mut store = coordinator.store().write().await;
            for note in &notes {
                store.save_note(subwallet(wallet), note).unwrap();
            }
        });
    }

    /// The library's scan marks `nullifiers` spent.
    fn mark_spent(&self, nullifiers: &[[u8; 32]]) {
        let (coordinator, wallet) = (self.coordinator(), self.wallet);
        let nullifiers = nullifiers.to_vec();
        self.engine.block_on(async move {
            let mut store = coordinator.store().write().await;
            for n in &nullifiers {
                store.mark_spent(subwallet(wallet), n).unwrap();
            }
        });
    }

    fn unspent(&self) -> HashSet<[u8; 32]> {
        let (coordinator, wallet) = (self.coordinator(), self.wallet);
        self.engine.block_on(async move {
            (coordinator.store().read().await)
                .get_unspent_notes(subwallet(wallet))
                .unwrap()
                .into_iter()
                .map(|n| n.nullifier)
                .collect()
        })
    }

    fn pass(&self) {
        pass(&self.engine, &self.session);
    }

    fn settlement(&self) -> Settlement {
        settlement(&self.engine, &self.session, self.wallet, 1)
    }

    /// The library's outcomes, as [`listing`].
    fn listing(&self) -> Vec<[u8; 32]> {
        (listing(&self.engine, &self.session, self.wallet)
            .unwrap()
            .into_iter())
        .map(|(st_hash, _)| st_hash)
        .collect()
    }

    /// The registration's "no unsettled spend" fact reads the library's
    /// held spends.
    fn held_spends(&self) -> HashSet<[u8; 32]> {
        (self.engine)
            .block_on(on_pool(&self.session, self.wallet, |_, lib| async move {
                Ok(pool::ShieldedLib::held_spends(&lib).await?)
            }))
            .unwrap()
    }

    fn reads_taken(&self) -> (usize, usize) {
        (
            self.script.anchor_reads.load(Ordering::SeqCst),
            self.script.nullifier_reads.load(Ordering::SeqCst),
        )
    }

    fn assert_held(&self) {
        assert_eq!(self.settlement(), held());
        assert!(self.listing().contains(&st_hash(1)));
        assert!(self.held_spends().contains(&st_hash(1)));
        assert!(nullifiers(1).iter().all(|n| !self.unspent().contains(n)));
    }

    fn assert_released(&self) {
        assert_eq!(
            self.settlement(),
            (
                ArtifactStatus::NotExecuted,
                stored("unresolved", Some("cancelled")),
                true
            )
        );
        assert!(!self.listing().contains(&st_hash(1)));
        assert!(!self.held_spends().contains(&st_hash(1)));
        assert!(nullifiers(1).iter().all(|n| self.unspent().contains(n)));
    }

    /// The library's own row of spend 1, which it flips to failed at the
    /// refusal.
    fn save_failed_row(&self) {
        self.save_failed_row_of(1);
    }

    /// The library's own row of spend `n`, failed (past spend 2, a row of
    /// no stored spend).
    fn save_failed_row_of(&self, n: u8) {
        let failed = ShieldedActivityEntry {
            id: [n; 32],
            kind: ShieldedActivityKind::Sent,
            direction: ShieldedDirection::Out,
            amount: 1_000,
            fee: None,
            counterparty: None,
            memo: None,
            block_height: None,
            status: ShieldedActivityStatus::Failed,
            created_at_ms: 0,
            min_note_position: None,
            note_cmxs: Vec::new(),
            spent_nullifiers: if n <= 2 { nullifiers(n) } else { Vec::new() },
        };
        let (coordinator, wallet) = (self.coordinator(), self.wallet);
        self.engine.block_on(async move {
            (coordinator.store().write().await)
                .save_activity(subwallet(wallet), &failed)
                .unwrap();
        });
    }

    /// Spend 1's history row as the facade reads it: status and refusal.
    fn row(&self) -> (Shown, Option<ShieldedRefusal>) {
        self.row_of(1)
    }

    /// Spend `n`'s history row as the facade reads it.
    fn row_of(&self, n: u8) -> (Shown, Option<ShieldedRefusal>) {
        let rows = (self.engine)
            .block_on(self.session.dashpay(self.wallet).shielded_history(0, 50))
            .unwrap();
        let row = (rows.into_iter())
            .find(|r| r.id == hex::encode([n; 32]))
            .unwrap();
        (row.status, row.refusal)
    }

    fn forget_backoff(&self) {
        self.session
            .platform
            .release_backoff
            .lock()
            .unwrap()
            .clear();
    }
}

fn held() -> Settlement {
    (
        ArtifactStatus::Unresolved { unrecorded: false },
        stored("unresolved", Some("cancelled")),
        false,
    )
}

/// T2: the anchor proved absent at Ha and every nullifier proved unspent at
/// Hn ≥ Ha: settled `not_executed` (the library's disposition kept) and
/// acknowledged, the notes selectable again, registration's fact cleared;
/// the wallet closes once spend 2 is settled too.
#[test]
fn a_pruned_anchor_and_unspent_nullifiers_release_the_notes() {
    let f = Fixture::new(pruned());
    f.pass();
    f.assert_released();
    assert_eq!(f.reads_taken(), (1, 1));

    f.cancel(2);
    f.platform().anchors = Some((vec![], 100));
    f.save_notes(&nullifiers(2));
    f.pass();
    assert!(f.listing().is_empty());
    let unloaded = f.engine.block_on(f.session.unload_wallet(f.wallet));
    assert!(unloaded.is_ok(), "{unloaded:?}");
}

/// T1 and T6: while the anchor is recorded (an idle pool keeps its highest
/// one) the spend stays held, and is read again only after
/// [`ANCHOR_RECHECK`]; once a later anchor replaced it and it was pruned,
/// the next read releases it.
#[test]
fn a_recorded_anchor_holds_until_it_is_pruned() {
    let f = Fixture::new(Platform {
        anchors: Some((vec![anchor()], 100)),
        nullifiers_height: Some(100),
        ..Default::default()
    });
    f.pass();
    f.assert_held();
    refused(f.engine.block_on(f.session.unload_wallet(f.wallet)));
    f.pass();
    assert_eq!(f.reads_taken(), (1, 1), "not read again before the recheck");
    assert!(ANCHOR_RECHECK >= Duration::from_secs(600));

    f.platform().anchors = Some((vec![anchor(), [0xbb; 32]], 200));
    f.platform().nullifiers_height = Some(200);
    f.forget_backoff();
    f.pass();
    f.assert_held();

    *f.platform() = Platform {
        anchors: Some((vec![[0xbb; 32]], 1300)),
        nullifiers_height: Some(1300),
        ..Default::default()
    };
    f.forget_backoff();
    f.pass();
    f.assert_released();
}

/// T4 and T5: a nullifier read one block below the anchors read (a lagging
/// node) holds, and so does an anchors read taken above the nullifier
/// read; at the same height it releases.
#[test]
fn the_nullifier_read_must_not_be_below_the_anchors_read() {
    let f = Fixture::new(Platform {
        anchors: Some((vec![], 100)),
        nullifiers_height: Some(99),
        ..Default::default()
    });
    f.pass();
    f.assert_held();
    f.platform().anchors = Some((vec![], 120));
    f.platform().nullifiers_height = Some(110);
    f.pass();
    f.assert_held();
    f.platform().nullifiers_height = Some(120);
    f.pass();
    f.assert_released();
}

/// T3: held while a nullifier is missing from the proof, while a held note
/// is unknown to the library, and while the journal flags one spent.
#[test]
fn unproved_or_contradicted_nullifiers_hold() {
    let f = Fixture::new(pruned());
    f.platform().missing.insert(nullifiers(1)[1]);
    f.pass();
    f.assert_held();

    let unknown = Fixture::new(pruned());
    let other = [0x77; 32];
    let s = Arc::clone(&unknown.session);
    let w = unknown.wallet;
    // A held note the library does not know: the disposition names it,
    // the account has no such note.
    unknown.engine.block_on(async move {
        let m = s.manager().unwrap();
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        let mut r = spends().remove(0);
        r.activity_id = [6; 32];
        r.nullifiers = vec![other];
        r.st_bytes = transition(1, r.anchor, &[other]);
        assert!(store.mark_pending(subwallet(w), &other).unwrap());
        store.arm_redrive(subwallet(w), r.clone()).unwrap();
        s.leases
            .record_shielded_artifact(
                w,
                NewShieldedArtifact {
                    activity: r.activity_id,
                    origin: ArtifactOrigin::Dispatched,
                    st_hash: Some(Sha256::digest(&r.st_bytes).into()),
                    kind: Some(ArtifactKind::Transfer),
                    anchor: Some(r.anchor),
                    identity_id: None,
                    selected: vec![other],
                    published: vec![other],
                },
            )
            .await
            .unwrap();
        store
            .resolve_redrive(
                subwallet(w),
                &[6; 32],
                ShieldedSpendDisposition::Unresolved(UnresolvedReason::Cancelled),
            )
            .unwrap();
    });
    unknown.pass();
    unknown.assert_released();
    let unknown_settled = (unknown.engine)
        .block_on(unknown.session.leases.shielded_artifact(w, [6; 32]))
        .unwrap()
        .unwrap();
    assert_eq!(unknown_settled.status, held().0);
    assert!(!unknown_settled.acked);

    let flagged = Fixture::new(pruned());
    spent_seen(&flagged.session, flagged.wallet, &nullifiers(1)[..1]);
    flagged.pass();
    assert_eq!(flagged.settlement(), held());
    assert!(flagged.listing().contains(&st_hash(1)));
}

/// ASK-2, all published nullifiers proved spent: it executed. Settled
/// `executed` (never failed), acknowledged once the journal reads its notes
/// spent, at a later pass. It published no padding nullifier, so another
/// device's spend of the same notes reads the same: its payout evidence
/// stays unknown.
#[test]
fn every_nullifier_spent_settles_executed() {
    let f = Fixture::new(Platform {
        anchors: Some((vec![anchor()], 100)),
        nullifiers_height: Some(100),
        spent: nullifiers(1).into_iter().collect(),
        ..Default::default()
    });
    f.pass();
    assert_eq!(
        f.settlement(),
        (
            ArtifactStatus::Executed,
            stored("unresolved", Some("cancelled")),
            false
        )
    );
    assert!(f.listing().contains(&st_hash(1)), "held until read spent");
    let evidence = (f.engine).block_on(f.session.shielded_payout_evidence(
        f.wallet,
        "payout",
        st_hash(1),
    ));
    assert_eq!(
        evidence,
        crate::platform::payment_locks::PayoutEvidence::Unknown
    );

    spent_seen(&f.session, f.wallet, &nullifiers(1));
    f.pass();
    assert_eq!(
        f.settlement(),
        (
            ArtifactStatus::Executed,
            stored("unresolved", Some("cancelled")),
            true
        )
    );
    assert!(!f.listing().contains(&st_hash(1)));
}

/// ASK-2, some but not all published nullifiers proved spent: it never
/// executed and never can. Held until the spent note reads spent in the
/// library and in the journal, which records it only once the wallet's
/// write succeeded (so no relaunch selects it), then released whatever the
/// anchor.
#[test]
fn some_nullifiers_spent_release_once_read_spent_locally() {
    let spent = nullifiers(1)[0];
    let f = Fixture::new(Platform {
        anchors: Some((vec![anchor()], 100)),
        nullifiers_height: Some(100),
        spent: [spent].into_iter().collect(),
        ..Default::default()
    });
    f.pass();
    assert_eq!(f.settlement(), held());

    f.mark_spent(&[spent]);
    f.pass();
    assert_eq!(f.settlement(), held(), "read spent in memory only");

    spent_seen(&f.session, f.wallet, &[spent]);
    f.pass();
    assert_eq!(f.settlement().0, ArtifactStatus::NotExecuted);
    assert!(f.settlement().2);
    assert!(!f.listing().contains(&st_hash(1)));
    let unspent = f.unspent();
    assert!(unspent.contains(&nullifiers(1)[1]));
    assert!(!unspent.contains(&spent));
}

/// T8: a crash between the journal's settle and the library's
/// acknowledgement: the next pass acknowledges it, with no read and no
/// journal error.
#[test]
fn a_crash_before_the_ack_is_acknowledged_at_the_next_pass() {
    let f = Fixture::new(pruned());
    f.save_failed_row();
    f.session
        .platform
        .crash_before_ack
        .store(true, Ordering::SeqCst);
    f.pass();
    assert_eq!(f.settlement().0, ArtifactStatus::NotExecuted);
    assert!(f.settlement().2, "acknowledged in the journal");
    assert!(f.listing().contains(&st_hash(1)), "not in the library");
    assert_eq!(
        f.row(),
        (Shown::Pending, Some(ShieldedRefusal::Held)),
        "never released while the library still holds the notes"
    );

    f.session
        .platform
        .crash_before_ack
        .store(false, Ordering::SeqCst);
    f.reads(Platform::default());
    let marks = &f.session.hub.pump.platform_marks;
    *marks.lock().unwrap() = Some(Vec::new());
    f.pass();
    f.assert_released();
    assert_eq!(f.row(), (Shown::Failed, Some(ShieldedRefusal::Released)));
    let raised = (marks.lock().unwrap().take().unwrap().into_iter())
        .any(|(w, c)| w == f.wallet && c.contains(&crate::events::PlatformChange::Shielded));
    assert!(raised, "the acknowledgement raises the Shielded change");
}

/// T11: a failed anchors read or nullifier read holds and is read again at
/// the next pass (no recheck delay).
#[test]
fn a_failed_read_holds_and_reads_again() {
    let f = Fixture::new(Platform {
        anchors: None,
        nullifiers_height: Some(100),
        ..Default::default()
    });
    f.pass();
    f.assert_held();
    f.platform().anchors = Some((vec![], 100));
    f.platform().nullifiers_height = None;
    f.pass();
    f.assert_held();
    f.platform().nullifiers_height = Some(100);
    f.pass();
    f.assert_released();
    assert_eq!(f.reads_taken(), (3, 3));
}

/// T10: a spend whose result the library does not know keeps its record
/// for the redrive; the release never reads for it, even with its anchor
/// pruned.
#[test]
fn an_unconfirmed_spend_is_left_to_the_redrive() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    store_spends(&engine, &session, wallet);
    let script = Arc::new(Script::default());
    *script.platform.lock().unwrap() = pruned();
    let net: Arc<dyn ReleaseNet> = script.clone();
    *session.platform.release_net.lock().unwrap() = Some(net);
    pass(&engine, &session);
    assert_eq!(script.anchor_reads.load(Ordering::SeqCst), 0);
    assert_eq!(script.nullifier_reads.load(Ordering::SeqCst), 0);
    assert_eq!(kept(&engine, &session, wallet), expected());
}

/// T13: the library's outcome acknowledged by something else between the
/// reads and the settle: the release writes nothing.
#[test]
fn a_change_after_the_reads_writes_nothing() {
    let f = Fixture::new(pruned());
    let (reached, reached_rx) = tokio::sync::oneshot::channel();
    let (resume, resume_rx) = tokio::sync::oneshot::channel();
    *f.session.platform.release_pause.lock().unwrap() = Some(pool::PassPause {
        reached,
        resume: resume_rx,
    });
    let coordinator = f.coordinator();
    let wallet = f.wallet;
    std::thread::scope(|scope| {
        let passing = scope.spawn(|| f.pass());
        let wait = async { tokio::time::timeout(Duration::from_secs(120), reached_rx).await };
        (f.engine.block_on(wait))
            .expect("the release reached its pause")
            .unwrap();
        f.engine
            .block_on(coordinator.ack_spend_disposition(
                wallet.0,
                subwallet(wallet).account_index,
                &st_hash(1),
            ))
            .unwrap();
        resume.send(()).unwrap();
        passing.join().unwrap();
    });
    assert_eq!(f.settlement(), held());
}

/// Q80 C2's premise: only the release settles a refused first send
/// `executed`. With the release offline (every read fails), passes leave
/// its artifact `unresolved` with the library's `unresolved/cancelled`,
/// never executed, and acknowledge nothing.
#[test]
fn an_offline_release_never_settles_executed() {
    let f = Fixture::new(pruned());
    *f.session.platform.release_net.lock().unwrap() = None;
    for _ in 0..2 {
        f.forget_backoff();
        f.pass();
        f.assert_held();
    }
    assert_eq!(f.reads_taken(), (0, 0));
}

/// Platform data that frees notes or confirms a contact payment must be
/// SPV-verified (rule 5, `strict_rule_5`): reads whose proofs used the
/// trusted service's quorum keys settle nothing, whatever they prove; with
/// the trusted-only toggle on, they do.
#[test]
fn reads_on_unverified_quorum_keys_settle_nothing() {
    let f = Fixture::new(Platform {
        unverified: true,
        ..pruned()
    });
    f.pass();
    f.assert_held();

    let executed = Fixture::new(Platform {
        unverified: true,
        anchors: Some((vec![anchor()], 100)),
        nullifiers_height: Some(100),
        spent: nullifiers(1).into_iter().collect(),
        ..Default::default()
    });
    executed.pass();
    assert_eq!(executed.settlement(), held());

    f.session.trust.set_trusted_only(true);
    f.forget_backoff();
    f.pass();
    f.assert_released();
}

/// Only an SPV-verified read that finds the anchor still recorded keeps
/// its height (rule 5): an unverified one keeps none, so a spend sent by an
/// earlier build stays without an Hp, and held, until a verified read.
#[test]
fn an_unverified_read_of_a_recorded_anchor_keeps_no_height() {
    let f = Fixture::stuck(Platform {
        unverified: true,
        anchors: Some((vec![anchor()], 100)),
        nullifiers_height: Some(100),
        ..Default::default()
    });
    f.pass();
    f.assert_held();
    assert_eq!(f.presence(1), None);

    f.platform().unverified = false;
    f.forget_backoff();
    f.pass();
    f.assert_held();
    assert_eq!(f.presence(1), Some(100));
}

/// An anchors read one block behind (the SDK's height tolerance) can be
/// from before the spend's anchor was recorded, whatever the clock said at
/// the refusal: once a read proved the anchor recorded at a height, its
/// absence counts only above that height.
#[test]
fn an_absence_at_or_below_a_proved_presence_holds() {
    let f = Fixture::new(Platform {
        anchors: Some((vec![anchor()], 100)),
        nullifiers_height: Some(100),
        ..Default::default()
    });
    f.pass();
    f.assert_held();

    f.platform().anchors = Some((vec![[0xbb; 32]], 99));
    f.platform().nullifiers_height = Some(99);
    f.forget_backoff();
    f.pass();
    f.assert_held();

    f.platform().anchors = Some((vec![[0xbb; 32]], 101));
    f.platform().nullifiers_height = Some(101);
    f.forget_backoff();
    f.pass();
    f.assert_released();
}

/// T7: whatever refused it (a consensus verdict, a transport failure), the
/// library writes the same `Unresolved(Cancelled)`, so a second refused
/// spend of the wallet is released by the same rule in the same pass.
/// `settle_failed_spend` takes no error: every send error but
/// `ShieldedSpendUnconfirmed` reaches it, and it writes the disposition
/// through `cancel_pending`. Pinned by rs-platform-wallet's
/// `wallet/shielded/operations.rs` tests at the pinned revision: which
/// errors are definitive, `consensus_cause_is_a_definitive_rejection` and
/// `broadcast_consensus_rejection_is_definitive` (a consensus verdict),
/// `broadcast_connection_failures_and_rejections_are_definitive` (a refused
/// connection, `Unavailable`, and `ResourceExhausted`); the disposition,
/// `a_failed_first_send_after_dispatch_holds_its_notes`,
/// `a_failed_cancel_write_leaves_the_activity_pending` and
/// `cancel_after_armed_first_send_records_disposition`.
#[test]
fn two_refused_spends_release_in_one_pass() {
    let f = Fixture::new(pruned());
    f.save_notes(&nullifiers(2));
    f.cancel(2);
    f.pass();
    f.assert_released();
    let second = settlement(&f.engine, &f.session, f.wallet, 2);
    assert_eq!(second.0, ArtifactStatus::NotExecuted);
    assert!(f.listing().is_empty());
    assert_eq!(f.reads_taken(), (1, 1), "one read of each for the wallet");
}

/// An anchor can be missing because the node read has not recorded it yet:
/// an anchors read whose block time is within [`ANCHOR_READ_MARGIN`] of the
/// refusal proves nothing, and the spend is read again at the next pass.
#[test]
fn an_anchors_read_too_soon_after_the_refusal_holds() {
    let f = Fixture::new(Platform {
        anchors_time_ms: Some(now_ms() + 30 * 60_000),
        ..pruned()
    });
    f.pass();
    f.assert_held();
    f.platform().anchors_time_ms = None;
    f.pass();
    f.assert_released();
}

/// C1 and C2: the history row of a held refused spend reads pending and
/// held (never failed) from the refusal on, before any pass too, and failed
/// and released only once the release settled it; the library's own row
/// reads failed throughout.
#[test]
fn the_history_row_follows_the_release() {
    let f = Fixture::new(Platform {
        anchors: Some((vec![anchor()], 100)),
        nullifiers_height: Some(100),
        ..Default::default()
    });
    f.save_failed_row();
    assert_eq!(f.row(), (Shown::Pending, Some(ShieldedRefusal::Held)));
    f.pass();
    assert_eq!(f.row(), (Shown::Pending, Some(ShieldedRefusal::Held)));

    f.platform().anchors = Some((vec![], 1300));
    f.platform().nullifiers_height = Some(1300);
    f.forget_backoff();
    f.pass();
    f.assert_released();
    assert_eq!(f.row(), (Shown::Failed, Some(ShieldedRefusal::Released)));
}

/// Q80 R3, a source unread: while the library's outcomes cannot be listed
/// (here between the release's settle and the library's acknowledgement,
/// so the journal alone reads released) a failed row reads held; while the
/// journal cannot be read, a failed row the library does not list (spend
/// 2, dispatched, its result unknown) reads held too. Neither reads failed.
#[test]
fn a_failed_row_reads_held_while_its_sources_cannot_be_read() {
    let f = Fixture::new(pruned());
    f.save_failed_row();
    f.save_failed_row_of(2);
    let crash = &f.session.platform.crash_before_ack;
    crash.store(true, Ordering::SeqCst);
    f.pass();
    assert_eq!(f.settlement().0, ArtifactStatus::NotExecuted);
    let fail_listing = &f.session.platform.fail_disposition_listing;
    f.save_failed_row_of(9);
    assert_eq!(
        f.row_of(9),
        (Shown::Failed, None),
        "no artifact: as the library reads it"
    );
    fail_listing.store(true, Ordering::SeqCst);
    assert_eq!(f.row(), (Shown::Pending, Some(ShieldedRefusal::Held)));
    assert_eq!(f.row_of(9), (Shown::Pending, Some(ShieldedRefusal::Held)));
    fail_listing.store(false, Ordering::SeqCst);
    assert_eq!(f.row(), (Shown::Pending, Some(ShieldedRefusal::Held)));

    f.engine.block_on(f.session.leases.close_journal());
    assert!(!f.session.leases.journal_ready());
    assert_eq!(f.row_of(2), (Shown::Pending, Some(ShieldedRefusal::Held)));
}

/// Q80 R3, unreconciled: a dispatched spend whose result the journal does
/// not hold yet (spend 2, registered by the hook, the library's row failed
/// before any outcome reached the journal) is never shown failed.
#[test]
fn an_unreconciled_dispatched_spend_never_reads_failed() {
    let f = Fixture::new(pruned());
    f.save_failed_row_of(2);
    assert_eq!(
        settlement(&f.engine, &f.session, f.wallet, 2).0,
        ArtifactStatus::Registered
    );
    assert_eq!(f.row_of(2), (Shown::Pending, Some(ShieldedRefusal::Held)));
}

/// Q80 S1: a release changes the wallet's Shielded balance and history, so
/// it raises the Shielded change for the next notification; a pass that
/// releases nothing raises none. (The first pass is not counted: its
/// offline scan fails, and a wallet's first failed pass raises the change
/// itself.)
#[test]
fn a_release_raises_the_shielded_change() {
    let f = Fixture::new(Platform {
        anchors: Some((vec![anchor()], 100)),
        nullifiers_height: Some(100),
        ..Default::default()
    });
    let marks = &f.session.hub.pump.platform_marks;
    let shielded = |m: &Vec<(
        WalletId,
        std::collections::BTreeSet<crate::events::PlatformChange>,
    )>| {
        m.iter()
            .any(|(w, c)| *w == f.wallet && c.contains(&crate::events::PlatformChange::Shielded))
    };
    f.pass();
    f.assert_held();
    *marks.lock().unwrap() = Some(Vec::new());
    f.forget_backoff();
    f.pass();
    f.assert_held();
    assert!(
        !shielded(marks.lock().unwrap().as_ref().unwrap()),
        "held: none"
    );

    f.platform().anchors = Some((vec![[0xbb; 32]], 1300));
    f.platform().nullifiers_height = Some(1300);
    f.forget_backoff();
    f.pass();
    f.assert_released();
    assert!(
        shielded(marks.lock().unwrap().as_ref().unwrap()),
        "released"
    );
}

/// Q80 S2: a failed nullifier read holds only the spends of its batch; the
/// pass goes on to the next batch. Spend 3 publishes enough nullifiers that
/// it reads apart from spend 1; whichever is read first fails.
#[test]
fn a_failed_batch_holds_only_its_own_spends() {
    let f = Fixture::new(pruned());
    let big = big_spend();
    f.save_notes(&big.nullifiers);
    let (s, w) = (Arc::clone(&f.session), f.wallet);
    let stored = big.clone();
    f.engine.block_on(async move {
        let m = s.manager().unwrap();
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        for n in &stored.nullifiers {
            assert!(store.mark_pending(subwallet(w), n).unwrap());
            (store.set_pending_spend(subwallet(w), n, stored.anchor, stored.activity_id)).unwrap();
        }
        store.arm_redrive(subwallet(w), stored.clone()).unwrap();
        drop(store);
        s.leases
            .record_shielded_artifact(
                w,
                NewShieldedArtifact {
                    activity: stored.activity_id,
                    origin: ArtifactOrigin::Dispatched,
                    st_hash: Some(Sha256::digest(&stored.st_bytes).into()),
                    kind: Some(ArtifactKind::Transfer),
                    anchor: Some(stored.anchor),
                    identity_id: None,
                    selected: stored.nullifiers.clone(),
                    published: stored.nullifiers.clone(),
                },
            )
            .await
            .unwrap();
    });
    let cancelled = ShieldedSpendDisposition::Unresolved(UnresolvedReason::Cancelled);
    resolve(&f.engine, &f.session, f.wallet, 3, cancelled);
    let big_hash: [u8; 32] = Sha256::digest(&big.st_bytes).into();
    let (s, w) = (Arc::clone(&f.session), f.wallet);
    (f.engine)
        .block_on(async move {
            crate::platform::shielded_release::record_presence(&s, w, big_hash, 1).await
        })
        .unwrap();
    f.platform().fail_nullifier_reads = 1;
    f.pass();
    assert_eq!(f.reads_taken(), (1, 2), "both batches read");
    let released =
        |n: u8| settlement(&f.engine, &f.session, f.wallet, n).0 == ArtifactStatus::NotExecuted;
    assert!(
        released(1) != released(3),
        "one batch released, the other held"
    );
}

/// Spend 3: 99 nullifiers on its own anchor, cancelled like spend 1.
fn big_spend() -> PendingRedrive {
    let anchor = [0xa3; 32];
    let nullifiers: Vec<[u8; 32]> = (0..99u8)
        .map(|i| {
            let mut n = [0x33; 32];
            n[0] = i;
            n
        })
        .collect();
    PendingRedrive {
        activity_id: [3; 32],
        st_bytes: transition(1, anchor, &nullifiers),
        anchor,
        nullifiers,
        attempts: 0,
        identity_nonce_finalized: false,
        identity_user_abandoned: false,
    }
}

/// Where [`a_wallet_stuck_before_the_upgrade_is_released_after_the_relaunch`]'s
/// child keeps its data root.
const STUCK_ROOT: &str = "DW_444_STUCK_ROOT";

/// The child of the next test: a wallet stuck as today's builds leave it
/// (spend 1 refused, its pass unable to prove anything), then a quit.
#[test]
#[ignore = "the child process of a_wallet_stuck_before_the_upgrade_is_released_after_the_relaunch"]
fn stuck_before_the_upgrade_child() {
    let Some(root) = std::env::var_os(STUCK_ROOT) else {
        return;
    };
    let engine = engine_at(Path::new(&root));
    let (session, wallet) = first_launch(&engine);
    let f = Fixture {
        _dir: dw_testutil::private_tempdir(),
        engine,
        session,
        wallet,
        script: Arc::default(),
    };
    f.save_notes(&nullifiers(1));
    store_spends(&f.engine, &f.session, f.wallet);
    f.cancel(1);
    f.pass();
    f.assert_held();
    std::fs::write(Path::new(&root).join("wallet"), wallet.to_string()).unwrap();
    f.engine.block_on(f.engine.shutdown()).unwrap();
    std::process::exit(0);
}

/// T12: a wallet stuck before the upgrade (the journal reads the spend
/// `unresolved`/`cancelled`, the library holds it, no height its anchor was
/// proved recorded at) is held while its anchor reads recorded, that read
/// keeping its height, and released at the first pass after it was pruned
/// above that height.
#[test]
fn a_wallet_stuck_before_the_upgrade_is_released_after_the_relaunch() {
    let dir = dw_testutil::private_tempdir();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "platform::shielded_close_tests::disposition_tests::release_tests::stuck_before_the_upgrade_child",
            "--exact",
            "--ignored",
            "--test-threads=1",
        ])
        .env(STUCK_ROOT, dir.path())
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "the stuck process failed: {}",
        String::from_utf8_lossy(&child.stderr)
    );
    let wallet: WalletId = (std::fs::read_to_string(dir.path().join("wallet")).unwrap())
        .parse()
        .unwrap();
    let engine = engine_at(dir.path());
    let session = open(&engine);
    engine
        .block_on(session.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
        .unwrap();
    assert!(bind(&engine, &session, wallet));
    engine.block_on(session.ensure_background());
    let f = Fixture {
        _dir: dw_testutil::private_tempdir(),
        engine,
        session,
        wallet,
        script: Arc::default(),
    };
    assert_eq!(f.settlement(), held());
    assert!(
        f.listing().contains(&st_hash(1)),
        "still held by the library"
    );
    assert_eq!(f.presence(1), None);
    // The notes as the library's bind restores them from its scan.
    f.save_notes(&nullifiers(1));
    f.reads(Platform {
        anchors: Some((vec![anchor()], 100)),
        nullifiers_height: Some(100),
        ..Default::default()
    });
    f.pass();
    f.assert_held();
    assert_eq!(f.presence(1), Some(100));
    *f.platform() = Platform {
        anchors: Some((vec![[0xbb; 32]], 1300)),
        nullifiers_height: Some(1300),
        ..Default::default()
    };
    f.forget_backoff();
    f.pass();
    f.assert_released();
}

/// A spend no read ever proved its anchor recorded at (sent by an earlier
/// build, its anchor pruned before any pass read it) has no height its
/// absence can be ordered above: it stays held, however late the read.
#[test]
fn a_spend_never_proved_recorded_stays_held() {
    let f = Fixture::stuck(Platform {
        anchors: Some((vec![[0xbb; 32]], 5000)),
        nullifiers_height: Some(5000),
        ..Default::default()
    });
    for _ in 0..2 {
        f.forget_backoff();
        f.pass();
        f.assert_held();
    }
    assert_eq!(f.presence(1), None);
    assert_eq!(f.reads_taken(), (2, 2));
}

fn nf(i: u16) -> [u8; 32] {
    let mut n = [0x5a; 32];
    n[..2].copy_from_slice(&i.to_le_bytes());
    n
}

/// A held spend of `actions` actions, its last one padding.
fn held_spend(first: u16, actions: u16) -> Held {
    let ns: Vec<[u8; 32]> = (first..first + actions).map(nf).collect();
    Held {
        subwallet: subwallet(WalletId([7; 32])),
        st_hash: nf(first),
        at_ms: 1_000,
        held: ns[..ns.len() - 1].iter().copied().collect(),
        anchor: Some([0xa1; 32]),
        selected: ns[..ns.len() - 1].iter().map(|&n| (n, false)).collect(),
        published: ns,
        presence: Some(50),
    }
}

/// Whole spends go into reads Platform answers, never split, in order: 7
/// spends of 16 actions (Platform refuses more than 100 nullifiers in one
/// read), and a spend that alone passes the limit reads on its own.
#[test]
fn spends_are_read_in_batches_platform_answers() {
    let spends: Vec<Held> = (0..7).map(|i| held_spend(i * 16, 16)).collect();
    let packed = batches(&spends);
    assert_eq!(packed, vec![vec![0, 1, 2, 3, 4, 5], vec![6]]);
    for batch in &packed {
        let all: HashSet<[u8; 32]> = batch
            .iter()
            .flat_map(|&i| spends[i].published.clone())
            .collect();
        assert!(all.len() <= NULLIFIER_READ_LIMIT);
    }
    let alone = [held_spend(200, 2), held_spend(300, 101), held_spend(500, 2)];
    assert_eq!(batches(&alone), vec![vec![0], vec![1], vec![2]]);
}

/// Every branch of the rule on one spend (two held notes and a padding
/// nullifier, its anchor proved recorded at 50): only the full evidence
/// releases, every published nullifier spent is executed, some spent
/// releases once the library and the journal read those spent, and
/// anything missing, unverified or contradicting holds.
#[test]
fn the_verdict_releases_only_on_the_whole_evidence() {
    let spend = held_spend(0x40, 3);
    let (n1, pad) = (spend.published[0], spend.published[2]);
    let margin = ANCHOR_READ_MARGIN.as_millis() as u64;
    let anchors = Proved {
        value: vec![[0xbb; 32]],
        height: 100,
        time_ms: 1_001 + margin,
        verified: true,
    };
    let read = |spent: &[[u8; 32]], height| Proved {
        value: (spend.nullifiers().into_iter())
            .map(|n| (n, spent.contains(&n)))
            .collect(),
        height,
        time_ms: 0,
        verified: true,
    };
    let unspent: HashMap<[u8; 32], bool> = spend.held.iter().map(|&n| (n, false)).collect();
    let none = HashSet::new();
    let judge =
        |s: &Held,
         a: Option<&Proved<Vec<[u8; 32]>>>,
         n: &Statuses,
         notes: &HashMap<[u8; 32], bool>,
         flagged: &HashSet<[u8; 32]>| verdict(s, a, n, notes, flagged, false);
    let fresh = read(&[], 100);
    assert_eq!(
        judge(&spend, Some(&anchors), &fresh, &unspent, &none),
        Verdict::NotExecuted
    );

    let at_margin = Proved {
        time_ms: 1_000 + margin,
        ..anchors.clone()
    };
    let recorded = Proved {
        value: vec![[0xa1; 32]],
        ..anchors.clone()
    };
    let mut no_anchor = spend.clone();
    no_anchor.anchor = None;
    let mut read_spent = unspent.clone();
    read_spent.insert(n1, true);
    let mut unknown = unspent.clone();
    unknown.remove(&n1);
    let flagged: HashSet<[u8; 32]> = [n1].into();
    let mut stray = spend.clone();
    stray.held.insert([0x99; 32]);
    let mut stray_selected = spend.clone();
    stray_selected.selected.push(([0x99; 32], false));
    let mut twice = spend.clone();
    twice.published.push(pad);
    let mut gap = read(&[], 100);
    gap.value.retain(|(n, _)| *n != pad);
    let mut no_presence = spend.clone();
    no_presence.presence = None;
    let mut present_at_ha = spend.clone();
    present_at_ha.presence = Some(100);
    let unverified_anchors = Proved {
        verified: false,
        ..anchors.clone()
    };
    let mut unverified_fresh = read(&[], 100);
    unverified_fresh.verified = false;
    let mut unverified_all = read(&spend.published, 100);
    unverified_all.verified = false;
    for (case, got) in [
        (
            "no height the anchor was proved recorded at",
            judge(&no_presence, Some(&anchors), &fresh, &unspent, &none),
        ),
        (
            "absent at the height it was proved recorded at",
            judge(&present_at_ha, Some(&anchors), &fresh, &unspent, &none),
        ),
        (
            "the anchors read unverified",
            judge(&spend, Some(&unverified_anchors), &fresh, &unspent, &none),
        ),
        (
            "the nullifier read unverified",
            judge(&spend, Some(&anchors), &unverified_fresh, &unspent, &none),
        ),
        (
            "every nullifier spent, unverified",
            judge(&spend, Some(&anchors), &unverified_all, &unspent, &none),
        ),
        (
            "a held note spent, read spent in memory only",
            judge(
                &spend,
                Some(&anchors),
                &read(&[n1], 100),
                &read_spent,
                &none,
            ),
        ),
        (
            "block time within the margin",
            judge(&spend, Some(&at_margin), &fresh, &unspent, &none),
        ),
        (
            "nullifiers read below Ha",
            judge(&spend, Some(&anchors), &read(&[], 99), &unspent, &none),
        ),
        (
            "anchors not read",
            judge(&spend, None, &fresh, &unspent, &none),
        ),
        (
            "no anchor in the journal",
            judge(&no_anchor, Some(&anchors), &fresh, &unspent, &none),
        ),
        (
            "the library reads a held note spent",
            judge(&spend, Some(&anchors), &fresh, &read_spent, &none),
        ),
        (
            "the library lacks a held note",
            judge(&spend, Some(&anchors), &fresh, &unknown, &none),
        ),
        (
            "the journal flags a held note",
            judge(&spend, Some(&anchors), &fresh, &unspent, &flagged),
        ),
        (
            "a held note not published",
            judge(&stray, Some(&anchors), &read(&[], 100), &unspent, &none),
        ),
        (
            "a selected note not published",
            judge(
                &stray_selected,
                Some(&anchors),
                &read(&[], 100),
                &unspent,
                &none,
            ),
        ),
        (
            "a nullifier published twice",
            judge(&twice, Some(&anchors), &fresh, &unspent, &none),
        ),
        (
            "a nullifier the proof leaves out",
            judge(&spend, Some(&anchors), &gap, &unspent, &none),
        ),
        (
            "a held note spent, not yet locally",
            judge(&spend, Some(&anchors), &read(&[n1], 100), &unspent, &none),
        ),
    ] {
        assert_eq!(got, Verdict::Hold, "{case}");
    }
    assert_eq!(
        judge(&spend, Some(&recorded), &fresh, &unspent, &none),
        Verdict::AnchorRecorded
    );
    let all = spend.published.clone();
    assert_eq!(
        judge(&spend, Some(&recorded), &read(&all, 100), &unspent, &none),
        Verdict::Executed
    );
    assert_eq!(
        judge(&spend, Some(&recorded), &read(&[pad], 100), &unspent, &none),
        Verdict::NotExecuted
    );
    assert_eq!(
        judge(
            &spend,
            Some(&recorded),
            &read(&[n1], 100),
            &read_spent,
            &flagged
        ),
        Verdict::NotExecuted
    );
    assert_eq!(
        verdict(
            &spend,
            Some(&unverified_anchors),
            &unverified_fresh,
            &unspent,
            &none,
            true
        ),
        Verdict::NotExecuted,
        "rule 5 off"
    );
}
