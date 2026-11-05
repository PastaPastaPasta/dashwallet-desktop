//! Unfinished funding asset locks (0.2 recovery): Core → Shielded (funding
//! type 5) and Core → Platform address (type 4) locks the wallet funded
//! whose Platform side is not known to have happened, its own and those a
//! restore read back from the chain, and the one hook each kind's apply
//! plugs in through ([`LockKind`]).
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
//! - **What cannot be told.** For these funding types nothing the wallet
//!   can read says whether Platform already took a lock: the library keeps
//!   a restored one "consumption unknown" (`asset_lock/tracked.rs`,
//!   `RecoveredFromChain`), and Platform answers no query of its spent
//!   outpoints. So a lock is unfinished until an apply is tried. Trying is
//!   safe: Platform executes at most one apply of an outpoint (every
//!   asset-lock transition's proof validation refuses a consumed one,
//!   rs-drive-abci `common/asset_lock/proof/verify_is_not_spent/v0/mod.rs`).
//! - **Outcomes.** Only Platform's proved execution finishes a lock. Its
//!   "already consumed" for the exact outpoint
//!   (`PlatformWalletError::AssetLockAlreadyConsumed`, the library's typed
//!   form of `IdentityAssetLockTransactionOutPointAlreadyConsumedError`
//!   matched to the outpoint, `error.rs` `is_asset_lock_already_consumed`)
//!   is unauthenticated: any node can say it. Any other failure after the
//!   lock's credit key signed may have executed. Neither settles a lock or
//!   changes a balance; balances come only from proved reads.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dashcore::secp256k1::{PublicKey, ecdsa};
use dashcore::{OutPoint, ScriptBuf, Txid};
use dw_vault::{SignerError, VaultSigner};
use key_wallet::bip32::{DerivationPath, ExtendedPubKey};
use platform_wallet::{AssetLockFundingType, PlatformWalletError};

use super::errors::PlatformError;
use super::flows::FlowKind;
use super::lease::Lease;
use super::registration_flow::{LockInfo, LockStatus};
pub(crate) use super::unfinished::{
    LockFinish, LockKindId, LockOrigin, UnfinishedLock, UnfinishedState,
};

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

/// The library's apply result as an [`ApplyReply`], for the lock at
/// `outpoint`; `signed`: the lock's credit key signed in the call. Only
/// `AssetLockAlreadyConsumed` naming `outpoint` is `AlreadyConsumed`;
/// one naming another outpoint, a timeout or any other failure is
/// `Unknown` after a signature and the refusal before one.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "type 4's kind (slice 5) uses it")
)]
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
        Err(e) if signed => {
            tracing::warn!(%outpoint, error = %e, "a lock's apply failed after its signature");
            Ok(ApplyReply::Unknown)
        }
        Err(e) => Err(e.into()),
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

/// One kind's side of finishing, implemented by the lane that owns its
/// apply. Every method is the wallet's.
pub(crate) trait LockKind: Send + Sync {
    fn id(&self) -> LockKindId;
    /// The lease flow a single finish of this kind runs under.
    fn flow(&self) -> FlowKind;
    /// The wallet's tracked locks of the kind's funding type.
    fn locks(&self) -> impl Future<Output = Vec<LockInfo>> + Send;
    /// The locks the kind's own funding steps signed; `None` until the
    /// dispatch journal is read.
    fn own_locks(&self) -> Option<HashSet<Txid>>;
    /// The locks the journal records settled (a proved apply); none when
    /// they cannot be read, so they list again (fail closed).
    fn settled_locks(&self) -> impl Future<Output = HashSet<Txid>> + Send;
    /// The locks whose apply may have executed, unproved, kept across
    /// restarts.
    fn unknown_locks(&self) -> impl Future<Output = HashSet<Txid>> + Send;
    /// Records `txid` among [`Self::unknown_locks`].
    fn mark_unknown(&self, txid: Txid) -> impl Future<Output = ()> + Send;
    /// Whether an apply may run now: the protocol-version gate, the
    /// destination account bound, the kind's sync.
    fn readiness(&self) -> impl Future<Output = Readiness> + Send;
    /// The refusal's code while the kind is not active on the network, as
    /// last proved: a local read (no Platform query, no bind), for a list.
    fn not_active(&self) -> Option<String>;
    /// Whether SPV and the wallet's bring-up (with it the library's
    /// restore scan) have caught up, so the tracked rows hold every lock the
    /// chain has: until then a list may be partial.
    fn caught_up(&self) -> bool;
    /// The lock serializing the wallet's calls of this kind: every apply,
    /// and the kind's own fundings, run under it.
    fn wallet_lock(&self) -> Arc<tokio::sync::Mutex<()>>;
    /// The script of the credit output at `outpoint` (the lock's payload
    /// `credit_outputs[vout]`), as the library tracks the lock's
    /// transaction; `None` when it is not tracked.
    fn credit_script(&self, outpoint: OutPoint) -> impl Future<Output = Option<ScriptBuf>> + Send;
    /// The library's `FromExistingAssetLock` of the proved lock at
    /// `outpoint` to the wallet's own destination of the kind, never a new
    /// lock, its proof signed through `signer` (which signs with that
    /// lock's credit key and no other), the result mapped by
    /// [`reply_from`] with [`LockKeySigner::signed`]. A refusal before the
    /// signature is the error.
    fn apply(
        &self,
        outpoint: OutPoint,
        signer: &LockKeySigner<'_>,
    ) -> impl Future<Output = Result<ApplyReply, PlatformError>> + Send;
    /// After a proved apply: records the lock settled and starts the
    /// kind's sync, so the balance a proved read shows takes it in.
    fn settle(&self, txid: Txid) -> impl Future<Output = ()> + Send;
}

/// The signer a finish hands a kind's apply: the lease's funding signer
/// narrowed to one key, the one whose P2PKH script is the lock's credit
/// output. Any other signature is refused before the vault is asked for it
/// (`SignerError::KeyMismatch`), so a finish signs the lock's asset-lock
/// proof and nothing else: no Core input, no other lock. Public keys pass
/// through (the vault's scope bounds them).
pub(crate) struct LockKeySigner<'a> {
    inner: &'a VaultSigner,
    credit_script: ScriptBuf,
    signed: AtomicBool,
}

impl<'a> LockKeySigner<'a> {
    pub(crate) fn new(inner: &'a VaultSigner, credit_script: ScriptBuf) -> Self {
        Self {
            inner,
            credit_script,
            signed: AtomicBool::new(false),
        }
    }

    /// Whether it returned a signature: nothing can reach Platform before.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "type 4's kind (slice 5) uses it")
    )]
    pub(crate) fn signed(&self) -> bool {
        self.signed.load(Ordering::SeqCst)
    }

    /// Whether `key` is the one the lock's credit output pays.
    fn is_the_lock_key(&self, key: &PublicKey) -> bool {
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
    ) -> Result<(ecdsa::Signature, PublicKey), SignerError> {
        if !self.is_the_lock_key(&self.inner.public_key(path).await?) {
            tracing::warn!(%path, "a finish refused to sign with a key other than the lock's");
            return Err(SignerError::KeyMismatch(path.to_string()));
        }
        let signature = self.inner.sign_ecdsa(path, sighash).await?;
        self.signed.store(true, Ordering::SeqCst);
        Ok(signature)
    }

    async fn public_key(&self, path: &DerivationPath) -> Result<PublicKey, SignerError> {
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

/// What, besides the library's rows, decides a lock's standing.
pub(crate) struct Evidence<'a> {
    /// [`LockKind::own_locks`], read.
    pub(crate) own: &'a HashSet<Txid>,
    pub(crate) settled: &'a HashSet<Txid>,
    pub(crate) unknown: &'a HashSet<Txid>,
    /// The txids of more than one tracked lock of the kind (a lock
    /// transaction with several credit outputs the wallet's pools pay):
    /// `settled` and `unknown`, kept by txid, say nothing of one of them,
    /// which only the library's own row (per outpoint) finishes.
    pub(crate) shared: &'a HashSet<Txid>,
    /// The restored locks Platform reported already consumed.
    pub(crate) reported_used: &'a HashSet<OutPoint>,
    pub(crate) readiness: &'a Readiness,
}

/// `lock`'s standing as an unfinished lock of `kind`; `None` once it is
/// finished (the library's proved `Consumed`, or settled in the journal).
pub(crate) fn classify(kind: LockKindId, lock: &LockInfo, ev: &Evidence) -> Option<UnfinishedLock> {
    let txid = lock.outpoint.txid;
    let by_txid = !ev.shared.contains(&txid);
    if lock.status == LockStatus::Consumed || (by_txid && ev.settled.contains(&txid)) {
        return None;
    }
    let origin = if ev.own.contains(&txid) {
        LockOrigin::ThisDevice
    } else {
        LockOrigin::Restored
    };
    let state = if !lock.status.has_proof() {
        UnfinishedState::Confirming
    } else if origin == LockOrigin::Restored && ev.reported_used.contains(&lock.outpoint) {
        UnfinishedState::ReportedUsed
    } else if matches!(ev.readiness, Readiness::NotActive(_)) {
        UnfinishedState::NotActive
    } else if (by_txid && ev.unknown.contains(&txid))
        // The library's "consumption unknown" for a lock of this device's.
        || (origin == LockOrigin::ThisDevice && lock.status == LockStatus::RecoveredFromChain)
    {
        UnfinishedState::Checking
    } else {
        UnfinishedState::Ready
    };
    Some(UnfinishedLock {
        txid: txid.to_string(),
        vout: lock.outpoint.vout,
        kind,
        duffs: lock.amount,
        origin,
        state,
    })
}

/// The txids of more than one of `locks`.
fn shared_txids(locks: &[LockInfo]) -> HashSet<Txid> {
    let mut seen = HashSet::new();
    (locks.iter())
        .filter(|l| !seen.insert(l.outpoint.txid))
        .map(|l| l.outpoint.txid)
        .collect()
}

/// The wallet's unfinished locks of `kind`, `reported_used` the restored
/// ones Platform reported already consumed; `platform.unavailable` until
/// the dispatch journal is read.
pub(crate) async fn detect(
    kind: &impl LockKind,
    reported_used: &HashSet<OutPoint>,
) -> Result<Vec<UnfinishedLock>, PlatformError> {
    let own = kind.own_locks().ok_or(PlatformError::Unavailable)?;
    let locks = kind.locks().await;
    let (settled, unknown) = (kind.settled_locks().await, kind.unknown_locks().await);
    let readiness = kind
        .not_active()
        .map_or(Readiness::Ready, Readiness::NotActive);
    let shared = shared_txids(&locks);
    let ev = Evidence {
        own: &own,
        settled: &settled,
        unknown: &unknown,
        shared: &shared,
        reported_used,
        readiness: &readiness,
    };
    let mut out: Vec<_> = (locks.iter())
        .filter_map(|l| classify(kind.id(), l, &ev))
        .collect();
    out.sort_by(|a, b| (&a.txid, a.vout).cmp(&(&b.txid, b.vout)));
    Ok(out)
}

/// Finishes `kind`'s unfinished lock at `outpoint` under the lease
/// `lease_of` gives, begun only once the lock is ready (a refusal before
/// keeps the grant): the lease's funding signer, narrowed to the lock's
/// credit key ([`LockKeySigner`]), and one call permit sign the kind's
/// `FromExistingAssetLock` of the same outpoint. It funds and builds
/// nothing and waits for no proof. Read again under the kind's wallet
/// lock: a lock finished meanwhile is not applied again.
///
/// Only Platform's proved execution settles the lock. "Already consumed"
/// for the lock and a failure after its signature leave it unfinished,
/// [`LockFinish::Checking`]; a refusal before the signature is
/// [`LockFinish::NotApplied`].
pub(crate) async fn finish<K, F, Fut>(
    kind: &K,
    outpoint: OutPoint,
    lease_of: F,
) -> Result<LockFinish, PlatformError>
where
    K: LockKind,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Lease, PlatformError>>,
{
    let wallet_lock = kind.wallet_lock();
    let _held = wallet_lock.lock().await;
    let own = kind.own_locks().ok_or(PlatformError::Unavailable)?;
    let locks = kind.locks().await;
    let Some(lock) = locks.iter().find(|l| l.outpoint == outpoint) else {
        return Err(PlatformError::InvalidArgument {
            detail: "no tracked lock of that kind has that outpoint".into(),
        });
    };
    let readiness = kind.readiness().await;
    let (settled, unknown) = (kind.settled_locks().await, kind.unknown_locks().await);
    let shared = shared_txids(&locks);
    // A txid's records would speak for its other outputs too.
    let by_txid = !shared.contains(&outpoint.txid);
    let ev = Evidence {
        own: &own,
        settled: &settled,
        unknown: &unknown,
        shared: &shared,
        reported_used: &HashSet::new(),
        readiness: &readiness,
    };
    let Some(unfinished) = classify(kind.id(), lock, &ev) else {
        return Ok(LockFinish::Applied);
    };
    if unfinished.state == UnfinishedState::Confirming {
        return Ok(LockFinish::ProofWaiting);
    }
    if let Readiness::NotActive(code) | Readiness::Held(code) = readiness {
        return Ok(LockFinish::NotApplied { error: code });
    }
    let script = kind
        .credit_script(outpoint)
        .await
        .ok_or(PlatformError::Unavailable)?;
    let lease = lease_of().await?;
    let funding = lease.uncounted_funding_signer()?;
    let signer = LockKeySigner::new(&funding, script);
    let reply = lease
        .scope(async {
            let permit = lease.call_permit(kind.id().label(), None)?;
            let reply = kind.apply(outpoint, &signer).await;
            permit.finish(reply.is_ok());
            reply
        })
        .await;
    Ok(match reply {
        Ok(ApplyReply::Done) => {
            if by_txid {
                kind.settle(outpoint.txid).await;
            }
            LockFinish::Applied
        }
        // Neither settles (SR1): the lock stays unfinished until a proved
        // apply.
        Ok(how @ (ApplyReply::AlreadyConsumed | ApplyReply::Unknown)) => {
            tracing::warn!(%outpoint, ?how, "a lock's apply may have executed, unproved");
            if by_txid {
                kind.mark_unknown(outpoint.txid).await;
            }
            LockFinish::Checking
        }
        Err(e) => {
            tracing::warn!(%outpoint, error = %e, "a lock's apply was refused before its signature");
            LockFinish::NotApplied {
                error: e.code().into(),
            }
        }
    })
}
