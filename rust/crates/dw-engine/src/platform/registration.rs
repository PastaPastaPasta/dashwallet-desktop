//! Registration: quote, the persisted state machine (§3.4), resume, discard
//! and stranded asset locks (DP1-02).

use std::future::Future;
use std::sync::Arc;

use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_appdb::{NewRegistration, RegistrationChange, RegistrationRow};
use dw_vault::{GrantKind, GrantPurpose};
use serde::{Deserialize, Serialize};

use super::dashpay::DashPay;
use super::errors::{IdentityError, PlatformError, RegistrationError};
use super::flows::{BudgetPurpose, FlowKind, GrantRequest, LeaseStateView};
use super::lease::Lease;
use super::names::valid_label;
use super::registration_flow::{self as flow, PwLib, RegistrationLib};
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

    pub async fn finish_asset_locks(
        &self,
        grant: String,
    ) -> Result<FinishReport, RegistrationError> {
        let wallet = self.wallet_id;
        self.on_lib(
            move |session, _, lib| async move { finish(&session, wallet, &*lib, grant).await },
        )
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
                let manager = session.manager().map_err(PlatformError::from)?;
                let wallet = manager
                    .get_wallet(&wallet_id.0)
                    .await
                    .ok_or(PlatformError::WalletNotFound)?;
                let lib = Arc::new(PwLib {
                    session: Arc::clone(&session),
                    wallet_id,
                    wallet,
                    sdk: manager.sdk_arc(),
                });
                body(Arc::clone(&session), manager, lib).await
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
        grant: GrantRequest {
            max_duffs: total_duffs,
            max_credits: plan.credits,
        },
    })
}

/// A row that counts towards `registration.in_progress`: one that runs or
/// can be resumed, or whose funds are committed and have no identity yet.
/// A flow that failed for good with nothing committed (an invitation row
/// restored here, a newer build's row) does not, and neither does one
/// whose identity exists: its funds are that identity's (MJ-1).
fn in_progress(row: &RegistrationRow, ev: &flow::Evidence) -> bool {
    match flow::failure_of(row) {
        None => row.phase != Phase::Done.as_str(),
        Some((_, retryable)) => {
            retryable || (row.identity.is_none() && flow::funds_committed(row, ev, false))
        }
    }
}

fn any_in_progress(ev: &flow::Evidence) -> bool {
    ev.rows().iter().any(|r| in_progress(r, ev))
}

/// The wallet's evidence for the funding gates.
async fn evidence<L: RegistrationLib>(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &L,
) -> Result<flow::Evidence, PlatformError> {
    Ok(flow::Evidence::new(
        lib.tracked_locks().await,
        lib.identities().await,
        registration_rows(session, wallet).await?,
    ))
}

/// The Funding cap `grant` gives, read without redeeming it: a refused
/// start leaves the grant for the next try (as `register_name` does).
async fn funding_cap(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    grant: &str,
) -> Result<u64, RegistrationError> {
    if let Some(lease) = session.lease_for(&wallet, grant) {
        let lease = lease.map_err(PlatformError::from)?;
        return Ok(lease
            .available(BudgetPurpose::Funding)
            .map_err(PlatformError::from)?);
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
        _ => Err(PlatformError::GrantInvalid.into()),
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
    let plan = plan(&*lib, &req, "DashPay.start_registration").await?;
    // Checked here so a refused start keeps its grant, and again with the
    // insert, in the one job of the wallet's actor (DEC-173).
    let (locks, identities) = (lib.tracked_locks().await, lib.identities().await);
    let rows = registration_rows(session, wallet).await?;
    let ev = flow::Evidence::new(locks.clone(), identities.clone(), rows);
    if any_in_progress(&ev) {
        return Err(RegistrationError::InProgress);
    }
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
    let draft = flow::rows_op(session, wallet, move |db, _| {
        let ev = flow::Evidence::new(locks, identities, db.registration_rows(&wallet_text)?);
        if any_in_progress(&ev) {
            return Ok(None);
        }
        db.insert_registration(&wallet_text, &row, now).map(Some)
    })
    .await?
    .ok_or(RegistrationError::InProgress)?;
    tracing::info!(draft, "registration started");
    flow::signal(session, wallet, draft);
    flow::start_run(session, wallet, lib, draft, Some(lease)).await?;
    Ok(draft.to_string())
}

async fn registration_rows(
    session: &NetworkSession,
    wallet: WalletId,
) -> Result<Vec<RegistrationRow>, PlatformError> {
    let wallet = wallet.to_string();
    Ok(session
        .appdb_op(move |db| db.registration_rows(&wallet))
        .await?)
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
    let ev = evidence(session, wallet, lib).await?;
    let mut spendable = None;
    let mut out = Vec::new();
    for row in ev.rows() {
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
        let waiting = match (&failure, phase) {
            (Some(_), _) | (None, Some(Phase::Done)) => None,
            _ if state.running() => state.wait,
            _ => Some(idle_wait(session, lib, row, phase, &ev)),
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
            funds_committed: flow::funds_committed(row, &ev, state.in_call),
            // DP1-04 seam: the contest's end from the vote poll.
            contest_ends_at: None,
            failure,
            name_failure: row.name_error.clone(),
            profile_failure: row.profile_error.clone(),
            created_at: row.created_at,
            updated_at: row.updated_at,
        });
    }
    Ok(out)
}

/// What a flow no task drives waits for: the funding hold before funding,
/// the ChainLock for a lock without a proof or for evidence at its index it
/// cannot attribute yet, else an authority.
fn idle_wait<L: RegistrationLib>(
    session: &NetworkSession,
    lib: &L,
    row: &RegistrationRow,
    phase: Option<Phase>,
    ev: &flow::Evidence,
) -> RegistrationWait {
    let unfunded = phase.is_some_and(Phase::unfunded) && row.identity.is_none();
    if unfunded && let Some(hold) = lib.funding_hold() {
        return hold;
    }
    if unfunded && row.asset_lock_outpoint.is_none() && ev.unattributed(row) {
        return RegistrationWait::ChainLock;
    }
    let proofless = matches!(phase, Some(Phase::FundingSent | Phase::ProofWaiting))
        && flow::outpoint_of(row)
            .and_then(|op| ev.locks.iter().find(|l| l.outpoint == op))
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
    flow::rows_op(session, wallet, move |db, _| {
        let ev = flow::Evidence::new(locks, identities, db.registration_rows(&w)?);
        let Some(row) = ev.rows().iter().find(|r| r.id == id) else {
            return Ok(Err("unknown draft".into()));
        };
        let Some(code) = row.error.as_deref() else {
            return Ok(Ok(()));
        };
        if !flow::resumable(row, &ev) {
            return Ok(Err(format!("registration {id} failed for good ({code})")));
        }
        let change = RegistrationChange {
            failure: Some(None),
            ..Default::default()
        };
        db.update_registration(&w, id, owner, &row.phase, true, &change, now)?;
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

/// Why a discard of `row` is refused, if it is.
fn discard_refused(
    row: &RegistrationRow,
    ev: &flow::Evidence,
    in_call: bool,
) -> Option<&'static str> {
    if flow::is_unsupported(row) {
        // DEC-171 (3): what this build cannot read, it does not delete.
        return Some("the registration was written by a newer version");
    }
    flow::funds_committed(row, ev, in_call).then_some("the registration's funds are committed")
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
    let ev = evidence(session, wallet, &*lib).await?;
    if let Some(why) = discard_refused(&row, &ev, flows.get(key).in_call) {
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
    let decided = flow::rows_op(session, wallet, move |db, flows| {
        let rows = db.registration_rows(&w)?;
        let Some(row) = rows.iter().find(|r| r.id == id).cloned() else {
            return Ok(Err("unknown draft"));
        };
        if let Some(why) =
            discard_refused(&row, &flow::Evidence::new(locks, identities, rows), false)
        {
            return Ok(Err(why));
        }
        let outpoint = row.asset_lock_outpoint.as_deref();
        if !db.delete_registration(&w, id, owner, &row.phase, outpoint)? {
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

/// Tools ▸ Repair "Finish transfers": every registration lock the wallet
/// tracks that no row holds and no identity of the wallet consumed gets a
/// row (the `KeysPrepared` row its known index names, else a new one at an
/// unused index), and every funded row runs once under `grant`. Builds no
/// lock and waits for no proof.
pub(crate) async fn finish<L: RegistrationLib>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &L,
    grant: String,
) -> Result<FinishReport, RegistrationError> {
    let lease = authority(session, wallet, &grant).await?;
    let mut report = FinishReport {
        resumed: 0,
        completed: 0,
        still_pending: 0,
    };
    let (locks, identities) = (lib.tracked_locks().await, lib.identities().await);
    let held = lib.funding_hold().is_some();
    let now = crate::events::unix_now();
    report.still_pending += flow::rows_op(session, wallet, move |db, flows| {
        flow::import_stranded(db, flows, wallet, locks, identities, held, now)
    })
    .await?;
    let ev = evidence(session, wallet, lib).await?;
    for row in ev.rows() {
        // DEC-171 (4): funded by its evidence, whatever its phase says.
        let Ok((phase, _)) = flow::interpret(row) else {
            continue;
        };
        let funded = row.asset_lock_outpoint.is_some() || !phase.unfunded();
        if !funded || phase == Phase::Done {
            continue;
        }
        if row.error.is_some() && !flow::resumable(row, &ev) {
            continue;
        }
        // One row's storage failure leaves it pending and the others run.
        match finish_row(session, wallet, lib, &lease, row.id).await {
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
    driver.run().await?;
    let done = registration_row(session, wallet, &draft.to_string())
        .await
        .is_ok_and(|r| r.error.is_none() && r.phase == Phase::Done.as_str());
    Ok(Finished::Ran { done })
}
