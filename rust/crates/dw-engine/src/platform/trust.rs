//! The layered quorum provider (DASHPAY §2.2 rules 1–4 and 6; E0-10b,
//! `docs/design/E0-10b-trust-provider.md`). Every Platform proof gets its
//! quorum key here: the SDK calls [`TrustProvider::quorum_public_key`]
//! synchronously from a tokio worker, and an `Err` fails the proof.
//!
//! Three ways to answer, by [`TrustMode`] and the developer toggle:
//! - `Degraded` (the default, DEC-168): the trusted service supplies the
//!   key, and whenever SPV holds the quorum its key is compared. A `Verified`
//!   SPV key is served when the service is unreachable.
//! - `Enforcing` (R7): SPV `Verified` keys only, with the trusted service as
//!   a read-only fallback before masternode sync. After it, an unknown quorum
//!   waits a bounded time for SPV, then fails closed.
//! - trusted-only ([`TRUSTED_ONLY_KEY`]): the trusted service alone.
//!
//! Whenever both sources answer, a mismatch refuses
//! (`platform.trust_mismatch`, `Notice{PlatformTrustMismatch}`). In every
//! mode a key SPV did not verify is served only after the durable fallback
//! latch ([`Provenance::fallback_in_use`]): if the latch cannot be made
//! durable, nothing is served.
//!
//! The provider holds SPV and provenance weakly: the SPV runtime's event
//! handlers reach it through the session hub, and a strong handle would keep
//! the closed session's storage claimed.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use dash_context_provider::{ContextProvider, ContextProviderError};
use dashcore::sml::llmq_entry_verification::LLMQEntryVerificationStatus;
use platform_wallet::SpvRuntime;
use tokio::sync::watch;

use super::provenance::Provenance;
use super::runtime::guard;
use super::startup::QuorumSource;
use crate::DashNetwork;
use crate::context::LazyTrustedContext;
use crate::events::{EngineEvent, EventSink, NoticeCode};

/// The developer toggle (global setting, `"1"` = on): quorum keys from the
/// trusted service only, SPV not consulted.
pub(crate) const TRUSTED_ONLY_KEY: &str = "trust.trusted_only";

/// How long an unknown quorum waits for SPV once masternode sync is done
/// (rule 3).
pub(crate) const UNKNOWN_QUORUM_WAIT: Duration = Duration::from_secs(120);
/// A quorum that failed closed fails at once for this long: the SDK retries
/// a failed proof on other nodes, which cite the same quorum.
const FAILED_CLOSED_FOR: Duration = Duration::from_secs(30);
const SPV_POLL: Duration = Duration::from_secs(1);
/// One SPV lookup; a lookup that takes longer counts as `Absent`.
const SPV_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);
/// Cached `Verified` keys; the cache is cleared when it reaches this size.
/// A cached key is not re-checked within the session: a mined quorum's key
/// does not change.
const CACHE_CAP: usize = 256;
/// Starts the `InvalidQuorum` text of a mismatch refusal; see
/// [`is_trust_mismatch`].
const MISMATCH_MARKER: &str = "quorum key mismatch between SPV and the trusted service";

/// Which policy the provider applies (design §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustMode {
    /// DASHPAY §2.2's degraded mode (DEC-168).
    Degraded,
    /// R7 in full. H-09 makes it the default once chain anchoring of quorum
    /// lists (dashpay/rust-dashcore#1117) is in the graph; until then no
    /// session opens in it, so nothing waits on [`TrustProvider::writes_wait`].
    Enforcing,
}

/// The mode sessions open with.
pub(crate) const DEFAULT_MODE: TrustMode = TrustMode::Degraded;

/// What rule 5 (money never moves on unverified data) is under the mode
/// and the toggle: DP3-04's money-move gate (`money_gate.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rule5 {
    /// `Enforcing`: money waits until the data re-verifies against SPV.
    Enforce,
    /// `Degraded`: rule 5 is suspended, a mismatch still refuses.
    Probe,
    /// The trusted-only toggle: SPV is not consulted.
    Off,
}

/// What SPV holds for a quorum (design §3, status mapping).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpvAnswer {
    /// dash-spv checked the commitment against the members it derives.
    Verified([u8; 48]),
    /// A key dash-spv holds but has not verified (`Unknown`, `Skipped`).
    Held([u8; 48]),
    /// dash-spv judged the quorum invalid.
    Invalid,
    /// Not found, or SPV not started.
    Absent,
}

impl SpvAnswer {
    pub(crate) fn of(public_key: [u8; 48], status: &LLMQEntryVerificationStatus) -> Self {
        match status {
            LLMQEntryVerificationStatus::Verified => Self::Verified(public_key),
            LLMQEntryVerificationStatus::Invalid(_) => Self::Invalid,
            LLMQEntryVerificationStatus::Unknown | LLMQEntryVerificationStatus::Skipped(_) => {
                Self::Held(public_key)
            }
        }
    }
}

pub(crate) type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// SPV's quorum lookup (platform-wallet's `SpvRuntime` in a session).
pub(crate) trait SpvQuorums: Send + Sync + 'static {
    fn lookup(&self, quorum_type: u32, quorum_hash: [u8; 32], height: u32) -> BoxFuture<SpvAnswer>;
}

/// platform-wallet's SPV lookup (platform#5366). Weak: see the module doc.
pub(crate) struct RuntimeQuorums(pub(crate) Weak<SpvRuntime>);

impl SpvQuorums for RuntimeQuorums {
    fn lookup(&self, quorum_type: u32, quorum_hash: [u8; 32], height: u32) -> BoxFuture<SpvAnswer> {
        let spv = self.0.upgrade();
        Box::pin(async move {
            let Some(spv) = spv else {
                return SpvAnswer::Absent;
            };
            match spv
                .get_quorum_public_key_with_status(quorum_type, quorum_hash, height)
                .await
            {
                Ok(q) => SpvAnswer::of(q.public_key, &q.status),
                Err(e) => {
                    tracing::debug!(error = %e, "SPV quorum lookup missed");
                    SpvAnswer::Absent
                }
            }
        })
    }
}

/// The trusted HTTPS quorum service.
pub(crate) trait TrustedQuorums: Send + Sync + 'static {
    fn quorum_key(
        &self,
        quorum_type: u32,
        quorum_hash: [u8; 32],
        height: u32,
    ) -> Result<[u8; 48], ContextProviderError>;
}

impl TrustedQuorums for Arc<LazyTrustedContext> {
    fn quorum_key(
        &self,
        quorum_type: u32,
        quorum_hash: [u8; 32],
        height: u32,
    ) -> Result<[u8; 48], ContextProviderError> {
        self.provider()?
            .get_quorum_public_key(quorum_type, quorum_hash, height)
    }
}

/// A key and whether SPV verified it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Served {
    key: [u8; 48],
    verified: bool,
}

impl Served {
    fn by_spv(key: [u8; 48]) -> Self {
        Self {
            key,
            verified: true,
        }
    }

    fn by_service(key: [u8; 48]) -> Self {
        Self {
            key,
            verified: false,
        }
    }
}

type Quorum = (u32, [u8; 32]);

struct Attached {
    spv: Arc<dyn SpvQuorums>,
    /// Gone once the session closed; then nothing unverified is served.
    provenance: Weak<Provenance>,
}

/// The session's quorum provider. Shared by the SDK's `ContextProvider`
/// handle, the sync tracker (masternode sync) and the re-fetch task.
pub(crate) struct TrustProvider {
    /// The mode is `Enforcing` (else `Degraded`). Set at construction;
    /// tests switch it ([`Self::set_mode`]).
    enforcing: AtomicBool,
    trusted_only: AtomicBool,
    trusted: Arc<dyn TrustedQuorums>,
    /// SPV and provenance, attached once the wallet manager exists. Until
    /// then no fallback can be served (the latch cannot be set).
    attached: OnceLock<Attached>,
    /// SPV's masternode state has synced in this session. Sticky (rule 3).
    synced: watch::Sender<bool>,
    unknown_quorum_wait: Duration,
    spv_poll: Duration,
    spv_lookup_timeout: Duration,
    cache: Mutex<HashMap<Quorum, [u8; 48]>>,
    failed_closed: Mutex<HashMap<Quorum, Instant>>,
    /// Quorums a mismatch notice was sent for.
    mismatched: Mutex<HashSet<Quorum>>,
    sink: Arc<dyn EventSink>,
    network: DashNetwork,
}

impl TrustProvider {
    pub(crate) fn new(
        mode: TrustMode,
        trusted: Arc<dyn TrustedQuorums>,
        sink: Arc<dyn EventSink>,
        network: DashNetwork,
    ) -> Self {
        Self {
            enforcing: AtomicBool::new(mode == TrustMode::Enforcing),
            trusted_only: AtomicBool::new(false),
            trusted,
            attached: OnceLock::new(),
            synced: watch::Sender::new(false),
            unknown_quorum_wait: UNKNOWN_QUORUM_WAIT,
            spv_poll: SPV_POLL,
            spv_lookup_timeout: SPV_LOOKUP_TIMEOUT,
            cache: Default::default(),
            failed_closed: Default::default(),
            mismatched: Default::default(),
            sink,
            network,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_waits(mut self, wait: Duration, poll: Duration) -> Self {
        self.unknown_quorum_wait = wait;
        self.spv_poll = poll;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_lookup_timeout(mut self, limit: Duration) -> Self {
        self.spv_lookup_timeout = limit;
        self
    }

    #[cfg(test)]
    pub(crate) fn set_mode(&self, mode: TrustMode) {
        (self.enforcing).store(mode == TrustMode::Enforcing, Ordering::SeqCst);
    }

    fn mode(&self) -> TrustMode {
        if self.enforcing.load(Ordering::SeqCst) {
            TrustMode::Enforcing
        } else {
            TrustMode::Degraded
        }
    }

    /// Gives the provider SPV and provenance. Later calls are ignored.
    pub(crate) fn attach(&self, spv: Arc<dyn SpvQuorums>, provenance: &Arc<Provenance>) {
        let provenance = Arc::downgrade(provenance);
        if self.attached.set(Attached { spv, provenance }).is_err() {
            tracing::warn!("the quorum provider was already attached");
        }
    }

    pub(crate) fn set_trusted_only(&self, on: bool) {
        self.trusted_only.store(on, Ordering::SeqCst);
    }

    pub(crate) fn trusted_only(&self) -> bool {
        self.trusted_only.load(Ordering::SeqCst)
    }

    /// SPV's masternode phase is done. Idempotent; never undone.
    pub(crate) fn note_masternodes_synced(&self) {
        self.synced
            .send_if_modified(|s| !std::mem::replace(s, true));
    }

    pub(crate) fn masternodes_synced(&self) -> bool {
        *self.synced.borrow()
    }

    /// Watches masternode sync: `true` once done, never `false` again.
    pub(crate) fn subscribe_synced(&self) -> watch::Receiver<bool> {
        self.synced.subscribe()
    }

    /// Where proofs get their quorum keys now (`sync_status`).
    pub(crate) fn source(&self) -> QuorumSource {
        match self.mode() {
            _ if self.trusted_only() => QuorumSource::Trusted,
            TrustMode::Degraded => QuorumSource::Trusted,
            TrustMode::Enforcing if self.masternodes_synced() => QuorumSource::Spv,
            TrustMode::Enforcing => QuorumSource::TrustedFallback,
        }
    }

    /// Platform writes must wait for masternode sync (rule 2). Only in
    /// `Enforcing`: the degraded mode suspends rule 5, and the toggle is the
    /// developer's choice. DP2-02's send reads it for a scan shown on the
    /// fallback's word; H-09 wires it into every write flow.
    pub(crate) fn writes_wait(&self) -> bool {
        self.mode() == TrustMode::Enforcing && !self.trusted_only() && !self.masternodes_synced()
    }

    /// Rule 5 now (DP3-04): the toggle wins over the mode, as for keys.
    pub(crate) fn rule_5(&self) -> Rule5 {
        match self.mode() {
            _ if self.trusted_only() => Rule5::Off,
            TrustMode::Enforcing => Rule5::Enforce,
            TrustMode::Degraded => Rule5::Probe,
        }
    }

    /// The `ContextProvider` lookup. Blocking: it may wait for SPV, the
    /// trusted service and app.sqlite.
    pub(crate) fn quorum_public_key(
        &self,
        quorum_type: u32,
        quorum_hash: [u8; 32],
        height: u32,
    ) -> Result<[u8; 48], ContextProviderError> {
        let served = self.resolve(quorum_type, quorum_hash, height);
        // BL-162: which source answered, for `DW_LOG`/`DWCLI_LOG` diagnosis.
        match &served {
            Ok(s) => tracing::debug!(
                quorum_type,
                quorum = %hex::encode(quorum_hash),
                verified = s.verified,
                "quorum key served"
            ),
            Err(e) => tracing::debug!(
                quorum_type,
                quorum = %hex::encode(quorum_hash),
                error = %e,
                "quorum key refused"
            ),
        }
        tally(served.as_ref().is_ok_and(|s| s.verified));
        served.map(|s| s.key)
    }

    fn resolve(
        &self,
        quorum_type: u32,
        quorum_hash: [u8; 32],
        height: u32,
    ) -> Result<Served, ContextProviderError> {
        let quorum = (quorum_type, quorum_hash);
        let trusted = || self.trusted.quorum_key(quorum_type, quorum_hash, height);
        let Some(attached) = self.attached.get() else {
            return Err(ContextProviderError::Generic(
                "the quorum provider is not ready yet".into(),
            ));
        };
        if self.trusted_only() {
            return self.unverified(attached, trusted()?);
        }
        let spv = self.spv_answer(attached, quorum, height);
        let mode = self.mode();
        match (mode, spv) {
            (_, SpvAnswer::Invalid) => Err(ContextProviderError::InvalidQuorum(format!(
                "SPV marks quorum {} invalid",
                hex::encode(quorum_hash)
            ))),
            (TrustMode::Degraded, SpvAnswer::Verified(key)) => match trusted() {
                Ok(t) if t != key => Err(self.mismatch(quorum)),
                // The service is unreachable or agrees: SPV's verified key.
                _ => Ok(Served::by_spv(key)),
            },
            (TrustMode::Enforcing, SpvAnswer::Verified(key)) => Ok(Served::by_spv(key)),
            (TrustMode::Enforcing, _) if self.masternodes_synced() => {
                self.wait_for_verified(attached, quorum, height)
            }
            // Degraded, or Enforcing before masternode sync: the trusted key,
            // compared with what SPV holds.
            (_, held) => {
                let t = trusted()?;
                if matches!(mode, TrustMode::Enforcing) && self.masternodes_synced() {
                    // Synced while the service answered: no fallback now.
                    return self.wait_for_verified(attached, quorum, height);
                }
                if matches!(held, SpvAnswer::Held(k) if k != t) {
                    return Err(self.mismatch(quorum));
                }
                self.unverified(attached, t)
            }
        }
    }

    /// SPV's answer, from the cache for a verified key.
    fn spv_answer(&self, attached: &Attached, quorum: Quorum, height: u32) -> SpvAnswer {
        if let Some(key) = guard(&self.cache).get(&quorum) {
            return SpvAnswer::Verified(*key);
        }
        let lookup = attached.spv.lookup(quorum.0, quorum.1, height);
        let answer =
            block_on(bounded(lookup, self.spv_lookup_timeout)).unwrap_or(SpvAnswer::Absent);
        if let SpvAnswer::Verified(key) = answer {
            self.remember(quorum, key);
        }
        answer
    }

    fn remember(&self, quorum: Quorum, key: [u8; 48]) {
        let mut cache = guard(&self.cache);
        if cache.len() >= CACHE_CAP {
            cache.clear();
        }
        cache.insert(quorum, key);
    }

    /// Rule 3: once SPV has synced an unknown quorum never falls back. Waits
    /// for SPV to verify it, then fails closed.
    fn wait_for_verified(
        &self,
        attached: &Attached,
        quorum: Quorum,
        height: u32,
    ) -> Result<Served, ContextProviderError> {
        let unknown = || {
            ContextProviderError::InvalidQuorum(format!(
                "quorum {} is not verified by SPV after masternode sync",
                hex::encode(quorum.1)
            ))
        };
        if guard(&self.failed_closed)
            .get(&quorum)
            .is_some_and(|at| at.elapsed() < FAILED_CLOSED_FOR)
        {
            return Err(unknown());
        }
        let spv = Arc::clone(&attached.spv);
        let (wait, poll) = (self.unknown_quorum_wait, self.spv_poll);
        let limit = self.spv_lookup_timeout;
        let verified = block_on(async move {
            let deadline = tokio::time::Instant::now() + wait;
            loop {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                let lookup = spv.lookup(quorum.0, quorum.1, height);
                if let SpvAnswer::Verified(key) = bounded(lookup, limit.min(left)).await {
                    return Some(key);
                }
                if tokio::time::Instant::now() + poll > deadline {
                    return None;
                }
                tokio::time::sleep(poll).await;
            }
        })
        .flatten();
        match verified {
            Some(key) => {
                self.remember(quorum, key);
                guard(&self.failed_closed).remove(&quorum);
                Ok(Served::by_spv(key))
            }
            None => {
                guard(&self.failed_closed).insert(quorum, Instant::now());
                Err(unknown())
            }
        }
    }

    /// Serves a key SPV did not verify, once the fallback latch is durable
    /// (DEC-135, DEC-139). Otherwise refuses: the key is not served.
    fn unverified(
        &self,
        attached: &Attached,
        key: [u8; 48],
    ) -> Result<Served, ContextProviderError> {
        let Some(provenance) = attached.provenance.upgrade() else {
            return Err(ContextProviderError::Generic(
                "the trusted quorum key is refused: the session is closed".into(),
            ));
        };
        blocking(|| provenance.fallback_in_use()).map_err(|e| {
            tracing::warn!(error = %e, "the trust latch is not durable: trusted quorum key refused");
            ContextProviderError::Generic(format!(
                "the trusted quorum key is refused: the fallback latch is not durable ({e})"
            ))
        })?;
        Ok(Served::by_service(key))
    }

    /// Rule 4. The notice goes out once per quorum and session.
    fn mismatch(&self, quorum: Quorum) -> ContextProviderError {
        let detail = format!("LLMQ type {} quorum {}", quorum.0, hex::encode(quorum.1));
        if guard(&self.mismatched).insert(quorum) {
            tracing::error!(%detail, "SPV and the trusted service disagree on a quorum key");
            self.sink.emit(EngineEvent::Notice {
                network: Some(self.network.clone()),
                code: NoticeCode::PlatformTrustMismatch,
                detail: detail.clone(),
            });
        }
        ContextProviderError::InvalidQuorum(format!("{MISMATCH_MARKER}: {detail}"))
    }
}

/// Whether `e` is a rule-4 refusal from [`TrustProvider`].
pub(crate) fn is_trust_mismatch(e: &ContextProviderError) -> bool {
    matches!(e, ContextProviderError::InvalidQuorum(m) if m.starts_with(MISMATCH_MARKER))
}

/// The quorum lookups of one scoped fetch (design §4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    lookups: u32,
    unverified: u32,
}

impl Tally {
    /// Every quorum key the fetch used was SPV-verified, and it used one.
    pub(crate) fn verified(self) -> bool {
        self.lookups > 0 && self.unverified == 0
    }
}

tokio::task_local! {
    static SCOPE: RefCell<Tally>;
}

/// Runs `fut`, counting the quorum lookups its proofs make on this task.
/// A nested scope's lookups count in the enclosing one too. A lookup on
/// another task (one the SDK spawned) is not counted, so a fetch whose
/// proofs run elsewhere records nothing: it fails closed.
pub(crate) async fn scoped<F: Future>(fut: F) -> (F::Output, Tally) {
    let (out, inner) = SCOPE
        .scope(RefCell::new(Tally::default()), async move {
            let out = fut.await;
            (out, SCOPE.with(|t| *t.borrow()))
        })
        .await;
    let _ = SCOPE.try_with(|outer| {
        let mut outer = outer.borrow_mut();
        outer.lookups += inner.lookups;
        outer.unverified += inner.unverified;
    });
    (out, inner)
}

/// A quorum lookup, as a proof would make one, for a test's scripted fetch.
#[cfg(test)]
pub(crate) fn note_lookup(verified: bool) {
    tally(verified);
}

fn tally(verified: bool) {
    let _ = SCOPE.try_with(|t| {
        let mut t = t.borrow_mut();
        t.lookups += 1;
        if !verified {
            t.unverified += 1;
        }
    });
}

/// `fut`, or `Absent` once `limit` passes.
async fn bounded(fut: BoxFuture<SpvAnswer>, limit: Duration) -> SpvAnswer {
    tokio::time::timeout(limit, fut)
        .await
        .unwrap_or(SpvAnswer::Absent)
}

/// Drives `fut` to completion from synchronous code on a tokio worker (the
/// SDK's `ContextProvider` calls): `block_in_place` on a multi-thread
/// runtime, a thread of its own otherwise. `None` if that thread failed.
fn block_on<F>(fut: F) -> Option<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match multi_thread_handle() {
        Some(h) => Some(tokio::task::block_in_place(|| h.block_on(fut))),
        None => std::thread::scope(|s| {
            s.spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()
                    .map(|rt| rt.block_on(fut))
            })
            .join()
            .ok()
            .flatten()
        }),
    }
}

/// Runs blocking work (app.sqlite) from a tokio worker.
fn blocking<T>(f: impl FnOnce() -> T) -> T {
    match multi_thread_handle() {
        Some(_) => tokio::task::block_in_place(f),
        None => f(),
    }
}

/// The current runtime, if it is multi-threaded (`block_in_place` panics on
/// any other flavor).
fn multi_thread_handle() -> Option<tokio::runtime::Handle> {
    tokio::runtime::Handle::try_current()
        .ok()
        .filter(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
}

/// Handle given to `SdkBuilder::with_context_provider`: quorum keys from the
/// layered provider, contracts, token configurations and the activation
/// height from the trusted service (rule 6).
pub(crate) struct SharedContext {
    pub(crate) trusted: Arc<LazyTrustedContext>,
    pub(crate) trust: Arc<TrustProvider>,
}

impl ContextProvider for SharedContext {
    fn get_data_contract(
        &self,
        id: &dpp::prelude::Identifier,
        platform_version: &dpp::version::PlatformVersion,
    ) -> Result<Option<Arc<dpp::prelude::DataContract>>, ContextProviderError> {
        self.trusted
            .provider()?
            .get_data_contract(id, platform_version)
    }

    fn register_data_contract(&self, contract: Arc<dpp::prelude::DataContract>) {
        if let Ok(p) = self.trusted.provider() {
            p.register_data_contract(contract);
        }
    }

    fn get_token_configuration(
        &self,
        token_id: &dpp::prelude::Identifier,
    ) -> Result<Option<dpp::data_contract::TokenConfiguration>, ContextProviderError> {
        self.trusted.provider()?.get_token_configuration(token_id)
    }

    fn get_quorum_public_key(
        &self,
        quorum_type: u32,
        quorum_hash: [u8; 32],
        core_chain_locked_height: u32,
    ) -> Result<[u8; 48], ContextProviderError> {
        self.trust
            .quorum_public_key(quorum_type, quorum_hash, core_chain_locked_height)
    }

    fn get_platform_activation_height(
        &self,
    ) -> Result<dpp::prelude::CoreBlockHeight, ContextProviderError> {
        self.trusted.provider()?.get_platform_activation_height()
    }
}

impl crate::NetworkSession {
    /// Where Platform proofs get their quorum keys now (DASHPAY §2.2,
    /// Tools ▸ Information). In-memory read.
    pub fn quorum_source(&self) -> Result<QuorumSource, crate::EngineError> {
        let _op = self.try_enter()?;
        Ok(self.trust.source())
    }

    /// The developer toggle: quorum keys from the trusted service only.
    pub fn trusted_quorums_only(&self) -> Result<bool, crate::EngineError> {
        let _op = self.try_enter()?;
        Ok(self.trust.trusted_only())
    }

    /// Sets the developer toggle, stored in app.sqlite (global) and applied
    /// to this session at once; other open sessions take it when reopened.
    pub async fn set_trusted_quorums_only(&self, on: bool) -> Result<(), crate::EngineError> {
        let _op = self.enter().await?;
        let value = on.then(|| "1".to_string());
        self.appdb_op(move |db| {
            db.set_setting(dw_appdb::GLOBAL_SCOPE, TRUSTED_ONLY_KEY, value.as_deref())
        })
        .await?;
        self.trust.set_trusted_only(on);
        Ok(())
    }
}
