//! Identity top-up (DP1-06, DASHPAY F8): an asset lock funded from the
//! Core balance, then Platform's IdentityTopUp from it.
//!
//! No table of its own: a top-up's state is platform-wallet's tracked
//! `IdentityTopUp` lock and P2b's dispatch journal (the funding step's
//! marker and its signed locks, E0-04 §2a). Platform takes a lock once, so
//! the lock is the idempotency key past the funding, and the funding is
//! held to at most once by its step:
//!
//! - **The step.** Identity `i`'s top-ups fund under `topup/<i>-<k>/funding`,
//!   `k` the first of its steps the journal does not read `Sent` (one that
//!   funded a lock). A step read `MaybeSent` or `WillBeSent` refuses the
//!   top-up (`platform.broadcast_unknown`, `platform.will_be_sent`), so a
//!   funding whose outcome is unknown is never repeated; one read `NotSent`
//!   is retried under the same step, which the funding gate admits only so.
//! - **The lock.** While a top-up lock of the identity is [`Standing::Open`]
//!   or [`Standing::ConsumptionUnknown`], a new top-up is refused
//!   `credits.top_up_pending{txid}`: the host applies that one
//!   (`resume_top_up`) rather than funding again. A lock is tracked before
//!   it is broadcast (H12), so a lock that may be out is always found.
//! - **The apply** builds nothing: `FromExistingAssetLock` signs only the
//!   transition's proof with the lock's credit key, so it runs under a
//!   plain call permit with the lease's uncounted funding signer, taken
//!   before the funding call charges the budget.
//!
//! Only the proved apply settles a lock: the library then marks it
//! `Consumed`, and the journal records it settled (`top_up_settled`,
//! schema 8). Platform's "already consumed" proves nothing (any node can
//! say it), so it reads [`TopUpState::ConsumptionUnknown`] and settles
//! nothing; the library keeps such a lock, and one a restore read back
//! from the chain, `RecoveredFromChain` (final, consumption unknown). A
//! lock a dw top-up funded is known by its funding step's signed-lock
//! record, which the journal keeps for good: such a lock stays pending
//! and blocking until a proved apply or the user's `resolve_top_up`, both
//! recorded in the journal too. Those records outlive a wallet's removal
//! like the signed locks do, while the library's `Consumed` mark goes with
//! `wallet.sqlite`: a wallet removed and imported again reads its applied
//! top-ups back settled (review). Any other recovered lock is historical,
//! an earlier top-up of the identity: it neither pends nor blocks, and
//! "Finish transfers" still offers it to Platform. Until the journal is
//! read nothing can be told apart, so every call but Finish answers
//! `platform.unavailable` (review L5).
//!
//! The identity's write lock (`identity_writes.rs`) is held throughout,
//! from before the grant is redeemed, so a balance read before and after
//! the apply measures it alone.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use dashcore::{OutPoint, Txid};
use dpp::prelude::Identifier;
use dw_appdb::dispatch::TopUpSettled;
use dw_vault::VaultSigner;
use platform_wallet::AssetLockFundingType;

use super::credits::{PendingTopUp, TopUpOutcome, TopUpQuote, TopUpState};
use super::errors::{CreditsError, IdentityError, PlatformError};
use super::flows::{BudgetPurpose, FlowKind, GrantRequest};
use super::identity_writes::identity_locks;
use super::lease::mode_b::TrackedRow;
use super::lease::{CallPermit, FundingKey, Lease, LeaseError};
use super::registration::{FinishReport, RegistrationWait, funding_cap};
use super::registration_flow::{
    FundError, LockInfo, LockStatus, PwLib, RegistrationLib as Reg, Spendable, parse_identity,
};
use crate::{EngineError, NetworkSession, WalletId};

/// How long `resume_top_up` waits for a lock's proof (a `Built` lock is
/// broadcast again first).
const RESUME_PROOF_WAIT: Duration = Duration::from_secs(30);

/// What the apply returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    /// Platform's new balance, proved.
    Balance(u64),
    /// Platform reported the lock taken already, unproved.
    Taken,
}

/// What a tracked top-up lock is to the gates (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// Out with no proof yet, or proved and not applied: it pends and
    /// blocks the identity's top-ups.
    Open,
    /// A lock a dw top-up funded, final, its consumption unknown: it pends
    /// and blocks until a proved apply or the user's resolution.
    ConsumptionUnknown,
    /// Applied with proof or resolved by the user (recorded in the
    /// journal), or historical.
    Settled,
}

/// What the gates know of the wallet's locks beyond their status.
struct Known {
    /// The locks a dw top-up funded; `None` until the journal is read, when
    /// every recovered lock counts as one (fail closed).
    ours: Option<HashSet<Txid>>,
    /// The locks the journal records settled (a proved apply or the
    /// user's word); none when they cannot be read (they pend, fail
    /// closed).
    settled: HashSet<Txid>,
}

impl Known {
    /// Also records settled each lock of ours among `locks` the library
    /// holds `Consumed` and the journal does not yet (one applied before
    /// the record existed, or whose record failed), so a wallet removed
    /// and imported again still reads it settled.
    async fn of(session: &NetworkSession, wallet: WalletId, locks: &[LockInfo]) -> Self {
        let settled = (session.leases.top_ups_settled(wallet).await).unwrap_or_else(|e| {
            tracing::warn!(wallet_id = %wallet, error = %e, "could not read the settled top-ups");
            HashSet::new()
        });
        let known = Self {
            ours: session.leases.top_up_lock_txids(wallet),
            settled,
        };
        for lock in locks {
            let txid = lock.outpoint.txid;
            if lock.status == LockStatus::Consumed
                && !known.settled.contains(&txid)
                && known.ours.as_ref().is_some_and(|o| o.contains(&txid))
            {
                settle(session, wallet, txid, TopUpSettled::Applied).await;
            }
        }
        known
    }

    fn standing(&self, lock: &LockInfo) -> Standing {
        let txid = lock.outpoint.txid;
        match lock.status {
            LockStatus::Consumed => Standing::Settled,
            _ if self.settled.contains(&txid) => Standing::Settled,
            LockStatus::RecoveredFromChain
                if self.ours.as_ref().is_some_and(|o| !o.contains(&txid)) =>
            {
                Standing::Settled
            }
            LockStatus::RecoveredFromChain => Standing::ConsumptionUnknown,
            _ => Standing::Open,
        }
    }
}

/// Records the lock `txid` settled in the journal; a failed write is
/// logged (the lock then reads from the library's mark, or pends).
async fn settle(session: &NetworkSession, wallet: WalletId, txid: Txid, how: TopUpSettled) {
    if let Err(e) = session.leases.settle_top_up(wallet, txid, how).await {
        tracing::warn!(%txid, ?how, error = %e, "could not record a top-up's lock settled");
    }
}

/// platform-wallet as a top-up uses it; [`PwLib`] is the real one, tests
/// script a fake.
pub(crate) trait TopUpLib: Send + Sync {
    /// The wallet's identities: index and base58 id.
    fn identities(&self) -> impl Future<Output = Vec<(u32, String)>> + Send;
    /// The wallet's tracked `IdentityTopUp` locks.
    fn top_up_locks(&self) -> impl Future<Output = Vec<LockInfo>> + Send;
    fn spendable(&self) -> impl Future<Output = Result<Spendable, PlatformError>> + Send;
    /// E0-04's `fee_bound_worst`: the funding permit's fee quote.
    fn fee_bound(&self) -> impl Future<Output = Result<u64, PlatformError>> + Send;
    /// What holds a funding back (SPV behind, the identities unknown), as
    /// for a registration.
    fn funding_hold(&self) -> Option<RegistrationWait>;
    fn funding_permit(
        &self,
        lease: &Lease,
        step: String,
        key: FundingKey,
        duffs: u64,
    ) -> impl Future<Output = Result<CallPermit, FundError>> + Send;
    /// Builds, tracks and broadcasts the lock with `permit`'s counting
    /// funding signer, and waits for its InstantSend lock.
    fn fund(
        &self,
        duffs: u64,
        identity_index: u32,
        permit: &CallPermit,
    ) -> impl Future<Output = Result<OutPoint, FundError>> + Send;
    fn wait_proof(
        &self,
        outpoint: OutPoint,
        timeout: Duration,
    ) -> impl Future<Output = Result<(), FundError>> + Send;
    /// The tracked lock at `outpoint` as P2b reads it, for the resume gate.
    fn tracked_row(&self, outpoint: OutPoint) -> impl Future<Output = Option<TrackedRow>> + Send;
    /// Platform's balance of `identity`, proved and stored; `None` when
    /// Platform has none.
    fn refresh_balance(
        &self,
        identity: Identifier,
    ) -> impl Future<Output = Result<Option<u64>, PlatformError>> + Send;
    /// IdentityTopUp of `identity` from the proved lock at `outpoint`.
    fn apply(
        &self,
        identity: Identifier,
        outpoint: OutPoint,
        signer: &VaultSigner,
    ) -> impl Future<Output = Result<Applied, PlatformError>> + Send;
}

/// Runs `body` on the engine runtime with the wallet's library.
pub(crate) async fn on_lib<T, E, F, Fut>(
    session: &Arc<NetworkSession>,
    wallet_id: WalletId,
    body: F,
) -> Result<T, E>
where
    T: Send + 'static,
    E: From<PlatformError> + Send + 'static,
    F: FnOnce(Arc<NetworkSession>, Arc<PwLib>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, E>> + Send + 'static,
{
    let session = Arc::clone(session);
    let rt = session.rt.clone();
    rt.spawn(async move {
        let _op = session.enter().await.map_err(PlatformError::from)?;
        session
            .require_wallet(&wallet_id)
            .map_err(PlatformError::from)?;
        let (_, lib) = PwLib::open(&session, wallet_id).await?;
        body(Arc::clone(&session), Arc::new(lib)).await
    })
    .await
    .map_err(|e| PlatformError::from(EngineError::from(e)))?
}

/// What a top-up of `duffs` to `identity` needs, checked by the quote and
/// by the top-up under the identity's write lock.
struct Plan {
    identity: Identifier,
    index: u32,
    fee_duffs: u64,
    total_duffs: u64,
    credits: u64,
}

async fn check<L: TopUpLib>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &L,
    identity: &str,
    duffs: u64,
) -> Result<Plan, CreditsError> {
    let id = parse_identity(identity)?;
    let index = (lib.identities().await.into_iter())
        .find(|(_, i)| *i == identity)
        .map(|(index, _)| index)
        .ok_or(PlatformError::Identity(IdentityError::NotFound))?;
    let table = session.cost_table()?;
    if duffs < table.top_up_min_duffs {
        return Err(CreditsError::BelowMinimum {
            min: table.top_up_min_duffs,
        });
    }
    if let Some((lock, _)) = (open_locks(session, wallet, lib).await?.into_iter())
        .find(|(l, _)| l.identity_index == index)
    {
        return Err(CreditsError::TopUpPending {
            txid: lock.outpoint.txid.to_string(),
        });
    }
    let fee_duffs = lib.fee_bound().await?;
    let spendable = lib.spendable().await?;
    let total_duffs = duffs.saturating_add(fee_duffs);
    if spendable.duffs < total_duffs {
        return Err(CreditsError::FundingInsufficient {
            needed: total_duffs,
            available: spendable.duffs,
        });
    }
    if spendable.protected > 0 {
        return Err(CreditsError::CoinsLocked);
    }
    Ok(Plan {
        identity: id,
        index,
        fee_duffs,
        total_duffs,
        credits: duffs
            .saturating_mul(table.credits_per_duff)
            .saturating_sub(table.top_up_fee),
    })
}

/// The wallet's top-up locks that pend and block (not [`Standing::Settled`]):
/// a new top-up of their identity would fund twice. `platform.unavailable`
/// until the dispatch journal is read: no recovered lock can be told
/// apart before (review L5).
async fn open_locks<L: TopUpLib>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &L,
) -> Result<Vec<(LockInfo, Standing)>, PlatformError> {
    if !session.leases.journal_ready() {
        return Err(PlatformError::Unavailable);
    }
    let locks = lib.top_up_locks().await;
    let known = Known::of(session, wallet, &locks).await;
    Ok((locks.into_iter())
        .filter_map(|l| {
            let standing = known.standing(&l);
            (standing != Standing::Settled).then_some((l, standing))
        })
        .collect())
}

/// The base58 id of the wallet's identity at `index`.
fn identity_at(identities: &[(u32, String)], index: u32) -> Option<&String> {
    (identities.iter())
        .find(|(i, _)| *i == index)
        .map(|(_, id)| id)
}

pub(crate) async fn quote<L: TopUpLib>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &L,
    identity: &str,
    duffs: u64,
) -> Result<TopUpQuote, CreditsError> {
    let plan = check(session, wallet, lib, identity, duffs).await?;
    Ok(TopUpQuote {
        fee_duffs: plan.fee_duffs,
        total_duffs: plan.total_duffs,
        credits: plan.credits,
        grant: GrantRequest {
            max_duffs: plan.total_duffs,
            max_credits: 0,
        },
    })
}

/// The lease `grant` names (a `TopUp` lease of this wallet), or a new one
/// redeemed from it.
async fn authority(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    grant: &str,
) -> Result<Lease, PlatformError> {
    match session.lease_for(&wallet, grant) {
        Some(lease) => {
            let lease = lease?;
            if lease.flow() != FlowKind::TopUp {
                return Err(PlatformError::GrantInvalid);
            }
            Ok(lease)
        }
        None => Ok(session
            .begin_lease(wallet, FlowKind::TopUp, &[grant.to_owned()])
            .await?),
    }
}

/// What holds a funding back, as the code the host sees: SPV behind,
/// the identities not unlocked yet, or not known.
fn held(wait: RegistrationWait) -> PlatformError {
    match wait {
        RegistrationWait::Unlock => PlatformError::SignerUnavailable,
        RegistrationWait::Network => PlatformError::Unavailable,
        _ => PlatformError::ContextUnavailable,
    }
}

/// Identity `index`'s top-up step `k` (module doc).
pub(super) fn top_up_step(index: u32, k: u32) -> String {
    format!("topup/{index}-{k}/funding")
}

/// The step the next top-up of identity `index` funds under: the first
/// the funding gate admits. A step that funded a lock (`Sent`), whose
/// marker stands over nothing sent (a crash before its signature) or that
/// Repair resolved is passed; one that may be out (`MaybeSent`,
/// `WillBeSent`) refuses the top-up.
fn next_step(
    session: &NetworkSession,
    wallet: WalletId,
    index: u32,
) -> Result<String, PlatformError> {
    use super::flows::DispatchState as D;
    use super::lease::FundingBlock as B;
    for k in 0.. {
        let step = top_up_step(index, k);
        match session.leases.funding_block(wallet, &step) {
            None => return Ok(step),
            Some(B::Step(Some(D::MaybeSent))) => {
                return Err(PlatformError::BroadcastUnknown { artifact: step });
            }
            Some(B::Step(Some(D::WillBeSent))) => {
                return Err(PlatformError::WillBeSent { artifact: step });
            }
            Some(B::Step(_) | B::StepCancelled) => {}
            Some(block) => return Err(LeaseError::FundingBlocked(block).into()),
        }
    }
    unreachable!("the steps are unbounded")
}

pub(crate) async fn top_up<L: TopUpLib>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &L,
    identity: &str,
    duffs: u64,
    grant: &str,
) -> Result<TopUpOutcome, CreditsError> {
    if let Some(wait) = lib.funding_hold() {
        return Err(held(wait).into());
    }
    // Until the dispatch journal is read, every step would read `MaybeSent`
    // and the recovered locks could not be told apart: a retry after the
    // load funds (review L5).
    if !session.leases.journal_ready() {
        return Err(PlatformError::Unavailable.into());
    }
    let locks = identity_locks(wallet, parse_identity(identity)?);
    let _write = locks.write.lock().await;
    // Under the write lock, so no concurrent top-up of the identity funds
    // in between, and before the grant is redeemed, so a refusal keeps it.
    let plan = check(session, wallet, lib, identity, duffs).await?;
    if !session.leases.refresh_funding(wallet).await {
        return Err(PlatformError::Unavailable.into());
    }
    let step = next_step(session, wallet, plan.index)?;
    // The cap is read without redeeming the grant, so a grant too small
    // for the quote is left for a smaller top-up.
    let remaining = funding_cap(session, wallet, grant).await?;
    if plan.total_duffs > remaining {
        return Err(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Funding,
            needed: plan.total_duffs,
            remaining,
        }
        .into());
    }
    let lease = authority(session, wallet, grant).await?;
    // Before the funding charges the budget (module doc).
    let signer = lease
        .uncounted_funding_signer()
        .map_err(PlatformError::from)?;
    let key = FundingKey {
        funding_type: AssetLockFundingType::IdentityTopUp,
        identity_index: plan.index,
        account: 0,
    };
    let funded = lease
        .scope_step(Some(step.clone()), async {
            let permit = lib.funding_permit(&lease, step.clone(), key, duffs).await?;
            let funded = lib.fund(duffs, plan.index, &permit).await;
            permit.finish(funded.is_ok());
            funded
        })
        .await;
    // The step's own lock decides, never the call's result alone: a lock
    // the step did not sign is not this top-up's.
    let signed = session.leases.signed_lock_txids(wallet, &step);
    let own = (lib.top_up_locks().await.into_iter()).find(|l| signed.contains(&l.outpoint.txid));
    let Some(lock) = own else {
        return Err(match funded {
            Err(FundError::Refused(e) | FundError::NotSent(e)) => e.into(),
            Err(FundError::CoinsLocked) => CreditsError::CoinsLocked,
            // Signed and not tracked (or tracked and gone): its outcome is
            // the step's to tell.
            _ => PlatformError::BroadcastUnknown { artifact: step }.into(),
        });
    };
    let txid = lock.outpoint.txid.to_string();
    if !lock.status.has_proof() {
        tracing::info!(%txid, ?funded, "a top-up's lock waits for its proof");
        return Ok(TopUpOutcome {
            txid,
            state: TopUpState::ProofWaiting,
        });
    }
    let state = apply(
        session,
        wallet,
        lib,
        &lease,
        &signer,
        plan.identity,
        lock.outpoint,
    )
    .await;
    Ok(TopUpOutcome { txid, state })
}

/// Applies the proved lock at `outpoint` to `identity` under a call
/// permit of `lease`, the identity's write lock held by the caller; a
/// proved apply is recorded settled in the journal.
async fn apply<L: TopUpLib>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &L,
    lease: &Lease,
    signer: &VaultSigner,
    identity: Identifier,
    outpoint: OutPoint,
) -> TopUpState {
    let before = balance_of(lib, identity).await;
    let applied = lease
        .scope(async {
            let permit = lease.call_permit("top-up", None)?;
            let applied = lib.apply(identity, outpoint, signer).await;
            permit.finish(applied.is_ok());
            applied
        })
        .await;
    match applied {
        Ok(Applied::Balance(balance)) => {
            settle(session, wallet, outpoint.txid, TopUpSettled::Applied).await;
            TopUpState::Applied {
                credits_added: before.map(|b| balance.saturating_sub(b)),
                balance: Some(balance),
            }
        }
        Ok(Applied::Taken) => {
            tracing::warn!(%outpoint, "Platform reports a top-up's lock taken, unproved; it stays pending");
            TopUpState::ConsumptionUnknown {
                balance: balance_of(lib, identity).await,
            }
        }
        Err(e) => {
            tracing::warn!(%outpoint, error = %e, "a top-up's apply failed; the lock stays tracked");
            TopUpState::NotApplied {
                error: e.code().to_owned(),
            }
        }
    }
}

fn invalid(detail: &str) -> PlatformError {
    PlatformError::InvalidArgument {
        detail: detail.into(),
    }
}

fn parse_txid(txid: &str) -> Result<Txid, PlatformError> {
    txid.parse().map_err(|_| invalid("txid is not a hex txid"))
}

/// `identity`'s balance, read again; `None` when it cannot be.
async fn balance_of<L: TopUpLib>(lib: &L, identity: Identifier) -> Option<u64> {
    lib.refresh_balance(identity).await.ok().flatten()
}

/// The wallet's top-up locks that pend ([`open_locks`]), oldest identity
/// index first.
pub(crate) async fn pending<L: TopUpLib>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &L,
) -> Result<Vec<PendingTopUp>, PlatformError> {
    let identities = lib.identities().await;
    let mut out: Vec<PendingTopUp> = (open_locks(session, wallet, lib).await?.into_iter())
        .map(|(l, standing)| PendingTopUp {
            txid: l.outpoint.txid.to_string(),
            identity: identity_at(&identities, l.identity_index).cloned(),
            identity_index: l.identity_index,
            duffs: l.amount,
            proved: l.status.has_proof(),
            consumption_unknown: standing == Standing::ConsumptionUnknown,
        })
        .collect();
    out.sort_by(|a, b| (a.identity_index, &a.txid).cmp(&(b.identity_index, &b.txid)));
    Ok(out)
}

pub(crate) async fn resume<L: TopUpLib>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &L,
    txid: &str,
    grant: &str,
) -> Result<TopUpOutcome, CreditsError> {
    let txid = parse_txid(txid)?;
    let lock_of = async || {
        (lib.top_up_locks().await.into_iter())
            .find(|l| l.outpoint.txid == txid)
            .ok_or_else(|| invalid("no top-up lock has that txid"))
    };
    let index = lock_of().await?.identity_index;
    let identity = identity_at(&lib.identities().await, index)
        .and_then(|id| parse_identity(id).ok())
        .ok_or(PlatformError::Identity(IdentityError::NotFound))?;
    let locks = identity_locks(wallet, identity);
    let _write = locks.write.lock().await;
    let txid_hex = txid.to_string();
    let outcome = |state| TopUpOutcome {
        txid: txid_hex.clone(),
        state,
    };
    // Read again under the write lock: a concurrent apply may have taken
    // it. Only the library's proved mark is taken: a lock Platform said
    // was taken, unproved, a resolved or a historical one, is asked again.
    let lock = lock_of().await?;
    if lock.status == LockStatus::Consumed {
        return Ok(outcome(TopUpState::AlreadyApplied {
            balance: balance_of(lib, identity).await,
        }));
    }
    if !lock.status.has_proof() {
        // The wait broadcasts a `Built` lock again: never one a funding
        // call of this process signed before its record is durable (E0-04
        // §2a.7).
        if let Some(row) = lib.tracked_row(lock.outpoint).await
            && let Some(step) = session.leases.unrecorded_signer_of(wallet, &row)
        {
            return Err(PlatformError::WillBeSent { artifact: step }.into());
        }
        if lib
            .wait_proof(lock.outpoint, RESUME_PROOF_WAIT)
            .await
            .is_err()
        {
            return Ok(outcome(TopUpState::ProofWaiting));
        }
    }
    // Only the apply needs the grant: every refusal above keeps it.
    let lease = authority(session, wallet, grant).await?;
    let signer = lease
        .uncounted_funding_signer()
        .map_err(PlatformError::from)?;
    let state = apply(
        session,
        wallet,
        lib,
        &lease,
        &signer,
        identity,
        lock.outpoint,
    )
    .await;
    Ok(outcome(state))
}

/// Records the user's resolution of the pending, proved top-up lock `txid`
/// (`resolve_top_up`) in the journal: a `consumption_unknown` one, or one
/// a resume answered `ConsumptionUnknown` while its ChainLock was still to
/// come. Under its identity's write lock, so no apply of it runs
/// meanwhile; a lock without a proof was never applied and is refused.
pub(crate) async fn resolve<L: TopUpLib>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &L,
    txid: &str,
) -> Result<(), CreditsError> {
    let txid = parse_txid(txid)?;
    let lock_of = async || {
        (open_locks(session, wallet, lib).await?.into_iter())
            .find(|(l, _)| l.outpoint.txid == txid && l.status.has_proof())
            .ok_or_else(|| invalid("no pending top-up of that txid is proved"))
    };
    let index = lock_of().await?.0.identity_index;
    let identity = identity_at(&lib.identities().await, index)
        .cloned()
        .ok_or(PlatformError::Identity(IdentityError::NotFound))?;
    let locks = identity_locks(wallet, parse_identity(&identity)?);
    let _write = locks.write.lock().await;
    lock_of().await?;
    (session.leases)
        .settle_top_up(wallet, txid, TopUpSettled::Resolved)
        .await
        .map_err(|e| {
            tracing::warn!(%txid, error = %e, "could not record a top-up's resolution");
            PlatformError::Unavailable
        })?;
    tracing::info!(%txid, "the user resolved a top-up lock");
    Ok(())
}

/// "Finish transfers" for top-ups: every proved top-up lock of a known
/// identity that the library does not hold applied is applied under
/// `lease`, a settled one too (a historical or resolved lock Platform had
/// not taken credits the identity). Builds no lock and waits for no proof:
/// a lock without one stays pending (E0-04's catch-up sends it again). An
/// unproved "already consumed" leaves a pending lock pending. A settled
/// lock is counted only when Platform applies it: any other answer
/// (already consumed, a cached refusal of the same transition, an error)
/// counts nothing, as `pending_top_ups` lists nothing for it.
pub(crate) async fn finish<L: TopUpLib>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &L,
    lease: &Lease,
    report: &mut FinishReport,
) {
    let identities = lib.identities().await;
    let signer = match lease.uncounted_funding_signer() {
        Ok(signer) => Some(signer),
        Err(e) => {
            tracing::warn!(error = %e, "Finish transfers has no funding signer for top-ups");
            None
        }
    };
    let all = lib.top_up_locks().await;
    let known = Known::of(session, wallet, &all).await;
    let applied = |l: &LockInfo| l.status == LockStatus::Consumed;
    for lock in all {
        if applied(&lock) {
            continue;
        }
        let settled = known.standing(&lock) == Standing::Settled;
        let identity =
            identity_at(&identities, lock.identity_index).and_then(|id| parse_identity(id).ok());
        let (Some(identity), Some(signer), true) = (identity, &signer, lock.status.has_proof())
        else {
            report.still_pending += u32::from(!settled);
            continue;
        };
        let locks = identity_locks(wallet, identity);
        let _write = locks.write.lock().await;
        // Read again under the write lock: a concurrent apply may have
        // taken it.
        let now = (lib.top_up_locks().await.into_iter()).find(|l| l.outpoint == lock.outpoint);
        if now.as_ref().is_none_or(applied) {
            continue;
        }
        match apply(session, wallet, lib, lease, signer, identity, lock.outpoint).await {
            TopUpState::Applied { .. } => {
                report.resumed += 1;
                report.completed += 1;
            }
            // A settled lock Platform did not apply: nothing was resumed.
            _ if settled => {}
            _ => {
                report.resumed += 1;
                report.still_pending += 1;
            }
        }
    }
}

impl TopUpLib for PwLib {
    async fn identities(&self) -> Vec<(u32, String)> {
        Reg::identities(self).await
    }

    async fn top_up_locks(&self) -> Vec<LockInfo> {
        (self
            .wallet
            .asset_locks()
            .list_tracked_locks()
            .await
            .into_iter())
        .filter(|l| l.funding_type == AssetLockFundingType::IdentityTopUp)
        .map(|l| LockInfo {
            outpoint: l.out_point,
            registration: false,
            identity_index: l.identity_index,
            account: l.account_index,
            amount: l.amount,
            status: super::registration_flow::lock_status(&l.status),
        })
        .collect()
    }

    async fn spendable(&self) -> Result<Spendable, PlatformError> {
        Reg::spendable(self).await
    }

    async fn fee_bound(&self) -> Result<u64, PlatformError> {
        self.session.fee_bound_worst(self.wallet_id).await
    }

    fn funding_hold(&self) -> Option<RegistrationWait> {
        Reg::funding_hold(self)
    }

    async fn funding_permit(
        &self,
        lease: &Lease,
        step: String,
        key: FundingKey,
        duffs: u64,
    ) -> Result<CallPermit, FundError> {
        Reg::funding_permit(self, lease, step, key, duffs).await
    }

    async fn fund(
        &self,
        duffs: u64,
        identity_index: u32,
        permit: &CallPermit,
    ) -> Result<OutPoint, FundError> {
        self.fund_lock(
            AssetLockFundingType::IdentityTopUp,
            duffs,
            identity_index,
            permit,
        )
        .await
    }

    async fn wait_proof(&self, outpoint: OutPoint, timeout: Duration) -> Result<(), FundError> {
        Reg::wait_proof(self, outpoint, timeout).await
    }

    async fn tracked_row(&self, outpoint: OutPoint) -> Option<TrackedRow> {
        Reg::tracked_row(self, outpoint).await
    }

    async fn refresh_balance(&self, identity: Identifier) -> Result<Option<u64>, PlatformError> {
        match self
            .wallet
            .identity()
            .refresh_identity_balance(&identity)
            .await
        {
            Ok(balance) => Ok(Some(balance)),
            Err(platform_wallet::PlatformWalletError::IdentityBalanceUnavailable(_)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn apply(
        &self,
        identity: Identifier,
        outpoint: OutPoint,
        signer: &VaultSigner,
    ) -> Result<Applied, PlatformError> {
        use platform_wallet::wallet::asset_lock::AssetLockFunding;
        let funding = AssetLockFunding::FromExistingAssetLock {
            out_point: outpoint,
            consume_invitation_voucher: false,
        };
        match (self.wallet.identity())
            .top_up_identity_with_funding(&identity, funding, signer, None)
            .await
        {
            Ok(balance) => Ok(Applied::Balance(balance)),
            // The library keeps the lock "consumption unknown" with its
            // ChainLock proof (it cannot authenticate the report).
            Err(platform_wallet::PlatformWalletError::AssetLockAlreadyConsumed(_)) => {
                Ok(Applied::Taken)
            }
            Err(e) => Err(e.into()),
        }
    }
}
