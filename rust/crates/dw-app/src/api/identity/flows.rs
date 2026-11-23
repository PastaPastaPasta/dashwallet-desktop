//! Grant sizes and the dispatch journal (E0-04 §4, §16).

use dw_engine::platform as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The caps of the `PlatformOp` grant an action needs: what to ask the
/// vault's grant prompt for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GrantRequest {
    /// Core duffs an asset lock may take.
    pub max_duffs: u64,
    /// Identity credits the state transitions may spend.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub max_credits: u64,
}

impl From<engine::GrantRequest> for GrantRequest {
    fn from(g: engine::GrantRequest) -> Self {
        let engine::GrantRequest {
            max_duffs,
            max_credits,
        } = g;
        GrantRequest {
            max_duffs,
            max_credits,
        }
    }
}

/// A write whose grant `grantRequest` sizes. Registration, top-up and
/// withdraw carry theirs in their quotes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GrantAction {
    SendRequest,
    AcceptRequest,
    RegisterName { label: String },
    UpdateProfile,
    PublishPrivateDetails,
    EnableDashPayKeys,
}

impl From<GrantAction> for engine::GrantAction {
    fn from(a: GrantAction) -> Self {
        match a {
            GrantAction::SendRequest => engine::GrantAction::SendRequest,
            GrantAction::AcceptRequest => engine::GrantAction::AcceptRequest,
            GrantAction::RegisterName { label } => engine::GrantAction::RegisterName { label },
            GrantAction::UpdateProfile => engine::GrantAction::UpdateProfile,
            GrantAction::PublishPrivateDetails => engine::GrantAction::PublishPrivateDetails,
            GrantAction::EnableDashPayKeys => engine::GrantAction::EnableDashPayKeys,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_request_credits_are_strings() {
        let g = GrantRequest::from(engine::GrantRequest {
            max_duffs: 7,
            max_credits: u64::MAX,
        });
        assert_eq!(
            serde_json::to_string(&g).unwrap(),
            r#"{"maxDuffs":7,"maxCredits":"18446744073709551615"}"#
        );
    }

    #[test]
    fn grant_actions_parse_with_a_kind_tag() {
        let a: GrantAction =
            serde_json::from_str(r#"{"kind":"registerName","label":"alice"}"#).unwrap();
        assert_eq!(
            engine::GrantAction::from(a),
            engine::GrantAction::RegisterName {
                label: "alice".into()
            }
        );
        let a: GrantAction = serde_json::from_str(r#"{"kind":"enableDashPayKeys"}"#).unwrap();
        assert_eq!(
            engine::GrantAction::from(a),
            engine::GrantAction::EnableDashPayKeys
        );
        assert!(serde_json::from_str::<GrantAction>(r#"{"kind":"nope"}"#).is_err());
    }
}
