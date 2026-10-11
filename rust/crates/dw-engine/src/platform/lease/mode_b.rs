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
//! never overwrites a newer snapshot, and a read that its wallet's removal
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
//! **Unattributed rows** (review H-1). The pin's reconstruction re-adds a
//! lost row at identity index 0 for every singleton funding family
//! ([`index_trusted`]), so such a row may be the lock of a step at any
//! index. The real source re-keys a registration row by the identity its
//! outpoint created when that identity is one of the wallet's
//! ([`attributed_row`]); a row whose index stays untrusted is
//! *unattributed*. A step it does not match by key (the marker's or the
//! draft's) still reads its matching rows as before, but while an
//! unattributed row of the step's funding type stands, the self-spend's
//! row (§2a.5 row 1) does not apply: the row may be the step's lock, on
//! the chain whatever the self-spend did. The step reads `MaybeSent`, and
//! Repair refuses it ([`StuckFundingRefusal::UnattributedLock`]). Fail
//! closed: such a step waits for the row's attribution.
//!
//! A registration step with no standing marker of its own (none, or one a
//! `NotSent` row superseded) reads `MaybeSent`, not `None` or `NotSent`,
//! while an unattributed `IdentityRegistration` row of its account stands
//! that it does not match by key ([`markerless_unattributed_row`]): after a
//! lost or rolled-back journal, that row may be the lock of the step's own
//! call, whose marker is gone. So the funding gate refuses a second funding
//! of it ([`FundingBlock::Step`]). Such a row is never attributed to another
//! step's standing marker instead: a rolled-back journal can keep one
//! draft's marker and lose another's. Liveness: a new registration waits
//! until the row is attributed (the identity it created is adopted, which
//! re-keys it).
//!
//! **The funding gate** (§2a.3, in the J step that would take a funding
//! call's permit): the wallet's rows must be known; the step must have no
//! standing marker and read `None` or `NotSent` (retry only on `NotSent`,
//! §16.6, so a possibly-sent funding blocks a second build of its step:
//! the withheld-T1 trace, and a markerless registration step while an
//! unattributed row may be its lock); and no tracked `Built` or `Broadcast` row of the
//! wallet may stand that the catch-up has not resumed this session
//! ([`LeaseTable::note_resumed`]), or the library's coin selection could
//! pick its inputs (F7).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use dashcore::blockdata::script::Instruction;
use dashcore::hashes::Hash;
use dashcore::secp256k1::{Message, ecdsa};
use dashcore::sighash::SighashCache;
use dashcore::{OutPoint, PublicKey, ScriptBuf, Transaction, Txid};
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::state_transition::asset_lock_proof::chain::ChainAssetLockProof;
use dpp::prelude::Identifier;
use platform_wallet::{AssetLockFundingType, AssetLockStatus, TrackedAssetLock};

use super::call::{CallPermit, CallStep, FundingKey, FundingQuote};
use super::fence::{CallStepInfo, CallStepStatus};
use super::resume::{Resumed, Resumer};
use super::table::{Effects, Entry, Inner, LeaseTable};
use super::{ArtifactId, FlowOutcome, Lease, LeaseError};
use crate::platform::errors::PlatformError;
use crate::platform::flows::{DispatchResolution, DispatchState, is_funding_step};
use crate::{CoinFence, NetworkSession, WalletId};

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
    /// The key's identity index names the identity the lock funds
    /// ([`index_trusted`], [`attributed_row`]). A row that is not is
    /// *unattributed*: it may be the lock of any step of its funding type
    /// (the module doc's "Unattributed rows").
    pub trusted: bool,
}

impl TrackedRow {
    pub fn of(lock: &TrackedAssetLock) -> Self {
        let key = FundingKey::of_tracked(lock);
        let status = TrackedStatus::from(&lock.status);
        Self {
            txid: lock.out_point.txid,
            key,
            status,
            inputs: lock
                .transaction
                .input
                .iter()
                .map(|i| i.previous_output)
                .collect(),
            trusted: index_trusted(&key, status),
        }
    }
}

/// An asset lock a changeset is about to store, as the provenance check
/// reads it (§2a.6, [`LeaseTable::record_signed_locks`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredLock {
    pub(crate) txid: Txid,
    pub(crate) inputs: Vec<OutPoint>,
    /// Each input's `SIGHASH_ALL` sighash, recomputed from the transaction,
    /// that the input's signature validly signs; `None` when an input
    /// carries no such signature: no call can then be shown to have signed
    /// the lock.
    pub(crate) sighashes: Option<Vec<[u8; 32]>>,
}

impl StoredLock {
    pub(crate) fn of(tx: &Transaction) -> Self {
        Self {
            txid: tx.txid(),
            inputs: tx.input.iter().map(|i| i.previous_output).collect(),
            sighashes: signed_sighashes(tx),
        }
    }
}

/// Per input of `tx`, the sighash its P2PKH scriptSig (`<DER signature +
/// SIGHASH_ALL> <public key>`, what key-wallet's builder assembles) signs:
/// the legacy `SIGHASH_ALL` sighash of the input over the P2PKH script of
/// that key, checked against the signature. `None` when any input's
/// scriptSig is anything else, or its signature does not verify.
fn signed_sighashes(tx: &Transaction) -> Option<Vec<[u8; 32]>> {
    let cache = SighashCache::new(tx);
    tx.input
        .iter()
        .enumerate()
        .map(|(n, input)| {
            let mut pushes = input.script_sig.instructions();
            let (
                Some(Ok(Instruction::PushBytes(sig))),
                Some(Ok(Instruction::PushBytes(key))),
                None,
            ) = (pushes.next(), pushes.next(), pushes.next())
            else {
                return None;
            };
            let (&hash_type, der) = sig.as_bytes().split_last()?;
            if hash_type != SIGHASH_ALL {
                return None;
            }
            let mut signature = ecdsa::Signature::from_der(der).ok()?;
            signature.normalize_s();
            let key = PublicKey::from_slice(key.as_bytes()).ok()?;
            let script = ScriptBuf::new_p2pkh(&key.pubkey_hash());
            let sighash = cache
                .legacy_signature_hash(n, &script, u32::from(SIGHASH_ALL))
                .ok()?
                .to_byte_array();
            ecdsa::verify(&signature, Message::from_digest(sighash), &key.inner).ok()?;
            Some(sighash)
        })
        .collect()
}

/// `SIGHASH_ALL`, the hash type key-wallet's builder signs with.
const SIGHASH_ALL: u8 = 0x01;

/// An asset lock a call of a funding step signed (§2a.6): the step's own
/// lock, exactly. Recorded against the step's marker, durably when the
/// journal took it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SignedLock {
    pub(crate) txid: Txid,
    pub(crate) inputs: Vec<OutPoint>,
    /// Its `signed_lock` row landed (or was loaded).
    pub(crate) durable: bool,
}

/// One of Repair's self-spends of a funding step, standing against its
/// marker (§2a.6, review P2b r3 R3-01).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfSpend {
    pub txid: [u8; 32],
    /// Each signed lock of the step it was checked to spend an input of,
    /// with that input, when it was recorded; `None` for one recorded
    /// before the journal kept this (schema 5): a legacy self-spend, which
    /// an engine that did not require a signed lock may have admitted.
    pub conflicts: Option<Vec<(Txid, OutPoint)>>,
}

impl SelfSpend {
    /// Whether this self-spend, once ChainLocked, shows the step's lock can
    /// no longer confirm: its record names at least one signed lock, and
    /// every signed lock of the step (`signed`) with one of that lock's
    /// inputs. A legacy self-spend, or one recorded with no signed lock,
    /// never does, whatever a later scan finds (review P2b r3 R3-01, r4
    /// R4-01): the step derives as if it were absent. A lock its record
    /// names that this process no longer knows (a signed lock whose write
    /// failed, before a restart) was the step's own all the same: at most
    /// one call of a step signs, as no call starts under its standing
    /// marker (`check_funding`), and that call builds one lock.
    fn shows_conflict(&self, signed: &[SignedLock]) -> bool {
        let Some(conflicts) = self.conflicts.as_deref() else {
            return false;
        };
        !conflicts.is_empty()
            && signed.iter().all(|l| {
                conflicts
                    .iter()
                    .any(|(lock, input)| *lock == l.txid && l.inputs.contains(input))
            })
    }
}

/// Whether a tracked row's identity index can be trusted as it stands
/// (review H-1). At the pin, `reconstruction.rs` re-adds a row the wallet
/// lost (a sweep's `remove_tracked_asset_locks_for_swept`, a restore) from
/// the chain, with status `RecoveredFromChain`, `InstantSendLocked` or
/// `Broadcast`, and `funding_family` gives every singleton family
/// (registration, unbound top-up, invitation, both address top-ups)
/// identity index 0: the funding account does not encode it. A bound
/// top-up's account is the identity's registration index, so its index is
/// exact. Trusted, then: a `Built` row (only the live pipeline builds), a
/// non-zero index, or an `IdentityTopUp` row.
pub(crate) fn index_trusted(key: &FundingKey, status: TrackedStatus) -> bool {
    status == TrackedStatus::Built
        || key.identity_index != 0
        || key.funding_type == AssetLockFundingType::IdentityTopUp
}

/// The identity an asset lock of `out_point` creates: Platform's id for a
/// proof of it (rs-dpp `create_identifier`, the same for both proof kinds;
/// DP1-02's derivation).
pub(crate) fn identity_for(out_point: OutPoint) -> Identifier {
    ChainAssetLockProof::new(0, out_point.into()).create_identifier()
}

/// `lock`'s row, re-keyed by the identity it created (review H-1): an
/// untrusted registration row whose outpoint's identity
/// ([`identity_for`]) is one of the wallet's (`identities`: their ids and
/// identity indices) takes that identity's index, and is trusted.
pub(crate) fn attributed_row(
    lock: &TrackedAssetLock,
    identities: &HashMap<Identifier, u32>,
) -> TrackedRow {
    let mut row = TrackedRow::of(lock);
    if !row.trusted
        && row.key.funding_type == AssetLockFundingType::IdentityRegistration
        && let Some(&index) = identities.get(&identity_for(lock.out_point))
    {
        row.key.identity_index = index;
        row.trusted = true;
    }
    row
}

/// A draft's own funding key (§2a.5, review Opus r3 F-2), from
/// `app.sqlite`: the draft's type and identity index, at the account
/// DP1-02 funds every registration from (0; its draft stores none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftKey {
    /// The draft's funding step, `registration/<draft>/funding`.
    pub step: String,
    pub key: FundingKey,
}

impl DraftKey {
    /// A `dp_registration` row's key: an identity registration at its
    /// identity index.
    pub fn registration(draft: i64, identity_index: u32) -> Self {
        Self {
            step: format!("registration/{draft}/funding"),
            key: FundingKey {
                funding_type: AssetLockFundingType::IdentityRegistration,
                identity_index,
                account: 0,
            },
        }
    }

    fn matches(&self, key: &FundingKey) -> bool {
        self.key == *key
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
    /// Per wallet, bumped by its removal: a refresh of it read before is
    /// dropped. Per wallet (review L-5), so one wallet's removal drops no
    /// other wallet's refresh; kept after the removal, so a read of the
    /// removed wallet still finds its epoch moved.
    epochs: HashMap<WalletId, u64>,
    /// The catch-up's resume passes (H6 part 2).
    pub(super) passes: super::resume::Passes,
}

impl ModeB {
    /// A removed wallet's rows and statuses.
    pub(super) fn forget(&mut self, wallet: &WalletId) {
        self.snapshots.remove(wallet);
        self.derived.retain(|(w, _), _| w != wallet);
        self.foreign.retain(|(w, _), _| w != wallet);
        self.wallet_closed(wallet);
        *self.epochs.entry(*wallet).or_default() += 1;
    }

    /// `wallet`'s removal epoch.
    fn epoch(&self, wallet: &WalletId) -> u64 {
        self.epochs.get(wallet).copied().unwrap_or_default()
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
///   marker is ChainLocked and shows the conflict (its record covers every
///   signed lock of the step: [`SelfSpend`], reviews P2b r3 R3-01, r4
///   R4-01);
/// - `unattributed`: an unattributed row that may be the step's lock
///   stands (module doc): the self-spend's row, a definite resolution and
///   "never started" then read `MaybeSent`, not `NotSent` or `None`.
///
/// A call that returned Ok (`status` `Sent`) and a row with a proof come
/// first: the lock reached a proof, so a ChainLocked self-spend cannot have
/// consumed its inputs, and §2a.5's first row ("a self-spend of one of the
/// lock's inputs") does not apply. Repair's self-spend spends every coin of
/// the funding accounts, among them an input of every tracked `Built` and
/// `Broadcast` row of the step, and only while no other spend of the
/// wallet's coins is short of a ChainLock (`repair.rs`, which refuses
/// otherwise), so once ChainLocked it reads `NotSent` over such a row too.
/// A row that came after the self-spend cannot be the step's lock either:
/// the self-spend shows the conflict with every lock the step's one
/// signing call built ([`SelfSpend`]).
pub(crate) fn derive_funding(
    status: CallStepStatus,
    rows: Option<&[TrackedStatus]>,
    self_spend_locked: bool,
    unattributed: bool,
) -> Option<DispatchState> {
    if status == CallStepStatus::Sent || rows.is_some_and(|r| r.iter().any(|s| s.proved())) {
        return Some(DispatchState::Sent);
    }
    let Some(rows) = rows else {
        return Some(DispatchState::MaybeSent);
    };
    if self_spend_locked {
        // An unattributed row may be the step's lock, which the self-spend
        // need not have conflicted with.
        return Some(if unattributed {
            DispatchState::MaybeSent
        } else {
            DispatchState::NotSent
        });
    }
    if rows.iter().any(|s| s.pending()) {
        return Some(DispatchState::WillBeSent);
    }
    match status {
        // An unattributed row may be the lock of a call whose marker a lost
        // or rolled-back journal dropped.
        CallStepStatus::NotSent | CallStepStatus::None if unattributed => {
            Some(DispatchState::MaybeSent)
        }
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

/// The first unattributed row (module doc) that may be `step`'s lock: of
/// the step's funding type (any type when the step has neither a marker
/// key nor a draft), and not matching the step by key, whether or not it
/// preceded the marker.
fn unattributed_row<'a>(
    s: &'a Snapshot,
    step: &str,
    info: &CallStepInfo,
) -> Option<&'a TrackedRow> {
    let draft = s.drafts.iter().find(|d| d.step == step);
    let step_type = info
        .funding
        .map(|k| k.funding_type)
        .or(draft.map(|d| d.key.funding_type));
    s.rows.iter().find(|r| {
        !r.trusted
            && step_type.is_none_or(|t| t == r.key.funding_type)
            && info.funding.as_ref() != Some(&r.key)
            && !draft.is_some_and(|d| d.matches(&r.key))
    })
}

/// The first unattributed registration row (module doc) that may be the
/// lock of registration `step` while no marker of the step stands (none,
/// or one a `NotSent` row superseded): of the step's account (any account
/// when its draft is unknown), and matching the step by neither the
/// marker's key nor the draft's. Never matched against another step's
/// marker: a rolled-back journal can keep one draft's marker and lose
/// another's.
fn markerless_unattributed_row<'a>(
    s: &'a Snapshot,
    step: &str,
    info: &CallStepInfo,
) -> Option<&'a TrackedRow> {
    if !step.starts_with("registration/")
        || !matches!(info.status, CallStepStatus::None | CallStepStatus::NotSent)
    {
        return None;
    }
    let draft = s.drafts.iter().find(|d| d.step == step);
    s.rows.iter().find(|r| {
        !r.trusted
            && r.key.funding_type == AssetLockFundingType::IdentityRegistration
            && draft.is_none_or(|d| d.key.account == r.key.account)
            && info.funding.as_ref() != Some(&r.key)
            && !draft.is_some_and(|d| d.matches(&r.key))
    })
}

/// `step`'s funding status now (J).
pub(super) fn funding_status(i: &Inner, wallet: WalletId, step: &str) -> Option<DispatchState> {
    derive_step(i, wallet, step, i.fence.call_step_info(&wallet, step))
}

/// [`funding_status`] from the step's journal state `info`.
fn derive_step(
    i: &Inner,
    wallet: WalletId,
    step: &str,
    info: CallStepInfo,
) -> Option<DispatchState> {
    let snap = i.mode_b.snapshots.get(&wallet);
    let foreign = i.mode_b.foreign.get(&(wallet, step.to_owned()));
    let rows: Option<Vec<TrackedStatus>> = snap.map(|s| {
        s.rows
            .iter()
            .filter(|r| matches_step(s, step, &info, foreign, r))
            .map(|r| r.status)
            .collect()
    });
    // A ChainLocked self-spend counts only once its record shows the
    // conflict (reviews P2b r3 R3-01, r4 R4-01).
    let signed = i.fence.signed_locks(&wallet, step);
    let locked = snap.is_some_and(|s| {
        info.self_spends
            .iter()
            .any(|spend| s.locked_spends.contains(&spend.txid) && spend.shows_conflict(signed))
    });
    let unattributed = snap.is_some_and(|s| {
        (locked && unattributed_row(s, step, &info).is_some())
            || markerless_unattributed_row(s, step, &info).is_some()
    });
    derive_funding(info.status, rows.as_deref(), locked, unattributed)
}

/// Whether tracked row `r` of `wallet` is a lock a call of `step` signed
/// (§2a.6, review P2b r2 R2-01): its txid is one of the step's
/// [`SignedLock`]s. Only such a row is known to be the step's own; one
/// that matches the step's key may be another step's lock, whatever its
/// index, and one that appeared while the step's call ran may be too.
fn own_row(i: &Inner, wallet: WalletId, step: &str, r: &TrackedRow) -> bool {
    i.fence
        .signed_locks(&wallet, step)
        .iter()
        .any(|l| l.txid == r.txid)
}

/// The key of a tracked row with a proof that matches `step` and is
/// trusted ([`index_trusted`]) or its own ([`own_row`]) (J): the step's
/// lock was seen out, so it reads `Sent` (§2a.5 row 2), and that is
/// journaled. Any other matching proved row reads `Sent` while it stands,
/// and is never made durable. A `Sent` only ever stops Repair, so a row
/// that matches by a trusted index is evidence enough for it.
fn proving_key(i: &Inner, wallet: WalletId, step: &str) -> Option<FundingKey> {
    step_rows(i, wallet, step)
        .into_iter()
        .find(|r| r.status.proved() && (r.trusted || own_row(i, wallet, step, r)))
        .map(|r| r.key)
}

/// The tracked rows of `wallet` that match funding `step` (J): by the
/// draft's key, or by the marker's less the rows foreign to it.
fn step_rows<'a>(i: &'a Inner, wallet: WalletId, step: &str) -> Vec<&'a TrackedRow> {
    let Some(s) = i.mode_b.snapshots.get(&wallet) else {
        return Vec::new();
    };
    let info = i.fence.call_step_info(&wallet, step);
    let foreign = i.mode_b.foreign.get(&(wallet, step.to_owned()));
    s.rows
        .iter()
        .filter(|r| matches_step(s, step, &info, foreign, r))
        .collect()
}

/// A funding step's outcome in a lock's report (§8.4, review R1-02): its
/// status now, from history (§2a.5). `NotSent` and never started are
/// `Cancelled`. A step whose calls here all ended definitely unsent reads
/// as it will once its `NotSent` row lands, not by the marker standing
/// until then (review P2b r1 F4).
pub(super) fn funding_outcome(i: &Inner, wallet: WalletId, step: &str) -> FlowOutcome {
    let mut info = i.fence.call_step_info(&wallet, step);
    if matches!(info.status, CallStepStatus::Marked { .. })
        && i.fence.call_ended_unsent(&(wallet, step.to_owned()))
    {
        info.status = CallStepStatus::NotSent;
    }
    match derive_step(i, wallet, step, info) {
        Some(DispatchState::Sent) => FlowOutcome::Sent,
        Some(DispatchState::WillBeSent) => FlowOutcome::WillBeSent,
        Some(DispatchState::MaybeSent) => FlowOutcome::MaybeSent,
        Some(DispatchState::NotSent) | None => FlowOutcome::Cancelled,
    }
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
    let Some(snap) = i.mode_b.snapshots.get(&wallet) else {
        return Some(FundingBlock::RowsUnknown);
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

/// Why Repair's "Resolve stuck funding" (§6.5, §16.10 C9) refuses a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StuckFundingRefusal {
    /// The wallet's tracked rows were never read this session.
    #[error("the wallet's tracked asset locks are not known yet")]
    RowsUnknown,
    /// Only a `MaybeSent` or `WillBeSent` funding is stuck: the others
    /// need nothing (`Sent`, `NotSent`) or never started (`None`).
    #[error("the funding is not stuck (its status is {0:?})")]
    NotStuck(Option<DispatchState>),
    /// No marker of the step stands to record the self-spend against: a
    /// row found only by the draft's own key, or a marker a `NotSent` row
    /// superseded.
    #[error("no marker of the funding step stands")]
    NoMarker,
    /// A call runs under the step's marker, or its `NotSent` row is being
    /// written: its outcome is not known yet.
    #[error("a call of the funding step is running")]
    CallRunning,
    #[error("the dispatch journal is unavailable")]
    JournalUnavailable,
    /// A transaction spending coins of the funding accounts is not
    /// ChainLocked yet (an earlier self-spend, a payment, the lost lock
    /// itself): a lock that conflicts only with it could still confirm
    /// over a self-spend of the other coins. Wait for its ChainLock, or
    /// abandon it.
    #[error("transaction {0} spends the wallet's coins and is not ChainLocked yet")]
    SpendUnsettled(Txid),
    /// A tracked `Built` or `Broadcast` lock of the step spends no coin the
    /// self-spend can take, so the self-spend would not conflict with it.
    #[error("tracked asset lock {0} spends no coin the self-spend can take")]
    LockInputsSpent(Txid),
    /// The self-spend to record spends no input of a lock a call of the step
    /// signed, so it would show nothing about that lock (review P2b r3).
    #[error("the self-spend spends no input of signed asset lock {0}")]
    MissesSignedLock(Txid),
    /// A tracked lock of the step's funding type whose identity index is
    /// not known (the module doc's "Unattributed rows") may be the step's
    /// own, already on the chain: a ChainLocked self-spend would prove
    /// nothing. Repair waits until the row is attributed.
    #[error("tracked asset lock {0} may be this funding's: its identity is not known yet")]
    UnattributedLock(Txid),
    /// No lock a call of the step signed is recorded ([`SignedLock`]; a
    /// call before schema 4, or one whose write failed, before a restart):
    /// nothing names the inputs the self-spend must take (reviews R1-01,
    /// P2b r2 R2-01, r4 R4-01). The step's lock may be on the chain
    /// already, its row lost and its inputs spent, or held by a peer over
    /// an input no scan of the chain shows (an unconfirmed parent's
    /// output), so a self-spend of the wallet's coins would prove nothing.
    /// A tracked row that only matches the step's key may be another
    /// step's lock. The step stays as it reads until its lock is found.
    #[error("no asset lock this funding signed is known")]
    NoKnownLock,
}

/// The locks Repair's self-spend of funding `step` must conflict with (J):
/// each one a call of the step signed ([`SignedLock`], with its row or
/// without: a lost row's lock may still confirm), then the tracked
/// `Built` and `Broadcast` rows of the step besides; their txids and
/// inputs.
pub(super) fn conflict_locks(
    i: &Inner,
    wallet: WalletId,
    step: &str,
) -> Vec<(Txid, Vec<OutPoint>)> {
    let mut locks: Vec<(Txid, Vec<OutPoint>)> = i
        .fence
        .signed_locks(&wallet, step)
        .iter()
        .map(|l| (l.txid, l.inputs.clone()))
        .collect();
    for r in step_rows(i, wallet, step) {
        if r.status.pending() && !locks.iter().any(|(t, _)| *t == r.txid) {
            locks.push((r.txid, r.inputs.clone()));
        }
    }
    locks
}

/// The input a self-spend spending `inputs` takes of each lock a call of
/// funding `step` signed (J), its record's provenance (§2a.6, review P2b
/// r3 R3-01); refused when it takes none of one.
pub(super) fn self_spend_conflicts(
    i: &Inner,
    wallet: WalletId,
    step: &str,
    inputs: &[OutPoint],
) -> Result<Vec<(Txid, OutPoint)>, StuckFundingRefusal> {
    i.fence
        .signed_locks(&wallet, step)
        .iter()
        .map(|l| {
            l.inputs
                .iter()
                .find(|o| inputs.contains(o))
                .map(|o| (l.txid, *o))
                .ok_or(StuckFundingRefusal::MissesSignedLock(l.txid))
        })
        .collect()
}

/// Whether Repair may resolve funding `step` with a self-spend now (J):
/// the wallet's rows are known, the step reads `MaybeSent` or
/// `WillBeSent` (a proof-less tracked row), its marker stands, no call
/// runs under it, no unattributed row may be its lock, and the self-spend
/// can be shown to conflict with the step's lock: a lock a call of the
/// step signed is recorded ([`SignedLock`]; Repair spends an input of each,
/// [`conflict_locks`], and records them, [`SelfSpend`]; reviews R1-01, P2b
/// r2 R2-01, r4 R4-01, fail closed).
pub(super) fn stuck_funding(
    i: &Inner,
    wallet: WalletId,
    step: &str,
) -> Result<(), StuckFundingRefusal> {
    if !i.mode_b.snapshots.contains_key(&wallet) {
        return Err(StuckFundingRefusal::RowsUnknown);
    }
    match funding_status(i, wallet, step) {
        Some(DispatchState::MaybeSent | DispatchState::WillBeSent) => {}
        other => return Err(StuckFundingRefusal::NotStuck(other)),
    }
    if !matches!(
        i.fence.call_step_status(&wallet, step),
        CallStepStatus::Marked { .. }
    ) {
        return Err(StuckFundingRefusal::NoMarker);
    }
    if i.fence.call_busy(&(wallet, step.to_owned())) {
        return Err(StuckFundingRefusal::CallRunning);
    }
    let info = i.fence.call_step_info(&wallet, step);
    if let Some(r) = i
        .mode_b
        .snapshots
        .get(&wallet)
        .and_then(|s| unattributed_row(s, step, &info))
    {
        return Err(StuckFundingRefusal::UnattributedLock(r.txid));
    }
    if i.fence.signed_locks(&wallet, step).is_empty() {
        return Err(StuckFundingRefusal::NoKnownLock);
    }
    Ok(())
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
///
/// A step its own proved row reads `Sent` (trusted, or one of its signed
/// locks: [`proving_key`]) is journaled (review R1-01): the
/// step's `Sent` row with its marker and funding key, one DEC-163 bundle,
/// begun after J and owed until it lands
/// ([`FenceState::note_proved_funding`](super::fence::FenceState::note_proved_funding)).
/// So the `Sent` stands once the row is gone (a restore, a sweep), across
/// restarts too, and Repair's self-spend can never turn it `NotSent`.
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
        if now == Some(DispatchState::Sent)
            && let Some(key) = proving_key(i, wallet, &step)
            && i.fence.note_proved_funding(wallet, &step, key)
        {
            fx.owe_sent(wallet);
        }
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
            fx.emit(t.resolved_event(&wallet, &step, resolution));
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
        let epoch = self.with_j(|i, _| i.mode_b.epoch(&wallet));
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

    /// Stores `wallet`'s snapshot and re-derives (J), unless `wallet` was
    /// removed since `epoch` (the read may be of the removed wallet).
    fn commit_read(&self, wallet: WalletId, snapshot: Snapshot, epoch: Option<u64>) -> bool {
        self.with_j(|i, fx| {
            if epoch.is_some_and(|e| e != i.mode_b.epoch(&wallet)) {
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

    /// The locks Repair's self-spend of funding `step` must conflict with
    /// ([`conflict_locks`]) when Repair may resolve it with a self-spend
    /// now, else the refusal ([`stuck_funding`]).
    pub(crate) fn check_stuck_funding(
        &self,
        wallet: WalletId,
        step: &str,
    ) -> Result<Vec<(Txid, Vec<OutPoint>)>, StuckFundingRefusal> {
        self.with_j(|i, _| {
            stuck_funding(i, wallet, step)?;
            Ok(conflict_locks(i, wallet, step))
        })
    }

    /// The own key of `step`'s draft in `wallet`'s snapshot, if any.
    pub(crate) fn draft_key(&self, wallet: WalletId, step: &str) -> Option<DraftKey> {
        self.with_j(|i, _| {
            i.mode_b
                .snapshots
                .get(&wallet)?
                .drafts
                .iter()
                .find(|d| d.step == step)
                .cloned()
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

    /// The changeset tap stored tracked rows of `wallet`, of any status
    /// (`tracked`: their txids and keys). A row is a running funding call's
    /// when the call released a signature, the row carries its key, and it
    /// is not foreign to the call's step (it was not standing when the step
    /// was marked): the call has signed its lock and the library tracks
    /// it, so its inputs read reserved from now on, and the call's coin
    /// fence is released, outside J ([`super::call::CallFunding`]). That is
    /// no evidence of whose lock the row is: only
    /// [`Self::record_signed_locks`] attributes a lock to a step. Takes no
    /// lock but J and awaits nothing: it runs inside the library's store
    /// call.
    pub(crate) fn note_tracked(&self, wallet: WalletId, tracked: &[(Txid, FundingKey)]) {
        if tracked.is_empty() {
            return;
        }
        self.with_j(|i, fx| {
            for (txid, key) in tracked {
                let calls: Vec<u64> = i
                    .calls
                    .iter()
                    .filter_map(|(id, run)| {
                        let f = run.funding.as_ref()?;
                        let signed = f.wallet == wallet
                            && f.signatures.released() > 0
                            && f.key == *key
                            && !i
                                .mode_b
                                .foreign
                                .get(&(wallet, f.step.clone()))
                                .is_some_and(|rows| rows.contains(txid));
                        signed.then_some(*id)
                    })
                    .collect();
                for id in calls {
                    if let Some(fence) = i.calls.get_mut(&id).and_then(|r| r.fence.take()) {
                        fx.drop_later(fence);
                    }
                }
            }
        });
    }

    /// The txids of the asset locks the calls of funding `step` signed
    /// ([`SignedLock`], §2a.6): exactly the step's own locks. One whose
    /// `signed_lock` write has not landed is known to this process only
    /// and is gone after a restart; a rejected broadcast leaves its txid
    /// here.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "DP1-02 attributes its rows by it")
    )]
    pub(crate) fn signed_lock_txids(&self, wallet: WalletId, step: &str) -> Vec<Txid> {
        self.with_j(|i, _| {
            i.fence
                .signed_locks(&wallet, step)
                .iter()
                .map(|l| l.txid)
                .collect()
        })
    }

    /// Whether a funding call of `wallet` runs (§2a.6: only its locks are
    /// checked for provenance).
    pub(crate) fn funding_call_runs(&self, wallet: WalletId) -> bool {
        self.with_j(|i, _| {
            i.calls
                .values()
                .any(|r| r.funding.as_ref().is_some_and(|f| f.wallet == wallet))
        })
    }

    /// Whether a running call of `wallet` holds a coin fence (tests).
    #[cfg(test)]
    pub(crate) fn call_holds_fence(&self, wallet: WalletId) -> bool {
        self.with_j(|i, _| {
            i.calls
                .values()
                .any(|r| r.fence.as_ref().is_some_and(|f| f.wallet_id() == wallet))
        })
    }

    /// Whether tracked row `txid` of `wallet` was resumed this session.
    #[cfg(test)]
    pub(crate) fn resumed(&self, wallet: WalletId, txid: Txid) -> bool {
        self.with_j(|i, _| i.mode_b.resumed.contains(&(wallet, txid)))
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
        // The wallet's own identities (never observed contacts), by id.
        let identities: HashMap<Identifier, u32> = info
            .identity_manager
            .wallet_managed_identities(&w.wallet_id())
            .filter_map(|m| Some((m.identity.id(), m.identity_index?)))
            .collect();
        Some(
            info.tracked_asset_locks
                .values()
                .map(|lock| attributed_row(lock, &identities))
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
    /// `fence` is the wallet's coin fence ([`Self::coin_fence`]) the
    /// caller's coin policy check ran under ([`Self::fenced_coins`]): the
    /// permit keeps it until the library has tracked the lock it signed
    /// (`Built`, whose inputs then read reserved) or the call ends
    /// ([`super::call::CallFunding`]), so no coin locked after the check
    /// is signed by the call: "Lock unspent" waits for it. Refused, the
    /// fence is released with the refusal. While the permit holds it, the
    /// caller asks for no coin fence of the wallet (it would wait for
    /// itself) and reads no coins through one.
    ///
    /// Of a refused step, the code names the step: `broadcast_unknown`
    /// (`MaybeSent`), `will_be_sent` (`WillBeSent`), or `invalid_argument`
    /// (a funded step, or a marker a self-spend resolved: a new funding
    /// needs a new draft).
    ///
    /// Refused `invalid_argument` before anything is charged or marked
    /// (review M-3, M-4): a step that is no funding step; a top-up step,
    /// which has no funding key of its own after a lost journal until the
    /// credit task gives it one (E0-04 §2a.6); and a registration step
    /// whose `key` is not its draft's own (`DraftKey`, from `app.sqlite`),
    /// or whose draft has no known key: a row under another key would be
    /// found by neither the marker nor the draft; and a `fence` of another
    /// wallet.
    pub async fn funding_call_permit(
        &self,
        lease: &Lease,
        label: impl Into<String>,
        amount: u64,
        step: String,
        key: FundingKey,
        fence: CoinFence,
    ) -> Result<CallPermit, PlatformError> {
        let refuse = |detail: String| Err(PlatformError::InvalidArgument { detail });
        if fence.wallet_id() != lease.wallet() {
            return refuse("the coin fence is another wallet's".into());
        }
        if !is_funding_step(&step) {
            return refuse(format!("{step} is not a funding step"));
        }
        if step.starts_with("topup/") {
            return refuse("a top-up step has no own funding key yet (E0-04 §2a.6)".into());
        }
        let wallet = lease.wallet();
        let fee_bound_worst = self.fee_bound_worst(wallet).await?;
        if !self.leases.refresh_funding(wallet).await {
            return Err(LeaseError::FundingBlocked(FundingBlock::RowsUnknown).into());
        }
        match self.leases.draft_key(wallet, &step) {
            Some(d) if d.matches(&key) => {}
            Some(_) => return refuse(format!("{key:?} is not the funding key of {step}'s draft")),
            None => return refuse(format!("{step}'s draft has no funding key")),
        }
        let quote = FundingQuote {
            amount,
            fee_bound_worst,
        };
        lease
            .call_permit_step_fenced(
                label,
                Some(quote.into()),
                CallStep::Funding {
                    step: step.clone(),
                    key,
                },
                Some(fence),
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
