//! "Add from Dash wallet" (0.2 X1b, ROUTE-06; m4 §2.12): Core → the
//! wallet's own Platform address, behind the facade calls of
//! `platform_addresses.rs`. Not part of the facade.
//!
//! It is a shield's two steps (`shielded_pool.rs`) with another lock type
//! and target:
//! - **The lock.** An asset lock of the Core balance (funding type
//!   `AssetLockAddressTopUp`, credit key `m/9'/coin'/5'/4'/0`) under the
//!   dispatch-journal step `paddr/<k>/funding`, held at most once by the
//!   journal as a shield's. Its signer is the lease's counted funding signer,
//!   capped at [`MAX_LOCK_INPUTS`] coins. The step's own signed lock decides
//!   whether a lock is out, never the call's result.
//! - **The apply.** platform-wallet's `fund_from_asset_lock` of the proved
//!   lock (`FromExistingAssetLock`, never a new lock) to the wallet's
//!   Platform receive address as the one remainder output, which pays
//!   Platform's fee; the transition is signed with the lock's credit key by
//!   the lease's uncounted funding signer, and no address input exists.
//!   Platform executes at most one apply of an outpoint: the transition's
//!   `transform_into_action` validates the asset-lock proof
//!   (rs-drive-abci
//!   `state_transitions/address_funding_from_asset_lock/transform_into_action/v0/mod.rs:73-81`),
//!   whose `verify_is_not_spent` refuses an outpoint already fully consumed
//!   (`common/asset_lock/proof/verify_is_not_spent/v0/mod.rs:37`), and an
//!   executed apply records the outpoint used (rs-drive
//!   `action_convert_to_operations/address_funds/address_funding_from_asset_lock_transition.rs:39`).
//! - **Outcomes,** as a shield's: a proved apply settles the lock in the
//!   journal (`top_up_settled`); Platform's unproved "already consumed", or
//!   a failure after the transition was signed, leaves it pending,
//!   `consumption_unknown` ("Checking", kept in `app.sqlite`), blocking no
//!   new fund; a refusal before the signature is `NotApplied`; a lock with
//!   no proof yet is `ProofWaiting`. An open lock that can still be
//!   finished (not of unknown consumption, not too small, not unsupported)
//!   refuses the next fund (`credits.fund_pending`) until [`resume`]
//!   applies it.
//! - **One apply path.** A fund's apply, "Check again" and the recovery's
//!   Finish all run the recovery's runner (`unfinished_locks::finish`) over
//!   the kind [`FundLocks`]: the lease's funding signer narrowed to the
//!   lock's credit key, one call permit, settled only on Platform's proved
//!   execution, its records by outpoint.
//! - **Only this install's locks.** A lock no `paddr/` step of this
//!   install signed (a restore on a new install, another app on the seed)
//!   is not listed here and blocks no fund: the
//!   recovery lists and finishes it (`unfinished.rs`: `unfinished_transfers`,
//!   `finish_transfer(PlatformAddress, …)` and Repair's Finish transfers),
//!   through the same kind.
//! - **Activity.** The lock's record (`platform_moves.rs`, `FundRecord`) is
//!   a `from_core` row: pending until a proved apply, then done with the
//!   credits the apply's proof shows added, or with the estimate once the
//!   lock is known used another way (`platform_moves::activity`). Its
//!   address's rise is the fund's, never `incoming` (`platform_moves.rs`'s
//!   module doc).
//! - **No abort.** Every call runs in a task of the session runtime, which
//!   the close waits for and never aborts, under no engine timeout: the
//!   library's call is not cancellation-safe
//!   (`fund_from_asset_lock.rs:135-159`).

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use dashcore::{Network, OutPoint, ScriptBuf, Txid};
use dpp::ProtocolError;
use dpp::address_funds::{AddressWitness, PlatformAddress};
use dpp::balances::credits::CREDITS_PER_DUFF;
use dpp::identity::signer::Signer;
use dpp::platform_value::BinaryData;
use dpp::state_transition::address_funding_from_asset_lock_transition::calculate_address_funding_from_asset_lock_min_required_fee;
use dpp::version::PlatformVersion;
use dw_vault::SignerError;
use platform_wallet::{AssetLockFundingType, PlatformWalletError, TrackedAssetLock};

use super::errors::{CreditsError, PlatformError};
use super::flows::{BudgetPurpose, FlowKind, GrantRequest};
use super::lease::{CallPermit, FundingKey, Lease, LeaseError};
use super::platform_addresses::{
    PendingPlatformFund, PlatformFundOutcome, PlatformFundQuote, PlatformFundState,
};
use super::platform_moves;
use super::registration::{RegistrationWait, funding_cap};
use super::registration_flow::{FundError, PwLib, RegistrationLib as Reg, Spendable};
use super::shielded_pool::{MAX_LOCK_INPUTS, shield_funding};
use super::unfinished_funds::FundLocks;
use super::unfinished_locks::{
    self as locks, ApplyAuthority, FundedLease, KindLock, LockFinish, LockOrigin, UnfinishedLock,
    UnfinishedState,
};
use crate::{NetworkSession, WalletId};

/// How long "Check again" waits for a lock's proof before it answers
/// `ProofWaiting`.
const RESUME_PROOF_WAIT: Duration = Duration::from_secs(30);

/// The Platform payment account the receive address is of.
const ACCOUNT: u32 = 0;

/// What a fund asks of platform-wallet; [`PwFund`] is the real one,
/// `platform_fund_tests.rs` scripts a fake.
pub(crate) trait FundLib: Send + Sync {
    fn network(&self) -> Network;
    /// The network's protocol version after one proved read (the one held
    /// when the read fails).
    fn refresh_protocol_version(&self) -> impl Future<Output = u32> + Send;
    /// The network's protocol version as the SDK last proved it; no read.
    fn protocol_version(&self) -> u32;
    /// The wallet's tracked `AssetLockAddressTopUp` locks ([`fund_rows`]).
    fn fund_lock_rows(&self) -> impl Future<Output = Vec<KindLock>> + Send;
    /// The script of the tracked lock's credit output at `outpoint` (its
    /// payload's `credit_outputs[vout]`); `None` when it is not tracked.
    fn credit_script(&self, outpoint: OutPoint) -> impl Future<Output = Option<ScriptBuf>> + Send;
    /// The values of the coins a funding may select (those
    /// [`Spendable::duffs`] counts).
    fn coin_values(&self) -> impl Future<Output = Result<Vec<u64>, PlatformError>> + Send;
    fn spendable(&self) -> impl Future<Output = Result<Spendable, PlatformError>> + Send;
    /// E0-04's `fee_bound_worst`: the lock's fee quote.
    fn fee_bound(&self) -> impl Future<Output = Result<u64, PlatformError>> + Send;
    /// What holds a funding back (SPV behind, the bring-up not answered).
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
        permit: &CallPermit,
    ) -> impl Future<Output = Result<OutPoint, FundError>> + Send;
    fn wait_proof(
        &self,
        outpoint: OutPoint,
        timeout: Duration,
    ) -> impl Future<Output = Result<(), FundError>> + Send;
    /// The wallet's Platform receive address (`DashPay::platform_receive_address`'s).
    fn receive_address(
        &self,
    ) -> impl Future<Output = Result<PlatformAddress, PlatformError>> + Send;
    /// AddressFundingFromAssetLock of the proved lock at `outpoint` to `to`,
    /// its asset-lock proof signed through `signer`: the library's result,
    /// with the credits its proof shows `to` gained (`None` when it does not
    /// say).
    fn apply<S>(
        &self,
        outpoint: OutPoint,
        to: PlatformAddress,
        signer: &S,
    ) -> impl Future<Output = Result<Option<u64>, PlatformWalletError>> + Send
    where
        S: key_wallet::ExtendedPubKeySigner + key_wallet::Signer<Error = SignerError> + Send + Sync;
    /// A sync pass of the wallet's Platform addresses, so the balance takes
    /// in an apply.
    fn sync_now(&self) -> impl Future<Output = ()> + Send;
}

/// The kind's rows of the library's tracked locks: the
/// `AssetLockAddressTopUp` ones, each built from its whole payload
/// (`KindLock::of_row`).
pub(crate) fn fund_rows<'a>(
    tracked: impl IntoIterator<Item = &'a TrackedAssetLock>,
) -> Vec<KindLock> {
    (tracked.into_iter())
        .filter(|l| l.funding_type == AssetLockFundingType::AssetLockAddressTopUp)
        .map(KindLock::of_row)
        .collect()
}

/// The script of `outpoint`'s credit output in its tracked lock's payload
/// (`credit_outputs[vout]`).
pub(crate) fn credit_script_of<'a>(
    tracked: impl IntoIterator<Item = &'a TrackedAssetLock>,
    outpoint: OutPoint,
) -> Option<ScriptBuf> {
    use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
    let lock = tracked.into_iter().find(|l| l.out_point == outpoint)?;
    let Some(TransactionPayload::AssetLockPayloadType(payload)) =
        &lock.transaction.special_transaction_payload
    else {
        return None;
    };
    let output = payload.credit_outputs.get(outpoint.vout as usize)?;
    Some(output.script_pubkey.clone())
}

// MARK: the rules

/// The rules at the network's protocol version after one proved read:
/// `platform.feature_off{platform_addresses}` below address funds.
pub(crate) async fn gate(lib: &impl FundLib) -> Result<&'static PlatformVersion, PlatformError> {
    let rules = super::protocol::platform_version(lib.refresh_protocol_version().await)?;
    platform_moves::sends_active(rules)?;
    Ok(rules)
}

/// Platform's fee estimate for an apply: dpp's minimum fee for no input and
/// one output.
fn fee_credits(rules: &PlatformVersion) -> u64 {
    calculate_address_funding_from_asset_lock_min_required_fee(0, 1, rules)
}

/// About what a lock of `duffs` adds at the version the SDK holds.
pub(crate) fn estimate(lib: &impl FundLib, duffs: u64) -> u64 {
    PlatformVersion::get(lib.protocol_version())
        .map_or(duffs.saturating_mul(CREDITS_PER_DUFF), |rules| {
            credits_of(duffs, fee_credits(rules))
        })
}

/// About what a lock of `duffs` adds: its credits less the fee estimate.
fn credits_of(duffs: u64, fee_credits: u64) -> u64 {
    duffs
        .saturating_mul(CREDITS_PER_DUFF)
        .saturating_sub(fee_credits)
}

/// The least fund: one duff above the fee estimate.
pub(crate) fn minimum_duffs(rules: &PlatformVersion) -> u64 {
    fee_credits(rules) / CREDITS_PER_DUFF + 1
}

/// The fewest of `coins` that cover `duffs`, largest first: the fewest
/// inputs a lock of `duffs` can have (the library's selection may take
/// more).
fn fewest_inputs(coins: &[u64], duffs: u64) -> usize {
    let mut coins = coins.to_vec();
    coins.sort_unstable_by(|a, b| b.cmp(a));
    let mut sum = 0u64;
    for (n, value) in coins.iter().enumerate() {
        if sum >= duffs {
            return n;
        }
        sum = sum.saturating_add(*value);
    }
    coins.len()
}

// MARK: the locks

/// This install's fund locks not applied yet, as the recovery lists them
/// ([`locks::detect`] over [`FundLocks`]): the ones a fund lists, blocks on
/// and resumes. `platform.unavailable` until the journal is read.
async fn own_open(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &impl FundLib,
) -> Result<Vec<UnfinishedLock>, PlatformError> {
    let kind = FundLocks::new(session, wallet, lib);
    Ok((locks::detect(&kind).await?.into_iter())
        .filter(|l| l.origin == LockOrigin::ThisDevice)
        .collect())
}

pub(crate) async fn pending(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &impl FundLib,
) -> Result<Vec<PendingPlatformFund>, CreditsError> {
    Ok((own_open(session, wallet, lib).await?.into_iter())
        .map(|l| PendingPlatformFund {
            txid: l.txid,
            duffs: l.duffs,
            proved: l.state != UnfinishedState::Confirming,
            consumption_unknown: l.state == UnfinishedState::Checking,
            state: l.state,
        })
        .collect())
}

// MARK: the fund

/// What a fund of `duffs` needs, checked by the quote and by the fund under
/// the wallet's fund lock.
async fn plan(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &impl FundLib,
    duffs: u64,
) -> Result<PlatformFundQuote, CreditsError> {
    let rules = gate(lib).await?;
    let fee = fee_credits(rules);
    let min = minimum_duffs(rules);
    if duffs < min {
        return Err(CreditsError::BelowMinimum { min });
    }
    // A lock of unknown consumption may never settle, and one too small or
    // unsupported can't be finished: each stays listed and blocks nothing
    // (as a shield's).
    if let Some(open) = (own_open(session, wallet, lib).await?.into_iter()).find(|l| {
        !matches!(
            l.state,
            UnfinishedState::Checking | UnfinishedState::TooSmall | UnfinishedState::Unsupported
        )
    }) {
        return Err(CreditsError::FundPending { txid: open.txid });
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
    let inputs = fewest_inputs(&lib.coin_values().await?, total_duffs);
    if inputs > MAX_LOCK_INPUTS {
        return Err(CreditsError::TooManyInputs {
            inputs: inputs as u64,
            max: MAX_LOCK_INPUTS as u64,
        });
    }
    let to = lib.receive_address().await?;
    Ok(PlatformFundQuote {
        duffs,
        fee_duffs,
        total_duffs,
        fee_credits: fee,
        credits: credits_of(duffs, fee),
        to: platform_moves::bech32m(&to, lib.network()),
        grant: GrantRequest {
            max_duffs: total_duffs,
            max_credits: 0,
        },
    })
}

pub(crate) async fn quote(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &impl FundLib,
    duffs: u64,
) -> Result<PlatformFundQuote, CreditsError> {
    plan(session, wallet, lib, duffs).await
}

/// The wallet's fund step `k`.
pub(crate) fn fund_step(k: u32) -> String {
    format!("paddr/{k}/funding")
}

/// The step the next fund funds under: the first the funding gate admits
/// (`shielded_pool.rs`'s `next_step`).
pub(crate) fn next_step(
    session: &NetworkSession,
    wallet: WalletId,
) -> Result<String, PlatformError> {
    use super::flows::DispatchState as D;
    use super::lease::FundingBlock as B;
    for k in 0.. {
        let step = fund_step(k);
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

/// The lock serializing the wallet's funds, their resumes and the
/// recovery's finishes of its type-4 locks.
pub(crate) fn fund_lock(wallet: WalletId) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: LazyLock<Mutex<BTreeMap<WalletId, Arc<tokio::sync::Mutex<()>>>>> =
        LazyLock::new(Mutex::default);
    let mut locks = LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
    Arc::clone(locks.entry(wallet).or_default())
}

/// The `PlatformAddressFund` lease `grant` names, or a new one redeemed
/// from it; a grant with no funding budget is refused before it is
/// redeemed ([`super::unfinished::authority`]).
async fn authority(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    grant: &str,
) -> Result<Lease, PlatformError> {
    super::unfinished::authority(session, wallet, grant, FlowKind::PlatformAddressFund).await
}

/// The address funding key: account 0, index 0 (the library's).
fn funding_key() -> FundingKey {
    FundingKey {
        funding_type: AssetLockFundingType::AssetLockAddressTopUp,
        identity_index: 0,
        account: 0,
    }
}

/// `DashPay::platform_fund`. The fund plans again under the fund lock, so
/// the address it records and pays can be a newer receive address of the
/// account than its quote's `to`.
pub(crate) async fn fund(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl FundLib,
    duffs: u64,
    grant: &str,
) -> Result<PlatformFundOutcome, CreditsError> {
    if let Some(wait) = lib.funding_hold() {
        if wait == RegistrationWait::Startup {
            session.platform.retry_startup(wallet);
        }
        return Err(super::topup::held(wait).into());
    }
    let lock = fund_lock(wallet);
    let _funding = lock.lock().await;
    let plan = plan(session, wallet, lib, duffs).await?;
    if !session.leases.refresh_funding(wallet).await {
        return Err(PlatformError::Unavailable.into());
    }
    let step = next_step(session, wallet)?;
    let remaining = funding_cap(session, wallet, grant).await?;
    if plan.total_duffs > remaining {
        return Err(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Funding,
            needed: plan.total_duffs,
            remaining,
        }
        .into());
    }
    // The apply's signer is taken before the funding charges the budget,
    // which an exact grant leaves at 0, and kept for the apply.
    let held = FundedLease::before_funding(authority(session, wallet, grant).await?)
        .map_err(PlatformError::from)?;
    let lease = held.lease();
    let funded = lease
        .scope_step(Some(step.clone()), async {
            let permit = (lib.funding_permit(lease, step.clone(), funding_key(), duffs)).await?;
            let funded = lib.fund(duffs, &permit).await;
            permit.finish(funded.is_ok());
            funded
        })
        .await;
    // The step's own lock decides, never the call's result alone.
    let signed = session.leases.signed_lock_txids(wallet, &step);
    let own =
        (lib.fund_lock_rows().await.into_iter()).find(|l| signed.contains(&l.info.outpoint.txid));
    let Some(lock) = own else {
        return Err(match funded {
            Err(FundError::Refused(e) | FundError::NotSent(e)) => e.into(),
            Err(FundError::CoinsLocked) => CreditsError::CoinsLocked,
            // "Combine coins first": at least one coin past the cap.
            Err(FundError::TooManyInputs { max }) => CreditsError::TooManyInputs {
                inputs: max + 1,
                max,
            },
            _ => PlatformError::BroadcastUnknown { artifact: step }.into(),
        });
    };
    let (outpoint, duffs) = (lock.info.outpoint, lock.info.amount);
    let txid = outpoint.txid.to_string();
    if let Err(e) = platform_moves::record_fund(
        session,
        wallet,
        (outpoint, duffs),
        plan.to.clone(),
        credits_of(duffs, plan.fee_credits),
    )
    .await
    {
        tracing::warn!(%txid, error = %e, "could not record a fund in the Platform activity");
    }
    if !lock.proof || !lock.info.status.has_proof() {
        tracing::info!(%txid, ?funded, "a fund's lock waits for its proof");
        return Ok(PlatformFundOutcome {
            txid,
            state: PlatformFundState::ProofWaiting,
        });
    }
    // The runner takes the fund lock itself; a fund in between finds this
    // lock open and is refused (`credits.fund_pending`).
    drop(_funding);
    let state = finish(session, wallet, lib, (outpoint, duffs), || async {
        Ok(held)
    })
    .await?;
    Ok(PlatformFundOutcome { txid, state })
}

/// Applies the pending fund lock `txid` again ("Pending — finish", "Check
/// again"), whatever its consumption: `FromExistingAssetLock` of the same
/// outpoint, to the address its record names (else the receive address),
/// never a new lock. Platform executes at most one apply of an outpoint
/// (module doc); only a proved apply settles it. The grant is redeemed only
/// once every refusal that can be read beforehand has passed.
pub(crate) async fn resume(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl FundLib,
    txid: &str,
    grant: &str,
) -> Result<PlatformFundOutcome, CreditsError> {
    gate(lib).await?;
    let txid: Txid = txid
        .trim()
        .parse()
        .map_err(|_| PlatformError::InvalidArgument {
            detail: "txid is not a hex txid".into(),
        })?;
    let pending = (own_open(session, wallet, lib).await?.into_iter())
        .find(|l| l.txid == txid.to_string())
        .ok_or_else(|| PlatformError::InvalidArgument {
            detail: "no pending fund has that txid".into(),
        })?;
    let outpoint = OutPoint::new(txid, pending.vout);
    let outcome = |state| PlatformFundOutcome {
        txid: txid.to_string(),
        state,
    };
    if pending.state == UnfinishedState::Confirming
        && lib.wait_proof(outpoint, RESUME_PROOF_WAIT).await.is_err()
    {
        return Ok(outcome(PlatformFundState::ProofWaiting));
    }
    let lease_of = || authority(session, wallet, grant);
    let state = finish(session, wallet, lib, (outpoint, pending.duffs), lease_of).await?;
    Ok(outcome(state))
}

/// The wallet's fund lock at `outpoint` of `duffs` finished through the
/// recovery's runner ([`locks::finish`] over [`FundLocks`]), the one apply
/// path of a fund, its resume and the recovery's Finish; `lease_of` is
/// asked only once every refusal that can be read beforehand has passed.
async fn finish<F, Fut, A>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &impl FundLib,
    (outpoint, duffs): (OutPoint, u64),
    lease_of: F,
) -> Result<PlatformFundState, CreditsError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<A, PlatformError>>,
    A: ApplyAuthority,
{
    let kind = FundLocks::new(session, wallet, lib);
    Ok(match locks::finish(&kind, outpoint, lease_of).await? {
        LockFinish::Applied => PlatformFundState::Applied {
            credits: (kind.credited()).unwrap_or_else(|| estimate(lib, duffs)),
        },
        LockFinish::ProofWaiting => PlatformFundState::ProofWaiting,
        // Neither settles (SR1).
        LockFinish::Checking | LockFinish::ReportedUsed => PlatformFundState::ConsumptionUnknown,
        LockFinish::NotApplied { error } => PlatformFundState::NotApplied { error },
    })
}

// MARK: the library

/// Runs `body` on the session runtime with the wallet's [`PwFund`]: a task
/// the close waits for and never aborts. A call that signs (`signing`) is
/// admitted under the session's `close_preflight`, as a shield is: it waits
/// out an in-app close's check, and while it runs that close is refused.
pub(crate) async fn on_lib<T, F, Fut>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    signing: bool,
    body: F,
) -> Result<T, CreditsError>
where
    T: Send + 'static,
    F: FnOnce(Arc<NetworkSession>, PwFund) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, CreditsError>> + Send + 'static,
{
    let session = Arc::clone(session);
    let rt = session.rt.clone();
    rt.spawn(async move {
        let _admitted = match signing {
            true => Some(session.platform.close_preflight.read().await),
            false => None,
        };
        let _op = session.enter().await.map_err(PlatformError::from)?;
        session
            .require_wallet(&wallet)
            .map_err(PlatformError::from)?;
        if !session.platform.enabled() {
            return Err(PlatformError::FeatureOff {
                feature: "platform".into(),
            }
            .into());
        }
        let lib = PwFund::open(&session, wallet).await?;
        body(Arc::clone(&session), lib).await
    })
    .await
    .map_err(|e| PlatformError::from(crate::EngineError::from(e)))?
}

/// The real [`FundLib`]: the wallet's library handles.
pub(crate) struct PwFund {
    pw: PwLib,
}

impl PwFund {
    /// The wallet's library handles, for a caller already admitted to the
    /// session (the recovery's list and Finish transfers).
    pub(crate) async fn open(
        session: &Arc<NetworkSession>,
        wallet: WalletId,
    ) -> Result<Self, PlatformError> {
        #[cfg(test)]
        if super::unfinished::open_fails::get(
            session,
            super::unfinished::LockKindId::PlatformAddress,
        ) {
            return Err(PlatformError::Storage {
                detail: "injected".into(),
            });
        }
        let (_, pw) = PwLib::open(session, wallet).await?;
        Ok(Self { pw })
    }
}

/// The address-input signer of an apply: it has no address inputs, so it
/// signs nothing.
#[derive(Debug)]
struct NoInputs;

impl NoInputs {
    fn refused() -> ProtocolError {
        ProtocolError::ExternalSignerError("an asset-lock fund has no address inputs".into())
    }
}

#[async_trait]
impl Signer<PlatformAddress> for NoInputs {
    async fn sign(&self, _: &PlatformAddress, _: &[u8]) -> Result<BinaryData, ProtocolError> {
        Err(Self::refused())
    }

    async fn sign_create_witness(
        &self,
        _: &PlatformAddress,
        _: &[u8],
    ) -> Result<AddressWitness, ProtocolError> {
        Err(Self::refused())
    }

    fn can_sign_with(&self, _: &PlatformAddress) -> bool {
        false
    }
}

impl FundLib for PwFund {
    fn network(&self) -> Network {
        self.pw.session.vault.network()
    }

    async fn refresh_protocol_version(&self) -> u32 {
        (self.pw.session.refresh_protocol_version().await)
            .unwrap_or_else(|_| self.pw.sdk.protocol_version_number())
    }

    fn protocol_version(&self) -> u32 {
        self.pw.sdk.protocol_version_number()
    }

    async fn fund_lock_rows(&self) -> Vec<KindLock> {
        fund_rows(&self.pw.wallet.asset_locks().list_tracked_locks().await)
    }

    async fn credit_script(&self, outpoint: OutPoint) -> Option<ScriptBuf> {
        let locks = self.pw.wallet.asset_locks().list_tracked_locks().await;
        credit_script_of(&locks, outpoint)
    }

    async fn coin_values(&self) -> Result<Vec<u64>, PlatformError> {
        let session = &self.pw.session;
        let fence = session.coin_fence(self.pw.wallet_id).await?;
        let snapshot = session.fenced_coins(&fence).await?;
        Ok((snapshot.coins.iter())
            .filter(|c| c.send_account && c.auto_selectable(snapshot.height))
            .map(|c| c.utxo.value())
            .collect())
    }

    async fn spendable(&self) -> Result<Spendable, PlatformError> {
        Reg::spendable(&self.pw).await
    }

    async fn fee_bound(&self) -> Result<u64, PlatformError> {
        self.pw.session.fee_bound_worst(self.pw.wallet_id).await
    }

    fn funding_hold(&self) -> Option<RegistrationWait> {
        Reg::funding_hold(&self.pw)
    }

    async fn funding_permit(
        &self,
        lease: &Lease,
        step: String,
        key: FundingKey,
        duffs: u64,
    ) -> Result<CallPermit, FundError> {
        Reg::funding_permit(&self.pw, lease, step, key, duffs).await
    }

    async fn fund(&self, duffs: u64, permit: &CallPermit) -> Result<OutPoint, FundError> {
        // At most `MAX_LOCK_INPUTS` coins, whatever key-wallet's selection
        // picks (as a shield's lock).
        let signer = (permit.funding_signer())
            .map_err(|e| FundError::Refused(e.into()))?
            .with_input_cap(MAX_LOCK_INPUTS);
        let funded = (self.pw)
            .fund_lock_with(
                AssetLockFundingType::AssetLockAddressTopUp,
                duffs,
                0,
                &signer,
            )
            .await;
        if signer.capped() {
            tracing::info!(
                ?funded,
                "a fund's lock needed more than {MAX_LOCK_INPUTS} coins"
            );
            return Err(FundError::TooManyInputs {
                max: MAX_LOCK_INPUTS as u64,
            });
        }
        funded
    }

    async fn wait_proof(&self, outpoint: OutPoint, timeout: Duration) -> Result<(), FundError> {
        Reg::wait_proof(&self.pw, outpoint, timeout).await
    }

    async fn receive_address(&self) -> Result<PlatformAddress, PlatformError> {
        let key = key_wallet::account::account_collection::PlatformPaymentAccountKey {
            account: ACCOUNT,
            key_class: 0,
        };
        Ok((self.pw.wallet.platform())
            .next_unused_receive_address(key)
            .await?)
    }

    async fn apply<S>(
        &self,
        outpoint: OutPoint,
        to: PlatformAddress,
        signer: &S,
    ) -> Result<Option<u64>, PlatformWalletError>
    where
        S: key_wallet::ExtendedPubKeySigner + key_wallet::Signer<Error = SignerError> + Send + Sync,
    {
        let platform = self.pw.wallet.platform();
        let before = (platform.addresses_with_balances().await.into_iter())
            .find(|(a, _)| *a == to)
            .map_or(0, |(_, c)| c);
        let changes = platform
            .fund_from_asset_lock(
                shield_funding(outpoint),
                ACCOUNT,
                BTreeMap::from([(to, None)]),
                Vec::new(),
                &NoInputs,
                signer,
                None,
            )
            .await?;
        let after = (changes.addresses.iter())
            .find(|e| PlatformAddress::P2pkh(e.address.to_bytes()) == to)
            .map(|e| e.funds.balance);
        Ok(after.map(|a| a.saturating_sub(before)))
    }

    async fn sync_now(&self) {
        let session = &self.pw.session;
        let synced = match session.manager() {
            Ok(manager) => platform_moves::sync(session, &manager, self.pw.wallet_id).await,
            Err(e) => Err(PlatformError::from(e).into()),
        };
        if let Err(e) = synced {
            tracing::info!(wallet_id = %self.pw.wallet_id, error = %e, "a fund's sync pass did not run");
        }
    }
}
