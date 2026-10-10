//! Registration: quote, the persisted state machine (§3.4), resume, discard
//! and stranded asset locks (DP1-02).

use std::future::Future;
use std::sync::Arc;

use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_appdb::{NewRegistration, RegistrationChange, RegistrationRow};
use serde::{Deserialize, Serialize};

use super::dashpay::DashPay;
use super::errors::{IdentityError, InvitationError, PlatformError, RegistrationError};
use super::flows::{BudgetPurpose, DispatchState, FlowKind, GrantRequest, LeaseStateView};
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
            move |session, _, lib| async move { discard(&session, wallet, &*lib, &draft).await },
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
            let spendable = lib.spendable().await;
            Ok(Plan {
                funding: Funding::CoreBalance { lock_duffs },
                lock_duffs,
                fee_duffs: flow::fee_bound_worst(spendable.inputs),
                credits,
                credits_held: lock_duffs.saturating_mul(flow::CREDITS_PER_DUFF),
                available_duffs: spendable.duffs,
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
/// can be resumed, or whose funds are committed. A flow that failed for
/// good with nothing committed (an invitation row restored here, a newer
/// build's row) does not.
fn in_progress(row: &RegistrationRow, locks: &[flow::LockInfo]) -> bool {
    match &row.error {
        None => row.phase != Phase::Done.as_str(),
        Some(_) => row.retryable || flow::funds_committed(row, locks, false),
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
    let flows = &session.platform.registrations;
    let _start = flows.start.lock().await;
    let rows = registration_rows(session, wallet).await?;
    let locks = lib.tracked_locks().await;
    if rows.iter().any(|r| in_progress(r, &locks)) {
        return Err(RegistrationError::InProgress);
    }
    let lease = authority(session, wallet, &grant).await?;
    if let Funding::CoreBalance { lock_duffs } = plan.funding {
        // The same check the funding step makes, before a row exists.
        let needed = lock_duffs.saturating_add(plan.fee_duffs);
        let remaining = lease
            .available(BudgetPurpose::Funding)
            .map_err(PlatformError::from)?;
        if needed > remaining {
            return Err(PlatformError::GrantExceeded {
                purpose: BudgetPurpose::Funding,
                needed,
                remaining,
            }
            .into());
        }
        if plan.available_duffs < needed {
            return Err(RegistrationError::FundingInsufficient {
                needed,
                available: plan.available_duffs,
            });
        }
    }
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
    let draft = session
        .appdb_op(move |db| db.insert_registration(&wallet_text, &row, now))
        .await
        .map_err(PlatformError::from)?;
    tracing::info!(draft, "registration started");
    flow::signal(session, wallet, draft);
    flow::spawn(session, wallet, lib, draft, Some(lease), true);
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

/// Fails a row this build cannot run (a newer encoding, an invitation
/// restored from another machine) so it stops counting as in progress.
/// Returns the row as it is now.
async fn sweep(
    session: &NetworkSession,
    wallet: WalletId,
    row: RegistrationRow,
) -> Result<RegistrationRow, PlatformError> {
    if row.error.is_some() {
        return Ok(row);
    }
    let code = match (Phase::parse(&row.phase), Funding::decode(&row.funding)) {
        (None, _) | (_, Decoded::Unsupported) => RegistrationError::Unsupported.code(),
        (_, Decoded::Known(Funding::Invitation { .. })) => InvitationError::Invalid.code(),
        _ => return Ok(row),
    };
    let (wallet, id, phase) = (wallet.to_string(), row.id, row.phase.clone());
    let change = RegistrationChange {
        failure: Some(Some((code.into(), false))),
        ..Default::default()
    };
    let now = crate::events::unix_now();
    Ok(session
        .appdb_op(move |db| {
            db.update_registration(&wallet, id, &phase, false, &change, now)?;
            db.registration_row(&wallet, id)
        })
        .await?
        .unwrap_or(row))
}

pub(crate) async fn statuses<L: RegistrationLib>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &L,
) -> Result<Vec<RegistrationStatus>, RegistrationError> {
    let locks = lib.tracked_locks().await;
    let mut spendable = None;
    let mut out = Vec::new();
    for row in registration_rows(session, wallet).await? {
        let row = sweep(session, wallet, row).await?;
        let state = session.platform.registrations.get((wallet, row.id));
        let phase = Phase::parse(&row.phase);
        let failure = match &row.error {
            Some(code) => {
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
                        None => *spendable.insert(lib.spendable().await),
                    };
                    needed = Some(lock_duffs.saturating_add(flow::fee_bound_worst(s.inputs)));
                    available = Some(s.duffs);
                }
                Some(RegistrationFailure {
                    phase: phase.map_or(RegistrationPhase::Draft, Phase::facade),
                    code: code.clone(),
                    retryable: row.retryable,
                    needed,
                    available,
                })
            }
            None => None,
        };
        let waiting = match (&failure, phase) {
            (Some(_), _) | (None, Some(Phase::Done)) => None,
            _ if state.wait.is_some() => state.wait,
            _ if state.running => None,
            _ => Some(idle_wait(session, lib, &row, phase, &locks)),
        };
        let holds_key = state
            .lease
            .and_then(|id| session.lease_table().lease(&id, &wallet).ok())
            .and_then(|lease| lease.view())
            .is_some_and(|v| {
                v.own_key
                    && matches!(
                        v.state,
                        LeaseStateView::Active | LeaseStateView::AwaitingProof
                    )
            });
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
            txid: flow::outpoint_of(&row).map(|o| o.txid.to_string()),
            waiting,
            holds_key,
            funds_committed: flow::funds_committed(&row, &locks, state.in_call),
            // DP1-04 seam: the contest's end from the vote poll.
            contest_ends_at: None,
            failure,
            created_at: row.created_at,
            updated_at: row.updated_at,
        });
    }
    Ok(out)
}

/// What a flow no task drives waits for: the sync hold before funding,
/// the ChainLock for a lock without a proof, else an authority.
fn idle_wait<L: RegistrationLib>(
    session: &NetworkSession,
    lib: &L,
    row: &RegistrationRow,
    phase: Option<Phase>,
    locks: &[flow::LockInfo],
) -> RegistrationWait {
    let unfunded = phase.is_some_and(Phase::unfunded);
    if unfunded && !lib.ready_to_fund() && row.identity.is_none() {
        return RegistrationWait::Sync;
    }
    let proofless = matches!(phase, Some(Phase::FundingSent | Phase::ProofWaiting))
        && flow::outpoint_of(row)
            .and_then(|op| locks.iter().find(|l| l.outpoint == op))
            .is_none_or(|l| !l.status.has_proof() && l.status != flow::LockStatus::Consumed);
    if proofless {
        return RegistrationWait::ChainLock;
    }
    flow::authority_wait(session)
}

pub(crate) async fn resume<L: RegistrationLib + 'static>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: Arc<L>,
    draft: &str,
    grant: Option<String>,
) -> Result<(), RegistrationError> {
    let row = sweep(
        session,
        wallet,
        registration_row(session, wallet, draft).await?,
    )
    .await?;
    if row.phase == Phase::Done.as_str() && row.error.is_none() {
        return Ok(());
    }
    let key = (wallet, row.id);
    let state = session.platform.registrations.get(key);
    if state.running {
        // A grant for a flow that waits keyless: its parked lease takes it
        // now (§4.3's rebind), so the flow signs when its proof arrives.
        if let (Some(grant), Some(id)) = (grant, state.lease)
            && let Ok(lease) = session.lease_table().lease(&id, &wallet)
            && lease.view().is_some_and(|v| {
                matches!(
                    v.state,
                    LeaseStateView::Parked { .. } | LeaseStateView::NeedsGrant
                )
            })
            && session.lease_for(&wallet, &grant).is_none()
        {
            session
                .rebind_lease(&lease, &[grant])
                .await
                .map_err(PlatformError::from)?;
        }
        return Ok(());
    }
    if let Some(code) = &row.error {
        let locks = lib.tracked_locks().await;
        // H12: a row is retried only when nothing of it can be on the wire,
        // or when its lock is now tracked (positive evidence it was sent,
        // so the flow adopts it instead of funding again).
        let resumable = row.retryable
            || (code == flow::broadcast_unknown()
                && matches!(
                    flow::funding_status(&row, &locks),
                    Some(DispatchState::Sent | DispatchState::WillBeSent)
                ));
        if !resumable {
            return Err(invalid(format!(
                "registration {draft} failed for good ({code})"
            )));
        }
        let (wallet_text, id, phase) = (wallet.to_string(), row.id, row.phase.clone());
        let change = RegistrationChange {
            failure: Some(None),
            ..Default::default()
        };
        let now = crate::events::unix_now();
        session
            .appdb_op(move |db| {
                db.update_registration(&wallet_text, id, &phase, true, &change, now)
            })
            .await
            .map_err(PlatformError::from)?;
    }
    let lease = match grant {
        Some(grant) => Some(authority(session, wallet, &grant).await?),
        None => None,
    };
    flow::spawn(session, wallet, lib, row.id, lease, true);
    Ok(())
}

pub(crate) async fn discard<L: RegistrationLib>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &L,
    draft: &str,
) -> Result<(), RegistrationError> {
    let row = registration_row(session, wallet, draft).await?;
    let flows = &session.platform.registrations;
    // Stops the flow unless a library call of it runs; atomic with the
    // driver's marking of its calls.
    if !flows.stop_unless_in_call((wallet, row.id)) {
        return Err(invalid("a registration call is running"));
    }
    let locks = lib.tracked_locks().await;
    if let Some(state) = flow::funding_status(&row, &locks)
        && state != DispatchState::NotSent
    {
        return Err(invalid("the registration's funds are committed"));
    }
    let (wallet_text, id) = (wallet.to_string(), row.id);
    session
        .appdb_op(move |db| db.delete_registration(&wallet_text, id))
        .await
        .map_err(PlatformError::from)?;
    flows.forget((wallet, row.id));
    flow::signal(session, wallet, row.id);
    tracing::info!(draft = row.id, "registration discarded");
    Ok(())
}

/// Tools ▸ Repair "Finish transfers": every registration lock the wallet
/// tracks and no finished flow consumed gets a row (adopting the flow that
/// holds it), and every funded flow runs once under `grant`. Builds no
/// lock and waits for no proof.
pub(crate) async fn finish<L: RegistrationLib>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &L,
    grant: String,
) -> Result<FinishReport, RegistrationError> {
    let lease = authority(session, wallet, &grant).await?;
    let wallet_text = wallet.to_string();
    for lock in lib.tracked_locks().await {
        if !lock.registration || lock.account != 0 || lock.status == flow::LockStatus::Consumed {
            continue;
        }
        let (w, outpoint, index) = (wallet_text.clone(), lock.outpoint, lock.identity_index);
        let funding = Funding::CoreBalance {
            lock_duffs: lock.amount,
        }
        .encode();
        let phase = if lock.status.has_proof() {
            Phase::ProofWaiting
        } else {
            Phase::FundingSent
        };
        let now = crate::events::unix_now();
        session
            .appdb_op(move |db| {
                if flow::note_lock_tracked(db, &w, outpoint, index, now)?.is_some() {
                    return Ok(());
                }
                let row = NewRegistration {
                    identity_index: Some(index),
                    identity: None,
                    // A stranded lock has no name to register.
                    label: String::new(),
                    temp_label: None,
                    funding,
                    initial_profile: None,
                    phase: phase.as_str().into(),
                };
                db.adopt_registration_lock(&w, &flow::canonical(outpoint), &row, now)?;
                Ok(())
            })
            .await
            .map_err(PlatformError::from)?;
    }
    let mut report = FinishReport {
        resumed: 0,
        completed: 0,
        still_pending: 0,
    };
    for row in registration_rows(session, wallet).await? {
        let funded = Phase::parse(&row.phase).is_some_and(|p| !p.unfunded() && p != Phase::Done);
        if !funded || row.error.as_ref().is_some_and(|_| !row.retryable) {
            continue;
        }
        let Some(_claim) = session.platform.registrations.claim((wallet, row.id)) else {
            report.still_pending += 1;
            continue;
        };
        if row.error.is_some() {
            let (w, id, phase) = (wallet_text.clone(), row.id, row.phase.clone());
            let change = RegistrationChange {
                failure: Some(None),
                ..Default::default()
            };
            let now = crate::events::unix_now();
            session
                .appdb_op(move |db| db.update_registration(&w, id, &phase, true, &change, now))
                .await
                .map_err(PlatformError::from)?;
        }
        report.resumed += 1;
        let driver = flow::Driver {
            session,
            wallet,
            lib,
            lease: Some(lease.clone()),
            wait_for_proof: false,
        };
        driver.run(row.id).await?;
        let done = registration_row(session, wallet, &row.id.to_string())
            .await
            .is_ok_and(|r| r.error.is_none() && r.phase == Phase::Done.as_str());
        if done {
            report.completed += 1;
        } else {
            report.still_pending += 1;
        }
    }
    Ok(report)
}
