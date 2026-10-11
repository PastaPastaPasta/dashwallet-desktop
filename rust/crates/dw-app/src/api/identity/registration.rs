//! Registration: quote, the persisted state machine, resume, discard and
//! stranded asset locks.

use dw_engine::platform as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::flows::GrantRequest;
use crate::dto::mirror_enum;

/// What to register. Passed to the quote and to the start alike.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationRequest {
    pub label: String,
    /// A non-contested name registered next to a contested `label`.
    pub temporary_label: Option<String>,
    pub funding: RegistrationFunding,
    pub initial_profile: Option<InitialProfile>,
}

impl From<RegistrationRequest> for engine::RegistrationRequest {
    fn from(r: RegistrationRequest) -> Self {
        let RegistrationRequest {
            label,
            temporary_label,
            funding,
            initial_profile,
        } = r;
        engine::RegistrationRequest {
            label,
            temporary_label,
            funding: funding.into(),
            initial_profile: initial_profile.map(Into::into),
        }
    }
}

/// The profile entered at the draft. `avatarCandidate` names a candidate
/// whose URL is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct InitialProfile {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar_candidate: Option<String>,
}

impl From<InitialProfile> for engine::InitialProfile {
    fn from(p: InitialProfile) -> Self {
        let InitialProfile {
            display_name,
            public_message,
            avatar_candidate,
        } = p;
        engine::InitialProfile {
            display_name,
            public_message,
            avatar_candidate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RegistrationFunding {
    CoreBalance,
    /// A `stashInvitation` id.
    Invitation { link_id: String },
    ExistingIdentity { identity: String },
    /// Developer builds only. `key` is a `prepareFaucetLock` key id and
    /// `proof` the hex `assetLockProof` the faucet returned for its public key.
    FaucetAssetLock { key: String, proof: String },
}

impl From<RegistrationFunding> for engine::RegistrationFunding {
    fn from(f: RegistrationFunding) -> Self {
        match f {
            RegistrationFunding::CoreBalance => engine::RegistrationFunding::CoreBalance,
            RegistrationFunding::Invitation { link_id } => {
                engine::RegistrationFunding::Invitation { link_id }
            }
            RegistrationFunding::ExistingIdentity { identity } => {
                engine::RegistrationFunding::ExistingIdentity { identity }
            }
            RegistrationFunding::FaucetAssetLock { key, proof } => {
                engine::RegistrationFunding::FaucetAssetLock { key, proof }
            }
        }
    }
}

/// An asset-lock key the engine derived for the faucet's
/// `POST /api/asset-lock-proof`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FaucetLockKey {
    pub key: String,
    /// The compressed secp256k1 public key, 66 hex characters.
    pub public_key: String,
}

impl From<engine::FaucetLockKey> for FaucetLockKey {
    fn from(k: engine::FaucetLockKey) -> Self {
        let engine::FaucetLockKey { key, public_key } = k;
        FaucetLockKey { key, public_key }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationQuote {
    pub contested: bool,
    /// Duffs locked into the asset lock.
    pub lock_duffs: u64,
    pub fee_duffs: u64,
    /// `lockDuffs + feeDuffs`: what the grant must cover.
    pub total_duffs: u64,
    /// Credits the identity keeps after the name and profile.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub remaining_credits: u64,
    /// The grant to ask for.
    pub grant: GrantRequest,
}

impl From<engine::RegistrationQuote> for RegistrationQuote {
    fn from(q: engine::RegistrationQuote) -> Self {
        let engine::RegistrationQuote {
            contested,
            lock_duffs,
            fee_duffs,
            total_duffs,
            remaining_credits,
            grant,
        } = q;
        RegistrationQuote {
            contested,
            lock_duffs,
            fee_duffs,
            total_duffs,
            remaining_credits,
            grant: grant.into(),
        }
    }
}

/// The registration state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RegistrationPhaseName {
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

mirror_enum!(engine::RegistrationPhase => RegistrationPhaseName {
    Draft, KeysPrepared, FundingSent, ProofWaiting, IdentityRegistered, NameRequested,
    NameRegistered, Contested, ProfileCreated, Done, Failed
});

/// What a parked or slow registration waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RegistrationWaitName {
    /// Parked keyless: "Unlock to finish". The resume needs a grant.
    Unlock,
    /// The grant died or its budget ran short: "Confirm to finish". The
    /// resume needs a grant.
    Authorize,
    Sync,
    InstantSend,
    ChainLock,
    Network,
}

mirror_enum!(engine::RegistrationWait => RegistrationWaitName {
    Unlock, Authorize, Sync, InstantSend, ChainLock, Network
});

/// Why a registration is `failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationFailure {
    /// The phase the flow stopped in.
    pub phase: RegistrationPhaseName,
    pub code: String,
    pub retryable: bool,
    /// `needed` and `available` of a `*_insufficient*` code, as decimal
    /// strings in that code's unit: duffs for `registration.funding_insufficient`,
    /// credits for `platform.insufficient_credits`.
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub needed: Option<u64>,
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub available: Option<u64>,
}

impl From<engine::RegistrationFailure> for RegistrationFailure {
    fn from(f: engine::RegistrationFailure) -> Self {
        let engine::RegistrationFailure {
            phase,
            code,
            retryable,
            needed,
            available,
        } = f;
        RegistrationFailure {
            phase: phase.into(),
            code,
            retryable,
            needed,
            available,
        }
    }
}

/// One persisted registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationStatus {
    pub draft: String,
    pub phase: RegistrationPhaseName,
    pub label: String,
    pub temporary_label: Option<String>,
    pub identity: Option<String>,
    pub txid: Option<String>,
    /// `resumeRegistration` needs a grant exactly when this is `unlock` or
    /// `authorize`.
    pub waiting: Option<RegistrationWaitName>,
    /// A lease holds a key for this flow ("Lock to cancel").
    pub holds_key: bool,
    /// A funding of the flow has committed and is not definitely unsent.
    pub funds_committed: bool,
    /// While `phase` is `contested`.
    pub contest_ends_at: Option<u64>,
    pub failure: Option<RegistrationFailure>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl From<engine::RegistrationStatus> for RegistrationStatus {
    fn from(s: engine::RegistrationStatus) -> Self {
        let engine::RegistrationStatus {
            draft,
            phase,
            label,
            temporary_label,
            identity,
            txid,
            waiting,
            holds_key,
            funds_committed,
            contest_ends_at,
            failure,
            created_at,
            updated_at,
        } = s;
        RegistrationStatus {
            draft,
            phase: phase.into(),
            label,
            temporary_label,
            identity,
            txid,
            waiting: waiting.map(Into::into),
            holds_key,
            funds_committed,
            contest_ends_at,
            failure: failure.map(Into::into),
            created_at,
            updated_at,
        }
    }
}

/// Tools ▸ Repair "Finish transfers".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FinishReport {
    pub resumed: u32,
    pub completed: u32,
    pub still_pending: u32,
}

impl From<engine::FinishReport> for FinishReport {
    fn from(r: engine::FinishReport) -> Self {
        let engine::FinishReport {
            resumed,
            completed,
            still_pending,
        } = r;
        FinishReport {
            resumed,
            completed,
            still_pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_parse_with_a_tagged_funding() {
        let r: RegistrationRequest = serde_json::from_str(
            r#"{"label":"alice","temporaryLabel":null,
                "funding":{"kind":"invitation","linkId":"abc"},
                "initialProfile":{"displayName":"Alice","publicMessage":null,"avatarCandidate":null}}"#,
        )
        .unwrap();
        let e = engine::RegistrationRequest::from(r);
        assert_eq!(
            e.funding,
            engine::RegistrationFunding::Invitation {
                link_id: "abc".into()
            }
        );
        assert_eq!(
            e.initial_profile.unwrap().display_name.as_deref(),
            Some("Alice")
        );
        let f: RegistrationFunding = serde_json::from_str(r#"{"kind":"coreBalance"}"#).unwrap();
        assert_eq!(
            engine::RegistrationFunding::from(f),
            engine::RegistrationFunding::CoreBalance
        );
    }

    #[test]
    fn a_failure_carries_its_figures_as_strings() {
        let f = RegistrationFailure::from(engine::RegistrationFailure {
            phase: engine::RegistrationPhase::FundingSent,
            code: "platform.insufficient_credits".into(),
            retryable: false,
            needed: Some(u64::MAX),
            available: None,
        });
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            serde_json::json!({
                "phase": "fundingSent",
                "code": "platform.insufficient_credits",
                "retryable": false,
                "needed": "18446744073709551615",
                "available": null,
            })
        );
    }

    #[test]
    fn waits_are_camel_case() {
        let w = RegistrationWaitName::from(engine::RegistrationWait::InstantSend);
        assert_eq!(serde_json::to_string(&w).unwrap(), r#""instantSend""#);
    }
}
