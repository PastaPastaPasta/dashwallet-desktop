//! Unfinished transfers (0.2 recovery; m4-dashpay-engine §2.14): funding
//! asset locks whose credit output pays one of the wallet's funding keys
//! and whose Platform side is not known to have happened, Core → Shielded
//! (funding type 5, [`ShieldLocks`]) and Core → the wallet's own Platform
//! address (type 4, [`FundLocks`]), its own and those a restore read back
//! from the chain, and finishing one on the same outpoint. Only Platform's
//! proved execution finishes a lock; nothing here funds, builds a lock or
//! changes a balance.

use std::collections::HashSet;
use std::sync::Arc;

use dashcore::{OutPoint, Txid};
use serde::{Deserialize, Serialize};

use super::dashpay::DashPay;
use super::errors::{CreditsError, PlatformError, ShieldedError};
use super::flows::FlowKind;
use super::lease::Lease;
use super::platform_fund::{self, FundLib, PwFund};
use super::shielded_pool::{PwShielded, on_pool, on_pool_signing};
use super::unfinished_funds::FundLocks;
use super::unfinished_locks::{self as locks, LockKind};
use super::unfinished_shields::ShieldLocks;
use crate::{NetworkSession, WalletId};

/// A kind of funding lock whose apply moves the lock's value to the
/// wallet's own Platform-side balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LockKindId {
    /// `AssetLockShieldedAddressTopUp` (type 5): to the wallet's shielded
    /// account 0.
    Shielded,
    /// `AssetLockAddressTopUp` (type 4): to one of the wallet's own
    /// Platform payment addresses.
    PlatformAddress,
}

/// Where an unfinished lock came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LockOrigin {
    /// One of the kind's funding steps on this install signed it.
    ThisDevice,
    /// No funding step here signed it: a restore read it back from the
    /// chain (an earlier install's, or another device's).
    Restored,
}

/// What an unfinished lock waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnfinishedState {
    /// No InstantSend or ChainLock proof yet: nothing to apply.
    Confirming,
    /// Proved and not tried, or tried and refused before its signature.
    Ready,
    /// An apply may have executed, unproved: it failed after its signature,
    /// or Platform answered "already consumed" for it.
    Checking,
    /// Platform answered "already consumed" for this restored lock,
    /// unproved: listed apart, and checked again on request. Reserved: not
    /// set in this version.
    ReportedUsed,
    /// The kind is not active on this network's Platform yet.
    NotActive,
    /// Can't be finished in this version: a credit output other than its
    /// transaction's first (`vout` ≠ 0), whose key the pinned library's
    /// apply does not re-derive. Never offered a finish.
    Unsupported,
    /// Too small to finish: worth less than the Platform fee its apply
    /// pays, so the apply would leave nothing. Never offered a finish.
    TooSmall,
}

/// A funding lock paying one of the wallet's funding keys whose Platform
/// side is not known to have happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnfinishedLock {
    pub txid: String,
    /// The credit output's index in the lock's payload.
    pub vout: u32,
    pub kind: LockKindId,
    /// The credit output's value.
    pub duffs: u64,
    pub origin: LockOrigin,
    pub state: UnfinishedState,
}

/// How finishing one lock ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LockFinish {
    /// Platform's proved execution: the lock is settled.
    Applied,
    /// Platform answered "already consumed" for this restored lock,
    /// unproved: nothing settled, nothing counted. Reserved: not returned
    /// in this version (the lock reads `Checking`).
    ReportedUsed,
    /// The apply may have executed, unproved.
    Checking,
    /// The lock has no proof yet.
    ProofWaiting,
    /// Refused before the lock's credit key signed anything: nothing reached
    /// Platform. `error` is the refusal's code.
    NotApplied { error: String },
}

/// What Repair's "Finish transfers" did with one kind's unfinished
/// transfers ([`super::FinishReport::by_kind`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransfersReport {
    pub kind: LockKindId,
    /// Applies tried.
    pub resumed: u32,
    /// Proved applied.
    pub completed: u32,
    /// Still unfinished: confirming, not active, not finishable in this
    /// version, too small, refused, or checking.
    pub still_pending: u32,
}

/// The wallet's unfinished transfers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnfinishedTransfers {
    /// By txid, then output.
    pub locks: Vec<UnfinishedLock>,
    /// SPV and the wallet's bring-up (with it a restore's scan) have caught
    /// up: until then a restored lock may be missing.
    pub complete: bool,
    /// The kinds whose locks could not be read (their side not open, the
    /// dispatch journal unread): none of theirs is listed, and the host says
    /// they couldn't be checked. The other kinds' are listed all the same.
    pub unread: Vec<LockKindId>,
}

impl DashPay {
    /// The wallet's unfinished transfers ([`UnfinishedTransfers`]), each
    /// kind read on its own side: one that can't be read (the shielded side
    /// not open, the dispatch journal unread) is `unread`, the others listed
    /// all the same. With no kind read, the first one's refusal
    /// (`platform.unavailable` until the journal is read).
    pub async fn unfinished_transfers(&self) -> Result<UnfinishedTransfers, PlatformError> {
        let wallet = self.wallet_id;
        let shields = on_pool(&self.session, wallet, move |session, lib| async move {
            Ok(kind_transfers(&ShieldLocks::new(&session, wallet, &lib)).await?)
        })
        .await
        .map_err(platform);
        let funds = platform_fund::on_lib(
            &self.session,
            wallet,
            false,
            move |session, lib| async move {
                Ok(kind_transfers(&FundLocks::new(&session, wallet, &lib)).await?)
            },
        )
        .await
        .map_err(credits);
        transfers([
            (LockKindId::Shielded, shields),
            (LockKindId::PlatformAddress, funds),
        ])
    }

    /// Finishes the unfinished transfer of `kind` at `txid`:`vout`: the
    /// lock's `FromExistingAssetLock` to the wallet's own destination of the
    /// kind, under a lease of the kind's flow begun from `grant` (or named
    /// by it), signed with the lock's credit key and no other key. Never a
    /// new lock, new funding or other coins; only a proved apply settles it.
    pub async fn finish_transfer(
        &self,
        kind: LockKindId,
        txid: String,
        vout: u32,
        grant: String,
    ) -> Result<LockFinish, PlatformError> {
        let wallet = self.wallet_id;
        let outpoint = move || -> Result<OutPoint, PlatformError> {
            let txid: Txid = (txid.trim().parse()).map_err(|_| PlatformError::InvalidArgument {
                detail: "txid is not a hex txid".into(),
            })?;
            Ok(OutPoint { txid, vout })
        };
        match kind {
            LockKindId::Shielded => {
                on_pool_signing(&self.session, wallet, move |session, lib| async move {
                    let outpoint = outpoint()?;
                    let kind = ShieldLocks::new(&session, wallet, &lib);
                    let lease = || authority(&session, wallet, &grant, kind.flow());
                    Ok(locks::finish(&kind, outpoint, lease).await?)
                })
                .await
                .map_err(platform)
            }
            LockKindId::PlatformAddress => platform_fund::on_lib(
                &self.session,
                wallet,
                true,
                move |session, lib| async move {
                    let outpoint = outpoint()?;
                    Ok(finish_fund(&session, wallet, &lib, outpoint, &grant).await?)
                },
            )
            .await
            .map_err(credits),
        }
    }
}

/// One kind's unfinished locks, and whether its side has caught up.
pub(crate) async fn kind_transfers(
    kind: &impl LockKind,
) -> Result<(Vec<UnfinishedLock>, bool), PlatformError> {
    Ok((
        locks::detect(kind, &HashSet::new()).await?,
        kind.caught_up(),
    ))
}

/// The kinds' unfinished transfers together, by txid, then output: a kind
/// that could not be read is `unread`, the others listed all the same;
/// with none read, the first kind's refusal.
pub(crate) fn transfers(
    kinds: impl IntoIterator<
        Item = (
            LockKindId,
            Result<(Vec<UnfinishedLock>, bool), PlatformError>,
        ),
    >,
) -> Result<UnfinishedTransfers, PlatformError> {
    let (mut listed, mut complete, mut unread, mut refused) = (Vec::new(), true, Vec::new(), None);
    let mut read = false;
    for (kind, got) in kinds {
        match got {
            Ok((locks, caught_up)) => {
                read = true;
                listed.extend(locks);
                complete &= caught_up;
            }
            Err(e) => {
                tracing::warn!(?kind, error = %e, "could not read a kind's unfinished transfers");
                unread.push(kind);
                refused.get_or_insert(e);
            }
        }
    }
    if let (false, Some(e)) = (read, refused) {
        return Err(e);
    }
    listed.sort_by(|a, b| (a.txid.as_str(), a.vout).cmp(&(b.txid.as_str(), b.vout)));
    Ok(UnfinishedTransfers {
        locks: listed,
        complete,
        unread,
    })
}

/// `finish_transfer` of the type-4 lock at `outpoint` over `lib`.
pub(crate) async fn finish_fund(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl FundLib,
    outpoint: OutPoint,
    grant: &str,
) -> Result<LockFinish, PlatformError> {
    let kind = FundLocks::new(session, wallet, lib);
    let lease = || authority(session, wallet, grant, kind.flow());
    locks::finish(&kind, outpoint, lease).await
}

/// Repair's "Finish transfers" over the unfinished transfers, after the
/// registrations and top-ups and under their `lease`: every kind's
/// unfinished locks, one at a time ([`locks::finish_kind`]).
pub(crate) async fn finish_all(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lease: &Lease,
    report: &mut super::FinishReport,
) {
    match PwShielded::open(session, wallet).await {
        Ok(lib) => {
            locks::finish_kind(&ShieldLocks::new(session, wallet, &lib), lease, report).await
        }
        Err(e) => {
            tracing::warn!(error = %e, "Finish transfers could not open the shielded side");
            report.unread.push(LockKindId::Shielded);
        }
    }
    match PwFund::open(session, wallet).await {
        Ok(lib) => locks::finish_kind(&FundLocks::new(session, wallet, &lib), lease, report).await,
        Err(e) => {
            tracing::warn!(error = %e, "Finish transfers could not open the Platform address side");
            report.unread.push(LockKindId::PlatformAddress);
        }
    }
}

/// The lease `grant` names (a `flow` lease of this wallet), or a new one
/// redeemed from it. A grant with no funding budget is refused before it is
/// redeemed (the funding signer needs one), so it is kept.
pub(super) async fn authority(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    grant: &str,
    flow: FlowKind,
) -> Result<Lease, PlatformError> {
    let funding = super::flows::BudgetPurpose::Funding;
    if super::registration::grant_cap(session, wallet, grant, funding).await? == 0 {
        return Err(PlatformError::NeedsGrant { purpose: funding });
    }
    match session.lease_for(&wallet, grant) {
        Some(lease) => {
            let lease = lease?;
            if lease.flow() != flow {
                return Err(PlatformError::GrantInvalid);
            }
            Ok(lease)
        }
        None => Ok(session
            .begin_lease(wallet, flow, &[grant.to_owned()])
            .await?),
    }
}

/// The fund side's refusals here are all Platform's.
fn credits(e: CreditsError) -> PlatformError {
    match e {
        CreditsError::Platform(e) => e,
        e => PlatformError::Internal {
            detail: e.to_string(),
        },
    }
}

/// The pool's refusals here are all Platform's: the bodies return only
/// [`PlatformError`].
fn platform(e: ShieldedError) -> PlatformError {
    match e {
        ShieldedError::Platform(e) => e,
        e => PlatformError::Internal {
            detail: e.to_string(),
        },
    }
}
