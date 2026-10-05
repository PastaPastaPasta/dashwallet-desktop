//! Per-domain event debouncing (reviews H2 and M4).
//!
//! Callbacks from dash-spv and platform-wallet only mark what changed. A
//! background task ([`EventPump::run`]) turns the marks into host events,
//! per domain at most once every [`MIN_INTERVAL`] (≤ 4 Hz):
//!
//! - the first change after a quiet period is delivered at once (leading edge);
//! - changes inside the interval are merged into a dirty set and delivered
//!   when the interval ends (trailing edge), so the last change of a burst is
//!   never dropped.
//!
//! Domains are `Sync`, `Balances` (dirty wallet set) and `History` (dirty
//! txids per wallet). Each has its own interval clock.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use dashcore::Txid;
use tokio::sync::{Notify, watch};
use tokio::time::{Instant, MissedTickBehavior};

use crate::WalletId;

/// Minimum spacing between two events of one domain.
pub const MIN_INTERVAL: Duration = Duration::from_millis(250);
/// Period of [`PumpTarget::tick`] (stall detection).
pub(crate) const TICK_INTERVAL: Duration = Duration::from_secs(1);

/// Transactions of one wallet whose history changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TxidSet {
    /// Re-query everything (e.g. a rescan or a removal).
    All,
    Some(BTreeSet<Txid>),
}

impl TxidSet {
    fn merge(&mut self, txids: Option<&[Txid]>) {
        match (self, txids) {
            (TxidSet::All, _) => {}
            (this, None) => *this = TxidSet::All,
            (TxidSet::Some(set), Some(new)) => set.extend(new.iter().copied()),
        }
    }
}

#[derive(Debug, Default)]
struct Pending {
    sync: bool,
    balances: BTreeSet<WalletId>,
    history: BTreeMap<WalletId, TxidSet>,
}

/// Receives the merged changes of one flush. Implemented by the session.
pub(crate) trait PumpTarget: Send + Sync {
    fn flush_sync(&self) -> impl Future<Output = ()> + Send;
    fn flush_balances(&self, wallets: BTreeSet<WalletId>);
    fn flush_history(&self, changes: BTreeMap<WalletId, TxidSet>);
    /// Called every [`TICK_INTERVAL`].
    fn tick(&self);
}

#[derive(Default)]
pub(crate) struct EventPump {
    pending: Mutex<Pending>,
    notify: Notify,
}

impl EventPump {
    fn with_pending(&self, f: impl FnOnce(&mut Pending)) {
        f(&mut self.pending.lock().unwrap_or_else(|p| p.into_inner()));
        self.notify.notify_one();
    }

    pub(crate) fn mark_sync(&self) {
        self.with_pending(|p| p.sync = true);
    }

    pub(crate) fn mark_balances(&self, wallet: WalletId) {
        self.with_pending(|p| {
            p.balances.insert(wallet);
        });
    }

    /// `txids = None` asks hosts to reload the whole history.
    pub(crate) fn mark_history(&self, wallet: WalletId, txids: Option<&[Txid]>) {
        self.with_pending(|p| {
            p.history
                .entry(wallet)
                .or_insert_with(|| TxidSet::Some(BTreeSet::new()))
                .merge(txids);
        });
    }

    /// Delivers marks to `target` until `stop` turns `true` (or its sender
    /// is dropped).
    pub(crate) async fn run<T: PumpTarget>(&self, target: &T, mut stop: watch::Receiver<bool>) {
        let mut last_sync: Option<Instant> = None;
        let mut last_balances: Option<Instant> = None;
        let mut last_history: Option<Instant> = None;
        let mut deadline: Option<Instant> = None;
        let mut ticker = tokio::time::interval(TICK_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            let sleep = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return;
                    }
                    continue;
                }
                _ = self.notify.notified() => {}
                _ = sleep => {}
                _ = ticker.tick() => target.tick(),
            }

            let now = Instant::now();
            deadline = None;
            let due = |last: Option<Instant>| match last {
                Some(t) if now.duration_since(t) < MIN_INTERVAL => Err(t + MIN_INTERVAL),
                _ => Ok(()),
            };
            let mut push_deadline = |at: Instant| {
                deadline = Some(deadline.map_or(at, |d: Instant| d.min(at)));
            };

            let (sync, balances, history) = {
                let p = self.pending.lock().unwrap_or_else(|p| p.into_inner());
                (p.sync, !p.balances.is_empty(), !p.history.is_empty())
            };
            if sync {
                match due(last_sync) {
                    Ok(()) => {
                        self.take(|p| std::mem::take(&mut p.sync));
                        target.flush_sync().await;
                        last_sync = Some(now);
                    }
                    Err(at) => push_deadline(at),
                }
            }
            if balances {
                match due(last_balances) {
                    Ok(()) => {
                        let wallets = self.take(|p| std::mem::take(&mut p.balances));
                        target.flush_balances(wallets);
                        last_balances = Some(now);
                    }
                    Err(at) => push_deadline(at),
                }
            }
            if history {
                match due(last_history) {
                    Ok(()) => {
                        let changes = self.take(|p| std::mem::take(&mut p.history));
                        target.flush_history(changes);
                        last_history = Some(now);
                    }
                    Err(at) => push_deadline(at),
                }
            }
        }
    }

    fn take<R>(&self, f: impl FnOnce(&mut Pending) -> R) -> R {
        f(&mut self.pending.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use dashcore::hashes::Hash;

    use super::*;

    #[derive(Default)]
    struct Recorder {
        /// (elapsed ms since start, what)
        log: Mutex<Vec<(u128, String)>>,
        start: Mutex<Option<Instant>>,
        value: Mutex<u32>,
    }

    impl Recorder {
        fn push(&self, what: String) {
            let start = self.start.lock().unwrap().expect("start set");
            self.log
                .lock()
                .unwrap()
                .push((Instant::now().duration_since(start).as_millis(), what));
        }
        fn log(&self) -> Vec<(u128, String)> {
            self.log.lock().unwrap().clone()
        }
    }

    impl PumpTarget for Recorder {
        fn flush_sync(&self) -> impl Future<Output = ()> + Send {
            let v = *self.value.lock().unwrap();
            self.push(format!("sync={v}"));
            async {}
        }
        fn flush_balances(&self, wallets: BTreeSet<WalletId>) {
            self.push(format!("balances={}", wallets.len()));
        }
        fn flush_history(&self, changes: BTreeMap<WalletId, TxidSet>) {
            let n: usize = changes
                .values()
                .map(|s| match s {
                    TxidSet::All => usize::MAX,
                    TxidSet::Some(s) => s.len(),
                })
                .sum();
            self.push(format!("history={n}"));
        }
        fn tick(&self) {}
    }

    fn txid(n: u8) -> Txid {
        Txid::from_byte_array([n; 32])
    }

    async fn settle() {
        // Let the pump task observe notifications at the current instant.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    /// Review H2: a burst is delivered at most once per interval, and the
    /// last value of the burst always arrives (trailing edge).
    #[tokio::test(start_paused = true)]
    async fn burst_is_throttled_and_keeps_the_trailing_edge() {
        let pump = Arc::new(EventPump::default());
        let rec = Arc::new(Recorder::default());
        *rec.start.lock().unwrap() = Some(Instant::now());
        let (stop_tx, stop_rx) = watch::channel(false);
        let task = {
            let (pump, rec) = (Arc::clone(&pump), Arc::clone(&rec));
            tokio::spawn(async move { pump.run(&*rec, stop_rx).await })
        };

        // 100 updates over ~990 ms, the last one with value 99.
        for i in 0..100u32 {
            *rec.value.lock().unwrap() = i;
            pump.mark_sync();
            settle().await;
            tokio::time::advance(Duration::from_millis(10)).await;
        }
        // Quiet period: the trailing flush must still deliver value 99.
        tokio::time::advance(Duration::from_millis(600)).await;
        settle().await;

        let log = rec.log();
        let syncs: Vec<_> = log.iter().filter(|(_, w)| w.starts_with("sync=")).collect();
        assert!(syncs.len() >= 4 && syncs.len() <= 6, "{syncs:?}");
        for pair in syncs.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= 250, "flushes too close: {pair:?}");
        }
        assert_eq!(syncs.first().unwrap().1, "sync=0", "leading edge");
        assert_eq!(syncs.last().unwrap().1, "sync=99", "trailing edge kept");

        stop_tx.send(true).unwrap();
        task.await.unwrap();
    }

    /// Review M4: many wallet events become one event per domain and
    /// interval, carrying the union of what changed.
    #[tokio::test(start_paused = true)]
    async fn wallet_marks_are_coalesced_per_domain() {
        let pump = Arc::new(EventPump::default());
        let rec = Arc::new(Recorder::default());
        *rec.start.lock().unwrap() = Some(Instant::now());
        let (stop_tx, stop_rx) = watch::channel(false);
        let task = {
            let (pump, rec) = (Arc::clone(&pump), Arc::clone(&rec));
            tokio::spawn(async move { pump.run(&*rec, stop_rx).await })
        };
        let wallet = WalletId([7; 32]);

        // First mark flushes at once.
        pump.mark_history(wallet, Some(&[txid(0)]));
        pump.mark_balances(wallet);
        settle().await;
        // 300 events inside one interval (one block with many transactions).
        for n in 1..=200u8 {
            pump.mark_history(wallet, Some(&[txid(n)]));
            pump.mark_balances(wallet);
        }
        pump.mark_balances(WalletId([8; 32]));
        settle().await;
        tokio::time::advance(MIN_INTERVAL).await;
        settle().await;

        let log = rec.log();
        let history: Vec<_> = log
            .iter()
            .filter(|(_, w)| w.starts_with("history="))
            .collect();
        let balances: Vec<_> = log
            .iter()
            .filter(|(_, w)| w.starts_with("balances="))
            .collect();
        assert_eq!(history.len(), 2, "{log:?}");
        assert_eq!(history[0].1, "history=1");
        assert_eq!(history[1].1, "history=200");
        assert_eq!(balances.len(), 2, "{log:?}");
        assert_eq!(balances[1].1, "balances=2");

        // A reload-all mark absorbs txid marks.
        pump.mark_history(wallet, None);
        pump.mark_history(wallet, Some(&[txid(1)]));
        tokio::time::advance(MIN_INTERVAL).await;
        settle().await;
        assert_eq!(
            rec.log().last().unwrap().1,
            format!("history={}", usize::MAX)
        );

        stop_tx.send(true).unwrap();
        task.await.unwrap();
    }
}
