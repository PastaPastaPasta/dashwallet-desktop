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
//! - **The lock.** While a top-up lock of the identity is tracked and not
//!   taken (no proof yet, or proved and not applied), a new top-up is
//!   refused `credits.top_up_pending{txid}`: the host applies that one
//!   (`resume_top_up`) rather than funding again. A lock is tracked before
//!   it is broadcast (H12), so a lock that may be out is always found.
//! - **The apply** builds nothing: `FromExistingAssetLock` signs only the
//!   transition's proof with the lock's credit key, so it runs under a
//!   plain call permit with the lease's uncounted funding signer, taken
//!   before the funding call charges the budget. Applied again, Platform
//!   answers "already consumed" (`AlreadyApplied`). The library marks such
//!   a lock only when it can prove it final, so [`TakenTopUps`] keeps it
//!   from blocking or being applied again in this process.
//!
//! The identity's write lock (`identity_writes.rs`) is held throughout,
//! from before the grant is redeemed, so a balance read before and after
//! the apply measures it alone.

use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use dashcore::{OutPoint, Txid};
use dpp::prelude::Identifier;
use dw_vault::VaultSigner;
use platform_wallet::AssetLockFundingType;

use super::credits::{PendingTopUp, TopUpOutcome, TopUpQuote, TopUpState};
use super::errors::{CreditsError, IdentityError, PlatformError};
use super::flows::{BudgetPurpose, FlowKind, GrantRequest};
use super::identity_writes::identity_locks;
use super::lease::mode_b::TrackedRow;
use super::lease::{CallPermit, FundingKey, Lease, LeaseError};
use super::registration::{FinishReport, RegistrationWait};
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
    /// Platform reported the lock taken already.
    Taken,
}

/// Top-up locks Platform reported taken in this process, by wallet. A
/// lock taken without a ChainLock proof stays tracked as InstantSend-locked
/// (the library marks only what it can prove final), so without this it
/// would block the identity's top-ups and Finish would apply it on every
/// run. After a restart the first apply finds it taken again.
#[derive(Default)]
pub(crate) struct TakenTopUps(Mutex<HashSet<(WalletId, OutPoint)>>);

impl TakenTopUps {
    fn set(&self) -> std::sync::MutexGuard<'_, HashSet<(WalletId, OutPoint)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, wallet: WalletId, outpoint: OutPoint) {
        self.set().insert((wallet, outpoint));
    }

    fn has(&self, wallet: WalletId, outpoint: OutPoint) -> bool {
        self.set().contains(&(wallet, outpoint))
    }

    pub(crate) fn forget(&self, wallet: WalletId) {
        self.set().retain(|(w, _)| *w != wallet);
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
    if let Some(lock) =
        (open_locks(session, wallet, lib).await.into_iter()).find(|l| l.identity_index == index)
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

/// Out or proved, and not reported taken: a new top-up of its identity
/// would fund twice. A lock read back from the chain (`RecoveredFromChain`:
/// final, Platform's consumption unknown, as a restore finds every earlier
/// top-up) is not one: "Finish transfers" applies it.
fn untaken(lock: &LockInfo) -> bool {
    matches!(
        lock.status,
        LockStatus::Built
            | LockStatus::Broadcast
            | LockStatus::InstantSendLocked
            | LockStatus::ChainLocked
    )
}

/// The wallet's [`untaken`] top-up locks that Platform has not reported
/// taken in this process.
async fn open_locks<L: TopUpLib>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &L,
) -> Vec<LockInfo> {
    let taken = &session.platform.taken_top_ups;
    (lib.top_up_locks().await.into_iter())
        .filter(|l| untaken(l) && !taken.has(wallet, l.outpoint))
        .collect()
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
    let locks = identity_locks(wallet, parse_identity(identity)?);
    let _write = locks.write.lock().await;
    // Under the write lock, so no concurrent top-up of the identity funds
    // in between, and before the grant is redeemed, so a refusal keeps it.
    let plan = check(session, wallet, lib, identity, duffs).await?;
    let lease = authority(session, wallet, grant).await?;
    let remaining = lease
        .available(BudgetPurpose::Funding)
        .map_err(PlatformError::from)?;
    if plan.total_duffs > remaining {
        return Err(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Funding,
            needed: plan.total_duffs,
            remaining,
        }
        .into());
    }
    // Before the funding charges the budget (module doc).
    let signer = lease
        .uncounted_funding_signer()
        .map_err(PlatformError::from)?;
    if !session.leases.refresh_funding(wallet).await {
        return Err(PlatformError::Unavailable.into());
    }
    let step = next_step(session, wallet, plan.index)?;
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
/// permit of `lease`, the identity's write lock held by the caller.
async fn apply<L: TopUpLib>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &L,
    lease: &Lease,
    signer: &VaultSigner,
    identity: Identifier,
    outpoint: OutPoint,
) -> TopUpState {
    let before = lib.refresh_balance(identity).await.ok().flatten();
    let applied = lease
        .scope(async {
            let permit = lease.call_permit("top-up", None)?;
            let applied = lib.apply(identity, outpoint, signer).await;
            permit.finish(applied.is_ok());
            applied
        })
        .await;
    match applied {
        Ok(Applied::Balance(balance)) => TopUpState::Applied {
            credits_added: before.map(|b| balance.saturating_sub(b)),
            balance: Some(balance),
        },
        Ok(Applied::Taken) => {
            session.platform.taken_top_ups.record(wallet, outpoint);
            already_applied(lib, identity).await
        }
        Err(e) => {
            tracing::warn!(%outpoint, error = %e, "a top-up's apply failed; the lock stays tracked");
            TopUpState::NotApplied {
                error: e.code().to_owned(),
            }
        }
    }
}

/// Platform took the lock before: its balance, read again (what that
/// apply added is unknown).
async fn already_applied<L: TopUpLib>(lib: &L, identity: Identifier) -> TopUpState {
    TopUpState::AlreadyApplied {
        balance: lib.refresh_balance(identity).await.ok().flatten(),
    }
}

/// The wallet's top-up locks Platform has not taken (in flight, or
/// proved and not applied), oldest identity index first.
pub(crate) async fn pending<L: TopUpLib>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &L,
) -> Vec<PendingTopUp> {
    let identities = lib.identities().await;
    let mut out: Vec<PendingTopUp> = (open_locks(session, wallet, lib).await.into_iter())
        .map(|l| PendingTopUp {
            txid: l.outpoint.txid.to_string(),
            identity: identity_at(&identities, l.identity_index).cloned(),
            identity_index: l.identity_index,
            duffs: l.amount,
            proved: l.status.has_proof(),
        })
        .collect();
    out.sort_by(|a, b| (a.identity_index, &a.txid).cmp(&(b.identity_index, &b.txid)));
    out
}

pub(crate) async fn resume<L: TopUpLib>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &L,
    txid: &str,
    grant: &str,
) -> Result<TopUpOutcome, CreditsError> {
    let invalid = |detail: &str| PlatformError::InvalidArgument {
        detail: detail.into(),
    };
    let txid: Txid = txid
        .parse()
        .map_err(|_| invalid("txid is not a hex txid"))?;
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
    // it. Taken, it is settled before the grant is redeemed.
    let lock = lock_of().await?;
    if lock.status == LockStatus::Consumed
        || session.platform.taken_top_ups.has(wallet, lock.outpoint)
    {
        return Ok(outcome(already_applied(lib, identity).await));
    }
    let lease = authority(session, wallet, grant).await?;
    let signer = lease
        .uncounted_funding_signer()
        .map_err(PlatformError::from)?;
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

/// "Finish transfers" for top-ups: every proved top-up lock of a known
/// identity that Platform has not taken is applied under `lease`, one
/// read back from the chain too (Platform's answer settles it). Builds no
/// lock and waits for no proof: a lock without one stays pending (E0-04's
/// catch-up sends it again).
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
    let taken = &session.platform.taken_top_ups;
    let settled = |l: &LockInfo| l.status == LockStatus::Consumed || taken.has(wallet, l.outpoint);
    for lock in lib.top_up_locks().await {
        if settled(&lock) {
            continue;
        }
        let identity =
            identity_at(&identities, lock.identity_index).and_then(|id| parse_identity(id).ok());
        let (Some(identity), Some(signer), true) = (identity, &signer, lock.status.has_proof())
        else {
            report.still_pending += 1;
            continue;
        };
        let locks = identity_locks(wallet, identity);
        let _write = locks.write.lock().await;
        // Read again under the write lock: a concurrent apply may have
        // taken it.
        let now = (lib.top_up_locks().await.into_iter()).find(|l| l.outpoint == lock.outpoint);
        if now.as_ref().is_none_or(settled) {
            continue;
        }
        match apply(session, wallet, lib, lease, signer, identity, lock.outpoint).await {
            TopUpState::Applied { .. } => {
                report.resumed += 1;
                report.completed += 1;
            }
            // Taken before: nothing was resumed.
            TopUpState::AlreadyApplied { .. } => {}
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
