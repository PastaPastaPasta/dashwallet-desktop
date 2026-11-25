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

use std::future::Future;
use std::sync::Arc;

use dpp::address_funds::PlatformAddress;
use dpp::prelude::Identifier;
use dpp::version::PlatformVersion;
use dw_appdb::dispatch::{ArtifactKind, ArtifactStatus, ShieldedArtifact};
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
    /// The create executed and its identity is proved absent at or above
    /// that: Platform ran its fallback and refunded the payment, less a
    /// penalty, to the fallback address, whose record is written.
    Fallback,
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
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let key = dw_appdb::dp_registration_dispatched(&b58(&Identifier::new(identity)));
    #[cfg(test)]
    let failing = guard(&session.platform.registration_faults).dispatch_record;
    #[cfg(not(test))]
    let failing = false;
    let written = match failing {
        true => Err("injected".to_owned()),
        false => (session.appdb_op(move |db| db.set_setting_durable(&scope, &key, Some("1"))))
            .await
            .map_err(|e| e.to_string()),
    };
    let Err(e) = written else {
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

/// Platform's proved reads a Shielded-funded create is settled by.
pub(crate) trait CreateEvidence: Send + Sync {
    /// The height of a proved read at which every one of `nullifiers` is
    /// spent; `None` when one is not.
    fn spent_at(
        &self,
        nullifiers: &[[u8; 32]],
    ) -> impl Future<Output = Result<Option<u64>, PlatformError>> + Send;
    /// Whether identity `id` exists, by a proved read, with its height.
    fn identity(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<(bool, u64), PlatformError>> + Send;
}

/// [`CreateEvidence`] from Platform through the SDK, proved.
pub(crate) struct SdkEvidence(pub(crate) Arc<dash_sdk::Sdk>);

impl CreateEvidence for SdkEvidence {
    async fn spent_at(&self, nullifiers: &[[u8; 32]]) -> Result<Option<u64>, PlatformError> {
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
        let all = !nullifiers.is_empty() && nullifiers.iter().all(|n| spent.contains(n));
        Ok(all.then_some(at.height))
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
        return evidence(session, wallet, net, &ask, identity).await;
    }
    let Some(lease) = lease else {
        return Err(PlatformError::SignerUnavailable);
    };
    pool::unhooked(lib).await.map_err(platform_error)?;
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
    // No pass runs from here until the call's outcome is read (`pool::arm`).
    let hold = pool::hold_passes(session).await.map_err(platform_error)?;
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
    let name = b58(&Identifier::new(identity));
    if let Ok((true, _)) = net.identity(Identifier::new(identity)).await {
        return Ok(ShieldedOutcome::Registered(name));
    }
    evidence(session, wallet, net, &ask, &name).await
}

/// What the evidence proves of the create that named `identity` in `ask`'s
/// row (C-funding F12). Its artifact first: none means never sent, but
/// only while the row was named on this install and the hook never
/// recorded the dispatch (else the journal is lost or older); not executed
/// means nothing was spent. An executed one, its nullifiers proved spent,
/// then the identity: present, registered; absent at or above that height,
/// Platform's fallback, its refund's record written. Anything else, a read
/// failure included, is unknown.
async fn evidence(
    session: &NetworkSession,
    wallet: WalletId,
    net: &impl CreateEvidence,
    ask: &ShieldedAsk,
    identity: &str,
) -> Result<ShieldedOutcome, PlatformError> {
    let id = parse_identity(identity)?;
    let Ok(artifacts) = session.leases.shielded_artifacts(wallet).await else {
        return Ok(ShieldedOutcome::Unknown);
    };
    let ours = |a: &&ShieldedArtifact| {
        a.kind == Some(ArtifactKind::IdentityCreate) && a.identity_id == Some(id.to_buffer())
    };
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let keys = (
        dw_appdb::dp_registration_named(identity),
        dw_appdb::dp_registration_dispatched(identity),
    );
    let markers = session
        .appdb_op(move |db| Ok((db.setting(&scope, &keys.0)?, db.setting(&scope, &keys.1)?)))
        .await;
    let Some(artifact) = artifacts.iter().find(ours) else {
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
    match artifact.status {
        ArtifactStatus::NotExecuted => return Ok(ShieldedOutcome::Unsent),
        ArtifactStatus::Executed => {}
        _ => {
            return Ok(match net.identity(id).await {
                Ok((true, _)) => ShieldedOutcome::Registered(identity.to_owned()),
                _ => ShieldedOutcome::Unknown,
            });
        }
    }
    let Ok(Some(executed_by)) = net.spent_at(&artifact.published).await else {
        return Ok(ShieldedOutcome::Unknown);
    };
    match net.identity(id).await {
        Ok((true, _)) => Ok(ShieldedOutcome::Registered(identity.to_owned())),
        Ok((false, at)) if at >= executed_by => {
            let Some(st_hash) = artifact.st_hash else {
                return Ok(ShieldedOutcome::Unknown);
            };
            let named_at = match markers {
                Ok((Some(at), _)) => at.parse().unwrap_or(0),
                _ => 0,
            };
            super::platform_moves::arm_registration_refund(
                session,
                wallet,
                (artifact.activity, st_hash),
                (ask.draft, ask.fallback.clone(), ask.denomination),
                (named_at, executed_by),
            )
            .await?;
            Ok(ShieldedOutcome::Fallback)
        }
        _ => Ok(ShieldedOutcome::Unknown),
    }
}

/// Test faults of the create's hook steps.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct RegistrationFaults {
    /// The dispatch record after the artifact fails to write.
    pub(crate) dispatch_record: bool,
}
