//! Invitation links: status of a stashed link.

use dw_engine::platform as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::mirror_enum;

/// The identity that sent an invitation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct InvitationInviter {
    pub identity: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

impl From<engine::Counterparty> for InvitationInviter {
    fn from(c: engine::Counterparty) -> Self {
        let engine::Counterparty {
            identity,
            username,
            display_name,
            avatar_url,
        } = c;
        InvitationInviter {
            identity,
            username,
            display_name,
            avatar_url,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum InvitationInvalidReason {
    Malformed,
    WrongNetwork,
    AssetLockNotFound,
}

mirror_enum!(engine::InvitationInvalidReason => InvitationInvalidReason {
    Malformed, WrongNetwork, AssetLockNotFound
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum InvitationStatus {
    Valid {
        inviter: Option<InvitationInviter>,
        funding_duffs: u64,
        contested_allowed: bool,
        expires_at: Option<u64>,
    },
    Claimed,
    Invalid {
        reason: InvitationInvalidReason,
    },
    Expired,
}

impl From<engine::InvitationStatus> for InvitationStatus {
    fn from(s: engine::InvitationStatus) -> Self {
        use engine::InvitationStatus as E;
        match s {
            E::Valid {
                inviter,
                funding_duffs,
                contested_allowed,
                expires_at,
            } => InvitationStatus::Valid {
                inviter: inviter.map(Into::into),
                funding_duffs,
                contested_allowed,
                expires_at,
            },
            E::Claimed => InvitationStatus::Claimed,
            E::Invalid { reason } => InvitationStatus::Invalid {
                reason: reason.into(),
            },
            E::Expired => InvitationStatus::Expired,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_tagged_in_camel_case() {
        let s = InvitationStatus::from(engine::InvitationStatus::Valid {
            inviter: None,
            funding_duffs: 10,
            contested_allowed: true,
            expires_at: None,
        });
        assert_eq!(
            serde_json::to_value(s).unwrap(),
            serde_json::json!({
                "kind": "valid", "inviter": null, "fundingDuffs": 10,
                "contestedAllowed": true, "expiresAt": null
            })
        );
        let s = InvitationStatus::from(engine::InvitationStatus::Invalid {
            reason: engine::InvitationInvalidReason::WrongNetwork,
        });
        assert_eq!(
            serde_json::to_value(s).unwrap(),
            serde_json::json!({"kind": "invalid", "reason": "wrongNetwork"})
        );
    }
}
