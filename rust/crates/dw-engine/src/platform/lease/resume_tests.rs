//! The catch-up's resume pass (E0-04 H6 part 2, §2a.1, §2a.3, §6.4,
//! §8.6), against a fake library.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use dash_spv::EventHandler;
use dash_spv::network::NetworkEvent;
use dashcore::Txid;
use tokio::sync::Semaphore;

use super::lock::Scope;
use super::mode_b::{TrackedLocks, funding_gate};
use super::resume::{Resumed, Resumer};
use super::tests::{
    FakeJournal, REG, Recorder, W, W2, begin, row, status, table, txid, until, with_journal,
};
use super::*;
use crate::platform::flows::RevokeCause;
use crate::{DashNetwork, WalletId};

use TrackedStatus as R;

/// The fake library: tracked rows per loaded wallet, and a resume whose
/// outcome and timing the test controls.
struct Fake {
    rows: Mutex<HashMap<WalletId, Vec<TrackedRow>>>,
    unloaded: Mutex<HashSet<WalletId>>,
    /// Every resume asked for, in order.
    calls: Mutex<Vec<(WalletId, Txid)>>,
    /// Row reads per wallet (passes and funding refreshes).
    reads: Mutex<HashMap<WalletId, usize>>,
    /// Reads of a wallet that is not loaded.
    unloaded_reads: Mutex<HashMap<WalletId, usize>>,
    /// While set, a row read panics.
    panic_rows: AtomicBool,
    /// What a row's resume returns (default `Ran`); `None` panics.
    outcomes: Mutex<HashMap<Txid, Option<Resumed>>>,
    /// Resumes of each row running now: never more than one.
    running: Mutex<HashMap<(WalletId, Txid), usize>>,
    /// While set, each resume waits for a permit of `release`.
    hold: AtomicBool,
    release: Semaphore,
}

impl Fake {
    fn new() -> Self {
        Self {
            rows: Mutex::default(),
            unloaded: Mutex::default(),
            calls: Mutex::default(),
            reads: Mutex::default(),
            unloaded_reads: Mutex::default(),
            panic_rows: AtomicBool::new(false),
            outcomes: Mutex::default(),
            running: Mutex::default(),
            hold: AtomicBool::new(false),
            release: Semaphore::new(0),
        }
    }

    fn set_rows(&self, wallet: WalletId, rows: Vec<TrackedRow>) {
        self.rows.lock().unwrap().insert(wallet, rows);
    }

    fn set_outcome(&self, n: u8, outcome: Option<Resumed>) {
        self.outcomes.lock().unwrap().insert(txid(n), outcome);
    }

    fn calls(&self) -> Vec<(WalletId, Txid)> {
        self.calls.lock().unwrap().clone()
    }

    fn calls_of(&self, wallet: WalletId, n: u8) -> usize {
        self.calls()
            .iter()
            .filter(|c| **c == (wallet, txid(n)))
            .count()
    }

    fn reads(&self, wallet: WalletId) -> usize {
        self.reads
            .lock()
            .unwrap()
            .get(&wallet)
            .copied()
            .unwrap_or(0)
    }

    fn open(&self) {
        self.hold.store(false, Ordering::SeqCst);
        self.release.add_permits(1_000);
    }
}

#[async_trait]
impl TrackedLocks for Fake {
    async fn rows(&self, wallet: WalletId) -> Option<Vec<TrackedRow>> {
        assert!(
            !self.panic_rows.load(Ordering::SeqCst),
            "injected: the row read panicked"
        );
        if self.unloaded.lock().unwrap().contains(&wallet) {
            *self
                .unloaded_reads
                .lock()
                .unwrap()
                .entry(wallet)
                .or_default() += 1;
            return None;
        }
        *self.reads.lock().unwrap().entry(wallet).or_default() += 1;
        Some(
            self.rows
                .lock()
                .unwrap()
                .get(&wallet)
                .cloned()
                .unwrap_or_default(),
        )
    }

    async fn drafts(&self, _: WalletId) -> Option<Vec<DraftKey>> {
        Some(Vec::new())
    }

    async fn chain_locked(&self, _: WalletId, _: &[u8; 32]) -> bool {
        false
    }
}

#[async_trait]
impl Resumer for Fake {
    async fn wallets(&self) -> Vec<WalletId> {
        let unloaded = self.unloaded.lock().unwrap().clone();
        self.rows
            .lock()
            .unwrap()
            .keys()
            .filter(|w| !unloaded.contains(w))
            .copied()
            .collect()
    }

    async fn resume(&self, wallet: WalletId, txid: Txid) -> Resumed {
        self.calls.lock().unwrap().push((wallet, txid));
        {
            let mut running = self.running.lock().unwrap();
            let n = running.entry((wallet, txid)).or_default();
            *n += 1;
            assert_eq!(*n, 1, "two resumes of one row at once");
        }
        struct Done<'a>(&'a Fake, WalletId, Txid);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                if let Some(n) = self.0.running.lock().unwrap().get_mut(&(self.1, self.2)) {
                    *n -= 1;
                }
            }
        }
        let _done = Done(self, wallet, txid);
        if self.hold.load(Ordering::SeqCst) {
            self.release.acquire().await.unwrap().forget();
        }
        let outcome = self
            .outcomes
            .lock()
            .unwrap()
            .get(&txid)
            .copied()
            .unwrap_or(Some(Resumed::Ran));
        outcome.expect("injected: the resume panicked")
    }
}

async fn setup() -> (Arc<LeaseTable>, Arc<Fake>, Arc<FakeJournal>) {
    let (t, _) = table();
    let j = with_journal(&t);
    let f = Arc::new(Fake::new());
    t.install_tracked(f.clone());
    t.install_resumer(f.clone());
    (t, f, j)
}

/// Lets spawned work run, for checks that something did not happen.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(30)).await;
}

/// The funding gate of `wallet` (§2a.3), on its last read rows.
fn gate(t: &LeaseTable, wallet: WalletId) -> Option<FundingBlock> {
    t.inspect(|i| funding_gate(i, wallet, REG))
}

fn row_block(n: u8) -> Option<FundingBlock> {
    Some(FundingBlock::Row(ArtifactId::from_txid(&txid(n))))
}

async fn idle(t: &LeaseTable, wallets: &[WalletId]) {
    until(|| wallets.iter().all(|w| !t.pass_running(*w))).await;
}

/// H6 part 2: nothing before SPV has started; then every tracked `Built`
/// or `Broadcast` row of every loaded wallet is resumed, and each stops
/// blocking its wallet's funding once its attempt returned.
#[tokio::test]
async fn resumes_tracked_rows_once_spv_started() {
    let (t, f, _j) = setup().await;
    f.set_rows(
        W,
        vec![
            row(1, 1, R::Built),
            row(2, 2, R::Broadcast),
            row(3, 3, R::ChainLocked),
            row(5, 5, R::Consumed),
        ],
    );
    f.set_rows(W2, vec![row(4, 4, R::Built)]);
    assert!(t.refresh_funding(W).await);
    assert!(t.refresh_funding(W2).await);
    assert_eq!(gate(&t, W), row_block(1));

    // Before SPV has started no trigger runs a pass.
    t.request_resume(None);
    t.request_resume(Some(W));
    t.wallet_opened(W);
    settle().await;
    assert!(f.calls().is_empty(), "no resume before SPV started");
    assert_eq!(gate(&t, W), row_block(1));

    t.spv_started();
    until(|| f.calls().len() == 3).await;
    idle(&t, &[W, W2]).await;
    let calls: HashSet<_> = f.calls().into_iter().collect();
    assert_eq!(
        calls,
        HashSet::from([(W, txid(1)), (W, txid(2)), (W2, txid(4))]),
        "only Built and Broadcast rows, of every loaded wallet"
    );
    assert_eq!(gate(&t, W), None, "note_resumed unblocks funding");
    assert_eq!(gate(&t, W2), None);
}

/// A wallet with no rows, or not loaded: the pass does nothing and ends.
#[tokio::test]
async fn a_pass_with_no_rows_is_harmless() {
    let (t, f, _j) = setup().await;
    f.set_rows(W, Vec::new());
    f.set_rows(W2, vec![row(4, 4, R::Built)]);
    f.unloaded.lock().unwrap().insert(W2);
    t.spv_started();
    until(|| f.reads(W) >= 1).await;
    idle(&t, &[W, W2]).await;
    // An unloaded wallet's pass, asked for directly, finds no rows.
    t.request_resume(Some(W2));
    until(|| f.unloaded_reads.lock().unwrap().get(&W2) == Some(&1)).await;
    idle(&t, &[W2]).await;
    settle().await;
    assert!(f.calls().is_empty());
}

/// A pass that panics ends, so the wallet's next trigger runs one again.
#[tokio::test]
async fn a_panicking_pass_does_not_block_later_passes() {
    let (t, f, _j) = setup().await;
    f.set_rows(W, vec![row(1, 1, R::Built)]);
    f.panic_rows.store(true, Ordering::SeqCst);
    t.spv_started();
    t.request_resume(Some(W));
    idle(&t, &[W]).await;
    f.panic_rows.store(false, Ordering::SeqCst);
    t.request_resume(Some(W));
    until(|| f.calls_of(W, 1) == 1).await;
}

/// A stop disarms the passes until SPV starts again.
#[tokio::test]
async fn no_pass_while_spv_is_stopped() {
    let (t, f, _j) = setup().await;
    f.set_rows(W, vec![row(1, 1, R::Built)]);
    t.spv_started();
    until(|| f.calls_of(W, 1) == 1).await;
    idle(&t, &[W]).await;
    t.spv_stopped();
    t.request_resume(Some(W));
    t.request_resume(None);
    t.wallet_opened(W);
    settle().await;
    assert_eq!(f.calls_of(W, 1), 1);
    t.spv_started();
    until(|| f.calls_of(W, 1) == 2).await;
}

/// The hub runs a pass for every loaded wallet on each SPV peers 0 → >0
/// transition, and on no other peer change.
#[tokio::test]
async fn peers_zero_to_some_reruns_the_pass() {
    let (t, f, _j) = setup().await;
    f.set_rows(W, vec![row(1, 1, R::Built)]);
    let hub = crate::events::SessionHub::new(DashNetwork::Regtest, Arc::new(Recorder::default()));
    hub.attach_leases(&t);
    t.spv_started();
    until(|| f.calls_of(W, 1) == 1).await;
    idle(&t, &[W]).await;

    let a: SocketAddr = "127.0.0.1:19999".parse().unwrap();
    let b: SocketAddr = "127.0.0.2:19999".parse().unwrap();
    hub.on_network_event(&NetworkEvent::PeerConnected { address: a });
    until(|| f.calls_of(W, 1) == 2).await;
    idle(&t, &[W]).await;
    // 1 → 2 peers is no transition.
    hub.on_network_event(&NetworkEvent::PeerConnected { address: b });
    settle().await;
    assert_eq!(f.calls_of(W, 1), 2);
    hub.on_network_event(&NetworkEvent::PeerDisconnected { address: a });
    hub.on_network_event(&NetworkEvent::PeerDisconnected { address: b });
    settle().await;
    assert_eq!(f.calls_of(W, 1), 2, "losing peers runs nothing");
    hub.on_network_event(&NetworkEvent::PeersUpdated {
        connected_count: 1,
        addresses: vec![b],
        best_height: None,
    });
    until(|| f.calls_of(W, 1) == 3).await;
}

/// One pass at a time per wallet: triggers during a pass schedule exactly
/// one more, which reads the rows again.
#[tokio::test]
async fn triggers_during_a_pass_coalesce_into_one_more() {
    let (t, f, _j) = setup().await;
    f.set_rows(W, vec![row(1, 1, R::Built)]);
    f.hold.store(true, Ordering::SeqCst);
    t.spv_started();
    until(|| f.calls_of(W, 1) == 1).await;
    t.request_resume(Some(W));
    t.request_resume(Some(W));
    t.request_resume(None);
    t.wallet_opened(W);
    settle().await;
    assert_eq!(f.calls_of(W, 1), 1, "no second pass while one runs");
    // A row that appeared meanwhile is resumed by the one more pass.
    f.set_rows(W, vec![row(1, 1, R::Built), row(2, 2, R::Broadcast)]);
    f.open();
    until(|| f.calls_of(W, 2) == 1).await;
    idle(&t, &[W]).await;
    settle().await;
    assert_eq!(f.calls_of(W, 1), 2, "exactly one more pass");
    assert_eq!(f.calls_of(W, 2), 1);
    assert_eq!(gate(&t, W), None);
}

/// A row stops blocking funding only once its own attempt returned; a
/// resume that did not run (or panicked) keeps it blocking until a later
/// pass runs it. A row the library could not settle stays tracked and is
/// resumed again by every pass.
#[tokio::test]
async fn a_failed_resume_keeps_the_row_blocking_until_a_later_pass() {
    let (t, f, _j) = setup().await;
    f.set_rows(
        W,
        vec![
            row(1, 1, R::Built),
            row(2, 2, R::Broadcast),
            row(3, 3, R::Built),
        ],
    );
    assert!(t.refresh_funding(W).await);
    f.set_outcome(2, Some(Resumed::NotRun));
    f.set_outcome(3, None);
    f.hold.store(true, Ordering::SeqCst);
    t.spv_started();
    until(|| f.calls().len() == 3).await;
    assert_eq!(gate(&t, W), row_block(1), "nothing noted while it runs");
    f.open();
    idle(&t, &[W]).await;
    assert_eq!(gate(&t, W), row_block(2), "row 1 noted, 2 not run");

    f.set_outcome(2, Some(Resumed::Ran));
    t.request_resume(Some(W));
    until(|| f.calls_of(W, 2) == 2).await;
    idle(&t, &[W]).await;
    assert_eq!(gate(&t, W), row_block(3), "row 3's resume panicked");

    f.set_outcome(3, Some(Resumed::Ran));
    t.request_resume(Some(W));
    until(|| f.calls_of(W, 3) == 3).await;
    idle(&t, &[W]).await;
    assert_eq!(gate(&t, W), None);
    assert_eq!(f.calls_of(W, 1), 3, "a still-tracked row is resumed again");
}

/// Close aborts a running pass (nothing is noted after it) and no pass
/// starts after close froze the table.
#[tokio::test]
async fn the_pass_stops_at_close() {
    let (t, f, _j) = setup().await;
    f.set_rows(W, vec![row(1, 1, R::Built)]);
    assert!(t.refresh_funding(W).await);
    f.hold.store(true, Ordering::SeqCst);
    t.spv_started();
    until(|| f.calls_of(W, 1) == 1).await;
    assert!(t.pass_running(W));
    t.freeze(RevokeCause::Close, Scope::All, false)
        .finish(status());
    t.abort_tasks();
    assert!(!t.pass_running(W));
    f.open();
    settle().await;
    assert_eq!(gate(&t, W), row_block(1), "nothing noted after close");
    t.spv_started();
    t.request_resume(Some(W));
    t.wallet_opened(W);
    settle().await;
    assert_eq!(f.calls_of(W, 1), 1, "no pass after close");
    assert!(!t.pass_running(W));
}

/// §8.6: closing a wallet ends its pass and its rows block its funding
/// again; opening it runs the pass again.
#[tokio::test]
async fn a_reopened_wallet_is_resumed_again() {
    let (t, f, _j) = setup().await;
    f.set_rows(W, vec![row(1, 1, R::Built)]);
    assert!(t.refresh_funding(W).await);
    t.spv_started();
    until(|| f.calls_of(W, 1) == 1).await;
    idle(&t, &[W]).await;
    assert_eq!(gate(&t, W), None);

    t.wallet_closed(W);
    assert_eq!(gate(&t, W), row_block(1), "blocks again once closed");
    t.wallet_opened(W);
    until(|| f.calls_of(W, 1) == 2).await;
    idle(&t, &[W]).await;
    assert_eq!(gate(&t, W), None);

    // Closed during a pass: the pass ends and notes nothing.
    f.hold.store(true, Ordering::SeqCst);
    t.request_resume(Some(W));
    until(|| f.calls_of(W, 1) == 3).await;
    t.wallet_closed(W);
    assert!(!t.pass_running(W));
    f.open();
    settle().await;
    assert_eq!(gate(&t, W), row_block(1));
}

/// §6.4: after the dispatch journal recovers from a write failure (the
/// next write succeeds), the pass runs again, once.
#[tokio::test]
async fn journal_recovery_reruns_the_pass() {
    let (t, f, j) = setup().await;
    f.set_rows(W, vec![row(1, 1, R::Built)]);
    t.spv_started();
    until(|| f.calls_of(W, 1) == 1).await;
    idle(&t, &[W]).await;
    let l = begin(&t, W, 10_000, 10_000, 0).await.unwrap();
    let step = |s: &str| CallStep::Resumable(format!("withdrawal/{s}/submit"));

    // The marker's write fails, and so does its `NotSent` row.
    j.fail_step.store(true, Ordering::SeqCst);
    j.fail_resolve.store(true, Ordering::SeqCst);
    assert!(l.call_permit_step("a", None, step("a")).await.is_err());
    settle().await;
    assert_eq!(f.calls_of(W, 1), 1, "a failure alone runs nothing");

    j.fail_step.store(false, Ordering::SeqCst);
    j.fail_resolve.store(false, Ordering::SeqCst);
    l.call_permit_step("b", None, step("b"))
        .await
        .unwrap()
        .finish(true);
    until(|| f.calls_of(W, 1) == 2).await;
    idle(&t, &[W]).await;
    l.call_permit_step("c", None, step("c"))
        .await
        .unwrap()
        .finish(true);
    settle().await;
    assert_eq!(f.calls_of(W, 1), 2, "only the first write after a failure");
}
