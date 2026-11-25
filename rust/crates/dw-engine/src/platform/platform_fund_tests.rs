//! Tests of platform_fund.rs (0.2 X1b R2, "Add from Dash wallet") and of
//! its locks as the recovery's type-4 kind ([`FundLocks`]), on
//! requests_tests.rs's fixture over a scripted library ([`FakeFund`]), with
//! the real dispatch journal, lease and `app.sqlite`: the quote's fee,
//! minimum and refusals; a fund's lock held at most once by its step; the
//! apply of the step's own lock to the receive address through the
//! recovery's runner, and how it ends; "Check again" on the same outpoint,
//! never a new lock; only this install's locks listed by the fund, every
//! one by the recovery; the conformance run; the lease's signers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashcore::hashes::Hash;
use dashcore::{Network, OutPoint, ScriptBuf, Txid};
use dpp::address_funds::PlatformAddress;
use dpp::state_transition::address_funding_from_asset_lock_transition::calculate_address_funding_from_asset_lock_min_required_fee;
use dpp::version::PlatformVersion;
use dw_appdb::dispatch::TopUpSettled;
use key_wallet::bip32::DerivationPath;
use platform_wallet::{
    AssetLockFundingType, AssetLockStatus, PlatformWalletError, TrackedAssetLock,
};

use super::StartupStatus;
use super::credits::FundAmount;
use super::errors::{CreditsError, PlatformError};
use super::flows::{BudgetPurpose, FlowKind, RevokeCause};
use super::lease::{CallPermit, FlowOutcome, FundingKey, Lease, StoredLock};
use super::platform_addresses::{PlatformActivity, PlatformActivityKind, PlatformActivityState};
use super::platform_addresses::{PlatformFundOutcome, PlatformFundQuote, PlatformFundState};
use super::platform_fund::{self as fund, FundLib};
use super::platform_moves::{self, tests::World};
use super::registration::RegistrationWait;
use super::registration_flow::{FundError, Spendable, fund_error};
use super::registration_tests::PartialBringUp;
use super::requests::tests::Fixture;
use super::runtime::{TestPause, guard};
use super::shielded_pool::MAX_LOCK_INPUTS;
use super::unfinished;
use super::unfinished_conformance::{self as conformance, End, Rig};
use super::unfinished_funds::FundLocks;
use super::unfinished_locks::{
    self as locks, KindLock, LockFinish, LockKind, LockKindId, LockOrigin, Mark, Marks, Minimum,
    SpentRead, UnfinishedLock, UnfinishedState,
};
use crate::{NetworkSession, WalletId};

/// The credit key a fund's lock pays (the library's address top-up key,
/// account 0, index 0), on testnet.
pub(super) const LOCK_KEY: &str = "m/9'/1'/5'/4'/0";

/// The P2PKH script of [`LOCK_KEY`] as the vault's `PlatformAddressFund`
/// lease derives it.
fn lock_script(fx: &Fixture) -> ScriptBuf {
    let (s, wallet, grant) = (Arc::clone(&fx.s), fx.wallet, grant(fx, 1, 0));
    fx.engine.block_on(async move {
        let lease = (s.begin_lease(wallet, FlowKind::PlatformAddressFund, &[grant]))
            .await
            .unwrap();
        let funding = lease.uncounted_funding_signer().unwrap();
        let path = DerivationPath::from_str(LOCK_KEY).unwrap();
        let key = key_wallet::Signer::public_key(&funding, &path)
            .await
            .unwrap();
        ScriptBuf::new_p2pkh(&dashcore::PublicKey::new(key).pubkey_hash())
    })
}

/// What the next funding does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    /// A lock out with its InstantSend proof.
    Proved,
    /// A lock out, its proof not seen yet.
    Unproved,
    /// Signed, nothing tracked: the outcome is unknown.
    LostUntracked,
    /// The input cap refused the build.
    TooMany,
    /// Signed, then platform-wallet's `WalletRemovedBeforeSend` before
    /// the tracking write, mapped by the engine's `fund_error`.
    RemovedBeforeSend,
    /// Signed, then platform-wallet's `AssetLockTooManyInputs`, likewise.
    OverInputCap,
}

/// How the next apply ends, as platform-wallet would answer it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// Proved, the address gaining this much (`None`: the library does not
    /// say).
    Done(Option<u64>),
    /// `AssetLockAlreadyConsumed` naming `of` (`None`: the applied
    /// outpoint).
    Consumed { of: Option<OutPoint> },
    /// Platform's "already consumed" for the applied outpoint, which the
    /// library cannot record at the session's real persister.
    Unrecorded,
    /// Another persistence failure.
    PersistFailed,
    /// A broadcast failure.
    Failed,
    /// The apply panics.
    Panics,
    /// A first attempt signs and fails; the library's retry in the same
    /// call (the CL-height or IS→CL resubmit) signs nothing more and hears
    /// Platform's "already consumed" for the applied outpoint.
    RetriedConsumed,
}

/// platform-wallet's side of a fund, scripted; the journal, the lease and
/// `app.sqlite` are the fixture's real ones.
struct FakeFund {
    session: Arc<NetworkSession>,
    wallet: WalletId,
    version: u32,
    pub(super) network: Network,
    /// Platform reads made: the version refreshed, a spent read.
    pub(super) platform_reads: AtomicU32,
    /// The library's tracked locks, every funding type (the kind's rows are
    /// [`fund::fund_rows`] of them).
    tracked: Mutex<Vec<TrackedAssetLock>>,
    /// The credit script a fund's lock pays.
    script: ScriptBuf,
    funding: Mutex<Script>,
    funds: AtomicU32,
    /// The next applies: whether each signs with `key` first, and how it
    /// ends.
    answers: Mutex<VecDeque<(bool, Answer)>>,
    /// The key an apply signs with.
    key: Mutex<DerivationPath>,
    /// Every apply: the outpoint and the address it paid.
    applied: Mutex<Vec<(OutPoint, PlatformAddress)>>,
    /// Whether each signature an apply asked for was given.
    signed: Mutex<Vec<bool>>,
    syncs: AtomicU32,
    spendable: Spendable,
    coins: Vec<u64>,
    hold: Option<RegistrationWait>,
    /// Held as the library holds a funding on the session's bring-up of
    /// the wallet ([`super::registration_flow::startup_hold`]).
    startup_held: AtomicBool,
    receive: PlatformAddress,
    /// A fund is in [`FundLib::fund`] (under the fund lock).
    in_fund: AtomicBool,
    /// The next funding waits for this once it is in.
    fund_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// The next apply, its funding call over, waits before its call
    /// permit: `(in, release)`, the first notified once it waits there.
    apply_gate: Mutex<Option<ApplyGate>>,
    /// The next apply locks the vault before it signs.
    lock_vault_in_apply: AtomicBool,
    /// BIP44 account 0 as a Core Max reads it; the coins its build spends
    /// (the plan's when unset); each Core Max lock's inputs; whether each
    /// Core Max build ran with the wallet's coin fence held.
    core: Mutex<super::core_max::CoreCoins>,
    core_built: Mutex<Option<Vec<super::shield_drain::DrainCoin>>>,
    core_drained: Mutex<Vec<Vec<OutPoint>>>,
    core_fenced: Mutex<Vec<bool>>,
    /// Platform's proved spent read of each outpoint; unread when unset.
    spent: Mutex<HashMap<OutPoint, SpentRead>>,
    /// Every recover (`apply_remainder`): the outpoint and the address it
    /// paid.
    pub(super) recovered: Mutex<Vec<(OutPoint, PlatformAddress)>>,
}

impl FakeFund {
    fn new(fx: &Fixture) -> Self {
        Self {
            session: Arc::clone(&fx.s),
            wallet: fx.wallet,
            version: PlatformVersion::latest().protocol_version,
            network: Network::Testnet,
            platform_reads: AtomicU32::new(0),
            tracked: Mutex::default(),
            script: lock_script(fx),
            funding: Mutex::new(Script::Proved),
            funds: AtomicU32::new(0),
            answers: Mutex::default(),
            key: Mutex::new(DerivationPath::from_str(LOCK_KEY).unwrap()),
            applied: Mutex::default(),
            signed: Mutex::default(),
            syncs: AtomicU32::new(0),
            spendable: Spendable {
                duffs: 10_000_000_000,
                inputs: 1,
                protected: 0,
            },
            coins: vec![10_000_000_000],
            hold: None,
            startup_held: AtomicBool::new(false),
            receive: PlatformAddress::P2pkh([0x42; 20]),
            in_fund: AtomicBool::new(false),
            fund_gate: Mutex::default(),
            apply_gate: Mutex::default(),
            lock_vault_in_apply: AtomicBool::new(false),
            core: Mutex::default(),
            core_built: Mutex::default(),
            core_drained: Mutex::default(),
            core_fenced: Mutex::default(),
            spent: Mutex::default(),
            recovered: Mutex::default(),
        }
    }

    /// Platform's proved state reads `read` for the lock at `at`.
    fn proves(&self, at: OutPoint, read: SpentRead) {
        self.spent.lock().unwrap().insert(at, read);
    }

    /// The next apply signs with the lock's key, then ends as `answer`.
    fn answer(&self, answer: Answer) {
        self.answers.lock().unwrap().push_back((true, answer));
    }

    /// The next apply ends as `answer` before any signature.
    fn refuse(&self, answer: Answer) {
        self.answers.lock().unwrap().push_back((false, answer));
    }

    fn applied(&self) -> Vec<OutPoint> {
        (self.applied.lock().unwrap().iter())
            .map(|(o, _)| *o)
            .collect()
    }

    /// The tracked lock `n`, of one credit output of `duffs` paying the
    /// lock key, of the fund's type.
    fn lock(&self, n: u8, duffs: u64, status: AssetLockStatus, proof: bool) -> TrackedAssetLock {
        let tx = conformance::transaction(n, 1, duffs, &self.script);
        conformance::tracked(
            &tx,
            0,
            AssetLockFundingType::AssetLockAddressTopUp,
            status,
            proof,
        )
    }

    fn track(&self, row: TrackedAssetLock) {
        self.tracked.lock().unwrap().push(row);
    }

    fn outpoint(&self, i: usize) -> OutPoint {
        self.tracked.lock().unwrap()[i].out_point
    }

    /// The next scripted apply: signs with `key` first when scripted to,
    /// then ends as its answer.
    #[allow(clippy::result_large_err)]
    async fn scripted<S>(
        &self,
        outpoint: OutPoint,
        signer: &S,
    ) -> Result<Option<u64>, PlatformWalletError>
    where
        S: key_wallet::Signer<Error = dw_vault::SignerError> + Send + Sync,
    {
        let (sign, answer) = (self.answers.lock().unwrap().pop_front()).expect("a scripted apply");
        if sign {
            if self.lock_vault_in_apply.swap(false, Ordering::SeqCst) {
                self.session.vault.lock();
            }
            let path = self.key.lock().unwrap().clone();
            let signed = key_wallet::Signer::sign_ecdsa(signer, &path, [7; 32]).await;
            self.signed.lock().unwrap().push(signed.is_ok());
        }
        match answer {
            Answer::Done(gained) => Ok(gained),
            Answer::Consumed { of } => Err(PlatformWalletError::AssetLockAlreadyConsumed(
                of.unwrap_or(outpoint),
            )),
            Answer::Unrecorded => Err(conformance::unrecorded_consumed(&self.session)),
            Answer::PersistFailed => Err(PlatformWalletError::Persistence(
                "database is locked".into(),
            )),
            Answer::Failed => Err(PlatformWalletError::TransactionBroadcast(
                "broadcast failed".into(),
            )),
            Answer::Panics => panic!("the apply panics"),
            Answer::RetriedConsumed => Err(PlatformWalletError::AssetLockAlreadyConsumed(outpoint)),
        }
    }

    /// The tracked lock at `outpoint` ChainLocked, with its proof.
    fn prove(&self, outpoint: OutPoint) {
        let mut tracked = self.tracked.lock().unwrap();
        let row = tracked
            .iter_mut()
            .find(|l| l.out_point == outpoint)
            .unwrap();
        *row = conformance::tracked(
            &row.transaction,
            outpoint.vout,
            row.funding_type,
            AssetLockStatus::ChainLocked,
            true,
        );
    }
}

impl FundLib for FakeFund {
    fn network(&self) -> Network {
        self.network
    }

    async fn refresh_protocol_version(&self) -> u32 {
        self.platform_reads.fetch_add(1, Ordering::SeqCst);
        self.version
    }

    fn protocol_version(&self) -> u32 {
        self.version
    }

    async fn fund_lock_rows(&self) -> Vec<KindLock> {
        fund::fund_rows(&*self.tracked.lock().unwrap())
    }

    async fn spent(&self, outpoint: OutPoint) -> SpentRead {
        self.platform_reads.fetch_add(1, Ordering::SeqCst);
        (self.spent.lock().unwrap().get(&outpoint)).map_or(SpentRead::Unread, |r| *r)
    }

    async fn credit_script(&self, outpoint: OutPoint) -> Option<ScriptBuf> {
        let gate = self.apply_gate.lock().unwrap().take();
        if let Some((waiting, release)) = gate {
            waiting.notify_one();
            release.notified().await;
        }
        fund::credit_script_of(&*self.tracked.lock().unwrap(), outpoint)
    }

    async fn coin_values(&self) -> Result<Vec<u64>, PlatformError> {
        Ok(self.coins.clone())
    }

    async fn spendable(&self) -> Result<Spendable, PlatformError> {
        Ok(self.spendable)
    }

    async fn fee_bound(&self) -> Result<u64, PlatformError> {
        self.session.fee_bound_worst(self.wallet).await
    }

    fn funding_hold(&self) -> Option<RegistrationWait> {
        if self.startup_held.load(Ordering::SeqCst) {
            // As the library's: a closed session holds.
            let Ok(startup) = self.session.dashpay_startup(&self.wallet) else {
                return Some(RegistrationWait::Sync);
            };
            if let Some(wait) = super::registration_flow::startup_hold(startup.startup) {
                return Some(wait);
            }
        }
        self.hold
    }

    async fn funding_permit(
        &self,
        lease: &Lease,
        step: String,
        key: FundingKey,
        duffs: u64,
    ) -> Result<CallPermit, FundError> {
        let fence = (self.session.coin_fence(lease.wallet).await)
            .map_err(|e| FundError::Refused(e.into()))?;
        (self.session)
            .funding_call_permit(lease, step.clone(), duffs, step, key, fence)
            .await
            .map_err(FundError::Refused)
    }

    /// A new lock (transaction `0x70 + n`) signed by the permit's funding
    /// signer and recorded as the step's own, as the script says.
    async fn fund(&self, duffs: u64, permit: &CallPermit) -> Result<OutPoint, FundError> {
        permit
            .funding_signer()
            .map_err(|e| FundError::Refused(e.into()))?;
        self.in_fund.store(true, Ordering::SeqCst);
        let gate = self.fund_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        let n = self.funds.fetch_add(1, Ordering::SeqCst) as u8 + 0x70;
        let (status, proof) = match *self.funding.lock().unwrap() {
            Script::LostUntracked => {
                permit.note_signed([0xfe; 32]);
                return Err(FundError::Failed(PlatformError::Unavailable));
            }
            Script::TooMany => {
                return Err(FundError::TooManyInputs {
                    max: MAX_LOCK_INPUTS as u64,
                });
            }
            Script::RemovedBeforeSend => {
                permit.note_signed([0xfe; 32]);
                return Err(fund_error(PlatformWalletError::WalletRemovedBeforeSend(
                    "w".into(),
                )));
            }
            Script::OverInputCap => {
                permit.note_signed([0xfe; 32]);
                return Err(fund_error(PlatformWalletError::AssetLockTooManyInputs {
                    count: 101,
                    max: 100,
                }));
            }
            Script::Proved => (AssetLockStatus::InstantSendLocked, true),
            Script::Unproved => (AssetLockStatus::Broadcast, false),
        };
        let row = self.lock(n, duffs, status, proof);
        let outpoint = row.out_point;
        let sighash = outpoint.txid.to_byte_array();
        permit.note_signed(sighash);
        self.session.leases.record_signed_locks(
            self.wallet,
            &[StoredLock {
                txid: outpoint.txid,
                inputs: (row.transaction.input.iter())
                    .map(|i| i.previous_output)
                    .collect(),
                sighashes: Some(vec![sighash]),
                built: true,
            }],
        );
        self.track(row);
        Ok(outpoint)
    }

    async fn wait_proof(&self, _: OutPoint, _: Duration) -> Result<(), FundError> {
        Err(FundError::Refused(PlatformError::Unavailable))
    }

    async fn receive_address(&self) -> Result<PlatformAddress, PlatformError> {
        Ok(self.receive)
    }

    /// Signs with `key` through the signer it is handed, as the library's
    /// transition does, then answers as scripted.
    async fn apply<S>(
        &self,
        outpoint: OutPoint,
        to: PlatformAddress,
        signer: &S,
    ) -> Result<Option<u64>, PlatformWalletError>
    where
        S: key_wallet::ExtendedPubKeySigner
            + key_wallet::Signer<Error = dw_vault::SignerError>
            + Send
            + Sync,
    {
        self.applied.lock().unwrap().push((outpoint, to));
        self.scripted(outpoint, signer).await
    }

    async fn shield_lock_rows(&self) -> Vec<KindLock> {
        (self.tracked.lock().unwrap().iter())
            .filter(|l| l.funding_type == AssetLockFundingType::AssetLockShieldedAddressTopUp)
            .map(KindLock::of_row)
            .collect()
    }

    /// As the library's: a proved apply records the tracked lock
    /// `Consumed`.
    async fn apply_remainder<S>(
        &self,
        outpoint: OutPoint,
        to: PlatformAddress,
        signer: &S,
    ) -> Result<Option<u64>, PlatformWalletError>
    where
        S: key_wallet::ExtendedPubKeySigner
            + key_wallet::Signer<Error = dw_vault::SignerError>
            + Send
            + Sync,
    {
        self.recovered.lock().unwrap().push((outpoint, to));
        let applied = self.scripted(outpoint, signer).await;
        if applied.is_ok() {
            let mut tracked = self.tracked.lock().unwrap();
            if let Some(row) = tracked.iter_mut().find(|l| l.out_point == outpoint) {
                row.status = AssetLockStatus::Consumed;
            }
        }
        applied
    }

    async fn sync_now(&self) {
        self.syncs.fetch_add(1, Ordering::SeqCst);
    }

    async fn core_coins(
        &self,
        _: &crate::coins::CoinFence,
    ) -> Result<super::core_max::CoreCoins, PlatformError> {
        Ok(self.core.lock().unwrap().clone())
    }

    /// The library's drain build (`core_built`'s coins, else the plan's)
    /// shown to and signed by the call's drain signer, then tracked as a
    /// proved lock of its credit and recorded as the step's own.
    async fn fund_core_max(
        &self,
        coins: &[super::shield_drain::DrainCoin],
        total_duffs: u64,
        minimum_lock_duffs: u64,
        permit: &CallPermit,
    ) -> Result<OutPoint, FundError> {
        use super::shielded_tests::{fake_credit_script, sign_fake_drain};
        let free = self.session.try_coin_fence(self.wallet).await.unwrap();
        self.core_fenced.lock().unwrap().push(free.is_none());
        drop(free);
        let signer = super::shield_drain::drain_signer(
            permit,
            coins,
            total_duffs,
            minimum_lock_duffs,
            fake_credit_script(),
            true,
        )?;
        let built = (self.core_built.lock().unwrap().clone()).unwrap_or_else(|| coins.to_vec());
        let (tx, sighashes) = sign_fake_drain(&signer, permit, &built).await?;
        let n = self.funds.fetch_add(1, Ordering::SeqCst) as u8 + 0x70;
        let row = self.lock(
            n,
            tx.output[0].value,
            AssetLockStatus::InstantSendLocked,
            true,
        );
        let outpoint = row.out_point;
        let inputs: Vec<OutPoint> = built.iter().map(|c| c.outpoint).collect();
        self.session.leases.record_signed_locks(
            self.wallet,
            &[StoredLock {
                txid: outpoint.txid,
                inputs: inputs.clone(),
                sighashes: Some(sighashes),
                built: true,
            }],
        );
        self.core_drained.lock().unwrap().push(inputs);
        self.track(row);
        Ok(outpoint)
    }
}

type ApplyGate = (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>);

fn run<T: Send + 'static, Fut>(
    fx: &Fixture,
    lib: &Arc<FakeFund>,
    body: impl FnOnce(Arc<NetworkSession>, Arc<FakeFund>) -> Fut,
) -> T
where
    Fut: std::future::Future<Output = T> + Send + 'static,
{
    let fut = body(Arc::clone(&fx.s), Arc::clone(lib));
    fx.engine.block_on(fut)
}

fn code<T: std::fmt::Debug>(r: Result<T, CreditsError>) -> &'static str {
    r.unwrap_err().code()
}

/// A `PlatformOp` grant of `max_duffs` (and `max_credits`).
fn grant(fx: &Fixture, max_duffs: u64, max_credits: u64) -> String {
    let purpose = dw_vault::GrantPurpose::PlatformOp {
        max_duffs,
        max_credits,
    };
    let granted =
        fx.s.vault
            .authorize(purpose, Some(&fx.wallet.0), dw_vault::Credential::None);
    granted.unwrap().id
}

fn quote_of(
    fx: &Fixture,
    lib: &Arc<FakeFund>,
    duffs: u64,
) -> Result<PlatformFundQuote, CreditsError> {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        fund::quote(&s, wallet, &*lib, FundAmount::Duffs { duffs }).await
    })
}

/// What a quote of `duffs` asks for: the amount and the wallet's
/// `fee_bound_worst`, exactly what the funding call's permit charges.
fn exact_grant(fx: &Fixture, duffs: u64) -> u64 {
    let (s, wallet) = (Arc::clone(&fx.s), fx.wallet);
    duffs
        + fx.engine
            .block_on(async move { s.fee_bound_worst(wallet).await })
            .unwrap()
}

/// A fund of 1,000,000 duffs under exactly its quote's grant, its apply
/// (if any) answered as scripted.
fn fund_of(fx: &Fixture, lib: &Arc<FakeFund>) -> Result<PlatformFundOutcome, CreditsError> {
    let (wallet, grant) = (fx.wallet, grant(fx, exact_grant(fx, 1_000_000), 0));
    run(fx, lib, move |s, lib| async move {
        fund::fund(
            &s,
            wallet,
            &*lib,
            FundAmount::Duffs { duffs: 1_000_000 },
            &grant,
        )
        .await
    })
}

/// "Check again" on the pending fund of `outpoint`.
fn resume_of(
    fx: &Fixture,
    lib: &Arc<FakeFund>,
    outpoint: OutPoint,
) -> Result<PlatformFundState, CreditsError> {
    let (wallet, grant) = (fx.wallet, grant(fx, 1, 0));
    let txid = outpoint.txid.to_string();
    run(fx, lib, move |s, lib| async move {
        fund::resume(&s, wallet, &*lib, &txid, &grant).await
    })
    .map(|o| o.state)
}

/// The pending funds: `(txid, proved, consumption_unknown)`.
fn pending_of(fx: &Fixture, lib: &Arc<FakeFund>) -> Vec<(String, bool, bool)> {
    let wallet = fx.wallet;
    let pending = run(fx, lib, move |s, lib| async move {
        fund::pending(&s, wallet, &*lib).await.unwrap()
    });
    (pending.into_iter())
        .map(|p| (p.txid, p.proved, p.consumption_unknown))
        .collect()
}

fn fee() -> u64 {
    calculate_address_funding_from_asset_lock_min_required_fee(0, 1, PlatformVersion::latest())
}

fn settled(fx: &Fixture) -> std::collections::HashSet<Txid> {
    (fx.engine.block_on(fx.s.leases.top_ups_settled(fx.wallet))).unwrap()
}

/// [`locks::finish`] of `outpoint` over the kind, its lease of a new
/// grant; whether that lease was asked for; and the outcomes of the calls
/// it recorded.
fn finish_with_outcomes(
    fx: &Fixture,
    lib: &Arc<FakeFund>,
    outpoint: OutPoint,
) -> (Result<LockFinish, PlatformError>, bool, Vec<FlowOutcome>) {
    let (wallet, grant) = (fx.wallet, grant(fx, 1, 0));
    let (kept, begun) = (Arc::new(Mutex::new(None)), Arc::new(AtomicBool::new(false)));
    let (keep, asked) = (Arc::clone(&kept), Arc::clone(&begun));
    let finished = run(fx, lib, move |s, lib| async move {
        let kind = FundLocks::new(&s, wallet, &*lib);
        let leases = Arc::clone(&s);
        let lease_of = || async move {
            asked.store(true, Ordering::SeqCst);
            let lease =
                (leases.begin_lease(wallet, FlowKind::PlatformAddressFund, &[grant])).await?;
            *keep.lock().unwrap() = Some(lease.clone());
            Ok::<_, PlatformError>(lease)
        };
        locks::finish(&kind, outpoint, lease_of).await
    });
    let kept: Option<Lease> = kept.lock().unwrap().take();
    let outcomes = (kept.as_ref()).map_or_else(Vec::new, conformance::call_outcomes);
    (finished, begun.load(Ordering::SeqCst), outcomes)
}

fn finish(
    fx: &Fixture,
    lib: &Arc<FakeFund>,
    outpoint: OutPoint,
) -> (Result<LockFinish, PlatformError>, bool) {
    let (finished, asked, _) = finish_with_outcomes(fx, lib, outpoint);
    (finished, asked)
}

/// [`locks::detect`] over the kind, nothing reported used.
fn listed(fx: &Fixture, lib: &Arc<FakeFund>) -> Result<Vec<UnfinishedLock>, PlatformError> {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        locks::detect(&FundLocks::new(&s, wallet, &*lib)).await
    })
}

fn state_of(fx: &Fixture, lib: &Arc<FakeFund>) -> Vec<(OutPoint, UnfinishedState)> {
    (listed(fx, lib).unwrap().into_iter())
        .map(|l| (OutPoint::new(l.txid.parse().unwrap(), l.vout), l.state))
        .collect()
}

/// The kind's settled and unknown records, as a finish reads them.
fn marks(fx: &Fixture, lib: &Arc<FakeFund>) -> (Marks, Marks) {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        let kind = FundLocks::new(&s, wallet, &*lib);
        (kind.settled_locks().await, kind.unknown_locks().await)
    })
}

/// The kind's locks Platform reported used, as a read finds them.
fn reported(fx: &Fixture, lib: &Arc<FakeFund>) -> HashSet<OutPoint> {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        FundLocks::new(&s, wallet, &*lib).reported_locks().await
    })
}

/// The quote: the lock, its Core fee bound, Platform's fee estimate (dpp's
/// minimum for one output, 56,000 duffs' worth at the pin) out of the
/// lock's credits, the receive address, and a grant of duffs only. The
/// least fund is one duff above that fee.
#[test]
fn a_fund_quote_names_the_fee_the_minimum_and_the_address() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    assert_eq!(fee(), 56_000_000);
    let q = quote_of(&fx, &lib, 1_000_000).unwrap();
    let total = exact_grant(&fx, 1_000_000);
    assert_eq!(
        (q.duffs, q.fee_duffs, q.total_duffs),
        (1_000_000, total - 1_000_000, total)
    );
    assert_eq!((q.fee_credits, q.credits), (fee(), 1_000_000_000 - fee()));
    assert_eq!(q.to, lib.receive.to_bech32m_string(Network::Testnet));
    assert_eq!((q.grant.max_duffs, q.grant.max_credits), (total, 0));
    let min = fee() / 1_000 + 1;
    assert_eq!(quote_of(&fx, &lib, min).unwrap().credits, 1_000);
    assert_eq!(
        quote_of(&fx, &lib, min - 1).unwrap_err(),
        CreditsError::BelowMinimum { min, max: None }
    );
}

/// Core falling short, a held coin and a lock of more than 50 coins are
/// refused before anything is signed; below address funds the feature is
/// off.
#[test]
fn a_fund_quote_refuses_what_core_cannot_fund() {
    let fx = Fixture::new(0);
    let with = |f: fn(&mut FakeFund)| {
        let mut lib = FakeFund::new(&fx);
        f(&mut lib);
        quote_of(&fx, &Arc::new(lib), 1_000_000)
    };
    assert_eq!(
        with(|l| l.spendable.duffs = 1_000_500).unwrap_err(),
        CreditsError::FundingInsufficient {
            needed: exact_grant(&fx, 1_000_000),
            available: 1_000_500,
        }
    );
    assert_eq!(
        code(with(|l| l.spendable.protected = 1)),
        "credits.coins_locked"
    );
    assert_eq!(
        with(|l| l.coins = vec![20_000; 60]).unwrap_err(),
        CreditsError::TooManyInputs {
            inputs: 51,
            max: 50
        }
    );
    assert_eq!(code(with(|l| l.version = 10)), "platform.feature_off");
}

/// A fund locks the Core coins under its own step and applies that lock,
/// proved, to the receive address: the lock settles in the journal, pends
/// no more, and its activity row is done with the credits the proof shows
/// (at most the lock's).
#[test]
fn a_fund_applies_its_own_lock_to_the_receive_address() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    lib.answer(Answer::Done(Some(944_000_000)));
    let out = fund_of(&fx, &lib).unwrap();
    let outpoint = lib.outpoint(0);
    assert_eq!(out.txid, outpoint.txid.to_string());
    assert_eq!(
        out.state,
        PlatformFundState::Applied {
            credits: 944_000_000
        }
    );
    assert_eq!(*lib.applied.lock().unwrap(), [(outpoint, lib.receive)]);
    assert!(settled(&fx).contains(&outpoint.txid));
    assert!(marks(&fx, &lib).0.outpoints.contains(&outpoint));
    assert!(pending_of(&fx, &lib).is_empty());
    assert_eq!(*lib.signed.lock().unwrap(), [true]);
    assert_eq!(lib.syncs.load(Ordering::SeqCst), 1, "the settle syncs");
    let (s, wallet, txid) = (Arc::clone(&fx.s), fx.wallet, out.txid.clone());
    let recorded = (fx.engine)
        .block_on(async move { platform_moves::fund_address(&s, wallet, &txid).await })
        .unwrap();
    assert_eq!(
        recorded,
        Some(lib.receive.to_bech32m_string(Network::Testnet))
    );

    // A rise beyond the lock's credits is not the fund's.
    lib.answer(Answer::Done(Some(5_000_000_000)));
    let out = fund_of(&fx, &lib).unwrap();
    assert_eq!(
        out.state,
        PlatformFundState::Applied {
            credits: 1_000_000_000
        }
    );
}

/// The step holds a fund's lock at most once: a funding whose outcome is
/// unknown (signed, nothing tracked) is never funded again, and every later
/// fund is refused on it.
#[test]
fn the_fund_step_is_held_once() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    *lib.funding.lock().unwrap() = Script::LostUntracked;
    let unknown = |r: Result<PlatformFundOutcome, CreditsError>| match r {
        Err(CreditsError::Platform(PlatformError::BroadcastUnknown { artifact })) => artifact,
        other => panic!("not broadcast_unknown: {other:?}"),
    };
    assert_eq!(unknown(fund_of(&fx, &lib)), "paddr/0/funding");
    *lib.funding.lock().unwrap() = Script::Proved;
    assert_eq!(unknown(fund_of(&fx, &lib)), "paddr/0/funding");
    assert_eq!(lib.funds.load(Ordering::SeqCst), 1, "funded once");
}

/// The input cap's refusal leaves nothing out: the next fund is not held.
#[test]
fn a_fund_of_more_than_50_coins_leaves_its_step_free() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    *lib.funding.lock().unwrap() = Script::TooMany;
    assert_eq!(
        fund_of(&fx, &lib).unwrap_err(),
        CreditsError::TooManyInputs {
            inputs: 51,
            max: 50
        }
    );
    assert!(fund::next_step(&fx.s, fx.wallet).is_ok());
}

/// A fund whose wallet was removed or re-created after its lock was
/// signed, before the library tracked it: the released signature leaves
/// the step `MaybeSent`, so the call reads `broadcast_unknown` for it, not
/// the library's error, and the next fund is refused on the step.
#[test]
fn a_fund_whose_wallet_left_after_its_signature_reads_broadcast_unknown() {
    refused_after_the_signature(Script::RemovedBeforeSend);
}

/// A fund the library refuses over the asset lock's input cap after
/// signing it reads the same.
#[test]
fn a_fund_over_the_asset_lock_input_cap_reads_broadcast_unknown() {
    refused_after_the_signature(Script::OverInputCap);
}

fn refused_after_the_signature(script: Script) {
    const STEP: &str = "paddr/0/funding";
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    *lib.funding.lock().unwrap() = script;
    let unknown = |r: Result<PlatformFundOutcome, CreditsError>| match r {
        Err(CreditsError::Platform(PlatformError::BroadcastUnknown { artifact })) => artifact,
        other => panic!("not broadcast_unknown: {other:?}"),
    };
    assert_eq!(unknown(fund_of(&fx, &lib)), STEP);
    assert_eq!(
        fx.s.leases.funding_status(fx.wallet, STEP),
        Some(super::flows::DispatchState::MaybeSent)
    );
    *lib.funding.lock().unwrap() = Script::Proved;
    assert_eq!(unknown(fund_of(&fx, &lib)), STEP);
    assert_eq!(lib.funds.load(Ordering::SeqCst), 1, "funded once");
    assert!(pending_of(&fx, &lib).is_empty());
    assert!(settled(&fx).is_empty());
}

/// A lock out without its proof waits ("Pending — finish"): it is listed,
/// refuses the next fund, and "Check again" without a proof applies
/// nothing.
#[test]
fn a_lock_waiting_for_its_proof_pends_and_blocks_the_next_fund() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    *lib.funding.lock().unwrap() = Script::Unproved;
    let out = fund_of(&fx, &lib).unwrap();
    assert_eq!(out.state, PlatformFundState::ProofWaiting);
    assert_eq!(pending_of(&fx, &lib), [(out.txid.clone(), false, false)]);
    assert_eq!(
        quote_of(&fx, &lib, 1_000_000).unwrap_err(),
        CreditsError::FundPending { txid: out.txid }
    );
    let outpoint = lib.outpoint(0);
    assert_eq!(
        resume_of(&fx, &lib, outpoint).unwrap(),
        PlatformFundState::ProofWaiting
    );
    assert!(lib.applied().is_empty());
}

/// A fund interrupted after its lock (no apply yet) resumes the same
/// outpoint once the proof is in, to the address it was recorded for,
/// never a new lock.
#[test]
fn an_interrupted_fund_resumes_the_same_outpoint_never_a_new_lock() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    *lib.funding.lock().unwrap() = Script::Unproved;
    fund_of(&fx, &lib).unwrap();
    let outpoint = lib.outpoint(0);
    lib.prove(outpoint);
    // The receive address moved on meanwhile.
    let mut moved = FakeFund::new(&fx);
    moved.receive = PlatformAddress::P2pkh([0x43; 20]);
    *moved.tracked.lock().unwrap() = lib.tracked.lock().unwrap().clone();
    let lib = Arc::new(moved);
    lib.answer(Answer::Done(None));
    let state = resume_of(&fx, &lib, outpoint).unwrap();
    assert_eq!(
        state,
        PlatformFundState::Applied {
            credits: 1_000_000_000 - fee()
        }
    );
    assert_eq!(
        *lib.applied.lock().unwrap(),
        [(outpoint, PlatformAddress::P2pkh([0x42; 20]))]
    );
    assert_eq!(lib.funds.load(Ordering::SeqCst), 0, "no new lock");
    assert!(pending_of(&fx, &lib).is_empty());
}

/// A failure after signing is no settle evidence: the lock pends
/// "Checking" however often a resume hears it, blocks no new fund, and
/// each resume applies the same outpoint. Platform's "already consumed"
/// for that outpoint, its proved state saying the same, is kept as
/// reported, not settled: the lock leaves the pending list (finished
/// elsewhere) and is not applied again.
#[test]
fn already_consumed_finishes_the_lock_elsewhere_and_a_failure_reads_checking() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    lib.answer(Answer::Failed);
    let out = fund_of(&fx, &lib).unwrap();
    assert_eq!(out.state, PlatformFundState::ConsumptionUnknown);
    let outpoint = lib.outpoint(0);
    assert_eq!(pending_of(&fx, &lib), [(out.txid.clone(), true, true)]);
    quote_of(&fx, &lib, 1_000_000).unwrap();
    lib.answer(Answer::Failed);
    assert_eq!(
        resume_of(&fx, &lib, outpoint).unwrap(),
        PlatformFundState::ConsumptionUnknown
    );
    assert!(marks(&fx, &lib).1.outpoints.contains(&outpoint));
    assert!(reported(&fx, &lib).is_empty());
    lib.proves(outpoint, SpentRead::Consumed);
    lib.answer(Answer::Consumed { of: None });
    assert_eq!(
        resume_of(&fx, &lib, outpoint).unwrap(),
        PlatformFundState::ConsumptionUnknown
    );
    assert_eq!(reported(&fx, &lib), HashSet::from([outpoint]));
    assert!(!settled(&fx).contains(&outpoint.txid));
    assert!(marks(&fx, &lib).0.outpoints.is_empty());
    assert!(pending_of(&fx, &lib).is_empty());
    quote_of(&fx, &lib, 1_000_000).unwrap();
    assert_eq!(code(resume_of(&fx, &lib, outpoint)), "invalid_argument");
    assert_eq!(lib.applied(), [outpoint; 3]);
}

/// A refusal before the lock's key signed anything is `NotApplied`: the
/// lock pends, not of unknown consumption, and blocks the next fund.
#[test]
fn a_refused_apply_is_not_applied_and_the_lock_pends() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    lib.refuse(Answer::Failed);
    let out = fund_of(&fx, &lib).unwrap();
    let refused = PlatformError::from(PlatformWalletError::TransactionBroadcast(String::new()));
    assert_eq!(
        out.state,
        PlatformFundState::NotApplied {
            error: refused.code().into()
        }
    );
    assert!(lib.signed.lock().unwrap().is_empty());
    assert_eq!(pending_of(&fx, &lib), [(out.txid.clone(), true, false)]);
    assert_eq!(code(quote_of(&fx, &lib, 1_000_000)), "credits.fund_pending");
}

/// Only this install's fund locks are listed: one no `paddr/` step signed
/// (a restore, another app on the seed) is not, blocks nothing, and is not
/// resumed.
#[test]
fn a_lock_no_fund_step_signed_is_not_listed() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    for status in [
        AssetLockStatus::InstantSendLocked,
        AssetLockStatus::RecoveredFromChain,
    ] {
        let row = lib.lock(9, 1_000_000, status, true);
        let foreign = row.out_point;
        *lib.tracked.lock().unwrap() = vec![row];
        assert!(pending_of(&fx, &lib).is_empty());
        quote_of(&fx, &lib, 1_000_000).unwrap();
        assert_eq!(code(resume_of(&fx, &lib, foreign)), "invalid_argument");
    }
    assert!(lib.applied().is_empty());
}

/// The recovery lists an address-fund lock no step of this install signed
/// (a restore's) and finishes it through the same kind, under its own
/// lease: it pays the receive address, records the lock's fund row there,
/// and settles it on the proved apply; the fund's own list still ignores
/// it.
#[test]
fn the_recovery_finishes_a_lock_no_fund_step_signed() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    lib.track(lib.lock(9, 1_000_000, AssetLockStatus::RecoveredFromChain, true));
    let foreign = lib.outpoint(0);
    assert_eq!(
        listed(&fx, &lib).unwrap(),
        [UnfinishedLock {
            txid: foreign.txid.to_string(),
            vout: 0,
            kind: LockKindId::PlatformAddress,
            duffs: 1_000_000,
            origin: LockOrigin::Restored,
            state: UnfinishedState::Checking,
            remaining_credits: None,
        }]
    );
    assert!(pending_of(&fx, &lib).is_empty());
    lib.answer(Answer::Done(Some(900_000_000)));
    let (finished, asked) = finish(&fx, &lib, foreign);
    assert_eq!(finished.unwrap(), LockFinish::Applied);
    assert!(asked);
    assert_eq!(*lib.applied.lock().unwrap(), [(foreign, lib.receive)]);
    assert!(settled(&fx).contains(&foreign.txid));
    assert!(listed(&fx, &lib).unwrap().is_empty());
    let (s, wallet, txid) = (Arc::clone(&fx.s), fx.wallet, foreign.txid.to_string());
    let recorded = (fx.engine)
        .block_on(async move { platform_moves::fund_address(&s, wallet, &txid).await })
        .unwrap();
    assert_eq!(
        recorded,
        Some(lib.receive.to_bech32m_string(Network::Testnet))
    );
}

/// Only Platform's "already consumed" naming the applied outpoint is the
/// kind's unproved answer, signed or not (the lock reported used, finished
/// elsewhere): one naming another outpoint is a failure like any other,
/// unknown after the lock key's signature (Checking) and the refusal
/// before it (the lock stays Ready, its call `Cancelled`).
#[test]
fn already_consumed_counts_only_for_the_applied_outpoint() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let other = OutPoint::new(Txid::from_byte_array([0x99; 32]), 0);
    for n in 1..=3 {
        lib.track(lib.lock(n, 1_000_000, AssetLockStatus::ChainLocked, true));
    }
    let (own, signed, unsigned) = (lib.outpoint(0), lib.outpoint(1), lib.outpoint(2));

    lib.refuse(Answer::Consumed { of: None });
    assert_eq!(finish(&fx, &lib, own).0.unwrap(), LockFinish::ReportedUsed);
    lib.answer(Answer::Consumed { of: Some(other) });
    assert_eq!(finish(&fx, &lib, signed).0.unwrap(), LockFinish::Checking);
    lib.refuse(Answer::Consumed { of: Some(other) });
    let refused = finish(&fx, &lib, unsigned).0.unwrap();
    assert!(
        matches!(refused, LockFinish::NotApplied { .. }),
        "{refused:?}"
    );

    assert_eq!(
        state_of(&fx, &lib),
        [
            (signed, UnfinishedState::Checking),
            (unsigned, UnfinishedState::Ready),
        ]
    );
    let (settled, unknown) = marks(&fx, &lib);
    assert!(settled.outpoints.is_empty());
    assert_eq!(unknown.outpoints, HashSet::from([signed]));
    assert_eq!(reported(&fx, &lib), HashSet::from([own]));
}

/// The apply's signer gives the lock's own credit key only: another credit
/// key the funding scope signs with (a shield's, an identity top-up's) is
/// refused, so a failure after it is no signature of the lock and leaves
/// it Ready, not Checking.
#[test]
fn the_apply_signs_with_the_locks_credit_key_only() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    lib.track(lib.lock(1, 1_000_000, AssetLockStatus::ChainLocked, true));
    let at = lib.outpoint(0);
    for other in ["m/9'/1'/5'/5'/0", "m/9'/1'/5'/2'/0", "m/9'/1'/5'/4'/1"] {
        *lib.key.lock().unwrap() = DerivationPath::from_str(other).unwrap();
        lib.answer(Answer::Failed);
        let refused = finish(&fx, &lib, at).0.unwrap();
        assert!(
            matches!(refused, LockFinish::NotApplied { .. }),
            "{other}: {refused:?}"
        );
        assert_eq!(
            state_of(&fx, &lib),
            [(at, UnfinishedState::Ready)],
            "{other}"
        );
    }
    assert_eq!(*lib.signed.lock().unwrap(), [false; 3]);
    assert!(
        fund_record(&fx, at.txid).is_none(),
        "the record arming made is removed when nothing was signed"
    );
    *lib.key.lock().unwrap() = DerivationPath::from_str(LOCK_KEY).unwrap();
    lib.answer(Answer::Failed);
    assert_eq!(finish(&fx, &lib, at).0.unwrap(), LockFinish::Checking);
    assert_eq!(lib.signed.lock().unwrap().last(), Some(&true));
}

/// While Core catches up, or the vault is locked, a fund is refused
/// before anything is signed.
#[test]
fn a_fund_waits_for_core_and_the_vault() {
    let fx = Fixture::new(0);
    for (hold, expected) in [
        (RegistrationWait::Sync, "platform.context_unavailable"),
        (RegistrationWait::Unlock, "platform.signer_unavailable"),
    ] {
        let mut lib = FakeFund::new(&fx);
        lib.hold = Some(hold);
        let lib = Arc::new(lib);
        assert_eq!(code(fund_of(&fx, &lib)), expected);
        assert_eq!(lib.funds.load(Ordering::SeqCst), 0);
    }
}

/// A `PlatformAddressFund` lease holds the asset-lock credit keys alone:
/// no Credits budget, no identity key, whatever its grant allows.
#[test]
fn a_fund_lease_issues_the_funding_signer_only() {
    let fx = Fixture::new(0);
    let token = grant(&fx, 2_000_000, 5_000_000);
    let s = Arc::clone(&fx.s);
    let wallet = fx.wallet;
    let lease = (fx.engine)
        .block_on(async move {
            s.begin_lease(wallet, FlowKind::PlatformAddressFund, &[token])
                .await
        })
        .unwrap();
    assert!(lease.uncounted_funding_signer().is_ok());
    assert_eq!(lease.available(BudgetPurpose::Funding).unwrap(), 2_000_000);
    assert_eq!(lease.available(BudgetPurpose::Credits).unwrap(), 0);
    assert!(lease.identity_signer(&[0]).is_err());
    assert!(lease.call_platform_address_signer().is_err());
}

/// [`FundLocks`] over [`FakeFund`]'s tracked rows, its apply scripted, its
/// records the session's own `app.sqlite` and journal.
struct FundRig {
    fx: Fixture,
    lib: Arc<FakeFund>,
    outcomes: Mutex<Vec<FlowOutcome>>,
}

impl Rig for FundRig {
    fn funding_type(&self) -> AssetLockFundingType {
        AssetLockFundingType::AssetLockAddressTopUp
    }

    fn credit_script(&self) -> ScriptBuf {
        self.lib.script.clone()
    }

    fn track(&self, rows: Vec<TrackedAssetLock>) {
        for row in rows {
            self.lib.track(row);
        }
    }

    /// The fund's records by txid are the journal's `top_up_settled`; it
    /// keeps none of unknown consumption.
    fn record_txid(&self, mark: Mark, txid: Txid) {
        if mark == Mark::Settled {
            let (s, wallet) = (Arc::clone(&self.fx.s), self.fx.wallet);
            (self.fx.engine)
                .block_on(async move {
                    (s.leases)
                        .settle_top_up(wallet, txid, TopUpSettled::Applied)
                        .await
                })
                .unwrap();
        }
    }

    fn next(&self, end: End) {
        match end {
            End::Done => self.lib.answer(Answer::Done(None)),
            End::AlreadyConsumed => self.lib.answer(Answer::Consumed { of: None }),
            End::Unknown => {
                let other = OutPoint::new(Txid::from_byte_array([0x98; 32]), 0);
                self.lib.answer(Answer::Consumed { of: Some(other) });
            }
            End::RefusedUnsigned => self.lib.refuse(Answer::Failed),
            End::SignedThenFailed => self.lib.answer(Answer::Failed),
        }
    }

    fn applies(&self) -> usize {
        self.lib.applied.lock().unwrap().len()
    }

    fn list(&self) -> Result<Vec<UnfinishedLock>, PlatformError> {
        listed(&self.fx, &self.lib)
    }

    fn finish(&self, outpoint: OutPoint) -> (Result<LockFinish, PlatformError>, bool) {
        let (finished, asked, outcomes) = finish_with_outcomes(&self.fx, &self.lib, outpoint);
        *self.outcomes.lock().unwrap() = outcomes;
        (finished, asked)
    }

    fn outcomes(&self) -> Vec<FlowOutcome> {
        self.outcomes.lock().unwrap().clone()
    }

    fn marks(&self) -> (Marks, Marks) {
        marks(&self.fx, &self.lib)
    }

    fn minimum(&self) -> Option<Minimum> {
        let wallet = self.fx.wallet;
        run(&self.fx, &self.lib, move |s, lib| async move {
            FundLocks::new(&s, wallet, &*lib).minimum()
        })
    }

    fn reports_used(&self) -> bool {
        true
    }

    fn prove_spent(&self, outpoint: OutPoint, read: SpentRead) {
        self.lib.proves(outpoint, read);
    }
}

/// Type 4 holds the contract every kind does (`unfinished_conformance`),
/// and every signature its applies asked for was the lock key's.
#[test]
fn fund_locks_conform() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let rig = FundRig {
        fx,
        lib,
        outcomes: Mutex::default(),
    };
    conformance::conformance(&rig);
    let signed = rig.lib.signed.lock().unwrap().clone();
    assert!(
        !signed.is_empty() && signed.iter().all(|s| *s),
        "{signed:?}"
    );
}

// MARK: the recovery's type 4, the activity, concurrency

/// The pending funds' states.
fn states_of(fx: &Fixture, lib: &Arc<FakeFund>) -> Vec<(String, UnfinishedState)> {
    let wallet = fx.wallet;
    let pending = run(fx, lib, move |s, lib| async move {
        fund::pending(&s, wallet, &*lib).await.unwrap()
    });
    (pending.into_iter()).map(|p| (p.txid, p.state)).collect()
}

/// An own fund's lock out with its proof, not applied yet.
fn own_open_fund(fx: &Fixture, lib: &Arc<FakeFund>) -> OutPoint {
    *lib.funding.lock().unwrap() = Script::Unproved;
    fund_of(fx, lib).unwrap();
    let n = lib.tracked.lock().unwrap().len();
    let outpoint = lib.outpoint(n - 1);
    lib.prove(outpoint);
    *lib.funding.lock().unwrap() = Script::Proved;
    outpoint
}

/// The address fund of 1,000,000 duffs adds about this much.
fn estimate() -> u64 {
    1_000_000_000 - fee()
}

/// Tracks `row` in the wallet as the library does: its state and, through
/// the session's store, `wallet.sqlite`.
async fn track_in_wallet(s: &Arc<NetworkSession>, wallet: WalletId, row: TrackedAssetLock) {
    use platform_wallet::changeset::{
        AssetLockChangeSet, AssetLockEntry, PlatformWalletChangeSet, PlatformWalletPersistence,
    };
    let (outpoint, entry) = (row.out_point, AssetLockEntry::from(&row));
    let w = s.wallet(&wallet).await.unwrap();
    w.state_mut()
        .await
        .tracked_asset_locks
        .insert(outpoint, row);
    let changes = PlatformWalletChangeSet {
        asset_locks: Some(AssetLockChangeSet {
            asset_locks: [(outpoint, entry)].into(),
            removed: Default::default(),
        }),
        ..Default::default()
    };
    (s.live().unwrap().store).store(wallet.0, changes).unwrap();
}

/// A wipe of the wallet, then the same seed imported again.
fn remove_and_reimport(fx: &Fixture) {
    let s = Arc::clone(&fx.s);
    let wipe = (fx.s.vault)
        .authorize(
            dw_vault::GrantPurpose::Wipe,
            Some(&fx.wallet.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    fx.engine
        .block_on(s.remove_wallet(fx.wallet, wipe))
        .unwrap();
    let again = fx.engine.block_on(s.import_wallet(
        zeroize::Zeroizing::new(super::journal_tests::ABANDON_12.to_vec()),
        zeroize::Zeroizing::new(Vec::new()),
        crate::ImportOptions::default(),
    ));
    assert_eq!(again.unwrap(), fx.wallet);
}

/// Repair's "Finish transfers" over the kind alone, under a Registration
/// lease, as `unfinished::finish_all` runs it.
fn finish_transfers(fx: &Fixture, lib: &Arc<FakeFund>) -> super::FinishReport {
    let (wallet, grant) = (fx.wallet, grant(fx, 1, 0));
    run(fx, lib, move |s, lib| async move {
        let lease = (s.begin_lease(wallet, FlowKind::Registration, &[grant]))
            .await
            .unwrap();
        let mut report = super::FinishReport::default();
        locks::finish_kind(&FundLocks::new(&s, wallet, &*lib), &lease, &mut report).await;
        report
    })
}

/// A wallet removed and imported again keeps the journal's `paddr/` steps:
/// its open lock is still this install's, listed and resumed by the fund.
/// The same seed on a new install has no such step: its lock is the
/// recovery's, listed as restored, finished by `finish_transfer`'s body on
/// the same outpoint and by Finish transfers, and it blocks no fund.
#[test]
fn a_fund_lock_without_this_installs_step_is_listed_and_finished_by_the_recovery() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let first = own_open_fund(&fx, &lib);
    remove_and_reimport(&fx);
    assert_eq!(
        states_of(&fx, &lib),
        [(first.txid.to_string(), UnfinishedState::Ready)]
    );
    assert_eq!(code(quote_of(&fx, &lib, 1_000_000)), "credits.fund_pending");

    // The seed restored on a new install: the library's locks, no steps.
    let fresh = Fixture::new(0);
    let mut other = FakeFund::new(&fresh);
    *other.tracked.get_mut().unwrap() = lib.tracked.lock().unwrap().clone();
    let lib = Arc::new(other);
    let second = lib.lock(9, 2_000_000, AssetLockStatus::RecoveredFromChain, true);
    let second = {
        let at = second.out_point;
        lib.track(second);
        at
    };
    let fx = fresh;
    assert!(states_of(&fx, &lib).is_empty());
    quote_of(&fx, &lib, 1_000_000).unwrap();
    let restored = listed(&fx, &lib).unwrap();
    assert_eq!(restored.len(), 2);
    let state_at = |at: OutPoint| {
        (restored.iter())
            .find(|l| l.txid == at.txid.to_string())
            .map(|l| (l.kind, l.origin, l.state))
    };
    assert_eq!(
        state_at(first),
        Some((
            LockKindId::PlatformAddress,
            LockOrigin::Restored,
            UnfinishedState::Ready
        ))
    );
    assert_eq!(
        state_at(second),
        Some((
            LockKindId::PlatformAddress,
            LockOrigin::Restored,
            UnfinishedState::Checking
        )),
        "read back from the chain"
    );

    lib.answer(Answer::Done(Some(900_000_000)));
    let (wallet, grant) = (fx.wallet, grant(&fx, 1, 0));
    let finished = run(&fx, &lib, move |s, lib| async move {
        super::unfinished::finish_fund(&s, wallet, &*lib, first, &grant).await
    });
    assert_eq!(finished.unwrap(), LockFinish::Applied);
    assert_eq!(lib.applied(), [first]);

    lib.answer(Answer::Done(None));
    let report = finish_transfers(&fx, &lib);
    assert_eq!(
        report.by_kind,
        [locks::TransfersReport {
            kind: LockKindId::PlatformAddress,
            resumed: 1,
            completed: 1,
            still_pending: 0,
        }]
    );
    assert_eq!(lib.applied(), [first, second]);
    assert!(listed(&fx, &lib).unwrap().is_empty());
    assert_eq!(lib.funds.load(Ordering::SeqCst), 0, "no new lock");
}

/// A restored lock of `kind` (no step of this install signed it) in a
/// real session's wallet, with the other kind's side failing to open:
/// `DashPay::unfinished_transfers` lists it and names the other kind
/// unread; `DashPay::finish_transfer` and `DashPay::finish_asset_locks`
/// reach it through `kind`'s path alone. Offline the apply itself cannot
/// run, so nothing is applied or settled and the lock stays listed. The
/// single Finish's answer is returned.
fn facade_with_the_other_side_down(kind: LockKindId) -> Result<LockFinish, PlatformError> {
    let (other, key, flow, funding) = match kind {
        LockKindId::PlatformAddress => (
            LockKindId::Shielded,
            LOCK_KEY,
            FlowKind::PlatformAddressFund,
            AssetLockFundingType::AssetLockAddressTopUp,
        ),
        LockKindId::Shielded => (
            LockKindId::PlatformAddress,
            "m/9'/1'/5'/5'/0",
            FlowKind::Shield,
            AssetLockFundingType::AssetLockShieldedAddressTopUp,
        ),
    };
    let dir = dw_testutil::private_tempdir();
    let events = Arc::new(super::journal_tests::Recorder::default());
    let engine = super::journal_tests::engine(dir.path(), events as Arc<dyn crate::EventSink>);
    let (s, wallet) = super::journal_tests::session(&engine, false);
    let grant = |max_duffs| {
        (s.vault)
            .authorize(
                dw_vault::GrantPurpose::PlatformOp {
                    max_duffs,
                    max_credits: 0,
                },
                Some(&wallet.0),
                dw_vault::Credential::None,
            )
            .unwrap()
            .id
    };
    let at = restored_lock(&engine, &s, wallet, (key, flow, funding), grant(1));
    let dp = s.dashpay(wallet);
    super::unfinished::open_fails::set(&s, other, true);

    let listed = engine.block_on(dp.unfinished_transfers()).unwrap();
    assert_eq!(listed.unread, [other]);
    let ours: Vec<_> = (listed.locks.iter())
        .filter(|l| l.txid == at.txid.to_string())
        .map(|l| (l.kind, l.origin, l.duffs))
        .collect();
    assert_eq!(ours, [(kind, LockOrigin::Restored, 1_000_000)]);

    let one = engine.block_on(dp.finish_transfer(kind, at.txid.to_string(), 0, grant(1)));
    let report = engine.block_on(dp.finish_asset_locks(grant(1))).unwrap();
    assert_eq!(report.unread, [other], "{report:?}");
    assert_eq!(
        (report.by_kind.iter()).map(|k| k.kind).collect::<Vec<_>>(),
        [kind],
        "{report:?}"
    );
    assert_eq!(report.completed, 0);
    super::unfinished::open_fails::set(&s, other, false);
    let listed = engine.block_on(dp.unfinished_transfers()).unwrap();
    assert!(
        (listed.locks.iter()).any(|l| l.txid == at.txid.to_string()),
        "still unfinished"
    );
    engine
        .block_on(engine.close_network(crate::DashNetwork::Regtest))
        .unwrap();
    one
}

/// A proved lock of 1,000,000 duffs recovered from the chain, paying `key`
/// as `flow`'s lease derives it under `grant`, tracked in the session's
/// wallet as the library does.
pub(super) fn restored_lock(
    engine: &crate::Engine,
    s: &Arc<NetworkSession>,
    wallet: WalletId,
    (key, flow, funding): (&str, FlowKind, AssetLockFundingType),
    grant: String,
) -> OutPoint {
    let script = {
        let (s, key) = (Arc::clone(s), key.to_owned());
        engine.block_on(async move {
            let lease = (s.begin_lease(wallet, flow, &[grant])).await.unwrap();
            let funding = lease.uncounted_funding_signer().unwrap();
            let path = DerivationPath::from_str(&key).unwrap();
            let key = key_wallet::Signer::public_key(&funding, &path)
                .await
                .unwrap();
            ScriptBuf::new_p2pkh(&dashcore::PublicKey::new(key).pubkey_hash())
        })
    };
    let tx = conformance::transaction(0x51, 1, 1_000_000, &script);
    let row = conformance::tracked(&tx, 0, funding, AssetLockStatus::RecoveredFromChain, true);
    let at = row.out_point;
    engine.block_on(track_in_wallet(s, wallet, row));
    at
}

/// The type-4 side lists and finishes with the shielded side down: the
/// single Finish runs the fund kind's apply, which offline is refused
/// before any signature as Platform's context is unavailable (the shield
/// path answers `Checking`).
#[test]
fn the_facade_lists_and_finishes_a_restored_fund_lock_without_the_shielded_side() {
    let one = facade_with_the_other_side_down(LockKindId::PlatformAddress);
    assert_eq!(
        one.unwrap(),
        LockFinish::NotApplied {
            error: "platform.context_unavailable".into()
        }
    );
}

/// The reverse: the shielded side lists and finishes a restored shield
/// lock with the type-4 side down. Offline its apply fails after its
/// signature: `Checking`.
#[test]
fn the_facade_lists_and_finishes_a_restored_shield_lock_without_the_fund_side() {
    let one = facade_with_the_other_side_down(LockKindId::Shielded);
    assert_eq!(one.unwrap(), LockFinish::Checking);
}

/// A lock whose minimum rose above it (TooSmall) is listed with its state,
/// offers no Finish and blocks no fund: the next fund builds its own lock.
#[test]
fn a_too_small_own_lock_blocks_no_fund() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let small = own_open_fund(&fx, &lib);
    assert_eq!(code(quote_of(&fx, &lib, 1_000_000)), "credits.fund_pending");
    lib.tracked.lock().unwrap()[0].amount = 1_000;
    assert_eq!(
        states_of(&fx, &lib),
        [(small.txid.to_string(), UnfinishedState::TooSmall)]
    );
    quote_of(&fx, &lib, 1_000_000).unwrap();
    lib.answer(Answer::Done(None));
    let out = fund_of(&fx, &lib).unwrap();
    assert!(matches!(out.state, PlatformFundState::Applied { .. }));
    assert_eq!(lib.funds.load(Ordering::SeqCst), 2);
    assert_eq!(lib.applied(), [lib.outpoint(1)]);
}

/// A fund that gets the fund lock between another fund's lock and that
/// fund's apply (the lock dropped, the runner not in yet) finds the open
/// lock and is refused, never building a second one.
#[test]
fn a_fund_in_the_handoff_to_the_apply_refuses_the_next() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let release = Arc::new(tokio::sync::Notify::new());
    *lib.fund_gate.lock().unwrap() = Some(Arc::clone(&release));
    lib.answer(Answer::Done(None));
    let exact = exact_grant(&fx, 1_000_000);
    let (wallet, ga, gb) = (fx.wallet, grant(&fx, exact, 0), grant(&fx, exact, 0));
    let (a, b) = run(&fx, &lib, move |s, lib| async move {
        let first = fund::fund(
            &s,
            wallet,
            &*lib,
            FundAmount::Duffs { duffs: 1_000_000 },
            &ga,
        );
        let in_fund = || lib.in_fund.load(Ordering::SeqCst);
        let second = async {
            while !in_fund() {
                tokio::task::yield_now().await;
            }
            // Queued on the fund lock while the first holds it: the lock is
            // fair, so this one has it before the first's runner.
            fund::fund(
                &s,
                wallet,
                &*lib,
                FundAmount::Duffs { duffs: 1_000_000 },
                &gb,
            )
            .await
        };
        let open = async {
            while !in_fund() {
                tokio::task::yield_now().await;
            }
            for _ in 0..100 {
                tokio::task::yield_now().await;
            }
            release.notify_one();
        };
        let (a, b, ()) = tokio::join!(first, second, open);
        (a, b)
    });
    assert!(matches!(
        a.unwrap().state,
        PlatformFundState::Applied { .. }
    ));
    assert_eq!(code(b), "credits.fund_pending");
    assert_eq!(lib.funds.load(Ordering::SeqCst), 1);
}

/// The apply's hold on its address ends with the apply whatever it
/// answered: unknown, a panic, proved (after the settle), or already
/// consumed.
#[test]
fn every_apply_releases_its_hold() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let held = || platform_moves::fund_holds(&fx.s, fx.wallet);
    lib.answer(Answer::Failed);
    let out = fund_of(&fx, &lib).unwrap();
    assert_eq!(out.state, PlatformFundState::ConsumptionUnknown);
    assert!(held().is_empty(), "unknown");
    let outpoint = lib.outpoint(0);

    lib.answer(Answer::Panics);
    let (wallet, grant_id, txid) = (fx.wallet, grant(&fx, 1, 0), outpoint.txid.to_string());
    let joined = run(&fx, &lib, move |s, lib| async move {
        tokio::spawn(async move {
            fund::resume(&s, wallet, &*lib, &txid, &grant_id)
                .await
                .map(drop)
        })
        .await
    });
    assert!(joined.unwrap_err().is_panic());
    assert!(held().is_empty(), "a panic");

    lib.answer(Answer::Done(None));
    assert!(matches!(
        resume_of(&fx, &lib, outpoint).unwrap(),
        PlatformFundState::Applied { .. }
    ));
    assert!(held().is_empty(), "proved");

    lib.answer(Answer::Consumed { of: None });
    let out = fund_of(&fx, &lib).unwrap();
    assert_eq!(out.state, PlatformFundState::ConsumptionUnknown);
    assert!(held().is_empty(), "already consumed");
}

/// Two "Check again"s of one lock at once apply it once.
#[test]
fn two_resumes_at_once_apply_the_lock_once() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let outpoint = own_open_fund(&fx, &lib);
    lib.answer(Answer::Done(None));
    let (wallet, ga, gb, txid) = (
        fx.wallet,
        grant(&fx, 1, 0),
        grant(&fx, 1, 0),
        outpoint.txid.to_string(),
    );
    let (a, b) = run(&fx, &lib, move |s, lib| async move {
        tokio::join!(
            fund::resume(&s, wallet, &*lib, &txid, &ga),
            fund::resume(&s, wallet, &*lib, &txid, &gb),
        )
    });
    for r in [a, b] {
        match r {
            Ok(o) => assert!(
                matches!(o.state, PlatformFundState::Applied { .. }),
                "{o:?}"
            ),
            Err(e) => assert_eq!(e.code(), "invalid_argument", "finished meanwhile"),
        }
    }
    assert_eq!(lib.applied(), [outpoint]);
    assert_eq!(lib.funds.load(Ordering::SeqCst), 1);
}

/// A fund over `World`'s wallet (its activity read through `FakeMoves`),
/// paying the wallet's first Platform address; one read already taken.
fn fund_world() -> (World, Arc<FakeFund>) {
    let w = World::new();
    let mut lib = FakeFund::new(&w.f);
    lib.receive = w.address(0);
    w.activity();
    (w, Arc::new(lib))
}

/// The activity counts the fund once, as `credits` (`estimated` or
/// proved), no rise of its address as incoming, and then an external
/// payment to the address in full.
fn counted_once(w: &World, credits: u64, estimated: bool) {
    let done = [(PlatformActivityState::Done, credits, estimated)];
    let rows = w.activity();
    assert_eq!(fund_rows(&rows), done, "{rows:?}");
    assert!(incoming(&rows).is_empty(), "{rows:?}");
    w.net.fund(w.address(0), 7 * 100_000_000_000);
    let rows = w.activity();
    assert_eq!(fund_rows(&rows), done, "{rows:?}");
    assert_eq!(incoming(&rows), [7 * 100_000_000_000], "{rows:?}");
}

/// Restarted after the library marked the lock consumed (the apply
/// executed) and before the settle: the next read completes the fund with
/// its estimate.
#[test]
fn a_fund_consumed_before_its_settle_completes_on_the_next_read() {
    let (w, lib) = fund_world();
    let outpoint = own_open_fund(&w.f, &lib);
    w.net.fund(w.address(0), estimate());
    let mut row = lib.tracked.lock().unwrap()[0].clone();
    row.status = AssetLockStatus::Consumed;
    assert_eq!(row.out_point, outpoint);
    let (s, wallet) = (Arc::clone(&w.f.s), w.f.wallet);
    w.f.engine.block_on(track_in_wallet(&s, wallet, row));
    counted_once(&w, estimate(), true);
    assert!(lib.applied().is_empty(), "no apply again");
}

/// Reopened after the library stored the lock consumed and before the
/// settle: the reopened wallet no longer loads the lock, and its stored
/// `Consumed` row completes the fund with its estimate, never applying it
/// again.
#[test]
fn a_fund_whose_stored_lock_is_consumed_completes_after_a_reopen() {
    let (mut w, lib) = fund_world();
    let outpoint = own_checking_fund(&w.f, &lib);
    w.net.fund(w.address(0), estimate());
    let mut row = lib.tracked.lock().unwrap()[0].clone();
    row.status = AssetLockStatus::Consumed;
    let (s, wallet) = (Arc::clone(&w.f.s), w.f.wallet);
    w.f.engine.block_on(track_in_wallet(&s, wallet, row));
    let applied = lib.applied().len();
    drop((s, lib));
    w.reopen();
    let (s, wallet) = (Arc::clone(&w.f.s), w.f.wallet);
    let loaded = (w.f.engine).block_on(async move {
        let pw = s.wallet(&wallet).await.unwrap();
        (pw.asset_locks().list_tracked_locks().await)
            .iter()
            .any(|l| l.out_point == outpoint)
    });
    assert!(!loaded, "a consumed lock is not loaded again");
    let record = fund_record(&w.f, outpoint.txid).unwrap();
    assert_eq!(record.state, PlatformActivityState::Pending);
    counted_once(&w, estimate(), true);
    assert_eq!(applied, 1, "applied once, before the reopen");
}

/// Restarted after the settled record and before the activity's: the
/// next read completes the fund with its estimate.
#[test]
fn a_fund_settled_before_its_activity_record_completes_on_the_next_read() {
    let (w, lib) = fund_world();
    let outpoint = own_open_fund(&w.f, &lib);
    w.net.fund(w.address(0), estimate());
    let (s, wallet) = (Arc::clone(&w.f.s), w.f.wallet);
    w.f.engine.block_on(async move {
        locks::record(
            &s,
            wallet,
            LockKindId::PlatformAddress,
            Mark::Settled,
            outpoint,
        )
        .await
    });
    counted_once(&w, estimate(), true);
    assert!(lib.applied().is_empty(), "no apply again");
}

/// The proved apply's activity record failed to write: the next read
/// completes it from the settled record, never by applying again.
#[test]
fn a_failed_fund_record_write_completes_on_the_next_read() {
    let (w, lib) = fund_world();
    w.net.fund(w.address(0), estimate());
    w.fail_next_outcome_write();
    lib.answer(Answer::Done(Some(estimate())));
    let out = fund_of(&w.f, &lib).unwrap();
    assert_eq!(
        out.state,
        PlatformFundState::Applied {
            credits: estimate()
        }
    );
    counted_once(&w, estimate(), true);
    assert_eq!(lib.applied().len(), 1);
    assert_eq!(lib.funds.load(Ordering::SeqCst), 1);
}

/// A lost reply reads Checking; the rise while it checks is the fund's,
/// and Platform's later "already consumed" completes the fund with its
/// estimate.
#[test]
fn a_checking_fund_completes_when_platform_says_its_lock_is_used() {
    let (w, lib) = fund_world();
    lib.answer(Answer::Failed);
    let out = fund_of(&w.f, &lib).unwrap();
    assert_eq!(out.state, PlatformFundState::ConsumptionUnknown);
    w.net.fund(w.address(0), estimate());
    let rows = w.activity();
    assert!(
        !(rows.iter()).any(|r| r.kind == PlatformActivityKind::Incoming),
        "{rows:?}"
    );
    let fund = (rows.iter())
        .find(|r| r.kind == PlatformActivityKind::FromCore)
        .unwrap();
    assert_eq!(fund.state, PlatformActivityState::Pending);
    lib.proves(lib.outpoint(0), SpentRead::Consumed);
    lib.answer(Answer::Consumed { of: None });
    assert_eq!(
        resume_of(&w.f, &lib, lib.outpoint(0)).unwrap(),
        PlatformFundState::ConsumptionUnknown
    );
    counted_once(&w, estimate(), true);
}

/// An activity read that took its view of the holds before an apply holds
/// the address keeps the apply out until it is done: the apply waits for
/// it, so the read never sees the apply's rise as unheld. The fund is
/// counted once and a later payment in full.
#[test]
fn an_activity_read_in_flight_keeps_the_apply_out() {
    let (w, lib) = fund_world();
    let outpoint = own_open_fund(&w.f, &lib);
    lib.answer(Answer::Done(Some(estimate())));
    w.net.block_next_read();
    let (grant_id, txid, to) = (grant(&w.f, 1, 0), outpoint.txid.to_string(), w.address(0));
    let fake = Arc::clone(&lib);
    let (read, (resumed, applied_during)) = w
        .call(move |net, s, wallet, id| async move {
            let read = platform_moves::activity(&*net, &s, &wallet, id);
            let apply = async {
                net.read_entered.notified().await;
                let resume = fund::resume(&s, id, &*fake, &txid, &grant_id);
                tokio::pin!(resume);
                let early = tokio::time::timeout(Duration::from_secs(2), &mut resume).await;
                assert!(early.is_err(), "the resume ran past the read");
                let during = fake.applied().len();
                net.read_release.notify_one();
                let resumed = resume.await;
                net.fund(to, estimate());
                (resumed, during)
            };
            let (read, apply) = tokio::join!(read, apply);
            Ok((read, apply))
        })
        .unwrap();
    read.unwrap();
    assert_eq!(applied_during, 0, "the apply waited for the read");
    assert!(matches!(
        resumed.unwrap().state,
        PlatformFundState::Applied { .. }
    ));
    counted_once(&w, estimate(), false);
}

/// The list joins the kinds read and names the ones that were not; only
/// with none read is it refused.
#[test]
fn a_kind_that_cannot_be_read_does_not_hide_the_others() {
    let lock = |kind, n: u8| UnfinishedLock {
        txid: format!("{n:02x}").repeat(32),
        vout: 0,
        kind,
        duffs: 1_000_000,
        origin: LockOrigin::Restored,
        state: UnfinishedState::Ready,
        remaining_credits: None,
    };
    let fund = lock(LockKindId::PlatformAddress, 2);
    let shield = lock(LockKindId::Shielded, 1);
    let read = super::unfinished::transfers([
        (LockKindId::Shielded, Err(PlatformError::Unavailable)),
        (LockKindId::PlatformAddress, Ok((vec![fund.clone()], true))),
    ])
    .unwrap();
    assert_eq!(
        (read.locks, read.complete, read.unread),
        (vec![fund.clone()], true, vec![LockKindId::Shielded])
    );
    let both = super::unfinished::transfers([
        (LockKindId::Shielded, Ok((vec![shield.clone()], false))),
        (LockKindId::PlatformAddress, Ok((vec![fund.clone()], true))),
    ])
    .unwrap();
    assert_eq!(
        (both.locks, both.complete, both.unread),
        (vec![shield, fund], false, vec![])
    );
    let none = super::unfinished::transfers([
        (LockKindId::Shielded, Err(PlatformError::WalletNotFound)),
        (LockKindId::PlatformAddress, Err(PlatformError::Unavailable)),
    ]);
    assert_eq!(none.unwrap_err().code(), "wallet_not_found");
}

/// "Check again" needs a grant with a funding budget (the lock's key is a
/// funding key): one without is refused before it is redeemed, and stays
/// usable.
#[test]
fn a_resume_refuses_a_grant_with_no_funding_budget_unredeemed() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let outpoint = own_open_fund(&fx, &lib);
    let (wallet, empty, txid) = (fx.wallet, grant(&fx, 0, 0), outpoint.txid.to_string());
    let held = empty.clone();
    let refused = run(&fx, &lib, move |s, lib| async move {
        fund::resume(&s, wallet, &*lib, &txid, &held).await
    });
    assert_eq!(code(refused), "platform.needs_grant");
    assert!(fx.unused(&empty));
    assert!(lib.applied().is_empty());
}

/// SPV's reading caught up, as the hub records it.
fn caught_up(fx: &Fixture) {
    use dash_spv::sync::{BlockHeadersProgress, SyncProgress, SyncState};
    let mut headers = BlockHeadersProgress::default();
    headers.update_target_height(1000);
    headers.update_tip_height(1000);
    headers.set_state(SyncState::Synced);
    let mut progress = SyncProgress::default();
    progress.update_headers(headers);
    let mut tracker = fx.s.hub.tracker.lock().unwrap();
    tracker.set_running(true);
    tracker.on_progress(&progress);
    assert!(tracker.caught_up());
}

fn fund_rows(rows: &[PlatformActivity]) -> Vec<(PlatformActivityState, u64, bool)> {
    (rows.iter())
        .filter(|r| r.kind == PlatformActivityKind::FromCore)
        .map(|r| (r.state, r.credits, r.estimated))
        .collect()
}

fn incoming(rows: &[PlatformActivity]) -> Vec<u64> {
    (rows.iter())
        .filter(|r| r.kind == PlatformActivityKind::Incoming)
        .map(|r| r.credits)
        .collect()
}

fn record_mark(fx: &Fixture, mark: Mark, outpoint: OutPoint) {
    let (s, wallet) = (Arc::clone(&fx.s), fx.wallet);
    fx.engine.block_on(async move {
        locks::record(&s, wallet, LockKindId::PlatformAddress, mark, outpoint).await;
    });
}

fn fund_record(fx: &Fixture, txid: Txid) -> Option<platform_moves::FundRecord> {
    let (s, wallet) = (Arc::clone(&fx.s), fx.wallet);
    (fx.engine)
        .block_on(async move { platform_moves::fund_record(&s, wallet, &txid.to_string()).await })
        .unwrap()
}

/// An own fund whose apply released a signature and failed: Checking, its
/// record armed.
fn own_checking_fund(fx: &Fixture, lib: &Arc<FakeFund>) -> OutPoint {
    let outpoint = own_open_fund(fx, lib);
    lib.answer(Answer::Failed);
    assert_eq!(
        resume_of(fx, lib, outpoint).unwrap(),
        PlatformFundState::ConsumptionUnknown
    );
    assert!(fund_record(fx, outpoint.txid).unwrap().armed);
    outpoint
}

/// Finish of a restored lock Platform answers is already used (applied
/// from another install), signed or not, single or by Finish transfers:
/// no activity row and no allowance; the lock leaves the recovery (and is
/// on no card), and a later payment to the address is incoming in full.
#[test]
fn a_restored_lock_used_elsewhere_leaves_no_activity_and_no_allowance() {
    for (bulk, signs) in [(false, true), (true, true), (false, false), (true, false)] {
        let (w, lib) = fund_world();
        lib.track(lib.lock(1, 1_000_000, AssetLockStatus::RecoveredFromChain, true));
        let at = lib.outpoint(0);
        let answer = Answer::Consumed { of: None };
        if signs {
            lib.proves(at, SpentRead::Consumed);
            lib.answer(answer);
        } else {
            lib.refuse(answer);
        }
        if bulk {
            let report = finish_transfers(&w.f, &lib);
            assert_eq!(report.already_used, 1, "{report:?}");
            assert_eq!(report.completed, 0, "{report:?}");
        } else {
            assert_eq!(finish(&w.f, &lib, at).0.unwrap(), LockFinish::ReportedUsed);
        }
        assert_eq!(lib.applied(), [at]);
        assert_eq!(
            *lib.signed.lock().unwrap(),
            [signs].repeat(usize::from(signs))
        );
        assert!(state_of(&w.f, &lib).is_empty(), "bulk {bulk}");
        assert!(pending_of(&w.f, &lib).is_empty(), "bulk {bulk}");
        assert!(fund_record(&w.f, at.txid).is_none(), "bulk {bulk}");
        let rows = w.activity();
        assert!(fund_rows(&rows).is_empty(), "{rows:?}");
        w.net.fund(w.address(0), 7 * 100_000_000_000);
        assert_eq!(incoming(&w.activity()), [7 * 100_000_000_000]);
        assert!(!settled(&w.f).contains(&at.txid));
    }
}

/// The same restored lock applied here, proved: its fund is recorded once
/// with what the proof showed added.
#[test]
fn a_restored_lock_applied_here_records_its_proved_done() {
    let (w, lib) = fund_world();
    lib.track(lib.lock(1, 1_000_000, AssetLockStatus::RecoveredFromChain, true));
    let at = lib.outpoint(0);
    w.net.fund(w.address(0), estimate());
    lib.answer(Answer::Done(Some(estimate())));
    assert_eq!(finish(&w.f, &lib, at).0.unwrap(), LockFinish::Applied);
    counted_once(&w, estimate(), false);
}

/// A fund waiting for its proof, or refused before any signature, is not
/// armed: a payment to its address meanwhile is incoming in full.
#[test]
fn a_fund_that_never_signed_absorbs_no_payment() {
    let (w, lib) = fund_world();
    *lib.funding.lock().unwrap() = Script::Unproved;
    let out = fund_of(&w.f, &lib).unwrap();
    assert_eq!(out.state, PlatformFundState::ProofWaiting);
    w.net.fund(w.address(0), estimate());
    let rows = w.activity();
    assert_eq!(incoming(&rows), [estimate()], "{rows:?}");
    assert_eq!(
        fund_rows(&rows),
        [(PlatformActivityState::Pending, estimate(), false)]
    );

    let outpoint = lib.outpoint(0);
    lib.prove(outpoint);
    lib.refuse(Answer::Failed);
    assert!(matches!(
        resume_of(&w.f, &lib, outpoint).unwrap(),
        PlatformFundState::NotApplied { .. }
    ));
    assert!(!fund_record(&w.f, outpoint.txid).unwrap().armed);
    w.net.fund(w.address(0), 3 * 100_000_000_000);
    let rows = w.activity();
    assert_eq!(
        incoming(&rows),
        [3 * 100_000_000_000, estimate()],
        "{rows:?}"
    );
}

/// A lock swept by a conflicting transaction leaves the library's tracked
/// set (platform-wallet `sync/reconstruction.rs`); once SPV caught up, its
/// fund that never signed closes (`failed`). Still tracked, or not caught
/// up, it pends.
#[test]
fn a_swept_lock_closes_its_unsigned_fund() {
    let (w, lib) = fund_world();
    *lib.funding.lock().unwrap() = Script::Unproved;
    fund_of(&w.f, &lib).unwrap();
    let row = lib.tracked.lock().unwrap()[0].clone();
    let outpoint = row.out_point;
    let pending = [(PlatformActivityState::Pending, estimate(), false)];
    assert_eq!(fund_rows(&w.activity()), pending, "not caught up");

    let (s, wallet) = (Arc::clone(&w.f.s), w.f.wallet);
    w.f.engine.block_on(track_in_wallet(&s, wallet, row));
    caught_up(&w.f);
    assert_eq!(fund_rows(&w.activity()), pending, "tracked");

    w.f.engine.block_on(async move {
        let w = s.wallet(&wallet).await.unwrap();
        w.state_mut().await.tracked_asset_locks.remove(&outpoint);
    });
    let rows = w.activity();
    assert_eq!(
        fund_rows(&rows),
        [(PlatformActivityState::Failed, estimate(), false)],
        "{rows:?}"
    );
    w.net.fund(w.address(0), estimate());
    assert_eq!(incoming(&w.activity()), [estimate()]);
}

/// Platform's "already consumed" is kept for the outpoint before the
/// fund's record is written: with that write failed, a read after a
/// restart completes the armed fund from it, never applying again.
#[test]
fn a_reported_fund_whose_record_write_failed_completes_after_a_restart() {
    let (mut w, lib) = fund_world();
    let outpoint = own_checking_fund(&w.f, &lib);
    w.net.fund(w.address(0), estimate());
    w.fail_next_outcome_write();
    lib.proves(outpoint, SpentRead::Consumed);
    lib.answer(Answer::Consumed { of: None });
    assert_eq!(
        resume_of(&w.f, &lib, outpoint).unwrap(),
        PlatformFundState::ConsumptionUnknown
    );
    let record = fund_record(&w.f, outpoint.txid).unwrap();
    assert_eq!(
        record.state,
        PlatformActivityState::Pending,
        "the write failed"
    );
    assert_eq!(lib.applied(), [outpoint; 2]);
    drop(lib);
    w.reopen();
    counted_once(&w, estimate(), true);
}

/// Evidence is by the fund's own output: a sibling output of its
/// transaction settled or reported used leaves the fund pending; its own
/// output settled completes it.
#[test]
fn a_fund_ends_on_its_own_outputs_evidence_only() {
    let (w, lib) = fund_world();
    let outpoint = own_checking_fund(&w.f, &lib);
    w.net.fund(w.address(0), estimate());
    let sibling = OutPoint::new(outpoint.txid, outpoint.vout + 1);
    record_mark(&w.f, Mark::Settled, sibling);
    record_mark(&w.f, Mark::Used, sibling);
    let rows = w.activity();
    assert_eq!(
        fund_rows(&rows),
        [(PlatformActivityState::Pending, estimate(), false)],
        "{rows:?}"
    );
    assert!(incoming(&rows).is_empty(), "{rows:?}");
    record_mark(&w.f, Mark::Settled, outpoint);
    counted_once(&w, estimate(), true);
}

/// A read while the fund's apply still holds its address (its settle not
/// run yet) leaves the fund to that settle, whatever the lock's evidence
/// says; the first read after completes it.
#[test]
fn a_read_leaves_a_fund_whose_apply_holds_its_address_to_its_settle() {
    let (w, lib) = fund_world();
    let outpoint = own_checking_fund(&w.f, &lib);
    w.net.fund(w.address(0), estimate());
    let address = fund_record(&w.f, outpoint.txid).unwrap().address;
    let (s, wallet) = (Arc::clone(&w.f.s), w.f.wallet);
    let hold = (w.f.engine).block_on(platform_moves::hold_fund(&s, wallet, address));
    record_mark(&w.f, Mark::Settled, outpoint);
    let rows = w.activity();
    assert_eq!(
        fund_rows(&rows),
        [(PlatformActivityState::Pending, estimate(), false)],
        "{rows:?}"
    );
    drop(hold);
    counted_once(&w, estimate(), true);
}

/// The fund completed with its estimate takes what of the address's rise
/// it absorbed: a rise below the estimate leaves the rest as an allowance
/// a later payment fills first (at most the estimate's excess over the
/// rise); a rise above it, up to the lock's credits, is all the fund's.
#[test]
fn a_completed_funds_rise_below_or_above_its_estimate() {
    const SHORT: u64 = 1_000;
    for rise in [estimate() - SHORT, estimate() + SHORT] {
        let (w, lib) = fund_world();
        let outpoint = own_checking_fund(&w.f, &lib);
        w.net.fund(w.address(0), rise);
        let rows = w.activity();
        assert!(incoming(&rows).is_empty(), "{rows:?}");
        record_mark(&w.f, Mark::Settled, outpoint);
        let rows = w.activity();
        assert_eq!(
            fund_rows(&rows),
            [(PlatformActivityState::Done, estimate(), true)],
            "{rows:?}"
        );
        w.net.fund(w.address(0), 7 * 100_000_000_000);
        let left = estimate().saturating_sub(rise);
        assert_eq!(
            incoming(&w.activity()),
            [7 * 100_000_000_000 - left],
            "rise {rise}"
        );
    }
}

/// How a later apply gets no signature: it asks for another credit key
/// (refused by the lock-key signer), or for the lock's own key with the
/// vault locked meanwhile (refused by the vault).
#[derive(Debug, Clone, Copy)]
enum NoSignature {
    OtherKey,
    VaultLocked,
}

impl NoSignature {
    fn set(self, lib: &FakeFund) {
        match self {
            Self::OtherKey => {
                *lib.key.lock().unwrap() = DerivationPath::from_str("m/9'/1'/5'/5'/0").unwrap();
            }
            Self::VaultLocked => lib.lock_vault_in_apply.store(true, Ordering::SeqCst),
        }
    }
}

/// A later apply that asks for a signature and gets none leaves the
/// arming an earlier signed apply wrote: that signature may still
/// execute, so the fund still absorbs the rise of its address.
#[test]
fn an_apply_that_signs_nothing_keeps_an_earlier_signatures_arming() {
    for how in [NoSignature::OtherKey, NoSignature::VaultLocked] {
        let (w, lib) = fund_world();
        let outpoint = own_checking_fund(&w.f, &lib);
        how.set(&lib);
        lib.answer(Answer::Failed);
        assert!(
            matches!(
                resume_of(&w.f, &lib, outpoint).unwrap(),
                PlatformFundState::NotApplied { .. }
            ),
            "{how:?}"
        );
        assert_eq!(lib.signed.lock().unwrap().last(), Some(&false), "{how:?}");
        assert!(fund_record(&w.f, outpoint.txid).unwrap().armed, "{how:?}");
        w.net.fund(w.address(0), estimate());
        let rows = w.activity();
        assert!(incoming(&rows).is_empty(), "{how:?}: {rows:?}");
        assert_eq!(
            fund_rows(&rows),
            [(PlatformActivityState::Pending, estimate(), false)]
        );
    }
}

/// A lock Platform reported used is finished elsewhere for every later
/// Finish too: never `Applied`, and never applied again.
#[test]
fn a_reported_lock_finishes_as_reported_every_time() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    lib.track(lib.lock(1, 1_000_000, AssetLockStatus::RecoveredFromChain, true));
    let at = lib.outpoint(0);
    lib.proves(at, SpentRead::Consumed);
    lib.answer(Answer::Consumed { of: None });
    assert_eq!(finish(&fx, &lib, at).0.unwrap(), LockFinish::ReportedUsed);
    assert_eq!(finish(&fx, &lib, at).0.unwrap(), LockFinish::ReportedUsed);
    assert_eq!(lib.applied(), [at]);
}

/// What a restored lock's armed record absorbed of a payment is the
/// payment's again when the record is removed on Platform's "already
/// consumed": incoming in full, nothing lost.
#[test]
fn a_removed_restored_record_gives_back_what_it_absorbed() {
    let (w, lib) = fund_world();
    lib.track(lib.lock(1, 1_000_000, AssetLockStatus::RecoveredFromChain, true));
    let at = lib.outpoint(0);
    lib.answer(Answer::Failed);
    assert_eq!(finish(&w.f, &lib, at).0.unwrap(), LockFinish::Checking);
    assert!(fund_record(&w.f, at.txid).unwrap().armed);
    let paid = 3 * 100_000_000_000;
    w.net.fund(w.address(0), paid);
    let rows = w.activity();
    assert_eq!(
        incoming(&rows).iter().sum::<u64>(),
        paid - 1_000_000_000,
        "{rows:?}"
    );
    lib.proves(at, SpentRead::Consumed);
    lib.answer(Answer::Consumed { of: None });
    assert_eq!(finish(&w.f, &lib, at).0.unwrap(), LockFinish::ReportedUsed);
    let rows = w.activity();
    assert!(fund_rows(&rows).is_empty(), "{rows:?}");
    assert_eq!(incoming(&rows).iter().sum::<u64>(), paid, "{rows:?}");
}

/// The journal's settled record of a lock alone in its transaction is
/// the fund's evidence too (its outpoint mark unwritten).
#[test]
fn a_fund_settled_by_txid_alone_completes_on_the_next_read() {
    let (w, lib) = fund_world();
    let outpoint = own_checking_fund(&w.f, &lib);
    w.net.fund(w.address(0), estimate());
    let row = lib.tracked.lock().unwrap()[0].clone();
    let (s, wallet) = (Arc::clone(&w.f.s), w.f.wallet);
    w.f.engine.block_on(track_in_wallet(&s, wallet, row));
    let rows = w.activity();
    assert_eq!(
        fund_rows(&rows),
        [(PlatformActivityState::Pending, estimate(), false)],
        "tracked, not settled: {rows:?}"
    );
    (w.f.engine)
        .block_on(async move {
            (s.leases)
                .settle_top_up(wallet, outpoint.txid, TopUpSettled::Applied)
                .await
        })
        .unwrap();
    counted_once(&w, estimate(), true);
}

/// The journal's settled record by txid is no evidence for a lock whose
/// transaction has another credit output: the fund stays pending until its
/// own output is settled.
#[test]
fn a_txid_settled_record_leaves_a_fund_sharing_its_transaction_pending() {
    let (w, lib) = fund_world();
    let tx = conformance::transaction(1, 2, 1_000_000, &lib.script);
    let row = conformance::tracked(
        &tx,
        0,
        AssetLockFundingType::AssetLockAddressTopUp,
        AssetLockStatus::RecoveredFromChain,
        true,
    );
    let at = row.out_point;
    lib.track(row.clone());
    lib.answer(Answer::Failed);
    assert_eq!(finish(&w.f, &lib, at).0.unwrap(), LockFinish::Checking);
    assert!(fund_record(&w.f, at.txid).unwrap().armed);
    w.net.fund(w.address(0), estimate());
    let (s, wallet) = (Arc::clone(&w.f.s), w.f.wallet);
    w.f.engine.block_on(track_in_wallet(&s, wallet, row));
    (w.f.engine)
        .block_on(async move {
            (s.leases)
                .settle_top_up(wallet, at.txid, TopUpSettled::Applied)
                .await
        })
        .unwrap();
    let rows = w.activity();
    assert_eq!(
        fund_rows(&rows),
        [(PlatformActivityState::Pending, estimate(), false)],
        "{rows:?}"
    );
    assert!(incoming(&rows).is_empty(), "{rows:?}");
    record_mark(&w.f, Mark::Settled, at);
    counted_once(&w, estimate(), true);
}

/// A fund closed as its lock left the library is armed again (pending)
/// by a later apply that signs: the lock came back, and that signature
/// may execute.
#[test]
fn a_closed_fund_is_armed_again_by_a_later_signature() {
    let (w, lib) = fund_world();
    *lib.funding.lock().unwrap() = Script::Unproved;
    fund_of(&w.f, &lib).unwrap();
    let row = lib.tracked.lock().unwrap()[0].clone();
    let outpoint = row.out_point;
    let (s, wallet) = (Arc::clone(&w.f.s), w.f.wallet);
    caught_up(&w.f);
    let failed = [(PlatformActivityState::Failed, estimate(), false)];
    assert_eq!(fund_rows(&w.activity()), failed, "untracked");
    w.f.engine.block_on(track_in_wallet(&s, wallet, row));
    lib.prove(outpoint);
    lib.answer(Answer::Failed);
    assert_eq!(
        resume_of(&w.f, &lib, outpoint).unwrap(),
        PlatformFundState::ConsumptionUnknown
    );
    assert!(fund_record(&w.f, outpoint.txid).unwrap().armed);
    w.net.fund(w.address(0), estimate());
    let rows = w.activity();
    assert!(incoming(&rows).is_empty(), "{rows:?}");
    assert_eq!(
        fund_rows(&rows),
        [(PlatformActivityState::Pending, estimate(), false)]
    );
}

/// Platform's "already consumed" after an apply asked for a signature and
/// got none: the fund never signed, so it closes rather than completing.
#[test]
fn already_consumed_with_no_signature_closes_the_fund() {
    for how in [NoSignature::OtherKey, NoSignature::VaultLocked] {
        let (w, lib) = fund_world();
        let outpoint = own_open_fund(&w.f, &lib);
        how.set(&lib);
        lib.answer(Answer::Consumed { of: None });
        assert_eq!(
            resume_of(&w.f, &lib, outpoint).unwrap(),
            PlatformFundState::ConsumptionUnknown,
            "{how:?}"
        );
        assert_eq!(lib.signed.lock().unwrap().last(), Some(&false), "{how:?}");
        assert_eq!(
            fund_rows(&w.activity()),
            [(PlatformActivityState::Failed, estimate(), false)],
            "{how:?}"
        );
    }
}

/// Arming says what it did, and undoing touches only that: a second
/// arming changes nothing and undoing it leaves the first's; a restored
/// lock's record is made, and undoing that removes it.
#[test]
fn arming_names_what_it_did_and_undoing_touches_only_that() {
    use platform_moves::{Arming, arm_fund, disarm_fund};
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let own = own_open_fund(&fx, &lib);
    lib.track(lib.lock(1, 1_000_000, AssetLockStatus::RecoveredFromChain, true));
    let restored = lib.outpoint(1);
    let wallet = fx.wallet;
    let arm = |at: OutPoint| {
        let s = Arc::clone(&fx.s);
        (fx.engine)
            .block_on(
                async move { arm_fund(&s, wallet, (at, 1_000_000), "a".into(), 1, false).await },
            )
            .unwrap()
    };
    let disarm = |at: OutPoint, arming| {
        let s = Arc::clone(&fx.s);
        (fx.engine)
            .block_on(async move { disarm_fund(&s, wallet, &at.txid.to_string(), arming).await })
            .unwrap();
    };
    assert_eq!(arm(own), Arming::Armed);
    assert_eq!(arm(own), Arming::Unchanged);
    disarm(own, Arming::Unchanged);
    assert!(fund_record(&fx, own.txid).unwrap().armed);
    disarm(own, Arming::Armed);
    assert!(!fund_record(&fx, own.txid).unwrap().armed);
    assert_eq!(arm(restored), Arming::Made);
    assert!(fund_record(&fx, restored.txid).unwrap().restored);
    disarm(restored, Arming::Made);
    assert!(fund_record(&fx, restored.txid).is_none());
}

/// 0.2 fund needs_grant: the funding call charges its whole quote, so a
/// grant of exactly the quote leaves no Funding budget, and the apply of
/// the lock that call made still runs, on the signer taken before it. One
/// Core lock, applied once, nothing pending.
#[test]
fn a_fund_under_exactly_its_quote_grant_applies_its_lock() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let quote = quote_of(&fx, &lib, 1_000_000).unwrap();
    assert_eq!(quote.grant.max_duffs, exact_grant(&fx, 1_000_000));
    lib.answer(Answer::Done(Some(944_000_000)));
    let (wallet, grant) = (fx.wallet, grant(&fx, quote.grant.max_duffs, 0));
    let out = run(&fx, &lib, move |s, lib| async move {
        fund::fund(
            &s,
            wallet,
            &*lib,
            FundAmount::Duffs { duffs: 1_000_000 },
            &grant,
        )
        .await
    })
    .unwrap();
    assert_eq!(
        out.state,
        PlatformFundState::Applied {
            credits: 944_000_000
        }
    );
    let outpoint = lib.outpoint(0);
    assert_eq!(lib.funds.load(Ordering::SeqCst), 1, "one Core lock");
    assert_eq!(lib.applied(), [outpoint], "applied once");
    assert_eq!(*lib.signed.lock().unwrap(), [true]);
    assert!(settled(&fx).contains(&outpoint.txid));
    assert!(pending_of(&fx, &lib).is_empty());
}

/// Applies the proved fund lock `outpoint` on a lease whose funding call
/// first spent its whole Funding budget, holding the signer that lease
/// issued before that call (`keep_signer`) or asking the lease for it.
fn apply_after_spent_funding(
    fx: &Fixture,
    lib: &Arc<FakeFund>,
    outpoint: OutPoint,
    keep_signer: bool,
) -> Result<LockFinish, PlatformError> {
    let (wallet, grant) = (fx.wallet, grant(fx, exact_grant(fx, 1_000_000), 0));
    run(fx, lib, move |s, lib| async move {
        let lease = (s.begin_lease(wallet, FlowKind::PlatformAddressFund, &[grant])).await?;
        let held = locks::FundedLease::before_funding(lease.clone()).unwrap();
        let key = FundingKey {
            funding_type: AssetLockFundingType::AssetLockAddressTopUp,
            identity_index: 0,
            account: 0,
        };
        let step = format!("paddr/spent-{keep_signer}/funding");
        let permit = (lib.funding_permit(&lease, step, key, 1_000_000))
            .await
            .unwrap();
        permit.finish(true);
        assert_eq!(lease.available(BudgetPurpose::Funding).unwrap(), 0);
        let kind = FundLocks::new(&s, wallet, &*lib);
        match keep_signer {
            true => locks::finish(&kind, outpoint, || async { Ok(held) }).await,
            false => locks::finish(&kind, outpoint, || async { Ok(lease) }).await,
        }
    })
}

/// A proved fund lock not applied yet, its funding call's lease gone.
fn proved_unapplied_fund(fx: &Fixture, lib: &Arc<FakeFund>) -> OutPoint {
    *lib.funding.lock().unwrap() = Script::Unproved;
    fund_of(fx, lib).unwrap();
    let outpoint = lib.outpoint(0);
    lib.prove(outpoint);
    lib.answer(Answer::Done(None));
    outpoint
}

/// The kept signer is the fund's own call only: once a funding call spent
/// a lease's whole Funding budget, an apply that asks the lease for its
/// signer (as a resume, Finish and the recovery do) is still refused
/// `needs_grant`, while one holding the signer the lease issued before
/// that call applies the lock.
#[test]
fn a_spent_funding_budget_refuses_an_apply_that_did_not_take_its_signer_first() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let outpoint = proved_unapplied_fund(&fx, &lib);
    assert_eq!(
        apply_after_spent_funding(&fx, &lib, outpoint, false).unwrap_err(),
        PlatformError::from(super::lease::LeaseError::NeedsGrant(BudgetPurpose::Funding))
    );
    assert!(lib.applied().is_empty(), "nothing applied on a spent lease");
    assert_eq!(
        apply_after_spent_funding(&fx, &lib, outpoint, true).unwrap(),
        LockFinish::Applied
    );
    assert_eq!(lib.applied(), [outpoint]);
    assert_eq!(lib.funds.load(Ordering::SeqCst), 1, "no new lock");
}

/// The lock an apply refused `needs_grant` leaves behind (as a fund did
/// before its signer was kept) is listed proved and blocks a new fund; its
/// Finish, a resume under a one-duff grant, applies that same lock.
#[test]
fn a_lock_a_refused_apply_left_finishes_under_one_duff_with_no_new_lock() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let outpoint = proved_unapplied_fund(&fx, &lib);
    apply_after_spent_funding(&fx, &lib, outpoint, false).unwrap_err();
    let txid = outpoint.txid.to_string();
    assert_eq!(pending_of(&fx, &lib), [(txid.clone(), true, false)]);
    assert_eq!(
        quote_of(&fx, &lib, 1_000_000).unwrap_err(),
        CreditsError::FundPending { txid }
    );
    lib.answer(Answer::Done(Some(944_000_000)));
    assert_eq!(
        resume_of(&fx, &lib, outpoint).unwrap(),
        PlatformFundState::Applied {
            credits: 944_000_000
        }
    );
    assert_eq!(lib.applied(), [outpoint]);
    assert_eq!(lib.funds.load(Ordering::SeqCst), 1, "no second Core lock");
    assert!(pending_of(&fx, &lib).is_empty());
}

/// A grant funds one lock: the exact grant a fund redeemed is refused
/// `grant_invalid` for a second (a redeemed grant begins no other lease),
/// whatever the first fund's apply did.
#[test]
fn an_exact_grant_funds_one_lock() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    lib.answer(Answer::Done(None));
    let (wallet, grant) = (fx.wallet, grant(&fx, exact_grant(&fx, 1_000_000), 0));
    let again = grant.clone();
    run(&fx, &lib, move |s, lib| async move {
        fund::fund(
            &s,
            wallet,
            &*lib,
            FundAmount::Duffs { duffs: 1_000_000 },
            &grant,
        )
        .await
    })
    .unwrap();
    let second = run(&fx, &lib, move |s, lib| async move {
        fund::fund(
            &s,
            wallet,
            &*lib,
            FundAmount::Duffs { duffs: 1_000_000 },
            &again,
        )
        .await
    });
    assert_eq!(code(second), "platform.grant_invalid");
    assert_eq!(lib.funds.load(Ordering::SeqCst), 1, "one Core lock");
}

/// The recovery's Finish (`finish_transfer`) of the lock a refused apply
/// left applies that same lock under a one-duff grant, with no new lock.
#[test]
fn the_recovery_finish_applies_the_lock_a_refused_apply_left() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let outpoint = proved_unapplied_fund(&fx, &lib);
    apply_after_spent_funding(&fx, &lib, outpoint, false).unwrap_err();
    lib.answer(Answer::Done(Some(944_000_000)));
    let (wallet, grant) = (fx.wallet, grant(&fx, 1, 0));
    let finished = run(&fx, &lib, move |s, lib| async move {
        unfinished::finish_fund(&s, wallet, &*lib, outpoint, &grant).await
    });
    assert_eq!(finished.unwrap(), LockFinish::Applied);
    assert_eq!(lib.applied(), [outpoint]);
    assert_eq!(lib.funds.load(Ordering::SeqCst), 1, "no new lock");
    assert!(pending_of(&fx, &lib).is_empty());
}

/// An exact-grant fund's lock not applied before a restart is listed
/// again after it, and a resume under a fresh lease applies that lock,
/// with no new one.
#[test]
fn an_unapplied_exact_grant_lock_finishes_on_a_fresh_lease_after_a_reopen() {
    let (mut w, lib) = fund_world();
    let outpoint = own_open_fund(&w.f, &lib);
    let row = (lib.tracked.lock().unwrap().iter())
        .find(|l| l.out_point == outpoint)
        .cloned()
        .unwrap();
    assert_eq!(lib.funds.load(Ordering::SeqCst), 1);
    drop(lib);
    w.reopen();
    let mut again = FakeFund::new(&w.f);
    again.receive = w.address(0);
    let again = Arc::new(again);
    again.track(row);
    let txid = outpoint.txid.to_string();
    assert_eq!(pending_of(&w.f, &again), [(txid, true, false)]);
    again.answer(Answer::Done(Some(944_000_000)));
    assert_eq!(
        resume_of(&w.f, &again, outpoint).unwrap(),
        PlatformFundState::Applied {
            credits: 944_000_000
        }
    );
    assert_eq!(again.applied(), [outpoint]);
    assert_eq!(again.funds.load(Ordering::SeqCst), 0, "no new lock");
    assert!(pending_of(&w.f, &again).is_empty());
}

/// The fund's lease revoked between its funding call and its apply: the
/// kept signer applies nothing (the apply's call permit is refused), the
/// fund ends `NotApplied`, the lock stays pending, and a later Finish
/// applies it once. A permit asked while the revoking call still holds the
/// barrier is refused as `platform.cancelled`; once the barrier is clear,
/// as `platform.lease_revoked`. The fund's copy reads both as the approval
/// ending with the lock pending.
#[test]
fn a_lease_revoked_between_the_funding_and_the_apply_leaves_the_lock_for_finish() {
    for (during_the_revocation, error) in [
        (false, "platform.lease_revoked"),
        (true, "platform.cancelled"),
    ] {
        let (fx, lib, out) = revoked_before_the_apply(during_the_revocation);
        assert_eq!(
            out.state,
            PlatformFundState::NotApplied {
                error: error.into()
            },
            "{error}"
        );
        let outpoint = lib.outpoint(0);
        assert!(
            lib.applied().is_empty(),
            "nothing applied on the revoked lease"
        );
        assert!(lib.signed.lock().unwrap().is_empty());
        assert_eq!(
            pending_of(&fx, &lib),
            [(outpoint.txid.to_string(), true, false)]
        );
        assert_eq!(
            resume_of(&fx, &lib, outpoint).unwrap(),
            PlatformFundState::Applied {
                credits: 944_000_000
            }
        );
        assert_eq!(lib.applied(), [outpoint], "applied once");
        assert_eq!(*lib.signed.lock().unwrap(), [true], "Finish signed it");
        assert_eq!(lib.funds.load(Ordering::SeqCst), 1, "no new lock");
    }
}

/// Only guards against a hang: a correct run ends in well under a second.
const HANDSHAKE_GUARD: Duration = Duration::from_secs(300);

/// A fund whose lease a passphrase change revokes once its funding call is
/// over and before its apply's call permit: with the apply asked while the
/// revoking call is still running (`during_the_revocation`), or after it.
fn revoked_before_the_apply(
    during_the_revocation: bool,
) -> (Fixture, Arc<FakeFund>, PlatformFundOutcome) {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let (waiting, release) = (Arc::default(), Arc::default());
    *lib.apply_gate.lock().unwrap() = Some((Arc::clone(&waiting), Arc::clone(&release)));
    lib.answer(Answer::Done(Some(944_000_000)));
    let (wallet, grant) = (fx.wallet, grant(&fx, exact_grant(&fx, 1_000_000), 0));
    let out = run(&fx, &lib, move |s, lib| async move {
        let (in_gate, gate_open) = (tokio::sync::oneshot::channel(), std::sync::mpsc::channel());
        let (in_gate_tx, in_gate_rx) = in_gate;
        let (open_tx, open_rx) = gate_open;
        let fund = async {
            let amount = FundAmount::Duffs { duffs: 1_000_000 };
            let out = fund::fund(&s, wallet, &*lib, amount, &grant).await;
            let _ = open_tx.send(());
            out
        };
        tokio::pin!(fund);
        let handshake = async {
            tokio::select! {
                early = &mut fund => panic!("the fund ended before its apply: {early:?}"),
                () = waiting.notified() => {}
            }
            let revoking = s.revoking_vault_op(RevokeCause::PassphraseChange, move |_| {
                if during_the_revocation {
                    in_gate_tx.send(()).unwrap();
                    open_rx.recv_timeout(HANDSHAKE_GUARD).unwrap();
                }
                Ok(())
            });
            let revoke = async {
                if during_the_revocation {
                    let apply = async {
                        in_gate_rx.await.unwrap();
                        release.notify_one();
                    };
                    let (revoked, ()) = tokio::join!(revoking, apply);
                    revoked.unwrap();
                } else {
                    revoking.await.unwrap();
                    release.notify_one();
                }
            };
            let (out, ()) = tokio::join!(&mut fund, revoke);
            out
        };
        tokio::time::timeout(HANDSHAKE_GUARD, handshake)
            .await
            .expect("hang guard: the fund's apply checkpoint, the revocation and the fund's end")
    })
    .unwrap();
    (fx, lib, out)
}

/// Closes the fixture's session and opens it again over the same
/// `app.sqlite`, `wallet.sqlite` and journal (a restart).
fn reopen(fx: &mut Fixture) {
    (fx.engine)
        .block_on(fx.engine.close_network(crate::DashNetwork::Regtest))
        .unwrap();
    let options = crate::SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        no_platform: true,
        ..Default::default()
    };
    fx.s = (fx.engine)
        .block_on(fx.engine.open_network(crate::DashNetwork::Regtest, options))
        .unwrap();
    assert!(fx.engine.block_on(fx.s.leases.refresh_funding(fx.wallet)));
}

/// F2 on type 4: a restored lock, `RecoveredFromChain` with its proof,
/// that Platform answers "already consumed" for, the library unable to
/// record the answer at the real persister, and Platform's proved state
/// says consumed: reported used, off the list and the card, settled
/// nowhere, and so after a restart, with no second apply.
#[test]
fn a_restored_lock_platform_reports_used_leaves_the_card_across_a_restart() {
    let mut fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let row = lib.lock(9, 1_000_000, AssetLockStatus::RecoveredFromChain, true);
    let at = row.out_point;
    lib.track(row.clone());
    lib.answer(Answer::Unrecorded);
    lib.proves(at, SpentRead::Consumed);
    let (finished, asked) = finish(&fx, &lib, at);
    assert!(asked);
    assert_eq!(finished.unwrap(), LockFinish::ReportedUsed);
    assert_eq!(reported(&fx, &lib), HashSet::from([at]));
    assert!(listed(&fx, &lib).unwrap().is_empty(), "off the card");
    assert!(!settled(&fx).contains(&at.txid), "settled nowhere");

    drop(lib);
    reopen(&mut fx);
    let lib = Arc::new(FakeFund::new(&fx));
    lib.track(row);
    assert!(listed(&fx, &lib).unwrap().is_empty(), "after a restart");
    let (again, asked) = finish(&fx, &lib, at);
    assert_eq!(again.unwrap(), LockFinish::ReportedUsed);
    assert!(!asked, "no lease, so no passphrase");
    assert!(lib.applied().is_empty());
}

/// F2's "Finish all" on type 4: both restored locks answered "already
/// consumed", unrecorded by the library and proved consumed, count as
/// already used.
#[test]
fn finish_transfers_counts_restored_fund_locks_platform_reports_used() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    for n in 1..=2 {
        let row = lib.lock(n, 1_000_000, AssetLockStatus::RecoveredFromChain, true);
        lib.proves(row.out_point, SpentRead::Consumed);
        lib.track(row);
        lib.answer(Answer::Unrecorded);
    }
    let report = finish_transfers(&fx, &lib);
    assert_eq!((report.resumed, report.completed), (2, 0));
    assert_eq!((report.still_pending, report.already_used), (0, 2));
    assert!(listed(&fx, &lib).unwrap().is_empty());
    assert_eq!(
        reported(&fx, &lib),
        HashSet::from([lib.outpoint(0), lib.outpoint(1)])
    );
}

/// Another persistence failure after the signature is no answer: the
/// restored lock reads Checking, an untried one stays Ready, and neither
/// is reported used.
#[test]
fn a_persistence_failure_leaves_a_restored_fund_lock_unfinished() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    for n in 1..=2 {
        lib.track(lib.lock(n, 1_000_000, AssetLockStatus::ChainLocked, true));
    }
    let (failed, untried) = (lib.outpoint(0), lib.outpoint(1));
    lib.answer(Answer::PersistFailed);
    assert_eq!(finish(&fx, &lib, failed).0.unwrap(), LockFinish::Checking);
    assert_eq!(
        state_of(&fx, &lib),
        [
            (failed, UnfinishedState::Checking),
            (untried, UnfinishedState::Ready),
        ]
    );
    assert!(reported(&fx, &lib).is_empty());
}

#[path = "core_max_fund_tests.rs"]
mod core_max_fund_tests;

#[path = "unfinished_remainder_tests.rs"]
mod unfinished_remainder_tests;

/// BL-921: Platform's "already consumed" for a restored fund lock is not
/// taken without Platform's proof. With no proved answer (an error, no
/// proof, a timeout) or a proof that it is unspent, the lock reads
/// Checking, stays on the card and is reported used nowhere.
#[test]
fn an_unproved_already_consumed_leaves_a_restored_fund_lock_checking() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    for (n, read) in [
        (1, None),
        (2, Some(SpentRead::Unread)),
        (3, Some(SpentRead::Unspent)),
    ] {
        let row = lib.lock(n, 1_000_000, AssetLockStatus::RecoveredFromChain, true);
        let at = row.out_point;
        lib.track(row);
        if let Some(read) = read {
            lib.proves(at, read);
        }
        lib.answer(Answer::Unrecorded);
        let (finished, asked) = finish(&fx, &lib, at);
        assert!(asked, "{read:?}");
        assert_eq!(finished.unwrap(), LockFinish::Checking, "{read:?}");
        assert!(
            state_of(&fx, &lib).contains(&(at, UnfinishedState::Checking)),
            "{read:?} stays on the card"
        );
        assert!(!reported(&fx, &lib).contains(&at), "{read:?}");
    }
    assert!(reported(&fx, &lib).is_empty());
}

/// BL-921: a restored fund lock Platform's proof shows partly used (a
/// remaining value) is not used: it reads Checking, and "Check again"
/// applies it again (here the library's apply is proved).
#[test]
fn a_proved_remaining_fund_lock_stays_checking_and_check_again_applies_it() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let row = lib.lock(4, 1_000_000, AssetLockStatus::RecoveredFromChain, true);
    let at = row.out_point;
    lib.track(row);
    lib.proves(at, SpentRead::Remaining { credits: 1 });
    lib.answer(Answer::Unrecorded);
    assert_eq!(finish(&fx, &lib, at).0.unwrap(), LockFinish::Checking);
    assert_eq!(state_of(&fx, &lib), [(at, UnfinishedState::Checking)]);
    assert!(reported(&fx, &lib).is_empty());

    lib.answer(Answer::Done(None));
    let (again, asked) = finish(&fx, &lib, at);
    assert!(asked);
    assert_eq!(again.unwrap(), LockFinish::Applied);
    assert_eq!(lib.applied.lock().unwrap().len(), 2, "applied again");
    assert!(state_of(&fx, &lib).is_empty());
}

/// BL-921: an earlier version kept Platform's unproved "already consumed"
/// as `recovery.address_fund.reported.<outpoint>`. That record no longer
/// hides the lock: it lists again as Checking (a restored lock), and leaves
/// the card only once Platform's proof says it is consumed.
#[test]
fn an_earlier_versions_unproved_report_no_longer_hides_a_fund_lock() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let row = lib.lock(5, 1_000_000, AssetLockStatus::RecoveredFromChain, true);
    let at = row.out_point;
    lib.track(row);
    let (s, wallet) = (Arc::clone(&fx.s), fx.wallet);
    let appdb = s.live().unwrap().appdb;
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let key = format!("recovery.address_fund.reported.{at}");
    appdb.set_setting_durable(&scope, &key, Some("1")).unwrap();

    assert!(reported(&fx, &lib).is_empty(), "not read");
    assert_eq!(
        state_of(&fx, &lib),
        [(at, UnfinishedState::Checking)],
        "listed again, Checking"
    );

    lib.proves(at, SpentRead::Consumed);
    lib.answer(Answer::Unrecorded);
    assert_eq!(finish(&fx, &lib, at).0.unwrap(), LockFinish::ReportedUsed);
    assert!(state_of(&fx, &lib).is_empty(), "off the card once proved");
    assert_eq!(reported(&fx, &lib), HashSet::from([at]));
}

/// BL-921 (funds review): the lock key counts as signed when it signed on
/// any attempt of the apply call, a library retry's included. An "already
/// consumed" a retry heard, with no signature of its own, is Platform's
/// answer, unproved: Checking without the proof, used only with it.
#[test]
fn an_already_consumed_after_a_signed_attempt_and_a_retry_needs_the_proof() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let (unread, proved) = (
        lib.lock(6, 1_000_000, AssetLockStatus::RecoveredFromChain, true),
        lib.lock(7, 1_000_000, AssetLockStatus::RecoveredFromChain, true),
    );
    let (a, b) = (unread.out_point, proved.out_point);
    lib.track(unread);
    lib.track(proved);
    lib.answer(Answer::RetriedConsumed);
    let (finished, asked, outcomes) = finish_with_outcomes(&fx, &lib, a);
    assert!(asked);
    assert_eq!(finished.unwrap(), LockFinish::Checking);
    assert_eq!(outcomes, [FlowOutcome::MaybeSent], "the call signed");
    assert!(marks(&fx, &lib).1.outpoints.contains(&a));
    assert!(!reported(&fx, &lib).contains(&a));

    lib.proves(b, SpentRead::Consumed);
    lib.answer(Answer::RetriedConsumed);
    assert_eq!(finish(&fx, &lib, b).0.unwrap(), LockFinish::ReportedUsed);
    assert_eq!(reported(&fx, &lib), HashSet::from([b]));
    assert_eq!(
        *lib.signed.lock().unwrap(),
        [true, true],
        "one signature per call"
    );
}

/// #465: one unauthenticated "already consumed" answer is never final. A
/// restored fund lock Platform answers that for, with no proved read,
/// reads Checking, and after a restart it is listed again with Check
/// again. Check again applies it once the proved read says unspent, and
/// a later proved "consumed" takes another such lock off the card.
#[test]
fn an_unauthenticated_already_consumed_is_never_final_across_a_restart() {
    let mut fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let (live, used) = (
        lib.lock(11, 1_000_000, AssetLockStatus::RecoveredFromChain, true),
        lib.lock(12, 1_000_000, AssetLockStatus::RecoveredFromChain, true),
    );
    let (a, b) = (live.out_point, used.out_point);
    lib.track(live.clone());
    lib.track(used.clone());
    for at in [a, b] {
        lib.answer(Answer::Unrecorded);
        assert_eq!(finish(&fx, &lib, at).0.unwrap(), LockFinish::Checking);
    }
    assert!(reported(&fx, &lib).is_empty(), "nothing taken as used");

    drop(lib);
    reopen(&mut fx);
    let lib = Arc::new(FakeFund::new(&fx));
    lib.track(live);
    lib.track(used);
    let listed = state_of(&fx, &lib);
    assert_eq!(listed.len(), 2, "{listed:?}");
    for at in [a, b] {
        assert!(
            listed.contains(&(at, UnfinishedState::Checking)),
            "listed again after a restart, with Check again: {listed:?}"
        );
    }

    lib.proves(a, SpentRead::Unspent);
    lib.answer(Answer::Done(None));
    let (applied, asked) = finish(&fx, &lib, a);
    assert!(asked);
    assert_eq!(applied.unwrap(), LockFinish::Applied, "unspent: applied");

    lib.proves(b, SpentRead::Consumed);
    lib.answer(Answer::Unrecorded);
    assert_eq!(finish(&fx, &lib, b).0.unwrap(), LockFinish::ReportedUsed);
    assert!(
        state_of(&fx, &lib).is_empty(),
        "the proved read settles both"
    );
}

/// The mainnet flag (`platform_address_sends`): on mainnet, at a protocol
/// with address funds, a new "Add from Dash wallet" is refused, quote and
/// fund, before the hold, the fund lock, a read, the grant or a lock; a
/// lock a build before the flag broadcast still finishes ("Check again").
#[test]
fn a_new_fund_is_off_on_mainnet_and_an_earlier_lock_still_finishes() {
    let off = |code: &str, e: CreditsError| match e {
        CreditsError::Platform(PlatformError::FeatureOff { feature })
            if feature == "platform_address_sends" => {}
        other => panic!("{code}: {other:?}"),
    };
    let fx = Fixture::new(0);
    let mut mainnet = FakeFund::new(&fx);
    mainnet.network = Network::Mainnet;
    mainnet.hold = Some(RegistrationWait::Startup);
    let mainnet = Arc::new(mainnet);
    off("quote", quote_of(&fx, &mainnet, 1_000_000).unwrap_err());
    let wallet = fx.wallet;
    let g = grant(&fx, exact_grant(&fx, 1_000_000), 0);
    let ga = g.clone();
    let funded = run(&fx, &mainnet, move |s, lib| async move {
        fund::fund(&s, wallet, &*lib, FundAmount::All, &ga).await
    });
    off("fund", funded.unwrap_err());
    assert_eq!(mainnet.funds.load(Ordering::SeqCst), 0, "no lock");
    assert!(mainnet.tracked.lock().unwrap().is_empty());
    assert!(mainnet.applied().is_empty());
    assert!(
        super::journal_tests::grant_unused(&fx.s, fx.wallet, &g),
        "the grant is not redeemed"
    );

    // A lock out before the flag, its proof in since.
    let lib = Arc::new(FakeFund::new(&fx));
    *lib.funding.lock().unwrap() = Script::Unproved;
    fund_of(&fx, &lib).unwrap();
    let outpoint = lib.outpoint(0);
    lib.prove(outpoint);
    let mut later = FakeFund::new(&fx);
    later.network = Network::Mainnet;
    *later.tracked.lock().unwrap() = lib.tracked.lock().unwrap().clone();
    let later = Arc::new(later);
    later.answer(Answer::Done(None));
    assert_eq!(
        resume_of(&fx, &later, outpoint).unwrap(),
        PlatformFundState::Applied {
            credits: 1_000_000_000 - fee()
        }
    );
    assert_eq!(later.applied().len(), 1);

    // The recovery's Finish of a lock found on chain, on mainnet.
    let mut found = FakeFund::new(&fx);
    found.network = Network::Mainnet;
    let found = Arc::new(found);
    found.track(found.lock(9, 1_000_000, AssetLockStatus::RecoveredFromChain, true));
    let at = found.outpoint(0);
    found.answer(Answer::Done(Some(900_000_000)));
    let (finished, asked) = finish(&fx, &found, at);
    assert_eq!(finished.unwrap(), LockFinish::Applied);
    assert!(asked);
}

// ---- #585: a fund held on the wallet's bring-up ----

/// SPV started on `fx`'s session over `platform`, whose first bring-up
/// ends `PartialNoIdentity`, as a restart's can: from then `lib` is held
/// at `Startup` until a rerun answers.
fn held_on_the_bring_up(fx: &Fixture, lib: &FakeFund, platform: &Arc<PartialBringUp>) {
    *guard(&fx.s.platform.recovery.mock) = Some(Arc::clone(platform) as _);
    fx.engine.block_on(fx.s.start_spv()).unwrap();
    let startup = || fx.s.dashpay_startup(&fx.wallet).map(|s| s.startup).ok();
    wait_for("the partial bring-up, then SPV", || {
        startup() == Some(StartupStatus::PartialNoIdentity)
            && fx.s.spv_state().ok() == Some(super::SpvState::Running)
    });
    lib.startup_held.store(true, Ordering::SeqCst);
    assert_eq!(lib.funding_hold(), Some(RegistrationWait::Startup));
}

fn wait_for(what: &str, done: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// "Check again" on `outpoint` under `grant`, admitted as
/// `DashPay::resume_platform_fund` admits it (`fund::on_lib`, signing):
/// the session's close preflight and an operation guard held throughout.
async fn admitted_resume(
    s: Arc<NetworkSession>,
    wallet: WalletId,
    lib: Arc<FakeFund>,
    outpoint: OutPoint,
    grant: String,
) -> Result<PlatformFundState, CreditsError> {
    let _admitted = (s.signing_preflight(true).await).map_err(PlatformError::from)?;
    let _op = s.enter().await.map_err(PlatformError::from)?;
    let txid = outpoint.txid.to_string();
    let resumed = fund::resume(&s, wallet, &*lib, &txid, &grant).await;
    resumed.map(|o| o.state)
}

/// #585 (Q63c): after a restart, the first "Check again" on a pending fund
/// of a wallet whose bring-up ended unanswered was refused as held (it
/// only asked for the rerun), and a second press went through. Now it
/// waits for the rerun it asks for and applies.
#[test]
fn a_first_resume_after_a_held_bring_up_waits_for_its_rerun() {
    let fx = Fixture::with_platform();
    let lib = Arc::new(FakeFund::new(&fx));
    let outpoint = own_open_fund(&fx, &lib);
    let platform = Arc::new(PartialBringUp::default());
    held_on_the_bring_up(&fx, &lib, &platform);
    lib.answer(Answer::Done(None));
    let press = admitted_resume(
        Arc::clone(&fx.s),
        fx.wallet,
        Arc::clone(&lib),
        outpoint,
        grant(&fx, 1, 0),
    );
    let (lib2, platform2) = (Arc::clone(&lib), Arc::clone(&platform));
    let answer = async move {
        while platform2.calls.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let waited = lib2.applied().is_empty();
        platform2.rerun.notify_one();
        waited
    };
    let (resumed, waited) = fx
        .engine
        .block_on(async move {
            tokio::time::timeout(Duration::from_secs(60), async {
                tokio::join!(press, answer)
            })
            .await
        })
        .expect("the press and the rerun end");
    assert!(waited, "nothing applied while the rerun ran");
    assert!(
        matches!(resumed, Ok(PlatformFundState::Applied { .. })),
        "{resumed:?}"
    );
    assert_eq!(lib.applied(), [outpoint]);
    assert_eq!(platform.calls.load(Ordering::SeqCst), 2, "one rerun");
}

/// #585 (Q63c): a "Check again" waiting on the rerun holds the session's
/// close preflight and gate, as a Finish does: an in-app close meanwhile is
/// refused as preparing and the grant stays unused. The session's own close ends the
/// wait at once, and the press is refused before it takes a lease.
#[test]
fn a_resume_waiting_on_the_rerun_refuses_a_close_and_ends_with_the_session() {
    let fx = Fixture::with_platform();
    let lib = Arc::new(FakeFund::new(&fx));
    let outpoint = own_open_fund(&fx, &lib);
    let platform = Arc::new(PartialBringUp::default());
    held_on_the_bring_up(&fx, &lib, &platform);
    lib.answer(Answer::Done(None));
    let g = grant(&fx, 1, 0);
    let press = fx.s.rt.spawn(admitted_resume(
        Arc::clone(&fx.s),
        fx.wallet,
        Arc::clone(&lib),
        outpoint,
        g.clone(),
    ));
    wait_for("the rerun the press asked for", || {
        platform.calls.load(Ordering::SeqCst) == 2
    });
    std::thread::sleep(Duration::from_millis(200));
    let closed = fx.engine.block_on(fx.engine.close_network_unless_busy(
        crate::DashNetwork::Regtest,
        Duration::from_millis(500),
        |_| None::<()>,
    ));
    assert!(
        matches!(closed, Err(crate::CloseRefused::Preparing)),
        "{closed:?}"
    );
    assert!(fx.unused(&g), "the grant is unused");
    assert!(!press.is_finished(), "it still waits");

    // The press ends within 10 s of the close's start. The close itself
    // can take longer: SPV's teardown waits for the Platform syncs in
    // flight against the offline address, each up to its request deadline.
    let s = Arc::clone(&fx.s);
    let closing = fx.s.rt.spawn(async move { s.close().await });
    let resumed = fx
        .engine
        .block_on(async { tokio::time::timeout(Duration::from_secs(10), press).await })
        .expect("the press ends with the session")
        .unwrap();
    fx.engine
        .block_on(async { tokio::time::timeout(Duration::from_secs(60), closing).await })
        .expect("the close ends")
        .unwrap();
    // A press reading its hold after the close is still held.
    assert_eq!(lib.funding_hold(), Some(RegistrationWait::Sync));
    assert!(
        matches!(&resumed, Ok(PlatformFundState::NotApplied { .. }) | Err(_)),
        "{resumed:?}"
    );
    assert!(lib.applied().is_empty(), "no apply");
    assert!(lib.signed.lock().unwrap().is_empty(), "no signature");
    platform.rerun.notify_one();
}

/// #585 (Q63c): Finish all waited on the bring-up's rerun whenever the
/// fund side opened, before its shielded pass and with no fund lock to
/// apply. Only a fund lock's apply reads the funding hold: with a shielded
/// lock alone, nothing waits and no rerun is asked for.
#[test]
fn finish_all_with_only_a_shielded_lock_does_not_wait_on_the_bring_up() {
    let dir = dw_testutil::private_tempdir();
    let events = Arc::new(super::journal_tests::Recorder::default());
    let engine = super::journal_tests::engine(dir.path(), events as Arc<dyn crate::EventSink>);
    let (s, wallet) = super::journal_tests::session(&engine, false);
    let grant = |max_duffs| {
        (s.vault)
            .authorize(
                dw_vault::GrantPurpose::PlatformOp {
                    max_duffs,
                    max_credits: 0,
                },
                Some(&wallet.0),
                dw_vault::Credential::None,
            )
            .unwrap()
            .id
    };
    let shielded = (
        "m/9'/1'/5'/5'/0",
        FlowKind::Shield,
        AssetLockFundingType::AssetLockShieldedAddressTopUp,
    );
    let at = restored_lock(&engine, &s, wallet, shielded, grant(1));
    let platform = Arc::new(PartialBringUp::default());
    *guard(&s.platform.recovery.mock) = Some(Arc::clone(&platform) as _);
    *guard(&s.platform.bring_up_waits) = Some((Duration::from_secs(120), Duration::from_secs(120)));
    engine.block_on(s.start_spv()).unwrap();
    let startup = || s.dashpay_startup(&wallet).map(|s| s.startup).ok();
    wait_for("the partial bring-up, then SPV", || {
        startup() == Some(StartupStatus::PartialNoIdentity)
            && s.spv_state().ok() == Some(super::SpvState::Running)
    });
    super::recovery_tests::caught_up(&s, wallet);
    let hold = || {
        engine.block_on(async {
            let lib = fund::PwFund::open(&s, wallet).await.unwrap();
            lib.funding_hold()
        })
    };
    assert_eq!(hold(), Some(RegistrationWait::Startup), "a fund would wait");

    let waited = Arc::new(TestPause::default());
    *guard(&s.platform.pause_bring_up_wait) = Some(Arc::clone(&waited));
    let finished = Arc::new(AtomicBool::new(false));
    let watchdog = {
        let (waited, finished, platform) = (
            Arc::clone(&waited),
            Arc::clone(&finished),
            Arc::clone(&platform),
        );
        std::thread::spawn(move || {
            while !finished.load(Ordering::SeqCst) {
                if waited.reached.load(Ordering::SeqCst) {
                    platform.rerun.notify_one();
                    waited.release.notify_one();
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    let started = std::time::Instant::now();
    let report = engine
        .block_on(s.dashpay(wallet).finish_asset_locks(grant(1)))
        .unwrap();
    let elapsed = started.elapsed();
    finished.store(true, Ordering::SeqCst);
    watchdog.join().unwrap();
    assert!(
        !waited.reached.load(Ordering::SeqCst),
        "Finish all waited on the bring-up"
    );
    guard(&s.platform.pause_bring_up_wait).take();
    assert_eq!(
        (report.by_kind.iter()).map(|k| k.kind).collect::<Vec<_>>(),
        [LockKindId::Shielded],
        "{report:?}"
    );
    assert!(elapsed < Duration::from_secs(60), "{elapsed:?}");
    assert!(report.unread.is_empty(), "{report:?}");
    assert_eq!(platform.calls.load(Ordering::SeqCst), 1, "no rerun asked");
    assert_eq!(startup(), Some(StartupStatus::PartialNoIdentity));
    assert_eq!(hold(), Some(RegistrationWait::Startup), "still held");
    let listed = engine
        .block_on(s.dashpay(wallet).unfinished_transfers())
        .unwrap();
    assert!(
        (listed.locks.iter()).any(|l| l.txid == at.txid.to_string()),
        "still unfinished offline"
    );
    platform.rerun.notify_one();
    engine
        .block_on(engine.close_network(crate::DashNetwork::Regtest))
        .unwrap();
}
