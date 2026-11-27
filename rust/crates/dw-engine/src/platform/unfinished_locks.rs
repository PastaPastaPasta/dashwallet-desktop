//! Unfinished funding asset locks (0.2 recovery): Core → Shielded (funding
//! type 5) and Core → Platform address (type 4) locks whose credit output
//! pays one of the wallet's funding keys and whose Platform side is not
//! known to have happened, its own and those a restore read back from the
//! chain, and the one hook each kind's apply plugs in through
//! ([`LockKind`]). The restore scan matches a credit output's script to the
//! wallet's funding pools only; it does not check who funded the lock, so a
//! third party's lock to one of those keys is listed too (and finishing it
//! credits the wallet).
//!
//! - **Detection.** platform-wallet's restore scan re-derives every funding
//!   account's pool and rebuilds a tracked lock for each credit output that
//!   pays one (`wallet/asset_lock/sync/reconstruction.rs`), chain-locked
//!   ones as `RecoveredFromChain` with their ChainLock proof. A lock is
//!   *this device's* when one of the kind's funding steps signed it (the
//!   dispatch journal's `signed_lock` rows), else *restored*: an earlier
//!   install's, or another device's of the same seed. The journal outlives
//!   a wallet's removal, so a wallet imported again on the same install
//!   reads its own locks back as its own.
//! - **What is known before an apply.** The library keeps a restored lock
//!   "consumption unknown" (`asset_lock/tracked.rs`, `RecoveredFromChain`),
//!   so a lock is unfinished until an apply is tried, and one read back
//!   from the chain reads Checking until then: it may already be in the
//!   balance. Its Check again, and Finish transfers, try it. Trying is safe:
//!   Platform executes at most one apply of an outpoint (every asset-lock
//!   transition's proof validation refuses a consumed one, rs-drive-abci
//!   `common/asset_lock/proof/verify_is_not_spent/v0/mod.rs`). Platform's
//!   spent outpoints can be read with a proof ([`proved_spent`], its
//!   `SpentAssetLockTransactions` tree); the runner reads it when an apply
//!   is answered "already consumed".
//! - **Outcomes.** Only Platform's proved execution finishes a lock. Its
//!   "already consumed" for the exact outpoint
//!   (`PlatformWalletError::AssetLockAlreadyConsumed`, the library's typed
//!   form of `IdentityAssetLockTransactionOutPointAlreadyConsumedError`
//!   matched to the outpoint, `error.rs` `is_asset_lock_already_consumed`)
//!   is unauthenticated: any node can say it, so it counts only when the
//!   proved read says consumed. Any other failure after the
//!   lock's credit key signed may have executed. Neither settles a lock or
//!   changes a balance; balances come only from proved reads.
//! - **Evidence by outpoint.** One lock transaction can carry several
//!   credit outputs, of one funding type or of several, each a lock of its
//!   own. Every record the runner writes (settled, consumption unknown) is
//!   by outpoint ([`Marks::outpoints`]); 0.1's records by txid
//!   ([`Marks::txids`]) are read for a lock only when its transaction has no
//!   other credit output ([`KindLock::credit_outputs`]).
//! - **One output per transaction in this version.** At the pinned
//!   platform-wallet the apply re-derives the credit key from the payload's
//!   first credit output (`sync/recovery.rs:1848`,
//!   `rederive_credit_output_path`), not from the tracked outpoint's: a lock
//!   at `vout` ≠ 0 lists [`UnfinishedState::Unsupported`] and a finish
//!   refuses it before its lease.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;

use dashcore::ecdsa::{EcdsaPublicKey, EcdsaSignature};
use dashcore::{OutPoint, ScriptBuf, Txid};
use dw_appdb::{AppDb, AppDbError};
use dw_vault::{SignerError, VaultSigner};
use key_wallet::bip32::{DerivationPath, ExtendedPubKey};
use platform_wallet::{AssetLockFundingType, PlatformWalletError, TrackedAssetLock};

use super::errors::PlatformError;
use super::flows::FlowKind;
use super::lease::{Lease, LeaseError, SignatureCount};
use super::registration_flow::{LockInfo, LockStatus, lock_status};
pub(crate) use super::unfinished::{
    LockFinish, LockKindId, LockOrigin, TransfersReport, UnfinishedLock, UnfinishedState,
};
use crate::{NetworkSession, WalletId};

impl LockKindId {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "type 4's kind (slice 5) uses it")
    )]
    pub(crate) const ALL: [Self; 2] = [Self::Shielded, Self::PlatformAddress];

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "type 4's kind (slice 5) uses it")
    )]
    pub(crate) fn funding_type(self) -> AssetLockFundingType {
        match self {
            Self::Shielded => AssetLockFundingType::AssetLockShieldedAddressTopUp,
            Self::PlatformAddress => AssetLockFundingType::AssetLockAddressTopUp,
        }
    }

    /// The kind of a funding type; `None` for the identity types, which
    /// their own flows finish (registration, top-up), and for invitation
    /// vouchers and unbound top-ups, whose destination a lock does not name.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "type 4's kind (slice 5) uses it")
    )]
    pub(crate) fn of(funding_type: AssetLockFundingType) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.funding_type() == funding_type)
    }

    /// The call permit's label of the kind's apply.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Shielded => "shield",
            Self::PlatformAddress => "address_fund",
        }
    }
}

/// How a kind's apply ended without an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyReply {
    /// Platform's proved execution.
    Done,
    /// Platform's "already consumed" for the applied outpoint itself
    /// ([`reply_from`]): unproved.
    AlreadyConsumed,
    /// Failed after the lock's credit key signed the transition: it may
    /// have reached Platform and executed.
    Unknown,
}

/// The start of platform-wallet's refusal to keep a lock "consumption
/// unknown" at a persister that does not attest asset-lock reconciliation
/// (`mark_asset_lock_consumption_unknown`, `sync/tracking.rs` at the pin).
/// The library raises it only on Platform's "already consumed" for the
/// outpoint its call applied, matched first (`asset_lock/orchestration.rs`).
/// The pinned storage attests reconciliation, so there the answer arrives as
/// `AssetLockAlreadyConsumed`; this is that answer from a backend that does
/// not.
const UNRECORDED_CONSUMED: &str = "asset-lock reconciliation requires persistence capabilities ";

/// Whether an apply's `error` is the library's [`UNRECORDED_CONSUMED`]
/// refusal: Platform's "already consumed" for the lock the call applied.
/// No other persistence error is.
pub(crate) fn consumed_unrecorded(error: &PlatformWalletError) -> bool {
    match error {
        PlatformWalletError::Persistence(detail) => detail.starts_with(UNRECORDED_CONSUMED),
        _ => false,
    }
}

/// The library's apply result as an [`ApplyReply`], for the lock at
/// `outpoint`; `signed`: the lock's credit key signed in the call. Only
/// `AssetLockAlreadyConsumed` naming `outpoint` is `AlreadyConsumed`, the
/// library's refusal to record it included ([`consumed_unrecorded`]);
/// one naming another outpoint, a timeout or any other failure is
/// `Unknown` after a signature and the refusal before one.
pub(crate) fn reply_from(
    result: Result<(), PlatformWalletError>,
    outpoint: OutPoint,
    signed: bool,
) -> Result<ApplyReply, PlatformError> {
    match result {
        Ok(()) => Ok(ApplyReply::Done),
        Err(PlatformWalletError::AssetLockAlreadyConsumed(o)) if o == outpoint => {
            Ok(ApplyReply::AlreadyConsumed)
        }
        Err(e) if consumed_unrecorded(&e) => Ok(ApplyReply::AlreadyConsumed),
        Err(e) if signed => {
            tracing::warn!(%outpoint, error = %e, "a lock's apply failed after its signature");
            Ok(ApplyReply::Unknown)
        }
        Err(e) => Err(e.into()),
    }
}

/// How long [`proved_spent`] waits for Platform's proved answer.
pub(crate) const SPENT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// What Platform's proved state says of an asset lock's outpoint: drive's
/// `SpentAssetLockTransactions` tree, keyed by the 36-byte outpoint,
/// holds an empty item once the lock is used up and the remaining value
/// otherwise (`add_asset_lock_outpoint_operations/v0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpentRead {
    /// No element: not consumed as of the proof's block.
    Unspent,
    /// An empty item, or an `AssetLockValue` with nothing left: consumed,
    /// for good.
    Consumed,
    /// Any other item: partly used, `credits` left (dpp's `AssetLockValue`
    /// `remaining_credit_value`, which drive serializes into the item).
    Remaining { credits: u64 },
    /// No proved answer: an error, no proof, a timeout, an item that is not
    /// an `AssetLockValue`, or an element of another sort.
    Unread,
}

/// The tree's key of `outpoint`: its consensus bytes, as drive-abci keys it.
pub(crate) fn spent_key(outpoint: OutPoint) -> Vec<u8> {
    <[u8; 36]>::from(outpoint).to_vec()
}

/// [`SpentRead`] of a verified read of `key`: `None` is the SDK's answer
/// when nothing at all came back.
pub(crate) fn read_spent(
    elements: Option<&dash_sdk::query_types::Elements>,
    key: &[u8],
) -> SpentRead {
    use drive::grovedb::Element;
    let Some(elements) = elements else {
        return SpentRead::Unread;
    };
    match elements.get(key) {
        None => SpentRead::Unread,
        Some(None) => SpentRead::Unspent,
        Some(Some(Element::Item(bytes, _))) if bytes.is_empty() => SpentRead::Consumed,
        Some(Some(Element::Item(bytes, _))) => remaining(bytes),
        Some(Some(_)) => SpentRead::Unread,
    }
}

/// [`SpentRead::Remaining`] of a stored `AssetLockValue`'s bytes,
/// [`SpentRead::Consumed`] when nothing is left; [`SpentRead::Unread`]
/// when they are not one.
fn remaining(bytes: &[u8]) -> SpentRead {
    use dpp::asset_lock::reduced_asset_lock_value::{AssetLockValue, AssetLockValueGettersV0};
    use dpp::serialization::PlatformDeserializableUntrusted;
    match AssetLockValue::deserialize_from_bytes_untrusted_exact(bytes) {
        Ok(value) => match value.remaining_credit_value() {
            0 => SpentRead::Consumed,
            credits => SpentRead::Remaining { credits },
        },
        Err(e) => {
            tracing::warn!(error = %e, "a spent asset lock's stored value could not be read");
            SpentRead::Unread
        }
    }
}

/// The path of drive's spent asset-lock tree, built as drive builds it
/// (`RootTree`'s `&[u8; 1]`, `drive/asset_lock/mod.rs`).
pub(crate) fn spent_path() -> Vec<Vec<u8>> {
    let byte: &'static [u8; 1] = drive::drive::RootTree::SpentAssetLockTransactions.into();
    vec![byte.to_vec()]
}

/// Platform's proved answer for the lock at `outpoint`
/// ([`SPENT_READ_TIMEOUT`]); [`SpentRead::Unread`] on any failure, and
/// from an SDK that does not ask for proofs.
pub(crate) async fn proved_spent(sdk: &dash_sdk::Sdk, outpoint: OutPoint) -> SpentRead {
    use dash_sdk::platform::FetchMany;
    use dash_sdk::query_types::KeysInPath;
    use drive::grovedb::Element;
    let key = spent_key(outpoint);
    let query = KeysInPath {
        path: spent_path(),
        keys: vec![key.clone()],
    };
    let fetch = || Element::fetch_many(sdk, query);
    read_fetched(sdk.prove(), outpoint, &key, SPENT_READ_TIMEOUT, fetch).await
}

/// [`proved_spent`] around its `fetch`: not made without proofs, bounded
/// by `limit`, and any error or timeout is [`SpentRead::Unread`].
pub(crate) async fn read_fetched<F, Fut, E>(
    prove: bool,
    outpoint: OutPoint,
    key: &[u8],
    limit: std::time::Duration,
    fetch: F,
) -> SpentRead
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<dash_sdk::query_types::Elements, E>>,
    E: std::fmt::Display,
{
    if !prove {
        tracing::warn!(%outpoint, "the SDK does not ask for proofs: a lock's spent read is not made");
        return SpentRead::Unread;
    }
    match tokio::time::timeout(limit, fetch()).await {
        Ok(Ok(elements)) => read_spent(Some(&elements), key),
        Ok(Err(e)) => {
            tracing::warn!(%outpoint, error = %e, "a lock's spent read failed");
            SpentRead::Unread
        }
        Err(_) => {
            tracing::warn!(%outpoint, "a lock's spent read timed out");
            SpentRead::Unread
        }
    }
}

/// Whether a kind's lock can be applied now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Readiness {
    Ready,
    /// Below the kind's protocol version: the refusal's code.
    NotActive(String),
    /// Held back for now (not synced, not bound, Platform unavailable):
    /// the refusal's code.
    Held(String),
}

/// A tracked lock of a kind, with what the runner reads of its row and its
/// transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KindLock {
    pub(crate) info: LockInfo,
    /// The library's row holds its proof. The status alone does not say: a
    /// restore files an InstantSend-context record (another device's lock
    /// still in flight) as `InstantSendLocked` with none
    /// (`sync/reconstruction.rs:139`), and the apply refuses that before
    /// its signature (`sync/recovery.rs:1685-1692`).
    pub(crate) proof: bool,
    /// The lock payload's credit outputs, whatever funding type pays each;
    /// 0 when the payload cannot be read.
    pub(crate) credit_outputs: usize,
}

impl KindLock {
    /// The kind's row of the library's tracked lock `row`. Every kind builds
    /// its rows with it: `credit_outputs` counts the whole payload, so a
    /// record by txid of another output of the transaction (another
    /// family's included) never speaks for this one (the conformance run's
    /// shared-transaction case fails a kind that counts otherwise).
    pub(crate) fn of_row(row: &TrackedAssetLock) -> Self {
        use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
        Self {
            info: LockInfo {
                outpoint: row.out_point,
                registration: false,
                identity_index: row.identity_index,
                account: row.account_index,
                amount: row.amount,
                status: lock_status(&row.status),
            },
            proof: row.proof.is_some(),
            credit_outputs: match &row.transaction.special_transaction_payload {
                Some(TransactionPayload::AssetLockPayloadType(p)) => p.credit_outputs.len(),
                _ => 0,
            },
        }
    }

    /// Its transaction has no other credit output, so a record by txid is
    /// this lock's.
    pub(crate) fn alone(&self) -> bool {
        self.credit_outputs == 1
    }
}

/// A kind's records of one sort (settled, or consumption unknown).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Marks {
    /// By outpoint: every record the runner writes ([`record`]).
    pub(crate) outpoints: HashSet<OutPoint>,
    /// By txid: 0.1's records (`top_up_settled`, `shielded.shield_unknown.`),
    /// each written for a lock transaction of one credit output; read for a
    /// lock only when its own transaction has no other ([`KindLock::alone`]).
    pub(crate) txids: HashSet<Txid>,
}

impl Marks {
    pub(crate) fn holds(&self, lock: &KindLock) -> bool {
        let at = lock.info.outpoint;
        self.outpoints.contains(&at) || (lock.alone() && self.txids.contains(&at.txid))
    }
}

/// A sort of record of a lock, kept by outpoint in the wallet's local scope
/// of `app.sqlite`, across restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    /// A proved apply.
    Settled,
    /// An apply that may have executed, unproved.
    Unknown,
    /// Platform's proof that the outpoint is consumed ([`SpentRead::Consumed`]),
    /// kept by a kind that takes it as finished elsewhere
    /// ([`LockKind::reported_locks`]); not settled. Earlier versions kept
    /// an unproved "already consumed" under `reported`, which is no longer
    /// read: such a lock lists again until a proved read settles it.
    Used,
    /// Platform's proof that the lock is partly used
    /// ([`SpentRead::Remaining`]), with the credits that read left: what
    /// the list offers for it ([`LockKind::remaining_locks`]). A recover
    /// reads Platform again before it signs anything.
    Remaining,
}

fn mark_prefix(kind: LockKindId, mark: Mark) -> String {
    let sort = match mark {
        Mark::Settled => "settled",
        Mark::Unknown => "unknown",
        Mark::Used => "used",
        Mark::Remaining => "remaining",
    };
    format!("recovery.{}.{sort}.", kind.label())
}

/// `wallet`'s `mark` records of `kind`'s locks, each with its stored value.
fn mark_rows(
    db: &AppDb,
    wallet: WalletId,
    kind: LockKindId,
    mark: Mark,
) -> Result<Vec<(OutPoint, String)>, AppDbError> {
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let prefix = mark_prefix(kind, mark);
    let rows = db.settings_with_prefix(&scope, &prefix)?;
    rows.into_iter()
        .map(|(key, value)| {
            let at = key[prefix.len()..].parse().map_err(|_| {
                AppDbError::Corrupt(format!("invalid outpoint in lock record {key}"))
            })?;
            Ok((at, value))
        })
        .collect()
}

/// `wallet`'s `mark` records of `kind`'s locks.
pub(crate) fn read_marks(
    db: &AppDb,
    wallet: WalletId,
    kind: LockKindId,
    mark: Mark,
) -> Result<HashSet<OutPoint>, AppDbError> {
    Ok((mark_rows(db, wallet, kind, mark)?.into_iter())
        .map(|(at, _)| at)
        .collect())
}

/// Records `mark` of `kind`'s lock at `outpoint`, durably.
pub(crate) fn write_mark(
    db: &AppDb,
    wallet: WalletId,
    kind: LockKindId,
    mark: Mark,
    outpoint: OutPoint,
) -> Result<(), AppDbError> {
    write_mark_value(db, wallet, kind, mark, outpoint, "1")
}

fn write_mark_value(
    db: &AppDb,
    wallet: WalletId,
    kind: LockKindId,
    mark: Mark,
    outpoint: OutPoint,
    value: &str,
) -> Result<(), AppDbError> {
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let key = format!("{}{outpoint}", mark_prefix(kind, mark));
    db.set_setting_durable(&scope, &key, Some(value))
}

/// `f` on `session`'s `app.sqlite`, off the async threads.
async fn on_appdb<T: Send + 'static>(
    session: &NetworkSession,
    f: impl FnOnce(&AppDb) -> Result<T, AppDbError> + Send + 'static,
) -> Result<T, PlatformError> {
    let appdb = session.live()?.appdb;
    tokio::task::spawn_blocking(move || f(&appdb))
        .await
        .map_err(|e| PlatformError::from(crate::EngineError::from(e)))?
        .map_err(|e| PlatformError::Storage {
            detail: e.to_string(),
        })
}

/// The proved remaining values, including old records of a used-up lock.
pub(crate) async fn remaining_of(
    session: &NetworkSession,
    wallet: WalletId,
    kind: LockKindId,
) -> Result<HashMap<OutPoint, u64>, PlatformError> {
    on_appdb(session, move |db| {
        let scope = dw_appdb::local_scope(&wallet.to_string());
        let prefix = mark_prefix(kind, Mark::Remaining);
        db.settings_with_prefix(&scope, &prefix)?
            .into_iter()
            .map(|(key, value)| {
                let at = key[prefix.len()..].parse().map_err(|_| {
                    AppDbError::Corrupt(format!("invalid outpoint in remaining value key {key}"))
                })?;
                let credits = value.parse().map_err(|_| {
                    AppDbError::Corrupt(format!("invalid remaining value for {at}"))
                })?;
                Ok((at, credits))
            })
            .collect()
    })
    .await
}

/// Records that `kind`'s lock at `outpoint` has `credits` left
/// ([`Mark::Remaining`]), durably, over any earlier record of it.
pub(crate) async fn record_remaining(
    session: &NetworkSession,
    wallet: WalletId,
    kind: LockKindId,
    outpoint: OutPoint,
    credits: u64,
) {
    let write = on_appdb(session, move |db| {
        write_mark_value(
            db,
            wallet,
            kind,
            Mark::Remaining,
            outpoint,
            &credits.to_string(),
        )
    });
    if let Err(e) = write.await {
        tracing::warn!(%outpoint, ?kind, error = %e, "could not record a lock's remaining value");
    }
}

/// [`read_marks`] on `session`'s `app.sqlite`, off the async threads; none
/// when they cannot be read (the locks then list again, fail closed).
pub(crate) async fn marks_of(
    session: &NetworkSession,
    wallet: WalletId,
    kind: LockKindId,
    mark: Mark,
) -> HashSet<OutPoint> {
    try_marks_of(session, wallet, kind, mark).await.unwrap_or_else(|e| {
        tracing::warn!(wallet_id = %wallet, ?kind, ?mark, error = %e, "could not read a kind's lock records");
        HashSet::new()
    })
}

pub(crate) async fn try_marks_of(
    session: &NetworkSession,
    wallet: WalletId,
    kind: LockKindId,
    mark: Mark,
) -> Result<HashSet<OutPoint>, PlatformError> {
    on_appdb(session, move |db| read_marks(db, wallet, kind, mark)).await
}

/// [`write_mark`] on `session`'s `app.sqlite`, off the async threads.
pub(crate) async fn record(
    session: &NetworkSession,
    wallet: WalletId,
    kind: LockKindId,
    mark: Mark,
    outpoint: OutPoint,
) {
    let write = on_appdb(session, move |db| {
        write_mark(db, wallet, kind, mark, outpoint)
    });
    if let Err(e) = write.await {
        tracing::warn!(%outpoint, ?kind, ?mark, error = %e, "could not record a lock");
    }
}

/// One kind's side of finishing, implemented by the lane that owns its
/// apply. Every method is the wallet's.
///
/// Every implementation runs the conformance run in its tests
/// (`unfinished_conformance::conformance` over a `Rig` of its library;
/// type 5's `ShieldLocks` here, PLAT-ROUTES's type 4 likewise; MIX-SHIELD's
/// drain lock is a `ShieldLocks` lock, with a case of its own): settled only
/// on a proved apply, Checking after a signature, refusals before the
/// lease, evidence by outpoint.
pub(crate) trait LockKind: Send + Sync {
    fn id(&self) -> LockKindId;
    /// The lease flow a single finish of this kind runs under.
    fn flow(&self) -> FlowKind;
    /// The wallet's tracked locks of the kind's funding type.
    fn locks(&self) -> impl Future<Output = Vec<KindLock>> + Send;
    /// The lock transactions the kind's own funding steps signed (each of
    /// one credit output); `None` until the dispatch journal is read.
    fn own_locks(&self) -> Option<HashSet<Txid>>;
    /// The locks recorded settled (a proved apply); none when they cannot
    /// be read, so they list again (fail closed).
    fn settled_locks(&self) -> impl Future<Output = Marks> + Send;
    /// The locks whose apply may have executed, unproved, kept across
    /// restarts.
    fn unknown_locks(&self) -> impl Future<Output = Marks> + Send;
    /// The locks Platform answered "already consumed" for (exact outpoint)
    /// that the kind takes as finished elsewhere: they leave the lists, and
    /// their finish reads [`LockFinish::ReportedUsed`]. None by default: such
    /// an answer then reads Checking ([`Self::mark_unknown`]).
    fn reported_locks(&self) -> impl Future<Output = HashSet<OutPoint>> + Send {
        std::future::ready(HashSet::new())
    }
    /// The locks Platform proved partly used ([`Mark::Remaining`]), with
    /// the credits left: they list as
    /// [`UnfinishedState::Recoverable`] (or too small) and are never
    /// applied again as the kind. None by default.
    fn remaining_locks(
        &self,
    ) -> impl Future<Output = Result<HashMap<OutPoint, u64>, PlatformError>> + Send {
        std::future::ready(Ok(HashMap::new()))
    }
    /// The least remaining value (credits) a recover of a partly used lock
    /// takes: Platform's minimum fee for moving it. `None` when it cannot
    /// be read locally.
    fn remainder_floor(&self) -> Option<u64> {
        None
    }
    /// Records `lock` among [`Self::unknown_locks`] by its outpoint, and
    /// (when the kind keeps 0.1's records) by txid only when
    /// [`KindLock::alone`].
    fn mark_unknown(&self, lock: &KindLock) -> impl Future<Output = ()> + Send;
    /// Waits, bounded, while the kind's apply is held only by the wallet's
    /// bring-up and a rerun of it is asked for or runs; before the kind's
    /// lock is taken, so [`Self::readiness`] reads the answer. Nothing by
    /// default.
    fn await_start_up(&self) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
    /// Whether an apply may run now: the protocol-version gate, the
    /// destination account bound, the kind's sync.
    fn readiness(&self) -> impl Future<Output = Readiness> + Send;
    /// The refusal's code while the kind is not active on the network, as
    /// the SDK holds the version (no Platform query, no bind), for a list.
    fn not_active(&self) -> Option<String>;
    /// The least value of a lock the apply takes (below it the apply's fee
    /// leaves nothing, and the apply refuses it before its signature), and
    /// the code of that refusal. A local read at the version the SDK holds;
    /// `None` when it cannot be read (no lock is then told too small).
    fn minimum(&self) -> Option<Minimum>;
    /// Whether SPV and the wallet's bring-up (with it the library's
    /// restore scan) have caught up, so the tracked rows hold every lock the
    /// chain has: until then a list may be partial.
    fn caught_up(&self) -> bool;
    /// The lock serializing the wallet's calls of this kind: every apply,
    /// and the kind's own fundings, run under it.
    fn wallet_lock(&self) -> Arc<tokio::sync::Mutex<()>>;
    fn before_apply(
        &self,
        _at: OutPoint,
        _lock: &UnfinishedLock,
    ) -> impl Future<Output = Option<LockFinish>> + Send {
        std::future::ready(None)
    }
    /// The script of the credit output at `outpoint` (the lock's payload
    /// `credit_outputs[vout]`), as the library tracks the lock's
    /// transaction; `None` when it is not tracked.
    fn credit_script(&self, outpoint: OutPoint) -> impl Future<Output = Option<ScriptBuf>> + Send;
    /// The library's `FromExistingAssetLock` of the proved lock at
    /// `outpoint` to the wallet's own destination of the kind, never a new
    /// lock, its proof signed through `signer` (which signs with that
    /// lock's credit key and no other), the result mapped by
    /// [`reply_from`] with [`LockKeySigner::signed`]. A refusal before the
    /// signature is the error; an error after it reads as
    /// [`ApplyReply::Unknown`] whatever the kind returns.
    fn apply(
        &self,
        outpoint: OutPoint,
        signer: &LockKeySigner<'_>,
    ) -> impl Future<Output = Result<ApplyReply, PlatformError>> + Send;
    /// After a proved apply: records `lock` settled (by outpoint, and by
    /// txid as [`Self::mark_unknown`] does) and starts the kind's sync, so
    /// the balance a proved read shows takes it in.
    fn settle(&self, lock: &KindLock) -> impl Future<Output = ()> + Send;
}

/// The signer a finish hands a kind's apply: the lease's funding signer
/// narrowed to one key, the one whose P2PKH script is the lock's credit
/// output. Any other signature is refused before the vault is asked for it
/// (`SignerError::KeyMismatch`), so a finish signs the lock's asset-lock
/// proof and nothing else: no Core input, no other lock. Public keys pass
/// through (the vault's scope bounds them). Every signature counts into the
/// call permit's [`SignatureCount`], so a call that signed and then failed
/// ends `MaybeSent`, and none is signed once the permit ended.
pub(crate) struct LockKeySigner<'a> {
    inner: &'a VaultSigner,
    credit_script: ScriptBuf,
    count: Arc<SignatureCount>,
}

impl<'a> LockKeySigner<'a> {
    pub(crate) fn new(
        inner: &'a VaultSigner,
        credit_script: ScriptBuf,
        count: Arc<SignatureCount>,
    ) -> Self {
        Self {
            inner,
            credit_script,
            count,
        }
    }

    /// Whether it returned a signature: nothing can reach Platform before.
    pub(crate) fn signed(&self) -> bool {
        self.count.released() > 0
    }

    /// Whether `key` is the one the lock's credit output pays.
    pub(crate) fn is_the_lock_key(&self, key: &EcdsaPublicKey) -> bool {
        let hash = dashcore::PublicKey::new(*key).pubkey_hash();
        ScriptBuf::new_p2pkh(&hash) == self.credit_script
    }
}

#[async_trait::async_trait]
impl key_wallet::Signer for LockKeySigner<'_> {
    type Error = SignerError;

    fn supported_methods(&self) -> &[key_wallet::SignerMethod] {
        self.inner.supported_methods()
    }

    async fn sign_ecdsa(
        &self,
        path: &DerivationPath,
        sighash: [u8; 32],
    ) -> Result<(EcdsaSignature, EcdsaPublicKey), SignerError> {
        if !self.is_the_lock_key(&self.inner.public_key(path).await?) {
            tracing::warn!(%path, "a finish refused to sign with a key other than the lock's");
            return Err(SignerError::KeyMismatch(path.to_string()));
        }
        // Counted before the vault is asked, refused once the permit ended.
        let Some(signing) = self.count.begin() else {
            return Err(SignerError::Locked);
        };
        match self.inner.sign_ecdsa(path, sighash).await {
            Ok(signature) => {
                signing.released();
                Ok(signature)
            }
            Err(e) => {
                self.count.observe_err(&e);
                Err(e)
            }
        }
    }

    async fn public_key(&self, path: &DerivationPath) -> Result<EcdsaPublicKey, SignerError> {
        self.inner.public_key(path).await
    }
}

#[async_trait::async_trait]
impl key_wallet::ExtendedPubKeySigner for LockKeySigner<'_> {
    async fn extended_public_key(
        &self,
        path: &DerivationPath,
    ) -> Result<ExtendedPubKey, SignerError> {
        self.inner.extended_public_key(path).await
    }
}

/// [`LockKind::minimum`]: the least value (duffs) of a lock the kind's
/// apply takes, and the code it refuses one below it with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Minimum {
    pub(crate) duffs: u64,
    pub(crate) code: &'static str,
}

impl Minimum {
    fn refuses(self, lock: &KindLock) -> bool {
        lock.info.amount < self.duffs
    }
}

/// What, besides the library's rows, decides a lock's standing.
pub(crate) struct Evidence<'a> {
    /// [`LockKind::own_locks`], read.
    pub(crate) own: &'a HashSet<Txid>,
    pub(crate) settled: &'a Marks,
    pub(crate) unknown: &'a Marks,
    /// [`LockKind::reported_locks`], read.
    pub(crate) reported_used: &'a HashSet<OutPoint>,
    pub(crate) readiness: &'a Readiness,
    /// [`LockKind::minimum`], read.
    pub(crate) minimum: Option<Minimum>,
    /// [`LockKind::remaining_locks`], read.
    pub(crate) remaining: &'a HashMap<OutPoint, u64>,
    /// [`LockKind::remainder_floor`], read.
    pub(crate) remainder_floor: Option<u64>,
}

/// Whose `lock` is: this device's when one of the kind's own funding steps
/// signed its transaction and it is alone in it, else restored.
pub(crate) fn origin(lock: &KindLock, own: &HashSet<Txid>) -> LockOrigin {
    if lock.alone() && own.contains(&lock.info.outpoint.txid) {
        LockOrigin::ThisDevice
    } else {
        LockOrigin::Restored
    }
}

/// `lock`'s standing as an unfinished lock of `kind`; `None` once it is
/// finished (the library's proved `Consumed`, or recorded settled).
pub(crate) fn classify(kind: LockKindId, lock: &KindLock, ev: &Evidence) -> Option<UnfinishedLock> {
    let info = &lock.info;
    let txid = info.outpoint.txid;
    if info.status == LockStatus::Consumed
        || ev.settled.holds(lock)
        || ev.reported_used.contains(&info.outpoint)
        || ev.remaining.get(&info.outpoint) == Some(&0)
    {
        return None;
    }
    let origin = origin(lock, ev.own);
    let remaining = ev.remaining.get(&info.outpoint).copied();
    let state = if info.outpoint.vout != 0 {
        UnfinishedState::Unsupported
    } else if !lock.proof || !info.status.has_proof() {
        UnfinishedState::Confirming
    } else if matches!(ev.readiness, Readiness::NotActive(_)) {
        UnfinishedState::NotActive
    } else if let Some(credits) = remaining {
        // Partly used, proved: never applied again as the kind, only
        // recovered, and only above Platform's fee for moving it (unknown
        // here: the recover's own priced read decides).
        match ev.remainder_floor {
            Some(floor) if credits < floor => UnfinishedState::TooSmall,
            _ => UnfinishedState::Recoverable,
        }
    } else if ev.unknown.holds(lock)
        // The library's "consumption unknown" for a lock of this device's.
        || (origin == LockOrigin::ThisDevice && info.status == LockStatus::RecoveredFromChain)
    {
        // An apply that may have executed reads Checking whatever the lock's
        // value; a finish of it below the minimum is still refused before
        // the lease (`run`).
        UnfinishedState::Checking
    } else if ev.minimum.is_some_and(|min| min.refuses(lock)) {
        UnfinishedState::TooSmall
    } else if info.status == LockStatus::RecoveredFromChain {
        // A restore's lock read back from the chain: Platform may already
        // have taken it, and nothing the wallet reads says whether it did.
        UnfinishedState::Checking
    } else {
        UnfinishedState::Ready
    };
    Some(UnfinishedLock {
        txid: txid.to_string(),
        vout: info.outpoint.vout,
        kind,
        duffs: info.amount,
        origin,
        state,
        remaining_credits: remaining,
    })
}

/// The wallet's unfinished locks of `kind`; `platform.unavailable` until
/// the dispatch journal is read.
pub(crate) async fn detect(kind: &impl LockKind) -> Result<Vec<UnfinishedLock>, PlatformError> {
    let own = kind.own_locks().ok_or(PlatformError::Unavailable)?;
    let locks = kind.locks().await;
    let (settled, unknown) = (kind.settled_locks().await, kind.unknown_locks().await);
    let reported_used = &kind.reported_locks().await;
    let readiness = kind
        .not_active()
        .map_or(Readiness::Ready, Readiness::NotActive);
    let remaining = &kind.remaining_locks().await?;
    let ev = Evidence {
        own: &own,
        settled: &settled,
        unknown: &unknown,
        reported_used,
        readiness: &readiness,
        minimum: kind.minimum(),
        remaining,
        remainder_floor: kind.remainder_floor(),
    };
    let mut out: Vec<_> = (locks.iter())
        .filter_map(|l| classify(kind.id(), l, &ev))
        .collect();
    out.sort_by(|a, b| (&a.txid, a.vout).cmp(&(&b.txid, b.vout)));
    Ok(out)
}

/// The code a finish of a lock at `vout` ≠ 0 is refused with (module doc).
pub(crate) const UNSUPPORTED: &str = "recovery.unsupported_output";

/// The code a finish of a lock Platform proved partly used is refused with
/// ([`Mark::Remaining`]): only a recover moves what is left.
pub(crate) const PARTLY_USED: &str = "recovery.partly_used";

/// What a lock's apply runs under: its lease, and the funding signer the
/// lock's credit key signs through ([`LockKeySigner`]).
pub(crate) trait ApplyAuthority {
    fn into_parts(self) -> Result<(Lease, VaultSigner), PlatformError>;
}

/// A lease the apply alone uses: its funding signer, refused once the
/// lease's Funding budget is spent (a resume's or Finish's one-duff grant).
impl ApplyAuthority for Lease {
    fn into_parts(self) -> Result<(Lease, VaultSigner), PlatformError> {
        let funding = self.uncounted_funding_signer()?;
        Ok((self, funding))
    }
}

/// A lease whose funding call makes the lock it then applies, and the
/// funding signer that lease issued before the call charged its budget:
/// the call may leave no Funding budget, and the apply spends none (as a
/// top-up's, `topup.rs`). The signer is kept for this apply only, which
/// still needs the lease's call permit: an ended or revoked lease refuses
/// it before anything is signed.
pub(crate) struct FundedLease {
    lease: Lease,
    funding: VaultSigner,
}

impl FundedLease {
    /// Takes `lease`'s funding signer; call it before the funding call.
    pub(crate) fn before_funding(lease: Lease) -> Result<Self, LeaseError> {
        let funding = lease.uncounted_funding_signer()?;
        Ok(Self { lease, funding })
    }

    pub(crate) fn lease(&self) -> &Lease {
        &self.lease
    }
}

impl ApplyAuthority for FundedLease {
    fn into_parts(self) -> Result<(Lease, VaultSigner), PlatformError> {
        Ok((self.lease, self.funding))
    }
}

/// Finishes `kind`'s unfinished lock at `outpoint` under the lease
/// `lease_of` gives: the lease's funding signer, narrowed to the lock's
/// credit key ([`LockKeySigner`]), and one call permit sign the kind's
/// `FromExistingAssetLock` of the same outpoint. It funds and builds
/// nothing and waits for no missing proof (the library's bounded
/// InstantSend → ChainLock fallback still runs). A tracked lock first
/// waits on the kind's start-up hold ([`LockKind::await_start_up`]). Read
/// again under the kind's wallet lock: a lock finished meanwhile is not
/// applied again.
///
/// `lease_of` is called only once every refusal the runner and the kind
/// can read beforehand has passed: no proof ([`LockFinish::ProofWaiting`]),
/// `vout` ≠ 0, a lock below the apply's minimum ([`LockKind::minimum`]),
/// the kind not ready. Those keep the grant. A refusal inside the
/// apply before its signature (the library's own checks, the prover, a
/// call permit refused) comes after the lease, so the grant it redeemed is
/// spent; it moves nothing and debits no budget.
///
/// Only Platform's proved execution settles the lock. "Already consumed"
/// for the lock and any failure after its signature leave it unfinished,
/// [`LockFinish::Checking`], recorded by outpoint, unless the kind took the
/// answer as finished elsewhere ([`LockKind::reported_locks`]:
/// [`LockFinish::ReportedUsed`], and the lock leaves the lists); a refusal
/// before the signature is [`LockFinish::NotApplied`].
pub(crate) async fn finish<K, F, Fut, A>(
    kind: &K,
    outpoint: OutPoint,
    lease_of: F,
) -> Result<LockFinish, PlatformError>
where
    K: LockKind,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<A, PlatformError>>,
    A: ApplyAuthority,
{
    if (kind.locks().await.iter()).any(|l| l.info.outpoint == outpoint) {
        kind.await_start_up().await;
    }
    Ok(run(kind, outpoint, lease_of)
        .await?
        .unwrap_or(LockFinish::Applied))
}

/// [`finish`]; `None` when the lock was already finished when read under
/// the kind's lock (nothing ran).
async fn run<K, F, Fut, A>(
    kind: &K,
    outpoint: OutPoint,
    lease_of: F,
) -> Result<Option<LockFinish>, PlatformError>
where
    K: LockKind,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<A, PlatformError>>,
    A: ApplyAuthority,
{
    let wallet_lock = kind.wallet_lock();
    let _held = wallet_lock.lock().await;
    let own = kind.own_locks().ok_or(PlatformError::Unavailable)?;
    let locks = kind.locks().await;
    let Some(lock) = locks.iter().find(|l| l.info.outpoint == outpoint) else {
        return Err(PlatformError::InvalidArgument {
            detail: "no tracked lock of that kind has that outpoint".into(),
        });
    };
    let lock = lock.clone();
    let readiness = kind.readiness().await;
    let (settled, unknown) = (kind.settled_locks().await, kind.unknown_locks().await);
    let reported = kind.reported_locks().await;
    let remaining = kind.remaining_locks().await?;
    let ev = Evidence {
        own: &own,
        settled: &settled,
        unknown: &unknown,
        reported_used: &reported,
        readiness: &readiness,
        minimum: kind.minimum(),
        remaining: &remaining,
        remainder_floor: kind.remainder_floor(),
    };
    let Some(unfinished) = classify(kind.id(), &lock, &ev) else {
        // Platform said it is used, nothing proves it: finished elsewhere.
        let proved = lock.info.status == LockStatus::Consumed || settled.holds(&lock);
        return Ok((!proved
            && (reported.contains(&outpoint) || remaining.get(&outpoint) == Some(&0)))
        .then_some(LockFinish::ReportedUsed));
    };
    match unfinished.state {
        UnfinishedState::Unsupported => {
            return Ok(Some(LockFinish::NotApplied {
                error: UNSUPPORTED.into(),
            }));
        }
        UnfinishedState::Confirming => return Ok(Some(LockFinish::ProofWaiting)),
        _ => {}
    }
    if remaining.contains_key(&outpoint) {
        // Proved partly used: the kind's apply would fail every time.
        return Ok(Some(LockFinish::NotApplied {
            error: PARTLY_USED.into(),
        }));
    }
    if let Some(min) = ev.minimum.filter(|min| min.refuses(&lock)) {
        return Ok(Some(LockFinish::NotApplied {
            error: min.code.into(),
        }));
    }
    if let Readiness::NotActive(code) | Readiness::Held(code) = readiness {
        return Ok(Some(LockFinish::NotApplied { error: code }));
    }
    if let Some(result) = kind.before_apply(outpoint, &unfinished).await {
        return Ok(Some(result));
    }
    let script = kind
        .credit_script(outpoint)
        .await
        .ok_or(PlatformError::Unavailable)?;
    let (lease, funding) = lease_of().await?.into_parts()?;
    let reply = lease
        .scope(async {
            let permit = lease.call_permit(kind.id().label(), None)?;
            let signer = LockKeySigner::new(&funding, script, permit.signatures());
            let reply = kind.apply(outpoint, &signer).await;
            let signed = signer.signed();
            // Only a proved apply was sent for certain; anything else ends
            // MaybeSent after a signature and Cancelled without one.
            permit.finish(matches!(reply, Ok(ApplyReply::Done)));
            Ok::<_, PlatformError>((reply, signed))
        })
        .await;
    let (reply, signed) = match reply {
        Ok(ran) => ran,
        Err(e) => (Err(e), false),
    };
    // The kind's word that nothing was signed is not taken for it.
    let reply = match reply {
        Err(e) if signed => {
            tracing::warn!(%outpoint, error = %e, "a lock's apply failed after its signature");
            Ok(ApplyReply::Unknown)
        }
        reply => reply,
    };
    Ok(Some(match reply {
        Ok(ApplyReply::Done) => {
            kind.settle(&lock).await;
            LockFinish::Applied
        }
        // Neither settles (SR1). A kind that took the answer as finished
        // elsewhere recorded it (`reported_locks`); otherwise the lock stays
        // unfinished until a proved apply.
        Ok(ApplyReply::AlreadyConsumed) if kind.reported_locks().await.contains(&outpoint) => {
            tracing::warn!(%outpoint, "a lock's apply was answered already consumed, unproved");
            LockFinish::ReportedUsed
        }
        Ok(how @ (ApplyReply::AlreadyConsumed | ApplyReply::Unknown)) => {
            tracing::warn!(%outpoint, ?how, "a lock's apply may have executed, unproved");
            kind.mark_unknown(&lock).await;
            LockFinish::Checking
        }
        Err(e) => {
            tracing::warn!(%outpoint, error = %e, "a lock's apply was refused before its signature");
            LockFinish::NotApplied {
                error: e.code().into(),
            }
        }
    }))
}

/// Finishes every unfinished lock of `kind` under the bulk's `lease`, one
/// at a time (a refusal leaves that lock pending and the next one runs),
/// and adds what came of them to `report`: its totals and, when the kind
/// had any, its [`TransfersReport`]. A Ready lock is tried, and so is a
/// restored one that reads Checking (Platform may already have taken it; an
/// apply of a taken lock is refused, so trying is how it is told). This
/// device's own Checking lock, one with no proof, of a kind not active, at
/// `vout` ≠ 0 or below the apply's minimum is pending, not tried (a Checking
/// lock's own "Check again" tries it); one Platform reported already used is
/// counted in `already_used`, not tried. A lock another finish settled
/// meanwhile counts in no field.
pub(crate) async fn finish_kind(
    kind: &impl LockKind,
    lease: &Lease,
    report: &mut super::FinishReport,
) {
    let listed = match detect(kind).await {
        Ok(listed) => listed,
        Err(e) => {
            tracing::warn!(kind = ?kind.id(), error = %e, "Finish transfers could not list a kind's locks");
            report.unread.push(kind.id());
            return;
        }
    };
    let mut counts = TransfersReport {
        kind: kind.id(),
        resumed: 0,
        completed: 0,
        still_pending: 0,
    };
    let mut waited = false;
    for lock in &listed {
        let restored_checking =
            lock.state == UnfinishedState::Checking && lock.origin == LockOrigin::Restored;
        let tried = match lock.state {
            UnfinishedState::Checking if !restored_checking => None,
            UnfinishedState::Confirming
            | UnfinishedState::NotActive
            | UnfinishedState::Unsupported
            | UnfinishedState::TooSmall
            | UnfinishedState::Recoverable
            | UnfinishedState::ReportedUsed => None,
            UnfinishedState::Ready | UnfinishedState::Checking => {
                let Ok(txid) = lock.txid.parse::<Txid>() else {
                    continue;
                };
                let outpoint = OutPoint {
                    txid,
                    vout: lock.vout,
                };
                if !std::mem::replace(&mut waited, true) {
                    kind.await_start_up().await;
                }
                match run(kind, outpoint, || async { Ok(lease.clone()) }).await {
                    // Finished meanwhile (another finish): not this run's.
                    Ok(None) => continue,
                    Ok(Some(finished)) => Some(Ok(finished)),
                    Err(e) => Some(Err(e)),
                }
            }
        };
        match tried {
            Some(Ok(LockFinish::Applied)) => {
                counts.resumed += 1;
                counts.completed += 1;
            }
            Some(Ok(LockFinish::ReportedUsed)) => {
                counts.resumed += 1;
                report.already_used += 1;
            }
            Some(Ok(LockFinish::Checking)) => {
                counts.resumed += 1;
                counts.still_pending += 1;
            }
            Some(Ok(LockFinish::ProofWaiting | LockFinish::NotApplied { .. })) | None => {
                counts.still_pending += 1;
            }
            Some(Err(e)) => {
                tracing::warn!(kind = ?kind.id(), error = %e, "Finish transfers could not finish a lock");
                counts.still_pending += 1;
            }
        }
    }
    if listed.is_empty() {
        return;
    }
    report.resumed += counts.resumed;
    report.completed += counts.completed;
    report.still_pending += counts.still_pending;
    report.by_kind.push(counts);
}
