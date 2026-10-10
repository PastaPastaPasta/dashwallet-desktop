//! Mode B's derived funding statuses (E0-04 §2a.5, H15) and its funding
//! gate (§2a.3).
//!
//! Without the platform PR a funding call has no registered journal entry,
//! so its status is derived from three sources:
//! - the library's tracked asset-lock rows ([`TrackedLocks`]), matched to a
//!   funding step by its marker's funding key and by the draft's own key,
//!   which `app.sqlite` keeps (so a lost journal still finds the row);
//! - the step's marker and the resolution written with it (the fence's
//!   [`CallStepInfo`]), with Repair's self-spends;
//! - how each call ended (its `Sent` bundle or its `NotSent` row).
//!
//! **Where it runs.** The rows are read outside J ([`LeaseTable::refresh_funding`])
//! and kept per wallet in J as a snapshot; every derivation is a pure
//! function ([`derive_funding`]) over that snapshot and the fence, so it
//! runs in any J step without I/O. It runs:
//! - at load: the session refreshes every loaded wallet once the journal is
//!   loaded, and `dispatch_status` refreshes a wallet it has no snapshot of;
//! - on every tracked-rows change: the changeset tap (E0-06) sees each
//!   `asset_locks` changeset once the persister accepted it, and requests a
//!   refresh. The tap is the cheapest reliable hook: every row write goes
//!   through `queue_asset_lock_changeset`, so through the tap, and the
//!   pump would only see them debounced and coalesced with other domains.
//!   The tap runs inside the library's store call, so the refresh is
//!   spawned, never awaited there;
//! - on a Core changeset of a wallet with one of Repair's self-spends not
//!   yet seen ChainLocked (the tap again), so the self-spend's ChainLock
//!   (§2a.5 row 1) is read when the wallet records it;
//! - at each funding call's end (in its permit's J step, then a refresh for
//!   the row the call may have left), at its permit's J step, and when a
//!   call step's `NotSent` row lands.
//!
//! Refreshes are serialized (one async mutex per table), so a slower read
//! never overwrites a newer snapshot, and a read that a wallet's removal
//! overtook is dropped. The last derived status of each funding step is
//! cached; a move to `Sent` or `NotSent` from an earlier derivation emits
//! `EngineEvent::DispatchResolved` (after J, through the table's effects).
//! A step's first derivation is its baseline and emits nothing (otherwise
//! every load would emit for every finished funding), unless its status
//! was reported before the wallet's first snapshot (`MaybeSent`, when
//! `dispatch_status` could not read the rows): that answer is the
//! baseline, so the first snapshot's resolution emits.
//!
//! The statuses read are as fresh as the last snapshot: `dispatch_status`
//! and [`LeaseTable::funding_status`] read the cached one (a changeset's
//! refresh is spawned, so a read right after it may precede it). A gate
//! that decides on them refreshes first, as [`NetworkSession::funding_call_permit`]
//! does.
//!
//! **Rows of earlier fundings.** The funding key (type, identity index,
//! account) is not unique per step: every top-up of an identity carries
//! the same one, and a `Consumed` row stays. The J step that marks a step
//! records the rows its marker's key matches then as foreign to the step:
//! they precede its call, so they are not its lock (the gate admitted the
//! step, so none of them read as its own). That holds in its process; after
//! a restart such rows count again, so a marked step whose key an earlier
//! funding shares reads `Sent` or `WillBeSent` (fail closed: never `None`
//! or `NotSent`). A row matching the draft's own key is never foreign, so
//! a lost journal still finds the draft's row. A step's `NotSent` row
//! drops its funding key with its marker, so later rows match it only by
//! the draft's key.
//!
//! **The funding gate** (§2a.3, in the J step that would take a funding
//! call's permit): the wallet's rows must be known; the step must have no
//! standing marker and read `None` or `NotSent` (retry only on `NotSent`,
//! §16.6, so a possibly-sent funding blocks a second build of its step:
//! the withheld-T1 trace); and no tracked `Built` or `Broadcast` row of the
//! wallet may stand that the catch-up has not resumed this session
//! ([`LeaseTable::note_resumed`]), or the library's coin selection could
//! pick its inputs (F7).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use dashcore::{OutPoint, Txid};
use platform_wallet::{AssetLockFundingType, AssetLockStatus, TrackedAssetLock};

use super::call::{CallPermit, CallStep, FundingKey, FundingQuote};
use super::fence::{CallStepInfo, CallStepStatus};
use super::resume::{Resumed, Resumer};
use super::table::{Effects, Entry, Inner, LeaseTable};
use super::{ArtifactId, Lease, LeaseError};
use crate::platform::errors::PlatformError;
use crate::platform::flows::{DispatchResolution, DispatchResolved, DispatchState};
use crate::{EngineEvent, NetworkSession, WalletId};

/// A tracked row's status (platform-wallet `AssetLockStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackedStatus {
    Built,
    Broadcast,
    InstantSendLocked,
    ChainLocked,
    Consumed,
    RecoveredFromChain,
}

impl From<&AssetLockStatus> for TrackedStatus {
    fn from(s: &AssetLockStatus) -> Self {
        // Exhaustive: a new library status fails to build until it is read.
        match s {
            AssetLockStatus::Built => Self::Built,
            AssetLockStatus::Broadcast => Self::Broadcast,
            AssetLockStatus::InstantSendLocked => Self::InstantSendLocked,
            AssetLockStatus::ChainLocked => Self::ChainLocked,
            AssetLockStatus::Consumed => Self::Consumed,
            AssetLockStatus::RecoveredFromChain => Self::RecoveredFromChain,
        }
    }
}

impl TrackedStatus {
    /// The wallet has seen the lock reach a proof, or adopted it from the
    /// chain: it is out (§2a.5 row 2).
    fn proved(self) -> bool {
        matches!(
            self,
            Self::InstantSendLocked | Self::ChainLocked | Self::Consumed | Self::RecoveredFromChain
        )
    }

    /// Signed, and resent by the catch-up and the deferred resume (§2a.5
    /// row 3); its inputs may still be selectable (F7).
    pub(crate) fn pending(self) -> bool {
        matches!(self, Self::Built | Self::Broadcast)
    }
}

/// One tracked asset-lock row, as the derivation reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedRow {
    pub txid: Txid,
    pub key: FundingKey,
    pub status: TrackedStatus,
    /// The lock transaction's inputs.
    pub inputs: Vec<OutPoint>,
}

impl TrackedRow {
    pub fn of(lock: &TrackedAssetLock) -> Self {
        Self {
            txid: lock.out_point.txid,
            key: FundingKey::of_tracked(lock),
            status: TrackedStatus::from(&lock.status),
            inputs: lock
                .transaction
                .input
                .iter()
                .map(|i| i.previous_output)
                .collect(),
        }
    }
}

/// A draft's own funding key (§2a.5, review Opus r3 F-2), from
/// `app.sqlite`. It names no account: DP1-02's draft does not store one, so
/// a row of any account with the draft's type and identity index matches
/// (more matches only fail closed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftKey {
    /// The draft's funding step, `registration/<draft>/funding`.
    pub step: String,
    pub funding_type: AssetLockFundingType,
    pub identity_index: u32,
}

impl DraftKey {
    /// A `dp_registration` row's key: an identity registration at its
    /// identity index.
    pub fn registration(draft: i64, identity_index: u32) -> Self {
        Self {
            step: format!("registration/{draft}/funding"),
            funding_type: AssetLockFundingType::IdentityRegistration,
            identity_index,
        }
    }

    fn matches(&self, key: &FundingKey) -> bool {
        self.funding_type == key.funding_type && self.identity_index == key.identity_index
    }
}

/// Where Mode B reads its evidence (§2a.5): platform-wallet's tracked rows,
/// the drafts' own keys and the ChainLock of Repair's self-spends. The real
/// one is [`SessionTracked`]; tests use a fake.
#[async_trait]
pub(crate) trait TrackedLocks: Send + Sync + 'static {
    /// The wallet's tracked rows; `None` when the wallet is not loaded.
    async fn rows(&self, wallet: WalletId) -> Option<Vec<TrackedRow>>;
    /// The wallet's drafts' funding keys; `None` when they cannot be read
    /// (the snapshot is then not replaced: a missed draft key could read
    /// `None` for a funded draft).
    async fn drafts(&self, wallet: WalletId) -> Option<Vec<DraftKey>>;
    /// Whether the wallet holds `txid` (display order, as the journal keeps
    /// it) ChainLocked.
    async fn chain_locked(&self, wallet: WalletId, txid: &[u8; 32]) -> bool;
}

/// What the derivation knows of one wallet, read outside J.
#[derive(Debug, Default, Clone)]
pub(crate) struct Snapshot {
    pub(crate) rows: Vec<TrackedRow>,
    pub(crate) drafts: Vec<DraftKey>,
    /// Repair's self-spends (display order) found ChainLocked.
    pub(crate) locked_spends: HashSet<[u8; 32]>,
}

/// Mode B's state in J.
#[derive(Debug, Default)]
pub(crate) struct ModeB {
    snapshots: HashMap<WalletId, Snapshot>,
    /// The last derived status of each funding step.
    derived: HashMap<(WalletId, String), Option<DispatchState>>,
    /// Tracked `Built`/`Broadcast` rows the catch-up resumed this session.
    resumed: HashSet<(WalletId, Txid)>,
    /// Per marked step, the rows its marker's key matched when it was
    /// marked in this process: earlier fundings', never its own.
    foreign: HashMap<(WalletId, String), HashSet<Txid>>,
    /// Bumped by every wallet removal: a refresh read before it is dropped.
    epoch: u64,
    /// The catch-up's resume passes (H6 part 2).
    pub(super) passes: super::resume::Passes,
}

impl ModeB {
    /// A removed wallet's rows and statuses.
    pub(super) fn forget(&mut self, wallet: &WalletId) {
        self.snapshots.remove(wallet);
        self.derived.retain(|(w, _), _| w != wallet);
        self.resumed.retain(|(w, _)| w != wallet);
        self.foreign.retain(|(w, _), _| w != wallet);
        self.passes.end(wallet);
        self.epoch += 1;
    }

    /// Tracked row `txid` of `wallet` was resumed this session.
    pub(super) fn note_resumed(&mut self, wallet: WalletId, txid: Txid) {
        self.resumed.insert((wallet, txid));
    }

    /// A closed wallet (§8.6): its pass ends and its rows count as not
    /// resumed, until the pass that runs when it is opened again.
    pub(super) fn wallet_closed(&mut self, wallet: &WalletId) {
        self.resumed.retain(|(w, _)| w != wallet);
        self.passes.end(wallet);
    }
}

/// Why a funding call does not start (§2a.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FundingBlock {
    /// The wallet's tracked rows were never read this session.
    RowsUnknown,
    /// A tracked `Built` or `Broadcast` row the catch-up has not resumed:
    /// its lock will be sent, and its inputs may still be selectable.
    Row(ArtifactId),
    /// The step's own funding has committed: a marker stands, or its status
    /// is not `None`/`NotSent`. A new funding needs a new step.
    Step(Option<DispatchState>),
}

/// §2a.5's table for one funding step, first matching row wins:
/// - `status`: the step's journal state (its marker and resolution);
/// - `rows`: the statuses of the tracked rows matching the step (by the
///   marker's key or the draft's own); `None` when the wallet's rows are
///   unknown, which never reads `None` or `NotSent`;
/// - `self_spend_locked`: one of Repair's self-spends answering the step's
///   marker is ChainLocked.
///
/// A call that returned Ok (`status` `Sent`) and a row with a proof come
/// first: the lock reached a proof, so a ChainLocked self-spend cannot have
/// consumed its inputs, and §2a.5's first row ("a self-spend of one of the
/// lock's inputs") does not apply. Repair's self-spend spends every coin of
/// the funding accounts and the inputs of every tracked `Built` and
/// `Broadcast` row (commit 7's obligation), so once ChainLocked it reads
/// `NotSent` over such a row too.
pub(crate) fn derive_funding(
    status: CallStepStatus,
    rows: Option<&[TrackedStatus]>,
    self_spend_locked: bool,
) -> Option<DispatchState> {
    if status == CallStepStatus::Sent || rows.is_some_and(|r| r.iter().any(|s| s.proved())) {
        return Some(DispatchState::Sent);
    }
    let Some(rows) = rows else {
        return Some(DispatchState::MaybeSent);
    };
    if self_spend_locked {
        return Some(DispatchState::NotSent);
    }
    if rows.iter().any(|s| s.pending()) {
        return Some(DispatchState::WillBeSent);
    }
    match status {
        // A definite resolution, written with the marker.
        CallStepStatus::NotSent => Some(DispatchState::NotSent),
        // The call never started: no lock was ever registered (§16.6).
        CallStepStatus::None => None,
        CallStepStatus::Marked { .. } | CallStepStatus::Sent => Some(DispatchState::MaybeSent),
    }
}

/// Whether tracked row `r` matches `step`: by the draft's own key, or by
/// the marker's unless the row preceded the marker (`foreign`).
fn matches_step(
    s: &Snapshot,
    step: &str,
    info: &CallStepInfo,
    foreign: Option<&HashSet<Txid>>,
    r: &TrackedRow,
) -> bool {
    s.drafts.iter().any(|d| d.step == step && d.matches(&r.key))
        || (info.funding.as_ref() == Some(&r.key) && !foreign.is_some_and(|f| f.contains(&r.txid)))
}

/// `step`'s funding status now (J).
pub(super) fn funding_status(i: &Inner, wallet: WalletId, step: &str) -> Option<DispatchState> {
    let info = i.fence.call_step_info(&wallet, step);
    let snap = i.mode_b.snapshots.get(&wallet);
    let foreign = i.mode_b.foreign.get(&(wallet, step.to_owned()));
    let rows: Option<Vec<TrackedStatus>> = snap.map(|s| {
        s.rows
            .iter()
            .filter(|r| matches_step(s, step, &info, foreign, r))
            .map(|r| r.status)
            .collect()
    });
    let locked = snap.is_some_and(|s| info.self_spends.iter().any(|t| s.locked_spends.contains(t)));
    derive_funding(info.status, rows.as_deref(), locked)
}

/// §16.5's Mode B `funds_committed` of a lease: one of its funding steps
/// reads anything but `None` or `NotSent`.
pub(super) fn funds_committed(i: &Inner, e: &Entry) -> bool {
    e.funding_steps.iter().any(|s| {
        matches!(
            funding_status(i, e.wallet, s),
            Some(DispatchState::WillBeSent | DispatchState::MaybeSent | DispatchState::Sent)
        )
    })
}

/// The funding gate (§2a.3), in the J step that would take `step`'s permit.
pub(super) fn funding_gate(i: &Inner, wallet: WalletId, step: &str) -> Option<FundingBlock> {
    let snap = match i.mode_b.snapshots.get(&wallet) {
        Some(s) => s,
        None => return Some(FundingBlock::RowsUnknown),
    };
    let marked = matches!(
        i.fence.call_step_status(&wallet, step),
        CallStepStatus::Marked { .. } | CallStepStatus::Sent
    );
    let status = funding_status(i, wallet, step);
    if marked || !matches!(status, None | Some(DispatchState::NotSent)) {
        return Some(FundingBlock::Step(status));
    }
    snap.rows
        .iter()
        .find(|r| r.status.pending() && !i.mode_b.resumed.contains(&(wallet, r.txid)))
        .map(|r| FundingBlock::Row(ArtifactId::from_txid(&r.txid)))
}

/// The J step that marks `step` under `key` records the wallet's rows
/// `key` matches now as foreign to it: an earlier funding's (the module
/// doc's "Rows of earlier fundings"). A new mark replaces the set.
pub(super) fn note_marked(i: &mut Inner, wallet: WalletId, step: &str, key: FundingKey) {
    let rows = i.mode_b.snapshots.get(&wallet).map(|s| {
        s.rows
            .iter()
            .filter(|r| r.key == key)
            .map(|r| r.txid)
            .collect::<HashSet<_>>()
    });
    i.mode_b
        .foreign
        .insert((wallet, step.to_owned()), rows.unwrap_or_default());
}

/// Re-derives every funding step of `wallet` (J): those with a marker, its
/// drafts', those derived before and those its leases took permits for.
/// A move to `Sent` or `NotSent` from an earlier derivation emits
/// `DispatchResolved`; a lease whose step moved emits `LeaseChanged`
/// (`funds_committed`). Nothing before the wallet's first snapshot.
pub(super) fn rederive(t: &LeaseTable, i: &mut Inner, fx: &mut Effects, wallet: WalletId) {
    let Some(snap) = i.mode_b.snapshots.get(&wallet) else {
        return;
    };
    let mut steps: BTreeSet<String> = snap.drafts.iter().map(|d| d.step.clone()).collect();
    steps.extend(i.fence.funding_steps_of(&wallet));
    steps.extend(
        i.mode_b
            .derived
            .keys()
            .filter(|(w, _)| *w == wallet)
            .map(|(_, s)| s.clone()),
    );
    for e in i.leases.values().filter(|e| e.wallet == wallet) {
        steps.extend(e.funding_steps.iter().cloned());
    }
    let mut moved = Vec::new();
    for step in steps {
        let now = funding_status(i, wallet, &step);
        let before = i.mode_b.derived.insert((wallet, step.clone()), now);
        if before == Some(now) {
            continue;
        }
        let resolution = match now {
            Some(DispatchState::Sent) => Some(DispatchResolution::Sent),
            Some(DispatchState::NotSent) => Some(DispatchResolution::NotSent),
            _ => None,
        };
        if let (Some(_), Some(resolution)) = (before, resolution) {
            fx.emit(EngineEvent::DispatchResolved {
                network: t.network.clone(),
                resolved: DispatchResolved {
                    wallet_id: wallet.to_string(),
                    artifact: step.clone(),
                    resolution,
                },
            });
        }
        moved.push(step);
    }
    if moved.is_empty() {
        return;
    }
    for (id, e) in &i.leases {
        if e.wallet == wallet && e.funding_steps.iter().any(|s| moved.contains(s)) {
            fx.changed(*id);
        }
    }
}

impl LeaseTable {
    /// Installs the session's evidence source (once).
    pub(crate) fn install_tracked(&self, source: Arc<dyn TrackedLocks>) {
        let _ = self.tracked.set(source);
    }

    /// Spawns [`Self::refresh_funding`] for `wallet`: the changeset tap and
    /// a funding call's end, neither of which may wait for it.
    pub(crate) fn request_refresh(self: &Arc<Self>, wallet: WalletId) {
        if self.tracked.get().is_none() {
            return;
        }
        let table = Arc::clone(self);
        self.rt.spawn(async move {
            table.refresh_funding(wallet).await;
        });
    }

    /// Reads `wallet`'s tracked rows, drafts and self-spend ChainLocks
    /// outside J, then stores them and re-derives the wallet's funding
    /// steps in one J step. Serialized per table, so a snapshot never goes
    /// back. `false` when nothing could be read (no source, the wallet is
    /// not loaded, its drafts are unreadable): the old snapshot stays.
    pub(crate) async fn refresh_funding(self: &Arc<Self>, wallet: WalletId) -> bool {
        let Some(source) = self.tracked.get().cloned() else {
            return false;
        };
        let _serial = self.refresh.lock().await;
        let epoch = self.with_j(|i, _| i.mode_b.epoch);
        let Some(rows) = source.rows(wallet).await else {
            return false;
        };
        let Some(drafts) = source.drafts(wallet).await else {
            tracing::warn!(wallet_id = %wallet, "could not read the registration drafts");
            return false;
        };
        let spends = self.with_j(|i, _| i.fence.self_spends_of(&wallet));
        let mut locked_spends = HashSet::new();
        for txid in spends {
            if source.chain_locked(wallet, &txid).await {
                locked_spends.insert(txid);
            }
        }
        self.commit_read(
            wallet,
            Snapshot {
                rows,
                drafts,
                locked_spends,
            },
            Some(epoch),
        )
    }

    /// Stores `wallet`'s snapshot and re-derives (J).
    #[cfg(test)]
    pub(crate) fn commit_snapshot(&self, wallet: WalletId, snapshot: Snapshot) {
        self.commit_read(wallet, snapshot, None);
    }

    /// Stores `wallet`'s snapshot and re-derives (J), unless a wallet was
    /// removed since `epoch` (the read may be of the removed wallet).
    fn commit_read(&self, wallet: WalletId, snapshot: Snapshot, epoch: Option<u64>) -> bool {
        self.with_j(|i, fx| {
            if epoch.is_some_and(|e| e != i.mode_b.epoch) {
                return false;
            }
            i.mode_b.snapshots.insert(wallet, snapshot);
            fx.notify();
            rederive(self, i, fx, wallet);
            true
        })
    }

    /// Whether a Core change of `wallet` may be the ChainLock of one of
    /// Repair's self-spends: one stands that its snapshot has not seen
    /// ChainLocked (no snapshot yet: the load's refresh reads them).
    pub(crate) fn awaits_self_spend_lock(&self, wallet: WalletId) -> bool {
        self.with_j(|i, _| {
            let Some(snap) = i.mode_b.snapshots.get(&wallet) else {
                return false;
            };
            i.fence
                .self_spends_of(&wallet)
                .iter()
                .any(|t| !snap.locked_spends.contains(t))
        })
    }

    /// Whether `wallet`'s tracked rows were read this session.
    pub(crate) fn has_tracked_snapshot(&self, wallet: WalletId) -> bool {
        self.with_j(|i, _| i.mode_b.snapshots.contains_key(&wallet))
    }

    /// A Mode B funding step's status (§2a.5, §16.6): `registration/<draft>/funding`
    /// or `topup/<id>/funding`. Before the wallet's rows are known it reads
    /// `MaybeSent` (or `Sent`), never `None` or `NotSent`.
    /// A status reported before the wallet's first snapshot is the
    /// baseline that snapshot's resolution moves from (`DispatchResolved`).
    pub fn funding_status(&self, wallet: WalletId, step: &str) -> Option<DispatchState> {
        self.with_j(|i, _| {
            let now = funding_status(i, wallet, step);
            if !i.mode_b.snapshots.contains_key(&wallet) {
                i.mode_b
                    .derived
                    .entry((wallet, step.to_owned()))
                    .or_insert(now);
            }
            now
        })
    }

    /// The catch-up's resume pass (H6 part 2) resumed tracked row `txid`
    /// this session: it no longer blocks the wallet's funding calls.
    pub fn note_resumed(&self, wallet: WalletId, txid: Txid) {
        self.with_j(|i, fx| {
            i.mode_b.note_resumed(wallet, txid);
            fx.notify();
        });
    }
}

/// The real evidence source: the session's platform wallets and its
/// `app.sqlite`. Holds the session weakly (the session owns the table).
pub(crate) struct SessionTracked {
    pub(crate) session: Weak<NetworkSession>,
}

#[async_trait]
impl TrackedLocks for SessionTracked {
    async fn rows(&self, wallet: WalletId) -> Option<Vec<TrackedRow>> {
        let session = self.session.upgrade()?;
        let w = session.wallet(&wallet).await.ok()?;
        // Not `list_tracked_locks`: it reads a wallet whose info is gone
        // (a removal in flight) as no rows, which would read `None` for a
        // funded step. Here that is unknown: the snapshot stays.
        let wm = w.wallet_manager().read().await;
        let info = wm.get_wallet_info(&w.wallet_id())?;
        Some(
            info.tracked_asset_locks
                .values()
                .map(TrackedRow::of)
                .collect(),
        )
    }

    async fn drafts(&self, wallet: WalletId) -> Option<Vec<DraftKey>> {
        let session = self.session.upgrade()?;
        let id = wallet.to_string();
        let drafts = session
            .appdb_op(move |db| db.registration_identity_indices(&id))
            .await
            .ok()?;
        Some(
            drafts
                .into_iter()
                .map(|(draft, index)| DraftKey::registration(draft, index))
                .collect(),
        )
    }

    async fn chain_locked(&self, wallet: WalletId, txid: &[u8; 32]) -> bool {
        use dashcore::hashes::Hash;
        let Some(session) = self.session.upgrade() else {
            return false;
        };
        let Ok(w) = session.wallet(&wallet).await else {
            return false;
        };
        let mut bytes = *txid;
        bytes.reverse();
        let txid = Txid::from_byte_array(bytes);
        // Not `state()`, which panics on a wallet whose info is gone.
        let wm = w.wallet_manager().read().await;
        wm.get_wallet_info(&w.wallet_id())
            .is_some_and(|info| crate::coins::tx_chain_locked(&info.core_wallet, &txid))
    }
}

#[async_trait]
impl Resumer for SessionTracked {
    async fn wallets(&self) -> Vec<WalletId> {
        let Some(session) = self.session.upgrade() else {
            return Vec::new();
        };
        let Ok(manager) = session.manager() else {
            return Vec::new();
        };
        manager
            .wallet_ids()
            .await
            .into_iter()
            .map(WalletId)
            .collect()
    }

    async fn resume(&self, wallet: WalletId, txid: Txid) -> Resumed {
        use platform_wallet::PlatformWalletError as E;
        // The session is not kept across the library call: only its wallet.
        let w = {
            let Some(session) = self.session.upgrade() else {
                return Resumed::NotRun;
            };
            match session.wallet(&wallet).await {
                Ok(w) => w,
                Err(_) => return Resumed::NotRun,
            }
        };
        let out_point = {
            let wm = w.wallet_manager().read().await;
            let Some(info) = wm.get_wallet_info(&w.wallet_id()) else {
                return Resumed::NotRun;
            };
            info.tracked_asset_locks
                .keys()
                .find(|op| op.txid == txid)
                .copied()
        };
        // No longer tracked: nothing to resume, and nothing to wait for.
        let Some(out_point) = out_point else {
            return Resumed::Ran;
        };
        match w.asset_locks().resume_asset_lock(&out_point, None).await {
            Ok(_) => Resumed::Ran,
            Err(E::WalletNotFound(_)) => Resumed::NotRun,
            Err(E::AssetLockNotTracked(_)) => Resumed::Ran,
            Err(e) => {
                // The attempt ran (pin semantics); the row stays tracked and
                // the next pass resumes it again.
                tracing::info!(wallet_id = %wallet, %txid, error = %e, "the catch-up's resume of a tracked asset lock did not settle it");
                Resumed::Ran
            }
        }
    }
}

impl NetworkSession {
    /// E0-04 §2a.3's `fee_bound_worst` for `wallet_id`: the fee of an asset
    /// lock spending every coin of its funding accounts
    /// ([`crate::fees::asset_lock_fee_bound`]). Computed outside J; the
    /// funding gate only compares.
    pub(crate) async fn fee_bound_worst(&self, wallet_id: WalletId) -> Result<u64, PlatformError> {
        let wallet = self.wallet(&wallet_id).await?;
        Ok(crate::fees::asset_lock_fee_bound(
            crate::coins::funding_coin_count(&wallet).await,
        ))
    }

    /// Takes a Mode B funding call's permit (§2a.3): the quote is `amount`
    /// plus the wallet's `fee_bound_worst`, the wallet's tracked rows are
    /// read first, so the gate decides on fresh rows, and the step's marker
    /// is durable when it returns. DP1-02's registration and top-up
    /// funding take their permits here.
    ///
    /// Of a refused step, the code names the step: `broadcast_unknown`
    /// (`MaybeSent`), `will_be_sent` (`WillBeSent`), or `invalid_argument`
    /// (a funded step, or a marker a self-spend resolved: a new funding
    /// needs a new draft).
    pub async fn funding_call_permit(
        &self,
        lease: &Lease,
        label: impl Into<String>,
        amount: u64,
        step: String,
        key: FundingKey,
    ) -> Result<CallPermit, PlatformError> {
        let wallet = lease.wallet();
        let fee_bound_worst = self.fee_bound_worst(wallet).await?;
        if !self.leases.refresh_funding(wallet).await {
            return Err(LeaseError::FundingBlocked(FundingBlock::RowsUnknown).into());
        }
        let quote = FundingQuote {
            amount,
            fee_bound_worst,
        };
        lease
            .call_permit_step(
                label,
                Some(quote.into()),
                CallStep::Funding {
                    step: step.clone(),
                    key,
                },
            )
            .await
            .map_err(|e| match e {
                LeaseError::FundingBlocked(FundingBlock::Step(Some(DispatchState::MaybeSent))) => {
                    PlatformError::BroadcastUnknown { artifact: step }
                }
                LeaseError::FundingBlocked(FundingBlock::Step(Some(DispatchState::WillBeSent))) => {
                    PlatformError::WillBeSent { artifact: step }
                }
                e => e.into(),
            })
    }
}
