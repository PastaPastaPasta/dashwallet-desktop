//! Tests of platform_fund.rs (0.2 X1b R2, "Add from Dash wallet") and of
//! its locks as the recovery's type-4 kind ([`FundLocks`]), on
//! requests_tests.rs's fixture over a scripted library ([`FakeFund`]), with
//! the real dispatch journal, lease and `app.sqlite`: the quote's fee,
//! minimum and refusals; a fund's lock held at most once by its step; the
//! apply of the step's own lock to the receive address through the
//! recovery's runner, and how it ends; "Check again" on the same outpoint,
//! never a new lock; only this install's locks listed by the fund, every
//! one by the recovery; the conformance run; the lease's signers.

use std::collections::{HashSet, VecDeque};
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

use super::errors::{CreditsError, PlatformError};
use super::flows::{BudgetPurpose, FlowKind};
use super::lease::{CallPermit, FlowOutcome, FundingKey, Lease, StoredLock};
use super::platform_addresses::{PlatformFundOutcome, PlatformFundQuote, PlatformFundState};
use super::platform_fund::{self as fund, FundLib};
use super::platform_moves;
use super::registration::RegistrationWait;
use super::registration_flow::{FundError, Spendable};
use super::requests::tests::Fixture;
use super::shielded_pool::MAX_LOCK_INPUTS;
use super::unfinished_conformance::{self as conformance, End, Rig};
use super::unfinished_funds::FundLocks;
use super::unfinished_locks::{
    self as locks, KindLock, LockFinish, LockKind, LockKindId, LockOrigin, Mark, Marks, Minimum,
    UnfinishedLock, UnfinishedState,
};
use crate::{NetworkSession, WalletId};

/// The credit key a fund's lock pays (the library's address top-up key,
/// account 0, index 0), on testnet.
const LOCK_KEY: &str = "m/9'/1'/5'/4'/0";

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
    /// A broadcast failure.
    Failed,
}

/// platform-wallet's side of a fund, scripted; the journal, the lease and
/// `app.sqlite` are the fixture's real ones.
struct FakeFund {
    session: Arc<NetworkSession>,
    wallet: WalletId,
    version: u32,
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
    receive: PlatformAddress,
}

impl FakeFund {
    fn new(fx: &Fixture) -> Self {
        Self {
            session: Arc::clone(&fx.s),
            wallet: fx.wallet,
            version: PlatformVersion::latest().protocol_version,
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
            receive: PlatformAddress::P2pkh([0x42; 20]),
        }
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
        Network::Testnet
    }

    async fn refresh_protocol_version(&self) -> u32 {
        self.version
    }

    fn protocol_version(&self) -> u32 {
        self.version
    }

    async fn fund_lock_rows(&self) -> Vec<KindLock> {
        fund::fund_rows(&*self.tracked.lock().unwrap())
    }

    async fn credit_script(&self, outpoint: OutPoint) -> Option<ScriptBuf> {
        fund::credit_script_of(&*self.tracked.lock().unwrap(), outpoint)
    }

    async fn coin_values(&self) -> Result<Vec<u64>, PlatformError> {
        Ok(self.coins.clone())
    }

    async fn spendable(&self) -> Result<Spendable, PlatformError> {
        Ok(self.spendable)
    }

    async fn fee_bound(&self) -> Result<u64, PlatformError> {
        Ok(1_000)
    }

    fn funding_hold(&self) -> Option<RegistrationWait> {
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
        let (sign, answer) = (self.answers.lock().unwrap().pop_front()).expect("a scripted apply");
        if sign {
            let path = self.key.lock().unwrap().clone();
            let signed = key_wallet::Signer::sign_ecdsa(signer, &path, [7; 32]).await;
            self.signed.lock().unwrap().push(signed.is_ok());
        }
        match answer {
            Answer::Done(gained) => Ok(gained),
            Answer::Consumed { of } => Err(PlatformWalletError::AssetLockAlreadyConsumed(
                of.unwrap_or(outpoint),
            )),
            Answer::Failed => Err(PlatformWalletError::TransactionBroadcast(
                "broadcast failed".into(),
            )),
        }
    }

    async fn sync_now(&self) {
        self.syncs.fetch_add(1, Ordering::SeqCst);
    }
}

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
        fund::quote(&s, wallet, &*lib, duffs).await
    })
}

/// A fund of 1,000,000 duffs, its apply (if any) answered as scripted.
fn fund_of(fx: &Fixture, lib: &Arc<FakeFund>) -> Result<PlatformFundOutcome, CreditsError> {
    let (wallet, grant) = (fx.wallet, grant(fx, 2_000_000, 0));
    run(fx, lib, move |s, lib| async move {
        fund::fund(&s, wallet, &*lib, 1_000_000, &grant).await
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
        locks::detect(&FundLocks::new(&s, wallet, &*lib), &HashSet::new()).await
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
    assert_eq!(
        (q.duffs, q.fee_duffs, q.total_duffs),
        (1_000_000, 1_000, 1_001_000)
    );
    assert_eq!((q.fee_credits, q.credits), (fee(), 1_000_000_000 - fee()));
    assert_eq!(q.to, lib.receive.to_bech32m_string(Network::Testnet));
    assert_eq!((q.grant.max_duffs, q.grant.max_credits), (1_001_000, 0));
    let min = fee() / 1_000 + 1;
    assert_eq!(quote_of(&fx, &lib, min).unwrap().credits, 1_000);
    assert_eq!(
        quote_of(&fx, &lib, min - 1).unwrap_err(),
        CreditsError::BelowMinimum { min }
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
            needed: 1_001_000,
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

/// Platform's unproved "already consumed", or a failure after signing, is
/// no settle evidence: the lock pends "Checking" however often a resume
/// hears it, blocks no new fund, and each resume applies the same outpoint
/// until a proved apply settles it.
#[test]
fn already_consumed_reads_checking_and_does_not_block() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    lib.answer(Answer::Consumed { of: None });
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
    assert!(!settled(&fx).contains(&outpoint.txid));
    assert!(marks(&fx, &lib).1.outpoints.contains(&outpoint));
    lib.answer(Answer::Done(None));
    assert!(matches!(
        resume_of(&fx, &lib, outpoint).unwrap(),
        PlatformFundState::Applied { .. }
    ));
    assert_eq!(lib.applied(), [outpoint; 3]);
    assert!(settled(&fx).contains(&outpoint.txid));
    assert!(pending_of(&fx, &lib).is_empty());
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
            state: UnfinishedState::Ready,
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
/// kind's unproved answer: one naming another outpoint is a failure like
/// any other, unknown after the lock key's signature (Checking) and the
/// refusal before it (the lock stays Ready, its call `Cancelled`).
#[test]
fn already_consumed_counts_only_for_the_applied_outpoint() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let other = OutPoint::new(Txid::from_byte_array([0x99; 32]), 0);
    for n in 1..=3 {
        lib.track(lib.lock(n, 1_000_000, AssetLockStatus::RecoveredFromChain, true));
    }
    let (own, signed, unsigned) = (lib.outpoint(0), lib.outpoint(1), lib.outpoint(2));

    lib.refuse(Answer::Consumed { of: None });
    assert_eq!(finish(&fx, &lib, own).0.unwrap(), LockFinish::Checking);
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
            (own, UnfinishedState::Checking),
            (signed, UnfinishedState::Checking),
            (unsigned, UnfinishedState::Ready),
        ]
    );
    let (settled, unknown) = marks(&fx, &lib);
    assert!(settled.outpoints.is_empty());
    assert_eq!(unknown.outpoints, HashSet::from([own, signed]));
}

/// The apply's signer gives the lock's own credit key only: another credit
/// key the funding scope signs with (a shield's, an identity top-up's) is
/// refused, so a failure after it is no signature of the lock and leaves
/// it Ready, not Checking.
#[test]
fn the_apply_signs_with_the_locks_credit_key_only() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    lib.track(lib.lock(1, 1_000_000, AssetLockStatus::RecoveredFromChain, true));
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
