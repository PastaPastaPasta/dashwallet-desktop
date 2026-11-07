//! The replay record (0.1.x row 58). A rewind of a wallet's filter
//! checkpoint credits again, from their funding blocks, the coins it spent
//! above the new checkpoint until their spends replay, and the stored coin
//! keeps that credit (`UPSERT_UTXO_SQL` resets the spent mark) while the
//! stored checkpoint does not go down (monotonic-max). A quit mid-replay
//! would leave the coin spendable for good. So each rewind first stores a
//! record (app.sqlite, the wallet's `replay` setting), which goes once a
//! store above its target is committed, or at a clean close with the
//! replay back and stored; a session that opens with a record rewinds the
//! wallet to it again and refuses its spends until the replay is back
//! (`NetworkSession::resume_replay`).

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use dw_appdb::AppDb;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use tokio::sync::{Mutex, mpsc};

use crate::coins::appdb_job;
use crate::events::SessionHub;
use crate::session::Manager;
use crate::{EngineError, WalletId};

const KEY: &str = "replay";

/// A wallet's unfinished replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReplayRecord {
    /// The highest checkpoint the wallet had when it was rewound: the
    /// replay is done once a store above it is committed.
    pub target: u32,
    /// The lowest checkpoint a rewind lowered it to, where a resume starts.
    /// It does not follow the replay: a committed checkpoint at or below
    /// the target may be one a batch made before the rewind (the
    /// persister's event channel lags the wallet).
    pub reached: u32,
    /// The user cancelled the replay: it is not resumed, and spends are
    /// refused until a rewind to `reached` or below.
    pub cancelled: bool,
}

impl ReplayRecord {
    /// What a stored value this build cannot read stands for: a hold, which
    /// the resume raises to the wallet's birth height, so that a rescan
    /// from the birth height replays whatever it was.
    pub(crate) const UNREADABLE: Self = Self {
        target: 0,
        reached: 0,
        cancelled: true,
    };

    fn encode(&self) -> String {
        format!(
            "1:{}:{}:{}",
            self.target,
            self.reached,
            u8::from(self.cancelled)
        )
    }

    /// A value this build cannot read, or one that starts its replay above
    /// its target, is [`Self::UNREADABLE`].
    fn decode(value: &str) -> Self {
        let parsed = (|| {
            let mut parts = value.split(':');
            if parts.next()? != "1" {
                return None;
            }
            let target = parts.next()?.parse().ok()?;
            let reached = parts.next()?.parse().ok()?;
            let cancelled = match parts.next()? {
                "0" => false,
                "1" => true,
                _ => return None,
            };
            (parts.next().is_none() && reached <= target).then_some(Self {
                target,
                reached,
                cancelled,
            })
        })();
        parsed.unwrap_or_else(|| {
            tracing::warn!(
                value,
                "unreadable replay record: spends wait for a rescan from the birth height"
            );
            Self::UNREADABLE
        })
    }

    /// `old` with a rewind to `checkpoint` of a replay done at `target`:
    /// overlapping rewinds replay from the lowest to the highest.
    pub(crate) fn merged(old: Option<Self>, checkpoint: u32, target: u32) -> Self {
        match old {
            None => Self {
                target,
                reached: checkpoint,
                cancelled: false,
            },
            Some(r) => Self {
                target: r.target.max(target),
                reached: r.reached.min(checkpoint),
                // As `WalletState::note_rewind`: a rewind to the hold or
                // below replays what the cancel skipped.
                cancelled: r.cancelled && checkpoint > r.reached,
            },
        }
    }
}

#[derive(Debug)]
pub(crate) enum ReplayNote {
    /// The persister committed a store whose checkpoint is `height`.
    Committed { wallet: WalletId, height: u32 },
    /// A fork truncated the wallet to `height`.
    Truncated { wallet: WalletId, height: u32 },
    /// Tests: answered once every note before it is applied.
    #[cfg(test)]
    Settled(tokio::sync::oneshot::Sender<()>),
}

/// The session's replay records: every read and write of one goes through
/// `records`, so each wallet has one writer.
pub(crate) struct ReplayRecords {
    appdb: Arc<AppDb>,
    /// What app.sqlite holds, for the wallets read so far (`None`: none).
    records: Mutex<HashMap<WalletId, Option<ReplayRecord>>>,
    notes: mpsc::UnboundedSender<ReplayNote>,
    receiver: std::sync::Mutex<Option<mpsc::UnboundedReceiver<ReplayNote>>>,
    /// This session's commits: each wallet's highest committed checkpoint,
    /// and whether a store of its Core changes failed (the persistence
    /// adapter then holds its stored checkpoint back for the session).
    commits: std::sync::Mutex<HashMap<WalletId, (u32, bool)>>,
    /// The highest checkpoint a batch made before one of this session's
    /// rewinds of the wallet can carry: the in-session clear needs a
    /// commit above it as well as above the target, which a fork or a
    /// resume can leave lower.
    stale: std::sync::Mutex<HashMap<WalletId, u32>>,
    /// Tests: a close clears nothing, as a crash.
    #[cfg(test)]
    pub(crate) crash_on_close: std::sync::atomic::AtomicBool,
    /// Tests: the next recorded rewind fails after its record is stored,
    /// as a quit there would leave it.
    #[cfg(test)]
    pub(crate) crash_after_write: std::sync::atomic::AtomicBool,
    /// Tests: every write fails.
    #[cfg(test)]
    pub(crate) fail_writes: std::sync::atomic::AtomicBool,
    /// Tests: every read fails.
    #[cfg(test)]
    pub(crate) fail_reads: std::sync::atomic::AtomicBool,
}

impl ReplayRecords {
    pub(crate) fn new(appdb: Arc<AppDb>) -> Arc<Self> {
        let (notes, receiver) = mpsc::unbounded_channel();
        Arc::new(Self {
            appdb,
            records: Mutex::new(HashMap::new()),
            notes,
            receiver: std::sync::Mutex::new(Some(receiver)),
            commits: Default::default(),
            stale: Default::default(),
            #[cfg(test)]
            crash_on_close: Default::default(),
            #[cfg(test)]
            crash_after_write: Default::default(),
            #[cfg(test)]
            fail_writes: Default::default(),
            #[cfg(test)]
            fail_reads: Default::default(),
        })
    }

    /// Queues `note` for the task [`Self::start`] runs.
    pub(crate) fn note(&self, note: ReplayNote) {
        if let ReplayNote::Committed { wallet, height } = note {
            let mut commits = self.commits.lock().unwrap_or_else(|p| p.into_inner());
            let entry = commits.entry(wallet).or_default();
            entry.0 = entry.0.max(height);
        }
        let _ = self.notes.send(note);
    }

    /// A store of `wallet`'s Core changes failed.
    pub(crate) fn note_store_failed(&self, wallet: WalletId) {
        let mut commits = self.commits.lock().unwrap_or_else(|p| p.into_inner());
        commits.entry(wallet).or_default().1 = true;
    }

    /// A batch made before a rewind of `wallet` read now can carry
    /// checkpoint `height`.
    pub(crate) fn note_stale(&self, wallet: WalletId, height: u32) {
        let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        let entry = stale.entry(wallet).or_default();
        *entry = (*entry).max(height);
    }

    /// Whether the hub still waits on `wallet`'s replay: a cancel's hold,
    /// a load not yet resumed, or a rescan not back. No record of such a
    /// wallet ends.
    fn held(hub: &SessionHub, wallet: WalletId) -> bool {
        hub.wallet_state(&wallet)
            .is_some_and(|s| s.replay_cancelled.is_some() || s.loading)
            || (hub.rescan().as_ref()).is_some_and(|r| r.replaying(&wallet))
    }

    /// After a clean shutdown of the manager, whose persistence adapter
    /// committed every event before it returned: a replay back at its
    /// target, with a store at the target or above committed and none of
    /// the wallet's failed this session, is stored whole, and its record
    /// goes, unless the hub still holds it. The strict rule of
    /// [`Self::committed`] is for a running session, whose adapter may
    /// still hold a batch made before the rewind.
    pub(crate) async fn close(&self, manager: &Manager, hub: &SessionHub) {
        #[cfg(test)]
        if self
            .crash_on_close
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let commits = self
            .commits
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let mut records = self.records.lock().await;
        for (wallet, (committed, failed)) in commits {
            if failed || Self::held(hub, wallet) {
                continue;
            }
            let Some(live) = (wm.get_wallet_info(&wallet.0)).map(|i| i.core_wallet.synced_height())
            else {
                continue;
            };
            match self.load(&mut records, wallet).await {
                Ok(Some(r)) if !r.cancelled && live >= r.target && committed >= r.target => {
                    if let Err(e) = self.write(&mut records, wallet, None).await {
                        tracing::warn!(error = %e, wallet = %wallet, "could not delete the finished replay record");
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, wallet = %wallet, "could not read the replay record")
                }
            }
        }
    }

    /// Applies the notes until the records are dropped (once).
    pub(crate) fn start(self: &Arc<Self>, manager: Weak<Manager>, hub: Weak<SessionHub>) {
        let receiver = self
            .receiver
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        let Some(mut receiver) = receiver else {
            return;
        };
        let records = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(note) = receiver.recv().await {
                let Some(records) = records.upgrade() else {
                    break;
                };
                match note {
                    ReplayNote::Committed { wallet, height } => {
                        records.committed(wallet, height, &manager, &hub).await
                    }
                    ReplayNote::Truncated { wallet, height } => {
                        records.truncated(wallet, height).await
                    }
                    #[cfg(test)]
                    ReplayNote::Settled(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
    }

    /// Tests: waits until the notes queued so far are applied.
    #[cfg(test)]
    pub(crate) async fn settled(&self) {
        let (done, applied) = tokio::sync::oneshot::channel();
        self.note(ReplayNote::Settled(done));
        let _ = applied.await;
    }

    async fn load(
        &self,
        records: &mut HashMap<WalletId, Option<ReplayRecord>>,
        wallet: WalletId,
    ) -> Result<Option<ReplayRecord>, EngineError> {
        #[cfg(test)]
        if self.fail_reads.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::Storage("replay record read failed".into()));
        }
        if let Some(record) = records.get(&wallet) {
            return Ok(*record);
        }
        let (db, scope) = (Arc::clone(&self.appdb), wallet.to_string());
        let record = appdb_job(db, move |db| db.setting(&scope, KEY))
            .await?
            .map(|value| ReplayRecord::decode(&value));
        records.insert(wallet, record);
        Ok(record)
    }

    async fn write(
        &self,
        records: &mut HashMap<WalletId, Option<ReplayRecord>>,
        wallet: WalletId,
        record: Option<ReplayRecord>,
    ) -> Result<(), EngineError> {
        let (db, scope) = (Arc::clone(&self.appdb), wallet.to_string());
        let value = record.map(|r| r.encode());
        #[cfg(test)]
        let written = if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst) {
            Err(EngineError::Storage("replay record write failed".into()))
        } else {
            appdb_job(db, move |db| {
                db.set_setting_durable(&scope, KEY, value.as_deref())
            })
            .await
        };
        #[cfg(not(test))]
        let written = appdb_job(db, move |db| {
            db.set_setting_durable(&scope, KEY, value.as_deref())
        })
        .await;
        match written {
            Ok(()) => {
                records.insert(wallet, record);
                Ok(())
            }
            Err(e) => {
                // Read again next time: the write may or may not have landed.
                records.remove(&wallet);
                Err(e)
            }
        }
    }

    pub(crate) async fn get(&self, wallet: WalletId) -> Result<Option<ReplayRecord>, EngineError> {
        let mut records = self.records.lock().await;
        self.load(&mut records, wallet).await
    }

    /// Stores, durably, a rewind of `wallet` to `checkpoint` whose replay
    /// is done at `target`, merged with its record. Returns the record.
    pub(crate) async fn note_rewind(
        &self,
        wallet: WalletId,
        checkpoint: u32,
        target: u32,
    ) -> Result<ReplayRecord, EngineError> {
        let mut records = self.records.lock().await;
        let old = self.load(&mut records, wallet).await?;
        let record = ReplayRecord::merged(old, checkpoint, target);
        if old != Some(record) {
            self.write(&mut records, wallet, Some(record)).await?;
        }
        Ok(record)
    }

    /// A read of checkpoint `live` below the highest seen, `target`: a
    /// rewind the record does not explain (none, or one below its start)
    /// is stored from `floor()`, the lowest checkpoint it can have had. A
    /// cancel's hold stays unless `live` is at it or below: a read above it
    /// is no sign that the rewind went back that far.
    pub(crate) async fn note_seen(
        &self,
        wallet: WalletId,
        live: u32,
        target: u32,
        floor: impl FnOnce() -> u32,
    ) -> Result<(), EngineError> {
        let mut records = self.records.lock().await;
        let old = self.load(&mut records, wallet).await?;
        let checkpoint = match old {
            Some(r) if live >= r.reached => r.reached,
            _ => floor().min(live),
        };
        let record = ReplayRecord {
            cancelled: old.is_some_and(|r| r.cancelled && live > r.reached),
            ..ReplayRecord::merged(old, checkpoint, target)
        };
        if old != Some(record) {
            self.write(&mut records, wallet, Some(record)).await?;
        }
        Ok(())
    }

    /// The user cancelled `wallet`'s replay, which had got to `reached` of
    /// `target`: its record becomes a hold. Returns what it replaces.
    pub(crate) async fn note_cancel(
        &self,
        wallet: WalletId,
        reached: u32,
        target: u32,
    ) -> Result<Option<ReplayRecord>, EngineError> {
        let mut records = self.records.lock().await;
        let old = self.load(&mut records, wallet).await?;
        let record = match old {
            Some(r) => ReplayRecord {
                cancelled: true,
                ..r
            },
            None => ReplayRecord {
                target,
                reached,
                cancelled: true,
            },
        };
        if old != Some(record) {
            self.write(&mut records, wallet, Some(record)).await?;
        }
        Ok(old)
    }

    /// Puts back `wallet`'s record as it was before a cancel that did not
    /// happen.
    pub(crate) async fn restore(
        &self,
        wallet: WalletId,
        record: Option<ReplayRecord>,
    ) -> Result<(), EngineError> {
        let mut records = self.records.lock().await;
        if self.load(&mut records, wallet).await? != record {
            self.write(&mut records, wallet, record).await?;
        }
        Ok(())
    }

    /// The resume found `wallet`'s hold below `floor`, the lowest
    /// checkpoint a rescan from its birth height rewinds to: the hold is
    /// stored at `floor`, so that rescan ends it. Returns the hold.
    pub(crate) async fn raise_hold(
        &self,
        wallet: WalletId,
        floor: u32,
    ) -> Result<u32, EngineError> {
        let mut records = self.records.lock().await;
        match self.load(&mut records, wallet).await? {
            Some(r) if r.cancelled && r.reached < floor => {
                let raised = ReplayRecord {
                    reached: floor,
                    target: r.target.max(floor),
                    ..r
                };
                self.write(&mut records, wallet, Some(raised)).await?;
                Ok(floor)
            }
            Some(r) => Ok(r.reached),
            None => Ok(floor),
        }
    }

    /// Forgets a removed wallet (its app.sqlite rows are deleted with it).
    pub(crate) async fn forget(&self, wallet: WalletId) {
        self.records.lock().await.remove(&wallet);
    }

    /// A committed store at `height`: above the target, with the wallet's
    /// checkpoint at the target or above, the replay is done and stored. No
    /// batch made before a rewind is above the checkpoint the rewind read
    /// under its lock (`stale`), which a new rewind's target is not below,
    /// so a store above both follows the replay. A fork's lower target, or
    /// a resume's below the stored checkpoint, waits for a commit above
    /// `stale` or a clean close. Nothing ends after a failed store of the
    /// wallet (its later checkpoints are not stored), or while the hub
    /// holds it.
    async fn committed(
        &self,
        wallet: WalletId,
        height: u32,
        manager: &Weak<Manager>,
        hub: &Weak<SessionHub>,
    ) {
        let (committed, stale) = (
            self.commits
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&wallet)
                .copied(),
            self.stale
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&wallet)
                .copied(),
        );
        if committed.is_some_and(|(_, failed)| failed) || height <= stale.unwrap_or(0) {
            return;
        }
        if hub.upgrade().is_none_or(|hub| Self::held(&hub, wallet)) {
            return;
        }
        let due = |r: Option<ReplayRecord>| r.filter(|r| !r.cancelled && height > r.target);
        match self.get(wallet).await {
            Ok(r) if due(r).is_some() => {}
            Ok(_) => return,
            Err(e) => {
                tracing::warn!(error = %e, wallet = %wallet, "could not read the replay record");
                return;
            }
        }
        let Some(manager) = manager.upgrade() else {
            return;
        };
        // The manager's lock before the records', as a rewind takes them.
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let Some(live) = (wm.get_wallet_info(&wallet.0)).map(|i| i.core_wallet.synced_height())
        else {
            return;
        };
        let mut records = self.records.lock().await;
        match self.load(&mut records, wallet).await {
            Ok(r) if due(r).is_some_and(|r| live >= r.target) => {
                if let Err(e) = self.write(&mut records, wallet, None).await {
                    tracing::warn!(error = %e, wallet = %wallet, "could not delete the finished replay record");
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, wallet = %wallet, "could not read the replay record")
            }
        }
    }

    /// A fork truncated the wallet to `height`: the replay is done at the
    /// new chain's height, not above it. The old target stays the bound of
    /// the in-session clear.
    async fn truncated(&self, wallet: WalletId, height: u32) {
        let mut records = self.records.lock().await;
        let old = match self.load(&mut records, wallet).await {
            Ok(Some(r)) => r,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, wallet = %wallet, "could not read the replay record");
                return;
            }
        };
        self.note_stale(wallet, old.target);
        let record = ReplayRecord {
            target: old.target.min(height),
            reached: old.reached.min(height),
            ..old
        };
        if record != old
            && let Err(e) = self.write(&mut records, wallet, Some(record)).await
        {
            tracing::warn!(error = %e, wallet = %wallet, "could not store the truncated replay record");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_and_overlapping_rewinds_merge() {
        let r = ReplayRecord {
            target: 1_000,
            reached: 399,
            cancelled: true,
        };
        assert_eq!(ReplayRecord::decode(&r.encode()), r);
        assert_eq!(
            ReplayRecord::decode("2:1000:399:0"),
            ReplayRecord::UNREADABLE
        );
        assert_eq!(
            ReplayRecord::decode("1:1000:1001:0"),
            ReplayRecord::UNREADABLE,
            "a replay that starts above its target"
        );
        assert_eq!(ReplayRecord::decode("1:1000:1000:0").reached, 1_000);
        let merged = ReplayRecord::merged(Some(r), 799, 1_200);
        assert_eq!((merged.target, merged.reached), (1_200, 399));
        assert!(merged.cancelled, "a rewind above the hold keeps it");
        assert!(!ReplayRecord::merged(Some(r), 399, 1_000).cancelled);
    }
}
