//! The money-move trust gate (DASHPAY §2.2 rule 5; DP3-04,
//! `docs/design/DP3-04-trust-gate.md`). Once the fallback latch is set, a
//! contact payment or a top-up whose payee the library holds unverified
//! (no `dp_trust_verified` record of its current content, DEC-183) waits
//! while the gate re-fetches it against SPV-held quorum keys, and refuses
//! if it does not verify in time. In the degraded mode rule 5 is suspended:
//! one bounded round of re-fetches looks for a mismatch, which still
//! refuses, and the payment proceeds otherwise. The trusted-only toggle
//! proceeds.
//!
//! The gate records what verifies itself, as the background re-fetch
//! (`reverify.rs`) does: under the wallet manager's read lock, and only for
//! content the library still holds unchanged.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dpp::identity::accessors::IdentityGettersV0;
use dpp::prelude::Identifier;
use platform_wallet::key_wallet_manager::WalletManager;
use platform_wallet::{ManagedIdentity, PlatformWalletInfo};
use tokio::sync::watch;
use tokio::time::Instant;

use super::contacts_model::ACCOUNT_OPS;
use super::errors::PlatformError;
use super::payment_locks::Pair;
use super::provenance::Provenance;
use super::reverify::{self, Check, Held, Refetcher, SdkRefetcher, ShownProfile};
use super::runtime::{LockView, guard};
use super::trust::Rule5;
use crate::{NetworkSession, WalletId};

/// How long money waits for its payee to verify (`Enforce`): half the
/// default grant life, so a grant authorized just before outlives it.
const WAIT: Duration = Duration::from_secs(60);
/// How often the wait re-fetches what is still unverified.
const RETRY: Duration = Duration::from_secs(3);
/// The degraded mode's one round of re-fetches, at most.
const PROBE: Duration = Duration::from_secs(10);

/// The gate's waits; tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Waits {
    pub(crate) wait: Duration,
    pub(crate) retry: Duration,
    pub(crate) probe: Duration,
}

impl Default for Waits {
    fn default() -> Self {
        Self {
            wait: WAIT,
            retry: RETRY,
            probe: PROBE,
        }
    }
}

/// The session's gate settings (`PlatformRuntime::money_gate`).
#[derive(Default)]
pub(crate) struct MoneyGate {
    waits: Mutex<Waits>,
    /// Tests: Platform's answers, instead of the SDK's.
    #[cfg(test)]
    refetcher: Mutex<Option<Arc<dyn Refetcher>>>,
    /// Tests: where the next contact payment is held.
    #[cfg(test)]
    pauses: Mutex<Vec<(Point, Arc<crate::coins::TestPause>)>>,
}

/// Tests: where a contact payment can be held.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Point {
    /// Right after its gate passed.
    Gated,
    /// After its address was reserved, before the re-check.
    Reserved,
}

impl MoneyGate {
    #[cfg(test)]
    pub(crate) fn set_waits(&self, waits: Waits) {
        *guard(&self.waits) = waits;
    }

    #[cfg(test)]
    pub(super) fn set_refetcher(&self, fetch: Arc<dyn Refetcher>) {
        *guard(&self.refetcher) = Some(fetch);
    }

    #[cfg(test)]
    pub(crate) fn pause_at(&self, point: Point, pause: Arc<crate::coins::TestPause>) {
        guard(&self.pauses).push((point, pause));
    }

    /// Tests: a contact payment at `point`; held there once if asked.
    #[cfg(test)]
    pub(crate) async fn reached(&self, point: Point) {
        let pause = {
            let mut pauses = guard(&self.pauses);
            let at = pauses.iter().position(|(p, _)| *p == point);
            at.map(|i| pauses.remove(i).1)
        };
        if let Some(pause) = pause {
            (pause.reached).store(true, std::sync::atomic::Ordering::SeqCst);
            pause.release.notified().await;
        }
    }

    fn refetcher(&self, sdk: Arc<dash_sdk::Sdk>) -> Arc<dyn Refetcher> {
        #[cfg(test)]
        if let Some(fetch) = guard(&self.refetcher).clone() {
            return fetch;
        }
        Arc::new(SdkRefetcher(sdk))
    }
}

/// What money moves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Subject {
    /// A contact payment from our `pair.0` to its contact `pair.1`.
    Payee(WalletId, Pair),
    /// A top-up of the wallet's own identity.
    Identity(WalletId, Identifier),
}

/// Why the gate refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Still unverified when the wait ended, or masternode sync did not
    /// finish within it.
    Unverified,
    /// SPV and the trusted service disagree on a quorum key a re-fetch
    /// needed (rule 4).
    Mismatch,
    /// The vault was locked during the wait.
    Locked,
    /// The session closed or another engine failure.
    Failed(PlatformError),
}

impl Refusal {
    /// The refusal as m4 §4 names it (design §6).
    pub(crate) fn platform(self) -> PlatformError {
        match self {
            Self::Unverified => PlatformError::ContextUnavailable,
            Self::Mismatch => PlatformError::TrustMismatch,
            Self::Locked => PlatformError::SignerUnavailable,
            Self::Failed(e) => e,
        }
    }
}

impl From<crate::EngineError> for Refusal {
    fn from(e: crate::EngineError) -> Self {
        Self::Failed(e.into())
    }
}

/// What the gate asks of the session; the tests script it.
pub(super) trait GateEnv: Send + Sync {
    /// The subject's entities the library holds unverified now; `None` if
    /// the library no longer holds the subject.
    fn pending(&self) -> impl Future<Output = Result<Option<Vec<Held>>, Refusal>> + Send;
    /// Records `verified` re-fetches the library still holds unchanged.
    fn record(&self, verified: Vec<Held>) -> impl Future<Output = ()> + Send;
    /// `Err` once the session is closing.
    fn open(&self) -> Result<(), Refusal>;
}

/// What the gate watches while it waits.
pub(super) struct Signals {
    pub(super) rule: Rule5,
    /// The vault's lock reports (`PlatformRuntime::lock`).
    pub(super) lock: watch::Receiver<LockView>,
}

/// The gate (design §3). `Ok` lets the money move.
pub(super) async fn run<E: GateEnv>(
    env: &E,
    fetch: impl FnOnce() -> Result<Arc<dyn Refetcher>, Refusal>,
    mut signals: Signals,
    waits: Waits,
) -> Result<(), Refusal> {
    let mut pending = match env.pending().await? {
        Some(pending) => pending,
        // The subject's own checks refuse it (design §3, last row).
        None if signals.rule != Rule5::Enforce => return Ok(()),
        None => return Err(Refusal::Unverified),
    };
    if pending.is_empty() || signals.rule == Rule5::Off {
        return Ok(());
    }
    let fetch = fetch()?;
    if signals.rule == Rule5::Probe {
        let deadline = Instant::now() + waits.probe;
        return round(env, &*fetch, &pending, deadline).await;
    }
    // Enforce: the provider serves SPV-verified keys only once SPV holds
    // them (before masternode sync too), so each round may verify.
    let deadline = Instant::now() + waits.wait;
    let locks = signals.lock.borrow().locks;
    loop {
        round(env, &*fetch, &pending, deadline).await?;
        if signals.lock.borrow().locks != locks {
            return Err(Refusal::Locked);
        }
        env.open()?;
        pending = env.pending().await?.ok_or(Refusal::Unverified)?;
        if pending.is_empty() {
            return Ok(());
        }
        if Instant::now() + waits.retry >= deadline {
            return Err(Refusal::Unverified);
        }
        // A record another task writes meanwhile counts at the next read.
        tokio::select! {
            () = tokio::time::sleep(waits.retry) => {}
            _ = signals.lock.wait_for(|v| v.locks != locks) => return Err(Refusal::Locked),
        }
        env.open()?;
        pending = env.pending().await?.ok_or(Refusal::Unverified)?;
        if pending.is_empty() {
            return Ok(());
        }
    }
}

/// The re-check after the reservation (design §4 step 5), under
/// `Enforce`: what became unverified since the gate (the payee replaced, a
/// shown name landed) gets one round of re-fetches, bounded like the
/// probe; anything still unverified then, or the subject gone, refuses.
pub(super) async fn recheck<E: GateEnv>(
    env: &E,
    fetch: impl FnOnce() -> Result<Arc<dyn Refetcher>, Refusal>,
    waits: Waits,
) -> Result<(), Refusal> {
    let pending = env.pending().await?.ok_or(Refusal::Unverified)?;
    if pending.is_empty() {
        return Ok(());
    }
    round(env, &*fetch()?, &pending, Instant::now() + waits.probe).await?;
    env.open()?;
    match env.pending().await?.ok_or(Refusal::Unverified)?.is_empty() {
        true => Ok(()),
        false => Err(Refusal::Unverified),
    }
}

/// One re-fetch of each of `pending`, each under the time left; records
/// what verified. A mismatch refuses at once.
async fn round<E: GateEnv>(
    env: &E,
    fetch: &dyn Refetcher,
    pending: &[Held],
    deadline: Instant,
) -> Result<(), Refusal> {
    let mut verified = Vec::new();
    for h in pending {
        env.open()?;
        let checked = tokio::time::timeout_at(deadline, reverify::check(h, fetch)).await;
        match checked {
            Ok(Check::Verified) => verified.push(h.clone()),
            Ok(Check::Mismatch) => return Err(Refusal::Mismatch),
            Ok(Check::Unverified) => {}
            Err(_) => break,
        }
    }
    if !verified.is_empty() {
        env.record(verified).await;
    }
    Ok(())
}

/// What a payment to `contact` rests on, as the contacts read model shows
/// it (DP2-01; design D1): the cached profile, each request of the relation
/// and the shown name. By the read model's precedence: established, then
/// ignored, then a pending incoming request, then a pending outgoing one.
pub(super) fn payee_held(
    m: &ManagedIdentity,
    contact: &Identifier,
    username: Option<&str>,
) -> Option<Vec<Held>> {
    let dp = m.dashpay();
    let requests = if let Some(c) = dp.established_contacts().get(contact) {
        vec![c.outgoing_request.clone(), c.incoming_request.clone()]
    } else if dp.ignored_senders().contains(contact) {
        Vec::new()
    } else {
        let incoming = dp.incoming_contact_requests().get(contact);
        vec![
            incoming
                .or_else(|| dp.sent_contact_requests().get(contact))?
                .clone(),
        ]
    };
    let cached = (dp.contact_profiles.get(contact)).and_then(|e| e.profile.as_ref());
    let profile = cached.map(|p| Held::Profile {
        id: *contact,
        shown: ShownProfile::of(p),
    });
    let label = username.map(|name| Held::label(name, *contact));
    Some(
        (profile.into_iter())
            .chain(requests.into_iter().map(Held::ContactRequest))
            .chain(label)
            .collect(),
    )
}

/// Design D12: the account a payment to `contact` derives its address
/// from was built from an older request than the one the gate verifies
/// (a rebuild deferred before a restart is lost with its queue), and no
/// queued op rebuilds it at the reservation. Until a DashPay sweep does,
/// the payee is not payable on verified data.
fn paying_account_stale(m: &ManagedIdentity, contact: &Identifier) -> bool {
    let dp = m.dashpay();
    let Some(c) = dp.established_contacts().get(contact) else {
        return false;
    };
    let rebuild_queued = (dp.pending_contact_crypto.iter())
        .any(|p| p.contact_id == *contact && ACCOUNT_OPS.contains(&p.op.kind()));
    c.external_account_reference != Some(c.incoming_request.account_reference) && !rebuild_queued
}

/// The gate's subject in the library: every entity it rests on.
struct SessionEnv<'a> {
    session: &'a NetworkSession,
    subject: Subject,
}

impl SessionEnv<'_> {
    fn held(&self, wm: &WalletManager<PlatformWalletInfo>) -> Option<Vec<Held>> {
        let (wallet, owner) = match self.subject {
            Subject::Payee(wallet, pair) => (wallet, pair.0),
            Subject::Identity(wallet, identity) => (wallet, identity),
        };
        let info = wm.get_wallet_info(&wallet.0)?;
        let m = (info.identity_manager.wallet_managed_identities(&wallet.0))
            .find(|m| m.identity.id() == owner)?;
        match self.subject {
            Subject::Payee(_, (_, contact)) if paying_account_stale(m, &contact) => None,
            Subject::Payee(_, (_, contact)) => {
                let username = self.session.platform.contacts.names.label(&contact);
                payee_held(m, &contact, username.as_deref())
            }
            Subject::Identity(..) => Some(vec![Held::Identity {
                id: owner,
                keys: m.identity.public_keys().clone(),
            }]),
        }
    }

    fn unverified(provenance: &Provenance, held: Vec<Held>) -> Vec<Held> {
        (held.into_iter())
            .filter(|h| provenance.is_unverified(&h.entity()))
            .collect()
    }
}

impl GateEnv for SessionEnv<'_> {
    async fn pending(&self) -> Result<Option<Vec<Held>>, Refusal> {
        let live = self.session.live()?;
        let wm = live.manager.wallet_manager_arc();
        let wm = wm.read().await;
        Ok((self.held(&wm)).map(|held| Self::unverified(&live.provenance, held)))
    }

    async fn record(&self, verified: Vec<Held>) {
        let Ok(live) = self.session.live() else {
            return;
        };
        let wm = live.manager.wallet_manager_arc();
        // Held across the writes: the library cannot change what is recorded.
        let wm = wm.read().await;
        let current = (self.held(&wm)).map(|held| Self::unverified(&live.provenance, held));
        let verified: Vec<&Held> = verified.iter().collect();
        let records = reverify::confirmed(&verified, &current.unwrap_or_default());
        let provenance = Arc::clone(&live.provenance);
        let written =
            tokio::task::spawn_blocking(move || reverify::record_all(&provenance, &records)).await;
        drop(wm);
        tracing::debug!(?written, subject = ?self.subject, "the money gate recorded verified fetches");
    }

    fn open(&self) -> Result<(), Refusal> {
        drop(self.session.try_enter()?);
        Ok(())
    }
}

impl NetworkSession {
    /// Rule 5's gate before money moves to `subject` (design §3): `Ok` once
    /// nothing it rests on is unverified, or the mode lets it move anyway.
    /// May wait (`Enforce`: up to a minute plus one quorum lookup's wait).
    pub(crate) async fn money_gate(&self, subject: Subject) -> Result<(), Refusal> {
        let signals = Signals {
            rule: self.trust.rule_5(),
            lock: self.platform.lock.subscribe(),
        };
        let waits = *guard(&self.platform.money_gate.waits);
        let env = SessionEnv {
            session: self,
            subject,
        };
        let gated = run(&env, || self.gate_refetcher(), signals, waits).await;
        if let Err(refusal) = &gated {
            tracing::info!(?subject, ?refusal, "the money gate refused");
        }
        gated
    }

    fn gate_refetcher(&self) -> Result<Arc<dyn Refetcher>, Refusal> {
        let sdk = self.live()?.manager.sdk_arc();
        Ok(self.platform.money_gate.refetcher(sdk))
    }

    /// The gate's re-check (design §4 step 5): `Ok(true)` once it checked
    /// `subject` under `Enforce`, `Ok(false)` when rule 5 does not apply.
    pub(crate) async fn money_recheck(&self, subject: Subject) -> Result<bool, Refusal> {
        if self.trust.rule_5() != Rule5::Enforce {
            return Ok(false);
        }
        let waits = *guard(&self.platform.money_gate.waits);
        let env = SessionEnv {
            session: self,
            subject,
        };
        let checked = recheck(&env, || self.gate_refetcher(), waits).await;
        if let Err(refusal) = &checked {
            tracing::info!(?subject, ?refusal, "the money gate's re-check refused");
        }
        checked.map(|()| true)
    }
}
