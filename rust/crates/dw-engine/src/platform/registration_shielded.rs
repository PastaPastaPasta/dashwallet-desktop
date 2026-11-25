//! A registration's identity create from the wallet's Shielded balance
//! (C-funding F11–F16): the library's keyset create through the engine's
//! dispatch hook, under the wallet's shielded lock. The hook's accept names
//! the identity the create makes in the row, durably, before its dispatch
//! artifact is written, and records the dispatch after it; the step reads
//! the outcome from evidence alone (F12): the artifact, the nullifiers'
//! proved spent read, then a proved read of the identity at or above it.
//! A create Platform ran as its fallback refunds the payment, less a
//! penalty, to the wallet's own Platform address (F13), whose record is
//! written only once that is proved.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use dpp::address_funds::PlatformAddress;
use dpp::prelude::Identifier;
use dpp::version::PlatformVersion;
use dw_appdb::dispatch::{ArtifactKind, ArtifactSettled, ArtifactStatus, ShieldedArtifact};
use platform_wallet::wallet::shielded::{ShieldedDispatchKind, ShieldedDispatchPayload};

use super::errors::{PlatformError, ShieldedError};
use super::lease::Lease;
use super::registration::{ShieldedCheck, ShieldedFacts, shielded_readiness};
use super::registration_flow::{b58, parse_identity};
use super::registration_rows::{Decoded, Funding};
use super::requests::{leased, write_permit};
use super::runtime::guard;
use super::shielded_pool::{self as pool, ShieldedCreate, ShieldedLib, SpendError};
use crate::{NetworkSession, PlatformChange, WalletId};

/// One funding step of a Shielded-funded row: its draft and claim, the
/// identity slot, what its quote fixed, and the identity a create of it
/// named, if any.
#[derive(Debug, Clone)]
pub(crate) struct ShieldedAsk {
    pub(crate) draft: i64,
    pub(crate) owner: u64,
    pub(crate) index: u32,
    pub(crate) denomination: u64,
    /// The wallet's own Platform address (bech32m) fixed at the start.
    pub(crate) fallback: String,
    /// The identity (base58) the hook named in the row.
    pub(crate) identity: Option<String>,
}

/// What one funding step of a Shielded-funded row ended with.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ShieldedOutcome {
    /// The named identity exists, proved (base58).
    Registered(String),
    /// The create executed and the identity it named (base58) is proved
    /// absent at or above that: Platform ran its fallback and refunded the
    /// payment, less a penalty, to the fallback address, whose record is
    /// written.
    Fallback(String),
    /// The notes a create of two or more notes selected read spent and the
    /// identity it named (base58) reads absent at or above that: they are
    /// gone, by this create's fallback or by another spend of the same
    /// notes, which no proof tells apart. Its proof is written, so the
    /// library's hold on them is acknowledged; no refund is claimed.
    Consumed(String),
    /// It may still execute and no evidence says it did: the row waits.
    Unknown,
    /// The named create never reached the journal, so it was never sent:
    /// the name is cleared and the row creates again.
    Cleared,
    /// Its artifact reads not executed: nothing was spent. The name is
    /// cleared; the row fails, retryable, so nothing loops.
    Unsent,
    /// A readiness check of F16 does not pass now; nothing was signed.
    NotReady(ShieldedCheck, Option<u64>),
    /// Refused before the hook named anything: nothing was sent.
    Refused(ShieldedError),
    /// The library reported a create the hook never accepted: impossible
    /// under the hook, and never retried.
    Unaccepted,
}

/// The wallet's Shielded-funded create the library is about to sign: what
/// the hook's accept checks the signed transition against, and the
/// identity it named.
#[derive(Debug, Clone)]
pub(crate) struct CreateIntent {
    draft: i64,
    owner: u64,
    denomination: u64,
    fallback: PlatformAddress,
    /// The row's funding as the step read it: the hook replaces exactly it.
    funding: String,
    named: Option<[u8; 32]>,
}

impl CreateIntent {
    /// **Test only**: the intent of row `draft` held by `owner`.
    #[cfg(test)]
    pub(crate) fn new(
        (draft, owner): (i64, u64),
        denomination: u64,
        fallback: PlatformAddress,
        funding: String,
    ) -> Self {
        Self {
            draft,
            owner,
            denomination,
            fallback,
            funding,
            named: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn named(&self) -> Option<[u8; 32]> {
        self.named
    }
}

/// The wallet's [`CreateIntent`], standing while this lives.
struct IntentSlot<'a> {
    session: &'a NetworkSession,
    wallet: WalletId,
}

impl<'a> IntentSlot<'a> {
    fn set(session: &'a NetworkSession, wallet: WalletId, intent: CreateIntent) -> Self {
        guard(&session.platform.registration_intents).insert(wallet, intent);
        Self { session, wallet }
    }

    fn named(&self) -> Option<[u8; 32]> {
        (guard(&self.session.platform.registration_intents).get(&self.wallet)).and_then(|i| i.named)
    }
}

impl Drop for IntentSlot<'_> {
    fn drop(&mut self) {
        guard(&self.session.platform.registration_intents).remove(&self.wallet);
    }
}

/// The hook's accept of a spend of `payload`'s wallet (C-funding F11): with
/// no create armed, anything but a create, which the engine did not start;
/// with one armed, only that create, and only once the identity its signed
/// transition makes is named in the row, durably, with the activity's
/// height then. The identity it named. Any failure refuses the spend, so
/// nothing is sent.
pub(crate) async fn accept_create(
    session: &NetworkSession,
    payload: &ShieldedDispatchPayload,
) -> Result<Option<[u8; 32]>, String> {
    use dpp::serialization::PlatformDeserializableTrusted;
    use dpp::state_transition::StateTransition;
    use dpp::state_transition::state_transitions::shielded::identity_create_from_shielded_pool_transition::accessors::IdentityCreateFromShieldedPoolTransitionAccessorsV0;
    use dpp::state_transition::state_transitions::shielded::identity_create_from_shielded_pool_transition::derive_identity_id_from_actions;
    let wallet = WalletId(payload.wallet_id);
    let intent = guard(&session.platform.registration_intents)
        .get(&wallet)
        .cloned();
    let (identity, intent) = match (&payload.kind, intent) {
        (ShieldedDispatchKind::IdentityCreate { .. }, None) => {
            return Err("an identity create the engine did not start".into());
        }
        (_, None) => return Ok(None),
        (_, Some(intent)) if intent.named.is_some() => {
            return Err("the registration's create was accepted already".into());
        }
        (ShieldedDispatchKind::IdentityCreate { identity_id }, Some(intent)) => {
            (*identity_id, intent)
        }
        (_, Some(_)) => return Err("a registration's create is armed: only it is sent".into()),
    };
    let st = StateTransition::deserialize_from_bytes_trusted(&payload.st_bytes)
        .map_err(|e| format!("the create does not decode: {e}"))?;
    let StateTransition::IdentityCreateFromShieldedPool(create) = st else {
        return Err("the create's transition is not an identity create".into());
    };
    let id = create.identity_id();
    if create.denomination() != intent.denomination
        || *create.send_to_address_on_creation_failure() != intent.fallback
        || id.to_buffer() != identity
        || derive_identity_id_from_actions(create.actions()) != id
    {
        return Err("the create is not the one the engine accepted".into());
    }
    let Decoded::Known(Funding::Shielded {
        denomination,
        fallback,
        identity: None,
    }) = Funding::decode(&intent.funding)
    else {
        return Err("the registration's funding is not an unnamed Shielded one".into());
    };
    let name = b58(&id);
    let funding = Funding::Shielded {
        denomination,
        fallback,
        identity: Some(name.clone()),
    }
    .encode();
    let named_at = (super::platform_moves::activity_height(session, wallet).await)
        .map_err(|e| format!("the Platform activity's height could not be read: {e}"))?;
    let (w, expected, marker) = (
        wallet.to_string(),
        intent.funding.clone(),
        dw_appdb::dp_registration_named(&name),
    );
    let (draft, owner, now) = (intent.draft, intent.owner, crate::events::unix_now());
    let written = session
        .appdb_op(move |db| {
            db.set_registration_funding_durable(
                (&w, draft, owner),
                (&expected, &funding),
                (&marker, &named_at.to_string()),
                now,
            )
        })
        .await;
    match written {
        Ok(true) => {}
        Ok(false) => return Err("the registration moved before its create was named".into()),
        Err(e) => return Err(format!("the registration's create was not named: {e}")),
    }
    if let Some(armed) = guard(&session.platform.registration_intents).get_mut(&wallet) {
        armed.named = Some(identity);
    }
    Ok(Some(identity))
}

/// After the hook wrote the artifact of the create `st_hash` whose identity
/// [`accept_create`] named: records, durably, that the library may now send
/// it ([`dw_appdb::dp_registration_dispatched`]). A failure settles the
/// artifact `not_executed` and refuses the spend. Nothing to do for no
/// create.
pub(crate) async fn confirm_create(
    session: &NetworkSession,
    wallet: WalletId,
    named: Option<[u8; 32]>,
    st_hash: [u8; 32],
) -> Result<(), String> {
    let Some(identity) = named else {
        return Ok(());
    };
    let Err(e) = record_dispatch(session, wallet, identity).await else {
        return Ok(());
    };
    let settled = (session.leases)
        .settle_shielded_artifact(wallet, st_hash, ArtifactStatus::NotExecuted, None, false)
        .await;
    if let Err(e) = settled {
        tracing::warn!(wallet_id = %wallet, error = %e, "a refused create's artifact was not settled");
    }
    Err(format!(
        "the registration's create dispatch was not recorded: {e}"
    ))
}

/// Records, durably, that the create naming `identity` may leave
/// ([`dw_appdb::dp_registration_dispatched`]): by the hook's dispatch
/// ([`confirm_create`]) and again before a pass re-sends it, so a stop
/// between the artifact and the dispatch's record leaves no send without
/// it.
pub(crate) async fn record_dispatch(
    session: &NetworkSession,
    wallet: WalletId,
    identity: [u8; 32],
) -> Result<(), String> {
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let key = dw_appdb::dp_registration_dispatched(&b58(&Identifier::new(identity)));
    #[cfg(test)]
    if guard(&session.platform.registration_faults).dispatch_record {
        return Err("injected".to_owned());
    }
    (session.appdb_op(move |db| db.set_setting_durable(&scope, &key, Some("1"))))
        .await
        .map_err(|e| e.to_string())
}

/// A proved read of a create's published nullifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NullifierRead {
    /// Every one is spent, at the read's height.
    Spent(u64),
    /// None is, at the read's height.
    Unspent(u64),
    /// Some are.
    Partly,
}

/// Platform's proved reads a Shielded-funded create is settled by.
pub(crate) trait CreateEvidence: Send + Sync {
    /// Whether `nullifiers` are spent, by one proved read.
    fn nullifiers(
        &self,
        nullifiers: &[[u8; 32]],
    ) -> impl Future<Output = Result<NullifierRead, PlatformError>> + Send;
    /// Whether `anchor` is among the anchors Platform's pool still records,
    /// by a proved read, with its height. A pruned anchor never returns.
    fn anchor_recorded(
        &self,
        anchor: [u8; 32],
    ) -> impl Future<Output = Result<(bool, u64), PlatformError>> + Send;
    /// Whether identity `id` exists, by a proved read, with its height.
    fn identity(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<(bool, u64), PlatformError>> + Send;
}

/// [`CreateEvidence`] from Platform through the SDK, proved.
pub(crate) struct SdkEvidence(pub(crate) Arc<dash_sdk::Sdk>);

impl CreateEvidence for SdkEvidence {
    async fn nullifiers(&self, nullifiers: &[[u8; 32]]) -> Result<NullifierRead, PlatformError> {
        use dash_sdk::platform::Fetch;
        use dash_sdk::query_types::{ShieldedNullifierStatuses, ShieldedNullifiersQuery};
        if !self.0.prove() {
            return Err(PlatformError::Unavailable);
        }
        let query = ShieldedNullifiersQuery(nullifiers.to_vec());
        let (statuses, at) =
            ShieldedNullifierStatuses::fetch_with_metadata(&self.0, query, None).await?;
        let spent: std::collections::HashSet<[u8; 32]> = (statuses.unwrap_or_default().0)
            .into_iter()
            .filter(|s| s.is_spent)
            .map(|s| s.nullifier)
            .collect();
        Ok(
            match nullifiers.iter().filter(|n| spent.contains(*n)).count() {
                _ if nullifiers.is_empty() => NullifierRead::Partly,
                0 => NullifierRead::Unspent(at.height),
                n if n == nullifiers.len() => NullifierRead::Spent(at.height),
                _ => NullifierRead::Partly,
            },
        )
    }

    async fn anchor_recorded(&self, anchor: [u8; 32]) -> Result<(bool, u64), PlatformError> {
        use dash_sdk::platform::fetch_current_no_parameters::FetchCurrent;
        use dash_sdk::query_types::ShieldedAnchors;
        if !self.0.prove() {
            return Err(PlatformError::Unavailable);
        }
        let (ShieldedAnchors(anchors), at) =
            ShieldedAnchors::fetch_current_with_metadata(&self.0).await?;
        Ok((anchors.contains(&anchor), at.height))
    }

    async fn identity(&self, id: Identifier) -> Result<(bool, u64), PlatformError> {
        use dash_sdk::platform::Fetch;
        use dpp::identity::Identity;
        if !self.0.prove() {
            return Err(PlatformError::Unavailable);
        }
        let (identity, at) = Identity::fetch_with_metadata(&self.0, id, None).await?;
        Ok((identity.is_some(), at.height))
    }
}

/// A [`ShieldedError`] as the registration's error.
fn platform_error(e: ShieldedError) -> PlatformError {
    match e {
        ShieldedError::Platform(e) => e,
        _ => PlatformError::Unavailable,
    }
}

/// One funding step of `ask`'s row (C-funding F11, F12, F16), under the
/// wallet's shielded lock: a named row is settled by evidence alone; an
/// unnamed one checks readiness again, holds the passes and creates
/// through the hook, which names it. Signs only under `lease`.
pub(crate) async fn fund(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    (lib, net, version): (&impl ShieldedLib, &impl CreateEvidence, &PlatformVersion),
    ask: ShieldedAsk,
    lease: Option<&Lease>,
) -> Result<ShieldedOutcome, PlatformError> {
    let lock = pool::wallet_lock(session, wallet);
    let _spending = lock.lock().await;
    if let Some(identity) = &ask.identity {
        return evidence(session, wallet, (lib, net), &ask, identity).await;
    }
    let Some(lease) = lease else {
        return Err(PlatformError::SignerUnavailable);
    };
    pool::unhooked(lib).await.map_err(platform_error)?;
    // No pass runs from here until the call's outcome is read (`pool::arm`):
    // the notes and the rest clock read below are the ones the create
    // selects from.
    let hold = pool::hold_passes(session).await.map_err(platform_error)?;
    let mut facts = ShieldedFacts {
        platform_enabled: session.platform.enabled(),
        ..ShieldedFacts::default()
    };
    pool::registration_facts(session, wallet, lib, &mut facts).await?;
    let set = (version.drive_abci.validation_and_processing.event_constants)
        .shielded_identity_create_denominations;
    let denomination = set.contains(&ask.denomination).then_some(ask.denomination);
    let readiness = shielded_readiness(version, &facts, denomination, crate::events::unix_now());
    if let Some(check) = readiness.checks.iter().find(|c| !c.ready) {
        let ready_at = readiness
            .ready_at
            .filter(|_| check.check == ShieldedCheck::Rested);
        return Ok(ShieldedOutcome::NotReady(check.check, ready_at));
    }
    let fallback =
        super::platform_moves::parse(&ask.fallback).ok_or_else(|| PlatformError::Internal {
            detail: "the registration's fallback address does not parse".into(),
        })?;
    let expected = Funding::Shielded {
        denomination: ask.denomination,
        fallback: ask.fallback.clone(),
        identity: None,
    }
    .encode();
    let permit = write_permit(lease, "identity_create", ask.denomination)?;
    let slot = IntentSlot::set(
        session,
        wallet,
        CreateIntent {
            draft: ask.draft,
            owner: ask.owner,
            denomination: ask.denomination,
            fallback,
            funding: expected,
            named: None,
        },
    );
    let create = ShieldedCreate {
        index: ask.index,
        denomination: ask.denomination,
        fallback,
    };
    let sent = leased(lease, lib.identity_create(&permit, &create)).await;
    let named = slot.named();
    drop(slot);
    permit.finish(sent.is_ok());
    session
        .hub
        .pump
        .mark_platform(wallet, [PlatformChange::Shielded]);
    let Some(identity) = named else {
        return match sent {
            Err(SpendError::Lease(e)) => Err(e.into()),
            Err(SpendError::Library(e)) => Ok(ShieldedOutcome::Refused(pool::spend_error(*e))),
            Ok(()) => {
                tracing::error!(
                    draft = ask.draft,
                    "a Shielded-funded create returned unaccepted"
                );
                Ok(ShieldedOutcome::Unaccepted)
            }
        };
    };
    drop(hold);
    if sent.is_ok() {
        lib.sync_now().await;
    }
    let id = Identifier::new(identity);
    let name = b58(&id);
    if let Ok((true, _)) = net.identity(id).await {
        let proved = match session.leases.shielded_artifacts(wallet).await {
            Ok(artifacts) => match create_artifact(&artifacts, &id) {
                Some(artifact) => proved_executed(session, wallet, artifact).await,
                None => true,
            },
            Err(_) => false,
        };
        if proved {
            return Ok(ShieldedOutcome::Registered(name));
        }
    }
    // A create the hook named and then refused is not signed again at once.
    match evidence(session, wallet, (lib, net), &ask, &name).await? {
        ShieldedOutcome::Cleared if sent.is_err() => Ok(ShieldedOutcome::Unsent),
        outcome => Ok(outcome),
    }
}

/// What the evidence proves of the create that named `identity` in `ask`'s
/// row (C-funding F12). Its artifact first: none means never sent, but
/// only while the row was named on this install and the hook never
/// recorded the dispatch (else the journal is lost or older); not executed
/// means nothing was spent. An executed or unresolved one (the library
/// resolves every failure after the dispatch `unresolved`), every published
/// nullifier proved spent, then the identity: present, registered; absent
/// at or above that height, Platform's fallback, its refund's record
/// written. An unresolved one none of whose nullifiers is spent may be
/// proved never to execute ([`never_executes`]); one proved executed is
/// settled so in the journal, whose outcome is then acknowledged once its
/// notes read spent ([`pool::consume_dispositions`]). Anything else, a read
/// failure included, is unknown.
async fn evidence(
    session: &NetworkSession,
    wallet: WalletId,
    (lib, net): (&impl ShieldedLib, &impl CreateEvidence),
    ask: &ShieldedAsk,
    identity: &str,
) -> Result<ShieldedOutcome, PlatformError> {
    let id = parse_identity(identity)?;
    let Ok(artifacts) = session.leases.shielded_artifacts(wallet).await else {
        return Ok(ShieldedOutcome::Unknown);
    };
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let keys = (
        dw_appdb::dp_registration_named(identity),
        dw_appdb::dp_registration_dispatched(identity),
    );
    let markers = session
        .appdb_op(move |db| Ok((db.setting(&scope, &keys.0)?, db.setting(&scope, &keys.1)?)))
        .await;
    let Some(artifact) = create_artifact(&artifacts, &id) else {
        return Ok(match markers {
            Ok((Some(_), None)) => ShieldedOutcome::Cleared,
            // Named on another install (a restore): only the identity says.
            Ok((None, None)) => match net.identity(id).await {
                Ok((true, _)) => ShieldedOutcome::Registered(identity.to_owned()),
                _ => ShieldedOutcome::Unknown,
            },
            Ok((_, Some(_))) | Err(_) => ShieldedOutcome::Unknown,
        });
    };
    let named_at = match markers {
        Ok((Some(at), _)) => at.parse().unwrap_or(0),
        _ => 0,
    };
    let outcome = by_artifact(
        session,
        wallet,
        (lib, net),
        ask,
        (identity, id),
        (artifact, named_at),
    )
    .await?;
    let executed = matches!(
        outcome,
        ShieldedOutcome::Registered(_) | ShieldedOutcome::Fallback(_)
    );
    if executed && !proved_executed(session, wallet, artifact).await {
        return Ok(ShieldedOutcome::Unknown);
    }
    Ok(outcome)
}

/// Whether `artifact` published only its own notes' nullifiers: no padding.
fn unpadded(artifact: &ShieldedArtifact) -> bool {
    (artifact.published.iter()).all(|n| artifact.selected.iter().any(|(s, _)| s == n))
}

/// The artifact of the create naming identity `id`. The same notes signed
/// again (a bundle of two or more, which has no padding) name the same
/// identity: one that may still execute answers before any that never did.
fn create_artifact<'a>(
    artifacts: &'a [ShieldedArtifact],
    id: &Identifier,
) -> Option<&'a ShieldedArtifact> {
    let ours = |a: &&ShieldedArtifact| {
        a.kind == Some(ArtifactKind::IdentityCreate) && a.identity_id == Some(id.to_buffer())
    };
    let held = |a: &&ShieldedArtifact| ours(a) && a.status != ArtifactStatus::NotExecuted;
    (artifacts.iter().find(held)).or_else(|| artifacts.iter().find(ours))
}

/// Records the proof that `artifact`'s create executed ([`settle_proved`])
/// while its outcome is not acknowledged: in flight, unresolved, or settled
/// executed with no proof (a success the library resolves unattributable),
/// so the library's hold on its notes is acknowledged once they read spent
/// whatever the library resolves. `true` when it is written or nothing is
/// left to prove (acknowledged, settled not executed, or no transition
/// hash); `false` when the write failed: the row then waits on evidence.
async fn proved_executed(
    session: &NetworkSession,
    wallet: WalletId,
    artifact: &ShieldedArtifact,
) -> bool {
    let unacked = !artifact.acked
        && matches!(
            artifact.status,
            ArtifactStatus::Registered
                | ArtifactStatus::Unresolved { unrecorded: false }
                | ArtifactStatus::Executed
        );
    match (unacked, artifact.st_hash) {
        (true, Some(st_hash)) => {
            settle_proved(session, wallet, st_hash, ArtifactStatus::Executed, false).await
        }
        _ => true,
    }
}

/// [`evidence`] from the create's artifact.
async fn by_artifact(
    session: &NetworkSession,
    wallet: WalletId,
    (lib, net): (&impl ShieldedLib, &impl CreateEvidence),
    ask: &ShieldedAsk,
    (identity, id): (&str, Identifier),
    (artifact, named_at): (&ShieldedArtifact, u64),
) -> Result<ShieldedOutcome, PlatformError> {
    let present = async || match net.identity(id).await {
        Ok((true, _)) => ShieldedOutcome::Registered(identity.to_owned()),
        _ => ShieldedOutcome::Unknown,
    };
    let unresolved = match artifact.status {
        ArtifactStatus::NotExecuted => return Ok(ShieldedOutcome::Unsent),
        ArtifactStatus::Executed => false,
        ArtifactStatus::Unresolved { unrecorded: false } => true,
        _ => return Ok(present().await),
    };
    let executed_by = match net.nullifiers(&artifact.published).await {
        // A bundle's padding is spent by this create alone. A bundle of two
        // or more notes has none: its nullifiers spent may be another
        // spend's of the same notes (another install of the seed), so they
        // prove nothing of this create; only its identity does.
        Ok(NullifierRead::Spent(at)) if unresolved && unpadded(artifact) => {
            return Ok(consumed(session, wallet, net, (identity, id), (artifact, at)).await);
        }
        Ok(NullifierRead::Spent(at)) => at,
        Ok(NullifierRead::Unspent(_)) if unresolved => {
            return Ok(match present().await {
                ShieldedOutcome::Unknown => {
                    never_executes(session, wallet, (lib, net), artifact).await
                }
                registered => registered,
            });
        }
        _ if unresolved => return Ok(present().await),
        _ => return Ok(ShieldedOutcome::Unknown),
    };
    match net.identity(id).await {
        Ok((true, _)) => Ok(ShieldedOutcome::Registered(identity.to_owned())),
        Ok((false, at)) if at >= executed_by => {
            let Some(st_hash) = artifact.st_hash else {
                return Ok(ShieldedOutcome::Unknown);
            };
            super::platform_moves::arm_registration_refund(
                session,
                wallet,
                (artifact.activity, st_hash),
                (ask.draft, ask.fallback.clone(), ask.denomination),
                (named_at, executed_by),
            )
            .await?;
            Ok(ShieldedOutcome::Fallback(identity.to_owned()))
        }
        _ => Ok(ShieldedOutcome::Unknown),
    }
}

/// An unresolved create of two or more notes whose notes read spent at
/// `spent_at`: its identity present is [`ShieldedOutcome::Registered`];
/// read absent at or above `spent_at`, the notes are gone whoever spent
/// them, so its proof is written as consumed (its artifact stays
/// unresolved) and the library's hold is acknowledged once they read spent
/// ([`ShieldedOutcome::Consumed`]). Anything else waits.
async fn consumed(
    session: &NetworkSession,
    wallet: WalletId,
    net: &impl CreateEvidence,
    (identity, id): (&str, Identifier),
    (artifact, spent_at): (&ShieldedArtifact, u64),
) -> ShieldedOutcome {
    match net.identity(id).await {
        Ok((true, _)) => ShieldedOutcome::Registered(identity.to_owned()),
        Ok((false, at)) if at >= spent_at => {
            let unresolved = ArtifactStatus::Unresolved { unrecorded: false };
            match artifact.st_hash {
                Some(st_hash)
                    if settle_proved(session, wallet, st_hash, unresolved, false).await =>
                {
                    ShieldedOutcome::Consumed(identity.to_owned())
                }
                _ => ShieldedOutcome::Unknown,
            }
        }
        _ => ShieldedOutcome::Unknown,
    }
}

/// The engine's proof of a create the library left unresolved, as the
/// value of [`dw_appdb::dp_registration_proved`]: executed, never executes,
/// or its notes spent with no identity (it stays unresolved).
const PROVED: [(ArtifactStatus, &str); 3] = [
    (ArtifactStatus::Executed, "executed"),
    (ArtifactStatus::NotExecuted, "pruned"),
    (ArtifactStatus::Unresolved { unrecorded: false }, "consumed"),
];

/// Writes the engine's proof that create `st_hash` settles as `status`
/// durably, then settles its artifact so in the journal (`acked` when its
/// notes may go at once). The proof is the marker: the journal's
/// disposition columns hold only the library's strings, empty until the
/// next read of its outcomes writes them back. `false` when either write
/// failed.
pub(crate) async fn settle_proved(
    session: &NetworkSession,
    wallet: WalletId,
    st_hash: [u8; 32],
    status: ArtifactStatus,
    acked: bool,
) -> bool {
    let Some(&(_, value)) = PROVED.iter().find(|(s, _)| *s == status) else {
        return false;
    };
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let key = dw_appdb::dp_registration_proved(&hex::encode(st_hash));
    #[cfg(test)]
    if guard(&session.platform.registration_faults).proof_write {
        tracing::warn!("a Shielded-funded create's proof was not written (injected)");
        return false;
    }
    let marked = session
        .appdb_op(move |db| db.set_setting_durable(&scope, &key, Some(value)))
        .await;
    if let Err(e) = marked {
        tracing::warn!(error = %e, "a Shielded-funded create's proof was not written");
        return false;
    }
    let settled = (session.leases)
        .settle_shielded_artifact(wallet, st_hash, status, None, acked)
        .await;
    let written = matches!(settled, Ok(ArtifactSettled::Settled));
    if !written {
        tracing::warn!(?settled, "a Shielded-funded create's proof was not settled");
    }
    written
}

/// Of transitions `listed`, those the engine proved ([`settle_proved`]),
/// by the status proved: the library's later `unresolved` listing of one
/// does not undo it ([`pool::consume_dispositions`]). The marker counts
/// while the journal has the artifact in flight or unresolved (a crash
/// before its settlement) or settled as proved; a journal status alone,
/// the library's own, proves nothing. A read failure proves none.
pub(crate) async fn proved_creates(
    session: &NetworkSession,
    wallet: WalletId,
    artifacts: &[ShieldedArtifact],
    listed: &[[u8; 32]],
) -> HashMap<[u8; 32], ArtifactStatus> {
    let settled: Vec<([u8; 32], ArtifactStatus)> = (artifacts.iter())
        .filter(|a| {
            matches!(
                a.status,
                ArtifactStatus::Registered
                    | ArtifactStatus::Unresolved { unrecorded: false }
                    | ArtifactStatus::Executed
                    | ArtifactStatus::NotExecuted
            )
        })
        .filter_map(|a| Some((a.st_hash?, a.status)))
        .filter(|(st_hash, _)| listed.contains(st_hash))
        .collect();
    if settled.is_empty() {
        return HashMap::new();
    }
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let read = session
        .appdb_op(move |db| {
            let mut proved = HashMap::new();
            for (st_hash, status) in settled {
                let key = dw_appdb::dp_registration_proved(&hex::encode(st_hash));
                let value = db.setting(&scope, &key)?;
                let marked = PROVED.iter().find(|(_, v)| Some(*v) == value.as_deref());
                let in_flight = matches!(
                    status,
                    ArtifactStatus::Registered | ArtifactStatus::Unresolved { unrecorded: false }
                );
                if let Some(&(proved_as, _)) = marked
                    && (in_flight || proved_as == status)
                {
                    proved.insert(st_hash, proved_as);
                }
            }
            Ok(proved)
        })
        .await;
    read.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "the proved Shielded-funded creates could not be read");
        HashMap::new()
    })
}

/// An unresolved create none of whose published nullifiers is spent and
/// whose identity is absent. Its anchor read still recorded at height q
/// raises the height kept for it; read absent at p above the highest such
/// q (a node behind q proves nothing), then a proved read at or above p of
/// none spent, means it never executed and never can: a pruned anchor never
/// returns, and a create that executed did so below p, spending them. Its
/// artifact is settled not executed and the library's hold on its notes
/// lifted ([`released`]); otherwise it may still execute.
async fn never_executes(
    session: &NetworkSession,
    wallet: WalletId,
    (lib, net): (&impl ShieldedLib, &impl CreateEvidence),
    artifact: &ShieldedArtifact,
) -> ShieldedOutcome {
    let (Some(anchor), Some(st_hash)) = (artifact.anchor, artifact.st_hash) else {
        return ShieldedOutcome::Unknown;
    };
    let Ok((recorded, read_at)) = net.anchor_recorded(anchor).await else {
        return ShieldedOutcome::Unknown;
    };
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let key = dw_appdb::dp_registration_anchor_seen(&hex::encode(st_hash));
    let seen = session
        .appdb_op(move |db| {
            let seen = db
                .setting(&scope, &key)?
                .and_then(|v| v.parse::<u64>().ok());
            if recorded && seen.is_none_or(|s| s < read_at) {
                db.set_setting_durable(&scope, &key, Some(&read_at.to_string()))?;
            }
            Ok(seen)
        })
        .await;
    let pruned_by = match seen {
        Ok(Some(seen)) if !recorded && read_at > seen => read_at,
        _ => return ShieldedOutcome::Unknown,
    };
    match net.nullifiers(&artifact.published).await {
        Ok(NullifierRead::Unspent(at)) if at >= pruned_by => {}
        _ => return ShieldedOutcome::Unknown,
    }
    released(session, wallet, lib, st_hash).await
}

/// Settles `artifact` not executed in the journal, then acknowledges the
/// library's outcome of it, which frees its notes: unsent once the journal
/// holds it, else unknown. An acknowledgement that fails is repeated by the
/// next read of the outcomes ([`pool::consume_dispositions`]).
async fn released(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &impl ShieldedLib,
    st_hash: [u8; 32],
) -> ShieldedOutcome {
    if !settle_proved(session, wallet, st_hash, ArtifactStatus::NotExecuted, true).await {
        return ShieldedOutcome::Unknown;
    }
    if let Err(e) = lib.ack_spend(st_hash).await {
        tracing::warn!(error = %e, "a Shielded-funded create's notes stay held until the next read");
    }
    ShieldedOutcome::Unsent
}

/// Test faults of the create's hook steps.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct RegistrationFaults {
    /// The dispatch record after the artifact fails to write.
    pub(crate) dispatch_record: bool,
    /// The engine's proof of a create fails to write.
    pub(crate) proof_write: bool,
}
