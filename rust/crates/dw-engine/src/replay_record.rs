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
    /// When a fork lowered the target: the highest checkpoint the wallet
    /// can have stored before it (0: no fork did). A stored checkpoint at
    /// it or below is the old chain's, which the replay does not reach, so
    /// the record stays until the wallet's checkpoint is back at it.
    pub forked_from: u32,
}

impl ReplayRecord {
    /// What a stored value this build cannot read stands for: a hold, which
    /// the resume raises to the wallet's birth height, so that a rescan
    /// from the birth height replays whatever it was.
    pub(crate) const UNREADABLE: Self = Self {
        target: 0,
        reached: 0,
        cancelled: true,
        forked_from: 0,
    };

    fn encode(&self) -> String {
        format!(
            "2:{}:{}:{}:{}",
            self.target,
            self.reached,
            u8::from(self.cancelled),
            self.forked_from
        )
    }

    /// Whether a wallet whose checkpoint is `live` is past the old chain's
    /// stored checkpoint, if a fork left one ([`Self::forked_from`]).
    fn past_fork(&self, live: u32) -> bool {
        live >= self.forked_from
    }

    /// A value this build cannot read, or one that starts its replay above
    /// its target, is [`Self::UNREADABLE`]. A version 1 value has no fork.
    fn decode(value: &str) -> Self {
        let parsed = (|| {
            let mut parts = value.split(':');
            let version = parts.next()?;
            if version != "1" && version != "2" {
                return None;
            }
            let target = parts.next()?.parse().ok()?;
            let reached = parts.next()?.parse().ok()?;
            let cancelled = match parts.next()? {
                "0" => false,
                "1" => true,
                _ => return None,
            };
            let forked_from = match version {
                "1" => 0,
                _ => parts.next()?.parse().ok()?,
            };
            (parts.next().is_none() && reached <= target).then_some(Self {
                target,
                reached,
                cancelled,
                forked_from,
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
                forked_from: 0,
            },
            Some(r) => Self {
                target: r.target.max(target),
                reached: r.reached.min(checkpoint),
                // As `WalletState::note_rewind`: a rewind to the hold or
                // below replays what the cancel skipped.
                cancelled: r.cancelled && checkpoint > r.reached,
                ..r
            },
        }
    }

    /// A hold below `floor`, the checkpoint a rescan from the wallet's
    /// birth height rewinds to, is that rescan's (as the resume holds it).
    fn lifted(self, floor: u32) -> Self {
        if self.cancelled && self.reached < floor {
            Self {
                reached: floor,
                target: self.target.max(floor),
                ..self
            }
        } else {
            self
        }
    }
}

#[derive(Debug)]
pub(crate) enum ReplayNote {
    /// The persister committed a store whose checkpoint is `height`.
    Committed { wallet: WalletId, height: u32 },
    /// A fork truncated the wallet to `height`; the highest checkpoint the
    /// hub had seen was `from`.
    Truncated {
        wallet: WalletId,
        height: u32,
        from: u32,
    },
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
    /// Truncations noted ([`ReplayNote::Truncated`]: height, and the
    /// highest checkpoint seen before) and not yet applied to the wallet's
    /// record, in order: every read applies them first, so a write that
    /// follows a note sees it.
    truncations: std::sync::Mutex<HashMap<WalletId, Vec<(u32, u32)>>>,
    /// The highest checkpoint a wallet can have stored before one of this
    /// session's forks, above the chain the fork left: a record written
    /// after it keeps it as `forked_from`, as one the fork truncated does.
    forked: std::sync::Mutex<HashMap<WalletId, u32>>,
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
            truncations: Default::default(),
            forked: Default::default(),
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
        match note {
            ReplayNote::Committed { wallet, height } => {
                let mut commits = self.commits.lock().unwrap_or_else(|p| p.into_inner());
                let entry = commits.entry(wallet).or_default();
                entry.0 = entry.0.max(height);
            }
            ReplayNote::Truncated {
                wallet,
                height,
                from,
            } => (self.truncations.lock().unwrap_or_else(|p| p.into_inner()))
                .entry(wallet)
                .or_default()
                .push((height, from)),
            #[cfg(test)]
            ReplayNote::Settled(_) => {}
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
    /// goes, unless the hub still holds it or the checkpoint is not back at
    /// the old chain's a fork left stored. A fork's record first takes the
    /// session's progress below that ([`Self::raise_forked`]), as its
    /// commits' notes may not be applied yet. The strict rule of
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
            if failed {
                continue;
            }
            let Some(live) = (wm.get_wallet_info(&wallet.0)).map(|i| i.core_wallet.synced_height())
            else {
                continue;
            };
            let record = match self.load(&mut records, wallet).await {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(error = %e, wallet = %wallet, "could not read the replay record");
                    continue;
                }
            };
            // The session's commits whose notes are not applied yet.
            let r = (self.raise(&mut records, wallet, record, committed.min(live))).await;
            if Self::held(hub, wallet) {
                continue;
            }
            if !r.cancelled
                && live >= r.target
                && committed >= r.target
                && r.past_fork(live)
                && let Err(e) = self.write(&mut records, wallet, None).await
            {
                tracing::warn!(error = %e, wallet = %wallet, "could not delete the finished replay record");
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
                    ReplayNote::Truncated { wallet, .. } => records.truncated(wallet).await,
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

    /// `wallet`'s record, with the truncations noted so far applied. Each
    /// stays noted until its record is stored: one that cannot be fails
    /// the read, and the next read applies it again.
    async fn load(
        &self,
        records: &mut HashMap<WalletId, Option<ReplayRecord>>,
        wallet: WalletId,
    ) -> Result<Option<ReplayRecord>, EngineError> {
        let mut record = self.load_stored(records, wallet).await?;
        while let Some((height, from)) = self.next_truncation(wallet) {
            record = (self.truncate_record(records, wallet, record, height, from)).await?;
            let mut noted = self.truncations.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(queue) = noted.get_mut(&wallet) {
                queue.remove(0);
                if queue.is_empty() {
                    noted.remove(&wallet);
                }
            }
        }
        Ok(record)
    }

    /// The first truncation of `wallet` noted and not yet applied.
    fn next_truncation(&self, wallet: WalletId) -> Option<(u32, u32)> {
        (self.truncations.lock().unwrap_or_else(|p| p.into_inner()))
            .get(&wallet)
            .and_then(|queue| queue.first().copied())
    }

    async fn load_stored(
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
    /// is done at `target`, merged with its record, whose hold is no lower
    /// than `floor` (the wallet's birth height less one). Returns the
    /// record.
    pub(crate) async fn note_rewind(
        &self,
        wallet: WalletId,
        checkpoint: u32,
        target: u32,
        floor: u32,
    ) -> Result<ReplayRecord, EngineError> {
        let mut records = self.records.lock().await;
        let old = self.load(&mut records, wallet).await?;
        let record = self.with_forks(
            wallet,
            ReplayRecord::merged(old.map(|r| r.lifted(floor)), checkpoint, target),
        );
        if old != Some(record) {
            self.write(&mut records, wallet, Some(record)).await?;
        }
        Ok(record)
    }

    /// A read of checkpoint `live` below the highest seen, `target`: a
    /// rewind the record does not explain (none, or one below its start)
    /// is stored from `floor()`, the lowest checkpoint it can have had. A
    /// cancel's hold stays unless `live` is at it or below: a read above it
    /// is no sign that the rewind went back that far. A hold is no lower
    /// than `birth`, the wallet's birth height less one.
    pub(crate) async fn note_seen(
        &self,
        wallet: WalletId,
        live: u32,
        target: u32,
        birth: u32,
        floor: impl FnOnce() -> u32,
    ) -> Result<(), EngineError> {
        let mut records = self.records.lock().await;
        let stored = self.load(&mut records, wallet).await?;
        let old = stored.map(|r| r.lifted(birth));
        let checkpoint = match old {
            Some(r) if live >= r.reached => r.reached,
            _ => floor().min(live),
        };
        let record = self.with_forks(
            wallet,
            ReplayRecord {
                cancelled: old.is_some_and(|r| r.cancelled && live > r.reached),
                ..ReplayRecord::merged(old, checkpoint, target)
            },
        );
        if stored != Some(record) {
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
            None => self.with_forks(
                wallet,
                ReplayRecord {
                    target,
                    reached,
                    cancelled: true,
                    forked_from: 0,
                },
            ),
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
        (self.truncations.lock().unwrap_or_else(|p| p.into_inner())).remove(&wallet);
        (self.forked.lock().unwrap_or_else(|p| p.into_inner())).remove(&wallet);
    }

    /// `record` with this session's forks of `wallet` (`forked`).
    fn with_forks(&self, wallet: WalletId, record: ReplayRecord) -> ReplayRecord {
        let forked = (self.forked.lock().unwrap_or_else(|p| p.into_inner()))
            .get(&wallet)
            .copied()
            .unwrap_or(0);
        ReplayRecord {
            forked_from: record.forked_from.max(forked),
            ..record
        }
    }

    /// A committed store at `height`: above the target, with the wallet's
    /// checkpoint at the target or above, the replay is done and stored. No
    /// batch made before a rewind is above the checkpoint the rewind read
    /// under its lock (`stale`), which a new rewind's target is not below,
    /// so a store above both follows the replay. A fork's lower target, or
    /// a resume's below the stored checkpoint, waits for a commit above
    /// `stale` or a clean close; a fork's, also for the checkpoint back at
    /// the old chain's stored one. Nothing ends after a failed store of the
    /// wallet (its later checkpoints are not stored), or while the hub
    /// holds it.
    async fn committed(
        &self,
        wallet: WalletId,
        height: u32,
        manager: &Weak<Manager>,
        hub: &Weak<SessionHub>,
    ) {
        if let Some(live) = Self::live(manager, wallet).await {
            self.raise_forked(wallet, height, live).await;
        }
        // Read first: the truncations noted so far raise `stale`.
        let due = |r: Option<ReplayRecord>| r.filter(|r| !r.cancelled && height > r.target);
        match self.get(wallet).await {
            Ok(r) if due(r).is_some() => {}
            Ok(_) => return,
            Err(e) => {
                tracing::warn!(error = %e, wallet = %wallet, "could not read the replay record");
                return;
            }
        }
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
            Ok(r) if due(r).is_some_and(|r| live >= r.target && r.past_fork(live)) => {
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

    /// A commit at `height` of a replay whose target a fork lowered, above
    /// that target and at most the old chain's height: the stored
    /// checkpoint cannot tell the new chain's progress from the old one's,
    /// so the target follows the commit, up to the wallet's checkpoint: the
    /// persister lags the wallet, so a store above it is the old chain's,
    /// stored late. Applied to the record as it was before the truncations
    /// noted after the commit, as the notes came. A commit above `live`,
    /// the wallet's checkpoint, after a fork is the old chain's: the
    /// highest checkpoint stored before the fork (`forked_from`, and
    /// `forked`) is at least its height.
    async fn raise_forked(&self, wallet: WalletId, height: u32, live: u32) {
        let lift = height > live && {
            let mut forked = self.forked.lock().unwrap_or_else(|p| p.into_inner());
            forked
                .get_mut(&wallet)
                .map(|f| *f = (*f).max(height))
                .is_some()
        };
        let mut records = self.records.lock().await;
        match self.load_stored(&mut records, wallet).await {
            Ok(Some(r)) => {
                let r = self.raise(&mut records, wallet, r, height.min(live)).await;
                if (lift || r.forked_from > 0) && height > live && r.forked_from < height {
                    let lifted = ReplayRecord {
                        forked_from: height,
                        ..r
                    };
                    if let Err(e) = self.write(&mut records, wallet, Some(lifted)).await {
                        tracing::warn!(error = %e, wallet = %wallet, "could not store the old chain's checkpoint of a fork");
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, wallet = %wallet, "could not read the replay record")
            }
        }
    }

    /// `record`, with its target raised to `height` when a fork lowered it
    /// below and `height` is at most the old chain's ([`Self::raise_forked`]).
    async fn raise(
        &self,
        records: &mut HashMap<WalletId, Option<ReplayRecord>>,
        wallet: WalletId,
        record: ReplayRecord,
        height: u32,
    ) -> ReplayRecord {
        if record.forked_from == 0 || record.target >= height || height > record.forked_from {
            return record;
        }
        let raised = ReplayRecord {
            target: height,
            ..record
        };
        match self.write(records, wallet, Some(raised)).await {
            Ok(()) => raised,
            Err(e) => {
                tracing::warn!(error = %e, wallet = %wallet, "could not store the replay's progress after a fork");
                record
            }
        }
    }

    /// `wallet`'s filter checkpoint, read under the manager's lock (taken
    /// before the records').
    async fn live(manager: &Weak<Manager>, wallet: WalletId) -> Option<u32> {
        let manager = manager.upgrade()?;
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        (wm.get_wallet_info(&wallet.0)).map(|i| i.core_wallet.synced_height())
    }

    /// Applies every truncation noted so far to its wallet's record. One
    /// that cannot be stored stays noted.
    pub(crate) async fn apply_truncations(&self) {
        let wallets: Vec<WalletId> = (self.truncations.lock().unwrap_or_else(|p| p.into_inner()))
            .keys()
            .copied()
            .collect();
        for wallet in wallets {
            self.truncated(wallet).await;
        }
    }

    /// Applies the truncations noted for `wallet` (its record's read does).
    async fn truncated(&self, wallet: WalletId) {
        let mut records = self.records.lock().await;
        if let Err(e) = self.load(&mut records, wallet).await {
            tracing::error!(error = %e, wallet = %wallet, "could not store the replay record of a fork: a reopen may scan from the old chain's checkpoint");
        }
    }

    /// A fork truncated the wallet to `height`, whose highest checkpoint
    /// the hub had seen was `from`: the replay is done at the new chain's
    /// height, not above it. The old target stays the bound of the
    /// in-session clear, and the record keeps the highest checkpoint the
    /// wallet can have stored before the fork, which a resume does not
    /// replay to; so does a record written later this session. With no
    /// record, a stored checkpoint above the fork gets one from the fork:
    /// the store keeps the old chain's checkpoint, which a reopen would
    /// otherwise scan from. Returns the record, or the failed write's
    /// error.
    async fn truncate_record(
        &self,
        records: &mut HashMap<WalletId, Option<ReplayRecord>>,
        wallet: WalletId,
        old: Option<ReplayRecord>,
        height: u32,
        from: u32,
    ) -> Result<Option<ReplayRecord>, EngineError> {
        let committed = (self.commits.lock().unwrap_or_else(|p| p.into_inner()))
            .get(&wallet)
            .map_or(0, |c| c.0);
        let stored = old.map_or(0, |r| r.target).max(committed).max(from);
        if stored > height {
            let mut forked = self.forked.lock().unwrap_or_else(|p| p.into_inner());
            let entry = forked.entry(wallet).or_default();
            *entry = (*entry).max(stored);
        }
        let Some(old) = old else {
            if stored <= height {
                return Ok(None);
            }
            let record = ReplayRecord {
                target: height,
                reached: height,
                cancelled: false,
                forked_from: stored,
            };
            self.note_stale(wallet, stored);
            self.write(records, wallet, Some(record)).await?;
            return Ok(Some(record));
        };
        self.note_stale(wallet, old.target);
        let record = ReplayRecord {
            target: old.target.min(height),
            reached: old.reached.min(height),
            forked_from: if stored > height {
                old.forked_from.max(stored)
            } else {
                old.forked_from
            },
            ..old
        };
        if record != old {
            self.write(records, wallet, Some(record)).await?;
        }
        Ok(Some(record))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_2_record_reads_and_a_version_1_record_still_does() {
        let forked = ReplayRecord::decode("2:900:399:0:1000");
        assert_eq!(
            (forked.target, forked.reached, forked.cancelled),
            (900, 399, false)
        );
        let old = ReplayRecord::decode("1:900:399:1");
        assert_eq!((old.target, old.reached, old.cancelled), (900, 399, true));
    }

    #[test]
    fn records_round_trip_and_overlapping_rewinds_merge() {
        let r = ReplayRecord {
            target: 1_000,
            reached: 399,
            cancelled: true,
            forked_from: 0,
        };
        assert_eq!(ReplayRecord::decode(&r.encode()), r);
        assert_eq!(
            ReplayRecord::decode("3:1000:399:0:0"),
            ReplayRecord::UNREADABLE
        );
        assert_eq!(
            ReplayRecord::decode("2:1000:399:0"),
            ReplayRecord::UNREADABLE,
            "a version 2 value without its fork"
        );
        assert_eq!(
            ReplayRecord::decode("1:1000:1001:0"),
            ReplayRecord::UNREADABLE,
            "a replay that starts above its target"
        );
        assert_eq!(ReplayRecord::decode("1:1000:1000:0").reached, 1_000);
        let forked = ReplayRecord {
            target: 900,
            forked_from: 1_000,
            cancelled: false,
            ..r
        };
        assert_eq!(forked.encode(), "2:900:399:0:1000");
        assert_eq!(ReplayRecord::decode(&forked.encode()), forked);
        assert_eq!(
            ReplayRecord::decode("1:900:399:1"),
            ReplayRecord {
                target: 900,
                reached: 399,
                cancelled: true,
                forked_from: 0,
            },
            "a version 1 value has no fork"
        );
        assert_eq!(
            ReplayRecord::decode("2:1000:1001:0:0"),
            ReplayRecord::UNREADABLE
        );
        let merged = ReplayRecord::merged(Some(r), 799, 1_200);
        assert_eq!((merged.target, merged.reached), (1_200, 399));
        assert!(merged.cancelled, "a rewind above the hold keeps it");
        assert!(!ReplayRecord::merged(Some(r), 399, 1_000).cancelled);
        assert_eq!(
            ReplayRecord::merged(Some(forked), 299, 950).forked_from,
            1_000,
            "a rewind keeps the fork"
        );
        let unreadable = ReplayRecord::UNREADABLE.lifted(499);
        assert_eq!((unreadable.reached, unreadable.target), (499, 499));
        assert!(
            !ReplayRecord::merged(Some(unreadable), 499, 1_000).cancelled,
            "a rescan from the birth height ends an unreadable record's hold"
        );
    }
}
