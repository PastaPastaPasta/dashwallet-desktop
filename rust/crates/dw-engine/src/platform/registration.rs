//! Registration: quote, the persisted state machine (§3.4), resume, discard
//! and stranded asset locks (DP1-02).

use std::future::Future;
use std::sync::Arc;

use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_appdb::{ContestWatch, NewRegistration, RegistrationChange, RegistrationRow};
use dw_vault::{GrantKind, GrantPurpose};
use serde::{Deserialize, Serialize};

use super::contest_watch::watch_of;
use super::dashpay::DashPay;
use super::errors::{IdentityError, PlatformError, RegistrationError};
use super::flows::{BudgetPurpose, FlowKind, GrantRequest, LeaseStateView};
use super::lease::Lease;
use super::names::valid_label;
use super::registration_flow::{self as flow, PwLib, RegistrationLib};
use super::registration_ledger::{Ledger, Standing};
use super::registration_rows::{Decoded, Funding, Phase, ProfileRecord};
use crate::session::Manager;
use crate::{EngineError, NetworkSession, WalletId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationRequest {
    pub label: String,
    pub temporary_label: Option<String>,
    pub funding: RegistrationFunding,
    pub initial_profile: Option<InitialProfile>,
}

/// The profile entered at Draft. Kept in `dp_registration.initial_profile`
/// until `ProfileCreated`, across restarts and restores.
/// `avatar_candidate` must name a candidate whose `url` is set (a `Url` or
/// `Gravatar` source, or a file after `upload_avatar`); the engine stores its
/// URL, hash and fingerprint, never the candidate id, and never uploads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialProfile {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar_candidate: Option<String>,
}

/// Persisted as `dp_registration.funding` in DP1-02's versioned encoding
/// (§3.4), never in this type's serde form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegistrationFunding {
    CoreBalance,
    /// A `stash_invitation` id.
    Invitation {
        link_id: String,
    },
    /// The id goes to the row's `identity` column.
    ExistingIdentity {
        identity: String,
    },
    /// Developer builds only. `key` is a `prepare_faucet_lock` key id and
    /// `proof` the hex `assetLockProof` the faucet returned for its public
    /// key. The engine re-derives the private key; nothing secret crosses
    /// the facade.
    FaucetAssetLock {
        key: String,
        proof: String,
    },
}

/// An asset-lock key the engine derived for the faucet's
/// `POST /api/asset-lock-proof`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaucetLockKey {
    pub key: String,
    /// The compressed secp256k1 public key, 66 hex characters.
    pub public_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationQuote {
    pub contested: bool,
    /// Duffs locked into the asset lock.
    pub lock_duffs: u64,
    pub fee_duffs: u64,
    /// `lock_duffs + fee_duffs`: what the grant must cover.
    pub total_duffs: u64,
    /// Credits the identity keeps after the name and profile.
    pub remaining_credits: u64,
    /// What the funding can spend now: the funding account's auto-selectable
    /// coins, the set `start_registration` checks `total_duffs` against
    /// (`registration.funding_insufficient`). 0 for funding that is not the
    /// Core balance.
    pub available_duffs: u64,
    /// The grant to ask for: `max_duffs` covers `total_duffs`, and
    /// `max_credits` the identity creation, name and profile.
    pub grant: GrantRequest,
}

/// One `dp_registration` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationStatus {
    pub draft: String,
    pub phase: RegistrationPhase,
    pub label: String,
    pub temporary_label: Option<String>,
    pub identity: Option<String>,
    pub txid: Option<String>,
    /// What a parked or slow flow waits for; `None` while it runs or after
    /// it ends. `resume_registration` needs a grant exactly when this is
    /// `Unlock` or `Authorize`.
    pub waiting: Option<RegistrationWait>,
    /// A lease holds a key for this flow ("Registration in progress — Lock
    /// to cancel").
    pub holds_key: bool,
    /// A driver runs the flow now (its task, or "Finish transfers"), and
    /// `waiting` is what that run waits for. Otherwise nothing moves the
    /// row until `resume_registration` claims it, and `waiting` is what the
    /// resumed run would wait for first (DP1-09 D26).
    pub running: bool,
    /// True once a funding of the flow has committed and is not definitely
    /// unsent (E0-04 §16.5, copied in the contract's §4.1). A live `Unsent`
    /// entry is not committed. "Funds committed — finishing".
    pub funds_committed: bool,
    /// While `phase` is `Contested`.
    pub contest_ends_at: Option<u64>,
    /// Set when `phase` is `Failed`.
    pub failure: Option<RegistrationFailure>,
    /// The code the name step ended with when the identity was registered
    /// but its name was not (`name.taken`, `name.contest_open`, …): the
    /// flow still ends `Done`, and the identity claims a name with
    /// `register_name`.
    pub name_failure: Option<String>,
    /// The same for the initial profile, which `update_profile` publishes.
    pub profile_failure: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

/// The §3.4 state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationPhase {
    Draft,
    KeysPrepared,
    FundingSent,
    ProofWaiting,
    IdentityRegistered,
    NameRequested,
    NameRegistered,
    Contested,
    ProfileCreated,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationWait {
    /// Parked keyless: "Unlock to finish". The resume needs a grant.
    Unlock,
    /// The vault is unlocked but the flow's grant died (an unlock, a scope
    /// or a passphrase change) or its budget ran short: "Confirm to
    /// finish". The resume needs a grant.
    Authorize,
    /// Held until SPV has synced: "Waiting for the network to sync".
    Sync,
    InstantSend,
    ChainLock,
    /// Platform or the peers are unreachable; retried on reconnect.
    Network,
    /// A funding call of the flow may have sent a lock this device cannot
    /// name (its signed record was lost): the flow waits until the lock is
    /// tracked or the funding is resolved unsent (DP1-02 §8, Q4).
    FundingUnknown,
    /// What holds the flow's index is a lock only wedged funding steps
    /// contest (DEC-198 am.1): "Finish transfers" recovers it, and the flow
    /// then funds at its own index.
    NeedsFinish,
    /// SPV has synced, but the wallet's DashPay bring-up has not answered
    /// which identity indexes the seed uses (§3.4 Restore rule 1): "Waiting
    /// for DashPay to start". A run waiting here has the bring-up run again,
    /// on a backoff.
    Startup,
}

/// Why a flow is `Failed`, with the parameters its code carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationFailure {
    /// The phase the flow stopped in.
    pub phase: RegistrationPhase,
    pub code: String,
    pub retryable: bool,
    /// `needed` and `available` of a `*_insufficient*` code.
    pub needed: Option<u64>,
    pub available: Option<u64>,
}

/// Tools ▸ Repair "Finish transfers".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishReport {
    pub resumed: u32,
    pub completed: u32,
    pub still_pending: u32,
}

impl DashPay {
    pub async fn registration_quote(
        &self,
        req: RegistrationRequest,
    ) -> Result<RegistrationQuote, RegistrationError> {
        self.on_lib(move |_, _, lib| async move { quote(&*lib, &req).await })
            .await
    }

    pub async fn start_registration(
        &self,
        req: RegistrationRequest,
        grant: String,
    ) -> Result<String, RegistrationError> {
        let wallet = self.wallet_id;
        self.on_lib(
            move |session, _, lib| async move { start(&session, wallet, lib, req, grant).await },
        )
        .await
    }

    /// [`Self::start_registration`] past earlier registrations whose
    /// funding is unknown (DEC-197, ownership model §8.1): `past` names
    /// them as `registration.in_progress` does. Call it only after the user
    /// confirmed that each one's payment may still confirm, and that its
    /// funds then show up as a lock the wallet can recover later (DEC-198:
    /// nothing attaches it to its registration). Each must be blocked
    /// only by its unknown funding, SPV caught up, and its funding released
    /// at least an hour ago; anything else still blocks.
    pub async fn start_registration_past_unknown_funding(
        &self,
        req: RegistrationRequest,
        grant: String,
        past: Vec<String>,
    ) -> Result<String, RegistrationError> {
        let wallet = self.wallet_id;
        self.on_lib(move |session, _, lib| async move {
            start_past(&session, wallet, lib, req, grant, &past).await
        })
        .await
    }

    pub async fn registrations(&self) -> Result<Vec<RegistrationStatus>, RegistrationError> {
        let wallet = self.wallet_id;
        self.on_lib(move |session, _, lib| async move { statuses(&session, wallet, &*lib).await })
            .await
    }

    pub async fn resume_registration(
        &self,
        draft: String,
        grant: Option<String>,
    ) -> Result<(), RegistrationError> {
        let wallet = self.wallet_id;
        self.on_lib(move |session, _, lib| async move {
            resume(&session, wallet, lib, &draft, grant).await
        })
        .await
    }

    pub async fn discard_registration(&self, draft: String) -> Result<(), RegistrationError> {
        let wallet = self.wallet_id;
        self.on_lib(
            move |session, _, lib| async move { discard(&session, wallet, lib, &draft).await },
        )
        .await
    }

    /// Also applies the wallet's proved top-up locks that Platform has not
    /// taken (DP1-06, `topup.rs`).
    pub async fn finish_asset_locks(
        &self,
        grant: String,
    ) -> Result<FinishReport, RegistrationError> {
        let wallet = self.wallet_id;
        self.on_lib(move |session, _, lib| async move {
            let lease = authority(&session, wallet, &grant).await?;
            let mut report = finish_under(&session, wallet, &*lib, &lease).await?;
            super::topup::finish(&session, wallet, &*lib, &lease, &mut report).await;
            Ok(report)
        })
        .await
    }

    /// No build is a developer build yet: the faucet stays off (contract
    /// §2.2).
    pub async fn prepare_faucet_lock(
        &self,
        grant: String,
    ) -> Result<FaucetLockKey, RegistrationError> {
        drop(grant);
        Err(faucet_off().into())
    }

    /// Runs `body` on the engine runtime with the wallet's library.
    async fn on_lib<T, F, Fut>(&self, body: F) -> Result<T, RegistrationError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<NetworkSession>, Arc<Manager>, Arc<PwLib>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, RegistrationError>> + Send + 'static,
    {
        let session = Arc::clone(&self.session);
        let wallet_id = self.wallet_id;
        self.session
            .rt
            .spawn(async move {
                let _op = session.enter().await.map_err(PlatformError::from)?;
                let (manager, lib) = PwLib::open(&session, wallet_id).await?;
                body(Arc::clone(&session), manager, Arc::new(lib)).await
            })
            .await
            .map_err(|e| PlatformError::from(EngineError::from(e)))?
    }
}

fn faucet_off() -> PlatformError {
    PlatformError::FeatureOff {
        feature: "faucet".into(),
    }
}

fn invalid(detail: impl Into<String>) -> RegistrationError {
    PlatformError::InvalidArgument {
        detail: detail.into(),
    }
    .into()
}

/// What a request costs, as both the quote and the start compute it.
struct Plan {
    funding: Funding,
    contested: bool,
    /// The temporary name, kept only for a contested label.
    temp_label: Option<String>,
    identity: Option<String>,
    profile: Option<ProfileRecord>,
    lock_duffs: u64,
    fee_duffs: u64,
    /// IdentityCreate (when funded here), the names and the profile.
    credits: u64,
    /// The credits the identity will hold before those writes.
    credits_held: u64,
    available_duffs: u64,
    /// Coins the funding could spend that the engine keeps from selection.
    protected_coins: usize,
}

/// `call` names the facade call in `platform.not_implemented`.
async fn plan<L: RegistrationLib>(
    lib: &L,
    req: &RegistrationRequest,
    call: &str,
) -> Result<Plan, RegistrationError> {
    let check = valid_label(&req.label)?;
    let temp_label = match (&req.temporary_label, check.contested) {
        (Some(temp), true) => {
            let temp_check = valid_label(temp)?;
            if temp_check.contested {
                return Err(invalid("the temporary name must not be contested"));
            }
            Some(temp.clone())
        }
        // Only a contested name waits for a vote.
        _ => None,
    };
    let profile = match &req.initial_profile {
        // DP4-02 seam: `prepare_avatar` resolves a candidate to its URL.
        Some(p) if p.avatar_candidate.is_some() => {
            return Err(invalid("an avatar candidate without a URL"));
        }
        Some(p) => Some(ProfileRecord::without_avatar(p)),
        None => None,
    };
    match &req.funding {
        // DP5-02 seam: the invitation-claim path.
        RegistrationFunding::Invitation { .. } => {
            return Err(PlatformError::NotImplemented { call: call.into() }.into());
        }
        RegistrationFunding::FaucetAssetLock { .. } => return Err(faucet_off().into()),
        RegistrationFunding::CoreBalance | RegistrationFunding::ExistingIdentity { .. } => {}
    }
    let mut credits = lib.name_credits(&req.label).await?;
    if let Some(temp) = &temp_label {
        credits = credits.saturating_add(lib.name_credits(temp).await?);
    }
    if profile.is_some() {
        credits = credits.saturating_add(flow::PROFILE_CREDITS);
    }
    let base = Plan {
        funding: Funding::ExistingIdentity,
        contested: check.contested,
        temp_label,
        identity: None,
        profile,
        lock_duffs: 0,
        fee_duffs: 0,
        credits,
        credits_held: 0,
        available_duffs: 0,
        protected_coins: 0,
    };
    match &req.funding {
        RegistrationFunding::CoreBalance => {
            let credits = credits.saturating_add(flow::IDENTITY_CREATE_CREDITS);
            let floor = if check.contested {
                flow::CONTESTED_LOCK_DUFFS
            } else {
                flow::LOCK_DUFFS
            };
            // A lock always covers what it is meant to pay for.
            let lock_duffs = floor.max(credits.div_ceil(flow::CREDITS_PER_DUFF));
            let spendable = lib.spendable().await?;
            Ok(Plan {
                funding: Funding::CoreBalance { lock_duffs },
                lock_duffs,
                fee_duffs: flow::fee_bound_worst(spendable.inputs),
                credits,
                credits_held: lock_duffs.saturating_mul(flow::CREDITS_PER_DUFF),
                available_duffs: spendable.duffs,
                protected_coins: spendable.protected,
                ..base
            })
        }
        RegistrationFunding::ExistingIdentity { identity } => {
            let id = Identifier::from_string(identity, Encoding::Base58)
                .map_err(|_| invalid("identity is not a base58 identifier"))?;
            let identity = flow::b58(&id);
            if !lib.identities().await.iter().any(|(_, i)| *i == identity) {
                return Err(PlatformError::Identity(IdentityError::NotFound).into());
            }
            let credits_held = lib.credit_balance(&identity).await?.unwrap_or(0);
            Ok(Plan {
                identity: Some(identity),
                credits_held,
                ..base
            })
        }
        RegistrationFunding::Invitation { .. } | RegistrationFunding::FaucetAssetLock { .. } => {
            unreachable!("refused above")
        }
    }
}

pub(crate) async fn quote<L: RegistrationLib>(
    lib: &L,
    req: &RegistrationRequest,
) -> Result<RegistrationQuote, RegistrationError> {
    let plan = plan(lib, req, "DashPay.registration_quote").await?;
    let total_duffs = plan.lock_duffs.saturating_add(plan.fee_duffs);
    Ok(RegistrationQuote {
        contested: plan.contested,
        lock_duffs: plan.lock_duffs,
        fee_duffs: plan.fee_duffs,
        total_duffs,
        remaining_credits: plan.credits_held.saturating_sub(plan.credits),
        available_duffs: plan.available_duffs,
        grant: GrantRequest {
            max_duffs: total_duffs,
            max_credits: plan.credits,
        },
    })
}

/// The wallet's registration ledger (DP1-02 §5), for the gates.
async fn ledger<L: RegistrationLib>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &L,
) -> Result<Ledger, PlatformError> {
    Ledger::load(session, wallet, lib).await
}

/// The Funding cap `grant` gives, read without redeeming it: a refused
/// start leaves the grant for the next try (as `register_name` does; a
/// top-up too).
pub(super) async fn funding_cap(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    grant: &str,
) -> Result<u64, PlatformError> {
    if let Some(lease) = session.lease_for(&wallet, grant) {
        return Ok(lease?.available(BudgetPurpose::Funding)?);
    }
    let vault = session.vault.clone();
    let grant = grant.to_owned();
    let purpose = tokio::task::spawn_blocking(move || {
        vault.grant_purpose(&grant, GrantKind::PlatformOp, Some(&wallet.0))
    })
    .await
    .map_err(|e| PlatformError::Internal {
        detail: e.to_string(),
    })?
    .map_err(PlatformError::from)?;
    match purpose {
        GrantPurpose::PlatformOp { max_duffs, .. } => Ok(max_duffs),
        _ => Err(PlatformError::GrantInvalid),
    }
}

/// A facade call's `grant`: a lease of this wallet, or a vault grant
/// redeemed into a new registration lease.
async fn authority(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    grant: &str,
) -> Result<Lease, RegistrationError> {
    let lease = match session.lease_for(&wallet, grant) {
        Some(lease) => lease,
        None => {
            session
                .begin_lease(wallet, FlowKind::Registration, &[grant.to_owned()])
                .await
        }
    };
    Ok(lease.map_err(PlatformError::from)?)
}

pub(crate) async fn start<L: RegistrationLib + 'static>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: Arc<L>,
    req: RegistrationRequest,
    grant: String,
) -> Result<String, RegistrationError> {
    start_past(session, wallet, lib, req, grant, &[]).await
}

pub(crate) async fn start_past<L: RegistrationLib + 'static>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: Arc<L>,
    req: RegistrationRequest,
    grant: String,
    past: &[String],
) -> Result<String, RegistrationError> {
    let plan = plan(&*lib, &req, "DashPay.start_registration").await?;
    let past = past
        .iter()
        .map(|p| past_draft(p).ok_or_else(|| invalid(format!("{p} names no registration"))))
        .collect::<Result<Vec<_>, _>>()?;
    // Checked here so a refused start keeps its grant, and again with the
    // insert, in the one job of the wallet's actor (DEC-173).
    let (locks, identities) = (lib.tracked_locks().await, lib.identities().await);
    let leases = Arc::clone(&session.leases);
    let (l, i, p) = (locks.clone(), identities.clone(), past.clone());
    let (blocking, passed) = flow::ledger_op(session, wallet, move |db, flows| {
        let ledger = Ledger::read(db, flows, &leases, wallet, l, i)?;
        Ok((ledger.blocking_past(&p), passed_releases(&ledger, &p)))
    })
    .await?;
    if let Some(draft) = blocking {
        return Err(RegistrationError::InProgress { draft });
    }
    may_pass(&*lib, &passed).await?;
    if let Funding::CoreBalance { lock_duffs } = plan.funding {
        // The same checks the funding step makes, before a row exists.
        let needed = lock_duffs.saturating_add(plan.fee_duffs);
        let remaining = funding_cap(session, wallet, &grant).await?;
        if needed > remaining {
            return Err(PlatformError::GrantExceeded {
                purpose: BudgetPurpose::Funding,
                needed,
                remaining,
            }
            .into());
        }
        if plan.protected_coins > 0 {
            return Err(RegistrationError::CoinsLocked);
        }
        if plan.available_duffs < needed {
            return Err(RegistrationError::FundingInsufficient {
                needed,
                available: plan.available_duffs,
            });
        }
    }
    let lease = authority(session, wallet, &grant).await?;
    let row = NewRegistration {
        identity_index: None,
        identity: plan.identity,
        label: req.label.clone(),
        temp_label: plan.temp_label,
        funding: plan.funding.encode(),
        initial_profile: plan.profile.as_ref().map(ProfileRecord::encode),
        phase: Phase::Draft.as_str().into(),
    };
    let wallet_text = wallet.to_string();
    let now = crate::events::unix_now();
    let (leases, checked) = (Arc::clone(&session.leases), passed.clone());
    let draft = flow::ledger_op(session, wallet, move |db, flows| {
        let ledger = Ledger::read(db, flows, &leases, wallet, locks, identities)?;
        // What passes now must be what was checked, with the release
        // checked: a step wedged or released again since is named as still
        // blocking.
        let unchecked = (passed_releases(&ledger, &past).into_iter())
            .find(|d| !checked.contains(d))
            .map(|(draft, _)| draft);
        if let Some(draft) = ledger.blocking_past(&past) {
            return Ok(Err(draft));
        }
        if let Some(draft) = unchecked {
            return Ok(Err(ledger.blocker_of(draft)));
        }
        db.insert_registration(&wallet_text, &row, now)
            .map(Ok)
            .map_err(flow::db_error)
    })
    .await?
    .map_err(|draft| RegistrationError::InProgress { draft })?;
    if passed.is_empty() {
        tracing::info!(draft, "registration started");
    } else {
        tracing::warn!(
            draft,
            ?passed,
            "registration started past unknown fundings (DEC-197)"
        );
    }
    flow::signal(session, wallet, draft);
    flow::start_run(session, wallet, lib, draft, Some(lease)).await?;
    Ok(draft.to_string())
}

/// The draft a `registration.in_progress` name gives: the draft id, or its
/// funding step id.
fn past_draft(name: &str) -> Option<i64> {
    let id = (name.strip_prefix("registration/"))
        .and_then(|rest| rest.strip_suffix("/funding"))
        .unwrap_or(name);
    id.parse().ok()
}

/// How long a funding step's release must stand before a start may pass
/// its unknown funding (DEC-197): a lock that was sent is most likely
/// InstantSend locked, and seen by a synced wallet, by then.
pub(crate) const PAST_UNKNOWN_FUNDING_AFTER: u64 = 3600;

/// The steps `ledger` lets a start pass with `past` confirmed, each with
/// its release time as its own view showed it (DEC-197).
fn passed_releases(ledger: &Ledger, past: &[i64]) -> Vec<(i64, Option<u64>)> {
    (ledger.passed(past).into_iter())
        .map(|draft| (draft, ledger.released_at(draft)))
        .collect()
}

/// Whether a start may pass the unknown fundings of `passed` (DEC-197): SPV
/// caught up, and each one's funding released an hour ago or more.
async fn may_pass<L: RegistrationLib>(
    lib: &L,
    passed: &[(i64, Option<u64>)],
) -> Result<(), RegistrationError> {
    if passed.is_empty() {
        return Ok(());
    }
    if !lib.spv_synced().await {
        return Err(RegistrationError::NotSynced);
    }
    let now = crate::events::unix_now();
    for &(draft, at) in passed {
        let wait = at.map(|at| (at + PAST_UNKNOWN_FUNDING_AFTER).saturating_sub(now));
        if wait != Some(0) {
            return Err(RegistrationError::FundingUnknownRecent {
                draft: draft.to_string(),
                wait_secs: wait,
            });
        }
    }
    Ok(())
}

async fn registration_row(
    session: &NetworkSession,
    wallet: WalletId,
    draft: &str,
) -> Result<RegistrationRow, RegistrationError> {
    let id: i64 = draft.parse().map_err(|_| invalid("unknown draft"))?;
    let wallet = wallet.to_string();
    session
        .appdb_op(move |db| db.registration_row(&wallet, id))
        .await
        .map_err(PlatformError::from)?
        .ok_or_else(|| invalid("unknown draft"))
}

pub(crate) async fn statuses<L: RegistrationLib>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &L,
) -> Result<Vec<RegistrationStatus>, RegistrationError> {
    let ledger = ledger(session, wallet, lib).await?;
    let mut spendable = None;
    let mut watches = None;
    let mut out = Vec::new();
    for row in ledger.rows() {
        let state = session.platform.registrations.get((wallet, row.id));
        let phase = Phase::parse(&row.phase);
        let failure = match flow::failure_of(row) {
            Some((code, retryable)) => {
                let (mut needed, mut available) = (None, None);
                if code
                    == (RegistrationError::FundingInsufficient {
                        needed: 0,
                        available: 0,
                    })
                    .code()
                    && let Decoded::Known(Funding::CoreBalance { lock_duffs }) =
                        Funding::decode(&row.funding)
                {
                    let s = match spendable {
                        Some(s) => s,
                        None => *spendable.insert(lib.spendable().await.ok()),
                    };
                    if let Some(s) = s {
                        needed = Some(lock_duffs.saturating_add(flow::fee_bound_worst(s.inputs)));
                        available = Some(s.duffs);
                    }
                }
                Some(RegistrationFailure {
                    phase: phase.map_or(RegistrationPhase::Draft, Phase::facade),
                    code: code.to_owned(),
                    retryable,
                    needed,
                    available,
                })
            }
            None => None,
        };
        // An `Awaiting` row waits for its step's lock, failed or not (D5).
        let awaiting = ledger.standing(row) == Standing::Awaiting;
        let waiting = match (&failure, phase) {
            (Some(_), _) if !awaiting => None,
            (None, Some(Phase::Done)) => None,
            _ if state.running() => state.wait,
            _ => Some(idle_wait(session, wallet, lib, row, phase, &ledger)),
        };
        out.push(RegistrationStatus {
            draft: row.id.to_string(),
            phase: match (&failure, phase) {
                (Some(_), _) => RegistrationPhase::Failed,
                (None, Some(p)) => p.facade(),
                (None, None) => RegistrationPhase::Failed,
            },
            label: row.label.clone(),
            temporary_label: row.temp_label.clone(),
            identity: row.identity.clone(),
            txid: flow::outpoint_of(row).map(|o| o.txid.to_string()),
            waiting,
            holds_key: state
                .lease
                .as_ref()
                .is_some_and(|held| flow::holds_own_key(&held.0)),
            running: state.running(),
            funds_committed: ledger.funds_committed(row),
            contest_ends_at: match (&failure, phase, &row.identity) {
                (None, Some(Phase::Contested), Some(identity)) => {
                    if watches.is_none() {
                        watches = Some(contest_watches(session, wallet).await);
                    }
                    watch_of(watches.as_deref().unwrap_or_default(), identity, &row.label)
                        .map(|w| w.ends_at)
                        .filter(|&end| end > 0)
                }
                _ => None,
            },
            failure,
            name_failure: row.name_error.clone(),
            profile_failure: row.profile_error.clone(),
            created_at: row.created_at,
            updated_at: row.updated_at,
        });
    }
    // Q7: a funding step whose row is gone, read-only (no resume, no
    // discard), until its lock is attributed or it resolves.
    for (draft, unknown) in ledger.orphans() {
        out.push(RegistrationStatus {
            draft: flow::funding_step(draft),
            phase: RegistrationPhase::KeysPrepared,
            label: String::new(),
            temporary_label: None,
            identity: None,
            txid: ledger.orphan_txid(draft).map(|t| t.to_string()),
            waiting: Some(if unknown {
                RegistrationWait::FundingUnknown
            } else {
                RegistrationWait::ChainLock
            }),
            holds_key: false,
            running: false,
            funds_committed: true,
            contest_ends_at: None,
            failure: None,
            name_failure: None,
            profile_failure: None,
            created_at: 0,
            updated_at: 0,
        });
    }
    Ok(out)
}

/// The wallet's contest watch rows (DP1-04), which know each contest's
/// end; none when they cannot be read.
async fn contest_watches(session: &NetworkSession, wallet: WalletId) -> Vec<ContestWatch> {
    let w = wallet.to_string();
    (session
        .appdb_op(move |db| db.wallet_contest_watches(&w))
        .await)
        .unwrap_or_else(|e| {
            tracing::warn!(wallet_id = %wallet, error = %e, "could not read the contest watches");
            Vec::new()
        })
}

/// What a flow no task drives waits for: the funding hold before funding,
/// the ChainLock for a lock without a proof or for evidence at its index it
/// cannot attribute yet, else an authority. A row held on the bring-up has
/// it run again, as a run waiting there does: no run of the row's asks, and
/// the host prompts for none until the hold lifts.
fn idle_wait<L: RegistrationLib>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &L,
    row: &RegistrationRow,
    phase: Option<Phase>,
    ledger: &Ledger,
) -> RegistrationWait {
    // A step of the row's is outstanding and its lock is not here (D5).
    if ledger.standing(row) == Standing::Awaiting {
        return if ledger.funding_unknown(row) {
            RegistrationWait::FundingUnknown
        } else if ledger.awaits_discovery(row) {
            // §5.2a: its consumed lock's identity is not discovered yet.
            RegistrationWait::Network
        } else {
            RegistrationWait::ChainLock
        };
    }
    let unfunded = phase.is_some_and(Phase::unfunded) && row.identity.is_none();
    if unfunded && let Some(hold) = lib.funding_hold() {
        if hold == RegistrationWait::Startup {
            session.platform.retry_startup(wallet);
        }
        return hold;
    }
    if unfunded && ledger.live(row).is_none() && ledger.unattributed(row) {
        return ledger.unattributed_wait(row);
    }
    let proofless = matches!(phase, Some(Phase::FundingSent | Phase::ProofWaiting))
        && flow::outpoint_of(row)
            .and_then(|op| ledger.locks().iter().find(|l| l.outpoint == op))
            .is_none_or(|l| !l.status.has_proof() && l.status != flow::LockStatus::Consumed);
    if proofless {
        return RegistrationWait::ChainLock;
    }
    flow::authority_wait(session)
}

/// Clears the failure of `claim`'s row (a resume), keeping its phase, if
/// the row may run again (read and written in one job of the actor);
/// otherwise why not.
async fn clear_failure<L: RegistrationLib>(
    session: &NetworkSession,
    claim: &flow::Claim,
    wallet: WalletId,
    lib: &L,
) -> Result<Result<(), String>, PlatformError> {
    let (locks, identities) = (lib.tracked_locks().await, lib.identities().await);
    let (w, id, owner) = (wallet.to_string(), claim.draft(), claim.owner());
    let now = crate::events::unix_now();
    let leases = Arc::clone(&session.leases);
    flow::ledger_op(session, wallet, move |db, flows| {
        let ledger = Ledger::read(db, flows, &leases, wallet, locks, identities)?;
        let Some(row) = ledger.rows().iter().find(|r| r.id == id) else {
            return Ok(Err("unknown draft".into()));
        };
        let Some(code) = row.error.as_deref() else {
            return Ok(Ok(()));
        };
        if !ledger.resumable(row) {
            return Ok(Err(format!("registration {id} failed for good ({code})")));
        }
        let change = RegistrationChange {
            failure: Some(None),
            ..Default::default()
        };
        db.update_registration(&w, id, owner, &row.phase, true, &change, now)
            .map_err(flow::db_error)?;
        Ok(Ok(()))
    })
    .await
}

pub(crate) async fn resume<L: RegistrationLib + 'static>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: Arc<L>,
    draft: &str,
    grant: Option<String>,
) -> Result<(), RegistrationError> {
    let row = registration_row(session, wallet, draft).await?;
    if let Err(code) = flow::interpret(&row) {
        return Err(invalid(format!(
            "registration {draft} failed for good ({code})"
        )));
    }
    if row.phase == Phase::Done.as_str() && row.error.is_none() {
        return Ok(());
    }
    let key = (wallet, row.id);
    let flows = &session.platform.registrations;
    let state = flows.get(key);
    let (mut grant, mut lease) = (grant, None);
    if state.running() {
        // The flow's run takes the grant (DEC-171: asked, not taken over):
        // a parked lease rebinds to it (§4.3); a run with no lease, or one
        // that no longer signs, is offered a new one, so it signs when its
        // proof arrives.
        let Some(grant) = grant.take() else {
            return Ok(());
        };
        let current = state
            .lease
            .as_ref()
            .and_then(|held| held.0.view().map(|v| (held.0.clone(), v.state)));
        match current {
            Some((_, LeaseStateView::Active | LeaseStateView::AwaitingProof)) => return Ok(()),
            Some((lease, LeaseStateView::Parked { .. } | LeaseStateView::NeedsGrant))
                if session.lease_for(&wallet, &grant).is_none() =>
            {
                session
                    .rebind_lease(&lease, &[grant])
                    .await
                    .map_err(PlatformError::from)?;
                return Ok(());
            }
            _ => match flows.offer(key, authority(session, wallet, &grant).await?) {
                Ok(()) => return Ok(()),
                // The run ended meanwhile: this resume runs the flow.
                Err(offered) => lease = Some(offered),
            },
        }
    }
    let Some(claim) = flows.claim(session, key).await? else {
        // Another run, or a discard deciding, holds it now.
        return Ok(());
    };
    if let Err(why) = clear_failure(session, &claim, wallet, &*lib).await? {
        return Err(invalid(why));
    }
    let lease = match (lease, grant) {
        (Some(lease), _) => Some(lease),
        (None, Some(grant)) => Some(authority(session, wallet, &grant).await?),
        (None, None) => None,
    };
    flow::spawn(session, lib, claim, lease);
    Ok(())
}

pub(crate) async fn discard<L: RegistrationLib + 'static>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: Arc<L>,
    draft: &str,
) -> Result<(), RegistrationError> {
    let row = registration_row(session, wallet, draft).await?;
    let key = (wallet, row.id);
    let flows = &session.platform.registrations;
    // Judged first without the flow, so a refusal leaves its run alone.
    let ledger = ledger(session, wallet, &*lib).await?;
    let Some(current) = ledger.rows().iter().find(|r| r.id == row.id) else {
        return Err(invalid("unknown draft"));
    };
    if let Some(why) = ledger.discard_refused(current) {
        return Err(invalid(why));
    }
    // Then with the flow taken from its run (DEC-171): no call of it starts
    // and no write of the run applies from here on.
    let Some((claim, preempted)) = flows.take(session, key).await? else {
        return Err(invalid("a registration call is running"));
    };
    // Judged again on the rows read in the delete's own job (DEC-173).
    let (locks, identities) = (lib.tracked_locks().await, lib.identities().await);
    let (w, id, owner) = (wallet.to_string(), row.id, claim.owner());
    let leases = Arc::clone(&session.leases);
    let decided = flow::ledger_op(session, wallet, move |db, flows| {
        let ledger = Ledger::read(db, flows, &leases, wallet, locks, identities)?;
        let Some(row) = ledger.rows().iter().find(|r| r.id == id).cloned() else {
            return Ok(Err("unknown draft"));
        };
        // The claim taken holds no call: the run it took has none running.
        if let Some(why) = ledger.discard_refused(&row) {
            return Ok(Err(why));
        }
        let outpoint = row.asset_lock_outpoint.as_deref();
        if !db
            .delete_registration(&w, id, owner, &row.phase, outpoint)
            .map_err(flow::db_error)?
        {
            return Ok(Err("the registration changed meanwhile"));
        }
        flows.forget(key);
        Ok(Ok(()))
    })
    .await
    .map_err(RegistrationError::from)
    .and_then(|judged| judged.map_err(invalid));
    if let Err(e) = decided {
        // Refused (funded since the first look), or the decision failed:
        // the run it took goes on.
        drop(claim);
        if preempted.ran {
            flow::start_run(session, wallet, lib, row.id, preempted.lease).await?;
        }
        return Err(e);
    }
    flow::signal(session, wallet, row.id);
    tracing::info!(draft = row.id, "registration discarded");
    Ok(())
}

/// [`finish_under`] under the lease `grant` gives (the tests' entry).
#[cfg(test)]
pub(crate) async fn finish<L: RegistrationLib>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &L,
    grant: String,
) -> Result<FinishReport, RegistrationError> {
    let lease = authority(session, wallet, &grant).await?;
    finish_under(session, wallet, lib, &lease).await
}

/// Tools ▸ Repair "Finish transfers": every registration lock the wallet
/// tracks that no row holds and no identity of the wallet consumed gets a
/// row (the `KeysPrepared` row its known index names, else a new one at an
/// unused index), and every funded row runs once under `lease`. Builds no
/// lock and waits for no proof.
async fn finish_under<L: RegistrationLib>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &L,
    lease: &Lease,
) -> Result<FinishReport, RegistrationError> {
    let mut report = FinishReport {
        resumed: 0,
        completed: 0,
        still_pending: 0,
    };
    let (locks, identities) = (lib.tracked_locks().await, lib.identities().await);
    let held = lib.funding_hold().is_some();
    let now = crate::events::unix_now();
    let leases = Arc::clone(&session.leases);
    let (pending, closed) = flow::ledger_op(session, wallet, move |db, flows| {
        flow::import_stranded(db, flows, &leases, wallet, locks, identities, held, now)
    })
    .await?;
    report.still_pending += pending;
    for draft in closed {
        flow::backup_again(session, wallet, draft);
        flow::signal(session, wallet, draft);
    }
    let ledger = ledger(session, wallet, lib).await?;
    for row in ledger.rows() {
        // DEC-171 (4): funded by its evidence, whatever its phase says.
        let Ok((phase, _)) = flow::interpret(row) else {
            continue;
        };
        let funded = row.asset_lock_outpoint.is_some() || !phase.unfunded();
        if !funded || phase == Phase::Done {
            continue;
        }
        if row.error.is_some() && !ledger.resumable(row) {
            continue;
        }
        // One row's storage failure leaves it pending and the others run.
        match finish_row(session, wallet, lib, lease, row.id).await {
            Ok(Finished::Skipped) => {}
            Ok(Finished::Held) => report.still_pending += 1,
            Ok(Finished::Ran { done }) => {
                report.resumed += 1;
                if done {
                    report.completed += 1;
                } else {
                    report.still_pending += 1;
                }
            }
            Ok(Finished::Failed(e)) => {
                tracing::warn!(draft = row.id, error = %e, "a registration's run failed in Finish transfers");
                report.resumed += 1;
                report.still_pending += 1;
            }
            Err(e) => {
                tracing::warn!(draft = row.id, error = %e, "Finish transfers could not run a registration");
                report.still_pending += 1;
            }
        }
    }
    Ok(report)
}

/// What "Finish transfers" did with one funded row.
enum Finished {
    /// Another claim holds it.
    Held,
    /// It failed for good: not run.
    Skipped,
    Ran {
        done: bool,
    },
    /// Its run ended with a storage error: still pending.
    Failed(PlatformError),
}

async fn finish_row<L: RegistrationLib>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &L,
    lease: &Lease,
    draft: i64,
) -> Result<Finished, RegistrationError> {
    let flows = &session.platform.registrations;
    let Some(claim) = flows.claim(session, (wallet, draft)).await? else {
        return Ok(Finished::Held);
    };
    if clear_failure(session, &claim, wallet, lib).await?.is_err() {
        return Ok(Finished::Skipped);
    }
    let driver = flow::Driver::new(session, lib, &claim, Some(lease.clone()), false);
    if let Err(e) = driver.run().await {
        return Ok(Finished::Failed(e));
    }
    let done = registration_row(session, wallet, &draft.to_string())
        .await
        .is_ok_and(|r| r.error.is_none() && r.phase == Phase::Done.as_str());
    Ok(Finished::Ran { done })
}
