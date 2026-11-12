//! An evonode's Platform credit withdrawal (MG-13, plan rev4: P1–P16), in
//! the engine: the library's `masternode_withdraw` builds, signs, sends
//! and waits in one call and keeps nothing of what it signed (F1), so the
//! engine builds the transition with dpp, signs it through the vault's
//! single-use `MasternodeWithdrawal` signer (P3), writes the record before
//! any network step (P5), sends the stored bytes unchanged and reads back
//! (F2, P7, P8). DP6-02's pieces are shared (`platform/withdrawal.rs`):
//! Drive's amount rules, `expected_duffs`, `document_id`, `destination`,
//! `takes` and `READ_BACK_TRIES`.
//!
//! - **Keys** (P2, `drive.rs`): the owner key (keychain `m/9'/coin'/3'/2'/i`
//!   or attached `owner`) signs with no output script, and Drive pays the
//!   destination its window over the identity's TRANSFER keys picks at
//!   execution (F3); the payout key (wallet-held or attached `payout`)
//!   signs the approved script. Both are found against the proved
//!   identity's keys.
//! - **The destination's check** (P4, `check.rs`): Platform has seen the
//!   synced header tip (a), no payout-change barrier of the masternode is
//!   pending (b, P16) and the identity read passed rule 5 (c, P12).
//! - **Records** (P5, P6): dw-appdb `mn_withdrawals`, app-wide and keyed by
//!   identity; at most one open per identity (the app-wide lock below, and
//!   the partial unique index across processes).
//! - **Nonces** (P10, `sign.rs`): from a proved nonce read at or above every
//!   floor of the identity's records, never the SDK's cache: the undecided
//!   group's nonce while Drive takes it, tip + 1 only once every group's
//!   nonce is used.
//! - **Recheck** (P8, `recheck.rs`): per nonce group, done only on a proved
//!   matching document (P7), `not_executed` only on P8's four conditions.
//!
//! Not part of the facade: `masternodes/withdrawals.rs` binds it
//! (`Masternodes`, mn-engine §2.5), and [`WithdrawalError`] maps into
//! `MasternodesError` below.

mod background;
mod check;
mod drive;
mod net;
mod recheck;
mod session;
mod sign;
#[cfg(test)]
mod tests;

use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use dpp::identity::IdentityPublicKey;
use dpp::prelude::Identifier;
use dpp::state_transition::StateTransition;
use dw_appdb::{AppDb, AppDbError, PayoutBarrierRow};
use dw_vault::WithdrawalKey;
use tokio::runtime::Handle;

use super::MasternodesError;
use super::WithdrawalCheckingReason;
use super::protx_records::ProTxOperationState;
use crate::EngineError;
use crate::platform::trust::Rule5;
use crate::platform::{CreditsError, Locks, PlatformError, keyed_lock};
use background::{Background, Job};

pub(crate) use check::{Destination, KeyView, Quote};
pub(crate) use session::WithdrawCall;

/// A masternode withdrawal's refusal (MG-13 P2, P13). The codes are the
/// facade's (`masternodes.withdrawal_*`), into which they map.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WithdrawalError {
    /// No usable owner or payout key, or the identity's matching key is
    /// disabled (P2).
    #[error("no usable withdrawal key")]
    KeyMissing,
    /// A shared masternode has no owner identity (F8).
    #[error("a shared masternode has no owner identity")]
    Shared,
    /// Drive's window holds no usable TRANSFER key (multi-payee, all
    /// disabled, or another key type; F3, F8).
    #[error("the identity has no usable payout key: {0}")]
    NoPayoutKey(&'static str),
    /// Platform knows no identity for the masternode.
    #[error("no Platform identity for this masternode")]
    NoIdentity,
    /// P4: (a)–(c) do not hold; nothing signed.
    #[error("the withdrawal's destination is still being checked: {}", reason.as_str())]
    DestinationChecking { reason: WithdrawalCheckingReason },
    /// An open record, a lost index race, or P10's "neither".
    #[error("a withdrawal of this masternode is pending")]
    Pending,
    /// Rule 5 under `Enforce` (P12).
    #[error("the Platform read is not SPV-verified")]
    Unverified,
    /// Try again from a state other than `refused`, or with the nonce no
    /// longer taken (P15).
    #[error("this withdrawal cannot be tried again")]
    RetryState,
    /// The call's masternode isn't its record's or its grant's.
    #[error("the withdrawal is for another masternode")]
    TargetMismatch,
    /// A mark from a state other than `couldnt_confirm` (P9).
    #[error("this withdrawal cannot be marked")]
    MarkState,
    #[error("unknown withdrawal")]
    NotFound,
    /// DP6-02's range and reserve refusals (`credits.withdrawal_out_of_range`,
    /// `platform.insufficient_credits`).
    #[error(transparent)]
    Credits(#[from] CreditsError),
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    Masternodes(#[from] MasternodesError),
    #[error(transparent)]
    Engine(#[from] EngineError),
}

impl From<WithdrawalError> for MasternodesError {
    /// The facade's codes (mn-engine §4, P13); DP6-02's amount refusals
    /// become `withdrawal_amount_*`.
    fn from(e: WithdrawalError) -> Self {
        type E = MasternodesError;
        match e {
            WithdrawalError::KeyMissing => E::WithdrawalKeyMissing,
            WithdrawalError::Shared => E::WithdrawalShared,
            WithdrawalError::NoPayoutKey(detail) => E::WithdrawalNoPayoutKey {
                detail: detail.into(),
            },
            WithdrawalError::NoIdentity => E::NoIdentity,
            WithdrawalError::DestinationChecking { reason } => {
                E::WithdrawalDestinationChecking { reason }
            }
            WithdrawalError::Pending => E::WithdrawalPending,
            WithdrawalError::Unverified => E::WithdrawalUnverified,
            WithdrawalError::RetryState => E::WithdrawalRetryState,
            WithdrawalError::TargetMismatch => E::WithdrawTargetMismatch,
            WithdrawalError::MarkState => E::WithdrawalMarkState,
            WithdrawalError::NotFound => E::WithdrawalNotFound,
            WithdrawalError::Credits(CreditsError::WithdrawalOutOfRange { min, max }) => {
                E::WithdrawalAmountOutOfRange { min, max }
            }
            WithdrawalError::Credits(CreditsError::Platform(
                PlatformError::InsufficientCredits { needed, available },
            ))
            | WithdrawalError::Platform(PlatformError::InsufficientCredits { needed, available }) => {
                E::WithdrawalAmountInsufficient { needed, available }
            }
            WithdrawalError::Credits(CreditsError::Platform(e)) | WithdrawalError::Platform(e) => {
                E::Platform(e)
            }
            // Rules::credits refuses with the two above only.
            WithdrawalError::Credits(e) => E::Engine(EngineError::Internal(e.to_string())),
            WithdrawalError::Masternodes(e) => e,
            WithdrawalError::Engine(e) => E::Engine(e),
        }
    }
}

impl From<AppDbError> for WithdrawalError {
    /// The index's and the barrier's refusals of an insert are P6's and
    /// P4 (b)'s; anything else is storage.
    fn from(e: AppDbError) -> Self {
        match e {
            AppDbError::WithdrawalPending(_) => Self::Pending,
            AppDbError::PayoutChangePending(_) => Self::DestinationChecking {
                reason: WithdrawalCheckingReason::PayoutChangePending,
            },
            e => Self::Engine(EngineError::Storage(format!("app database: {e}"))),
        }
    }
}

impl From<tokio::task::JoinError> for WithdrawalError {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::Engine(e.into())
    }
}

/// The header tip SPV has synced to (P4 (a)); `None` while unsynced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Chain {
    pub(crate) header_tip: Option<u32>,
}

/// The bounded waits (P4, P12); tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Waits {
    /// The quote's wait for Platform to catch up (P4): one Platform
    /// empty-block interval plus margin.
    pub(crate) quote: Duration,
    /// `withdraw`'s wait before the lock and the grant (P4): at most half
    /// the default grant life (`DEFAULT_GRANT_TTL_SECS`).
    pub(crate) sign: Duration,
    /// How often both waits read again.
    pub(crate) poll: Duration,
    /// DP3-04 §3's single round under `Probe` (P12).
    pub(crate) probe: Duration,
    /// How often the background recheck runs while anything is undecided.
    pub(crate) recheck: Duration,
}

impl Default for Waits {
    fn default() -> Self {
        Self {
            quote: Duration::from_secs(240),
            sign: Duration::from_secs(60),
            poll: Duration::from_secs(20),
            probe: Duration::from_secs(10),
            recheck: Duration::from_secs(60),
        }
    }
}

/// An invariant violation the code refuses on and surfaces (P6, P10): the
/// recheck's and the nonce choice's alarms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Alarm {
    /// The identity, in the engine's encoding.
    pub(crate) identity: String,
    pub(crate) nonce: u64,
    pub(crate) what: String,
}

/// What the withdrawal's core reads of the session (tests script it).
pub(crate) trait Env: Send + Sync {
    /// The app database (records and barriers are app-wide).
    fn db(&self) -> &Arc<AppDb>;
    /// Rule 5 now (DP3-04, P12).
    fn rule_5(&self) -> Rule5;
    fn chain(&self) -> Chain;
    /// Whether the open's barrier backfill completed (P16); `false` reads
    /// as a pending payout change.
    fn barriers_ready(&self) -> bool;
    /// The barrier's operation in its creating wallet's history (P16's
    /// "confirmed and covered" and "conflicted"); `None` when that can't
    /// be read (a deleted or unloaded wallet), fail closed.
    fn barrier_state(
        &self,
        barrier: &PayoutBarrierRow,
    ) -> impl Future<Output = Result<Option<ProTxOperationState>, WithdrawalError>> + Send;
    fn waits(&self) -> Waits;
    /// UNIX seconds.
    fn now(&self) -> u64;
    /// Surfaces an invariant violation (logged by the caller).
    fn alarm(&self, alarm: Alarm);
    /// The [`Hold`] each blocking app database call keeps until it returns
    /// (`on_db`).
    fn hold(&self) -> Hold;
}

/// A key the wallet holds that matches one of the identity's keys (P2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeldKey {
    pub(crate) key: WithdrawalKey,
    /// hash160 of its compressed public key: the identity key's data.
    pub(crate) hash: [u8; 20],
}

/// The wallet's withdrawal keys (P2), found by the identity's keys.
pub(crate) trait WalletKeys: Send + Sync {
    /// The owner keys among `owner` and the payout keys among `payout`
    /// (hash160s of the identity's ECDSA_HASH160 OWNER and TRANSFER keys)
    /// the wallet holds, keychain or attached.
    fn held(
        &self,
        owner: Vec<[u8; 20]>,
        payout: Vec<[u8; 20]>,
    ) -> impl Future<Output = Result<Vec<HeldKey>, WithdrawalError>> + Send;
}

/// The grant redeemed into the vault's single-use signer and its lease
/// (P3, P12): `sign` charges `credits` (the amount and the fee reserve) and
/// returns the 65-byte signature of `transition`, unsigned.
pub(crate) trait Authorize: Send {
    fn sign(
        self,
        credits: u64,
        key: &WithdrawalKey,
        identity_key: &IdentityPublicKey,
        transition: &StateTransition,
        pro_tx_hash: [u8; 32],
    ) -> impl Future<Output = Result<[u8; 65], WithdrawalError>> + Send;
}

/// The app-wide lock of one masternode identity's withdrawals (P6), keyed
/// by the identity alone: held from the open-record check through the
/// nonce read, the signature and the record's insert, and by each group's
/// recheck. Unlike `identity_locks` it is not per wallet: records are
/// app-wide.
pub(crate) fn identity_lock(identity: Identifier) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: LazyLock<Locks<Identifier>> = LazyLock::new(Locks::default);
    #[cfg(test)]
    if UNLOCKED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(&identity)
    {
        // The lock mutated away (§3 "the index alone"): a fresh one each
        // call, so nothing is serialised.
        return Arc::default();
    }
    keyed_lock(&LOCKS, &identity)
}

/// Tests: the identities whose lock is taken away (per identity, so the
/// tests running beside it keep theirs).
#[cfg(test)]
pub(crate) static UNLOCKED: Mutex<Vec<Identifier>> = Mutex::new(Vec::new());

/// The engine's fault points at the network steps (MG-13 §3 "Fault sweep"),
/// with dw-appdb's mechanism (`dw_appdb::fault`): each has a `before_` and an
/// `after_` stage.
pub(crate) mod points {
    /// The first hand-off of a signed record: before it, after `sent_at`;
    /// after it, before its answer is recorded.
    pub(crate) const BROADCAST: &str = "mn_withdraw.broadcast";
    /// A recheck's hand-off of the open record's stored bytes.
    pub(crate) const REBROADCAST: &str = "mn_withdraw.rebroadcast";
    /// The signature: before the grant is redeemed, after it signed and
    /// before the record's insert.
    pub(crate) const SIGN: &str = "mn_withdraw.sign";
    /// P8 step 1's document reads.
    pub(crate) const READ_DOCUMENTS: &str = "mn_withdraw.read_documents";
    /// P8 step 2's nonce read.
    pub(crate) const READ_NONCE: &str = "mn_withdraw.read_nonce";
    /// P8's (ii) and (iv) reads, before `not_executed`.
    pub(crate) const READ_PROOF: &str = "mn_withdraw.read_proof";

    #[cfg(test)]
    pub(crate) const ALL: [&str; 6] = [
        BROADCAST,
        REBROADCAST,
        SIGN,
        READ_DOCUMENTS,
        READ_NONCE,
        READ_PROOF,
    ];
}

/// The engine's fault point `point` before its step on `db`: armed, it fails
/// the step (`storage`), and `DW_FAULT` naming it aborts the process (T2
/// (4)). Compiled out of release builds and builds without `fault-inject`.
pub(crate) fn fault_before(db: &AppDb, point: &str) -> Result<(), WithdrawalError> {
    fault(db, point, true)
}

/// [`fault_before`]'s `after_` stage, once the step is done.
pub(crate) fn fault_after(db: &AppDb, point: &str) -> Result<(), WithdrawalError> {
    fault(db, point, false)
}

#[cfg(all(debug_assertions, any(test, feature = "fault-inject")))]
fn fault(db: &AppDb, point: &str, before: bool) -> Result<(), WithdrawalError> {
    use dw_appdb::fault::Stage;
    let stage = if before { Stage::Before } else { Stage::After };
    Ok(db.fault_point(point, stage)?)
}

#[cfg(not(all(debug_assertions, any(test, feature = "fault-inject"))))]
#[inline(always)]
fn fault(_db: &AppDb, _point: &str, _before: bool) -> Result<(), WithdrawalError> {
    Ok(())
}

/// A blocking app database call's token: close waits for it (m1), so a
/// call an aborted task started still ends before close returns.
pub(crate) struct Hold {
    _token: background::Token,
    /// Tests: runs on the blocking thread before the call.
    #[cfg(test)]
    before: Option<Box<dyn FnOnce() + Send>>,
}

/// `call` on the app database, on the blocking pool, holding `env`'s
/// [`Hold`] until it returned and let go of the database.
pub(crate) async fn on_db<T, F>(env: &impl Env, call: F) -> Result<T, WithdrawalError>
where
    T: Send + 'static,
    F: FnOnce(&AppDb) -> Result<T, AppDbError> + Send + 'static,
{
    let (db, hold) = (Arc::clone(env.db()), env.hold());
    Ok(tokio::task::spawn_blocking(move || {
        #[cfg(test)]
        let mut hold = hold;
        #[cfg(test)]
        if let Some(before) = hold.before.take() {
            before();
        }
        let result = call(&db);
        drop(db);
        drop(hold);
        result
    })
    .await??)
}

/// The withdrawal record `id`, or `withdrawal_not_found`.
pub(crate) async fn load(
    env: &impl Env,
    id: &str,
) -> Result<dw_appdb::WithdrawalRow, WithdrawalError> {
    let id = id.to_owned();
    on_db(env, move |db| db.withdrawal(&id))
        .await?
        .ok_or(WithdrawalError::NotFound)
}

/// A record's identity, as the records encode it (Base58).
pub(crate) fn parse_id(identity: &str) -> Result<Identifier, WithdrawalError> {
    Identifier::from_string(
        identity,
        dpp::platform_value::string_encoding::Encoding::Base58,
    )
    .map_err(|_| WithdrawalError::NotFound)
}

/// The session's withdrawal state: the alarms, the background recheck (P8)
/// and the quotes' waits for Platform (P4), on `MasternodesRuntime`.
#[derive(Default)]
pub(crate) struct WithdrawalRuntime {
    alarms: Mutex<Vec<Alarm>>,
    /// The recheck, the waits and every blocking app database call,
    /// tracked for close (M1, m1).
    work: Arc<Background>,
    /// Tests: the scripted Platform, header tip and waits the session's
    /// calls use instead of the SDK's, SPV's and the defaults.
    #[cfg(test)]
    seam: Mutex<Option<tests::Seam>>,
    /// Tests: holds the next barrier's operation row read
    /// (`SessionEnv::barrier_state`) on its blocking thread.
    #[cfg(test)]
    pub(crate) pause_op_read: Mutex<Option<Arc<crate::coins::TestPause>>>,
}

impl WithdrawalRuntime {
    pub(crate) fn alarm(&self, alarm: Alarm) {
        tracing::error!(identity = %alarm.identity, nonce = alarm.nonce, what = %alarm.what,
            "masternode withdrawal invariant violated (MG-13 P6, P10)");
        let mut alarms = self.alarms.lock().unwrap_or_else(PoisonError::into_inner);
        if !alarms.contains(&alarm) {
            alarms.push(alarm);
        }
    }

    /// The alarms raised this session (logged as they are raised; MG-10
    /// surfaces them).
    #[expect(dead_code, reason = "MG-10 surfaces them")]
    pub(crate) fn alarms(&self) -> Vec<Alarm> {
        self.alarms
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn waits(&self) -> Waits {
        #[cfg(test)]
        if let Some(seam) = &*self.seam.lock().unwrap_or_else(PoisonError::into_inner) {
            return seam.waits;
        }
        Waits::default()
    }

    /// Runs `wait` as `pro_tx_hash`'s `withdrawal_ready` wait on `rt`,
    /// unless one runs already or [`Self::stop`] has begun.
    pub(crate) fn wait_ready(
        &self,
        rt: &Handle,
        pro_tx_hash: [u8; 32],
        wait: impl Future<Output = ()> + Send + 'static,
    ) {
        self.work.spawn(rt, Job::Ready(pro_tx_hash), wait);
    }

    /// A blocking app database call's [`Hold`].
    pub(crate) fn hold(&self) -> Hold {
        Hold {
            _token: self.work.hold(),
            #[cfg(test)]
            before: None,
        }
    }

    /// Close: starts no recheck or wait after it begins, aborts the running
    /// ones, and returns once each has been dropped and every blocking app
    /// database call has returned.
    pub(crate) async fn stop(&self) {
        self.work.close().await;
    }
}
