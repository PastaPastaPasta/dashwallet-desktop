//! The masternodes facade's error (mn-engine §4, MG-10). The strings are the
//! engine's `code()`s; `tests::codes_match_the_engine` holds them equal. The
//! common codes come through `Engine`, and a lease's refusals through
//! `Platform`. Codes of calls no command binds yet carry no `data`; the slice
//! that binds the call adds it.

use dw_engine::masternodes::MasternodesError;

use super::{AppError, ErrorData, codes};

codes! {
    MasternodesCode {
        ListUnavailable = "masternodes.list_unavailable",
        NotFound = "masternodes.not_found",
        AlreadyTracked = "masternodes.already_tracked",
        NotTracked = "masternodes.not_tracked",
        /// `data` gives the role.
        KeyMismatch = "masternodes.key_mismatch",
        NotEvonode = "masternodes.not_evonode",
        NoIdentity = "masternodes.no_identity",
        PlatformUnavailable = "masternodes.platform_unavailable",
        /// `data` gives the feature and the field that needs it.
        Unsupported = "masternodes.unsupported",
        /// `data` names the call.
        NotImplemented = "masternodes.not_implemented",
        ProtxRefused = "masternodes.protx_refused",
        CollateralUnavailable = "masternodes.collateral_unavailable",
        SignatureMismatch = "masternodes.signature_mismatch",
        OperationNotFound = "masternodes.operation_not_found",
        OperationState = "masternodes.operation_state",
        InFlight = "masternodes.in_flight",
        KeysExhausted = "masternodes.keys_exhausted",
        NoOwnerKey = "masternodes.no_owner_key",
        UpdatePending = "masternodes.update_pending",
        UpdateSuperseded = "masternodes.update_superseded",
        SharedSessionNotFound = "masternodes.shared_session_not_found",
        SharedStage = "masternodes.shared_stage",
        SharedMessage = "masternodes.shared_message",
        SharedFundingMismatch = "masternodes.shared_funding_mismatch",
        ShareNotMine = "masternodes.share_not_mine",
        SharedTooEarly = "masternodes.shared_too_early",
        SharedRegistrationUnknown = "masternodes.shared_registration_unknown",
        SharedActorFee = "masternodes.shared_actor_fee",
        WithdrawalKeyMissing = "masternodes.withdrawal_key_missing",
        WithdrawalShared = "masternodes.withdrawal_shared",
        WithdrawalNoPayoutKey = "masternodes.withdrawal_no_payout_key",
        WithdrawalDestinationChecking = "masternodes.withdrawal_destination_checking",
        WithdrawalPending = "masternodes.withdrawal_pending",
        WithdrawalUnverified = "masternodes.withdrawal_unverified",
        WithdrawalRetryState = "masternodes.withdrawal_retry_state",
        WithdrawTargetMismatch = "masternodes.withdraw_target_mismatch",
        WithdrawalMarkState = "masternodes.withdrawal_mark_state",
        WithdrawalNotFound = "masternodes.withdrawal_not_found",
        WithdrawalAmountOutOfRange = "masternodes.withdrawal_amount_out_of_range",
        WithdrawalAmountInsufficient = "masternodes.withdrawal_amount_insufficient",
    }
}

impl From<MasternodesError> for AppError {
    fn from(e: MasternodesError) -> Self {
        use MasternodesCode as C;
        use MasternodesError as M;
        let message = e.to_string();
        let (code, data) = match e {
            M::Engine(e) => return e.into(),
            M::Platform(e) => return e.into(),
            M::KeyMismatch { role } => (
                C::KeyMismatch,
                Some(ErrorData::MasternodeKey { role: role.into() }),
            ),
            M::Unsupported { feature, field } => (
                C::Unsupported,
                Some(ErrorData::Unsupported { feature, field }),
            ),
            M::NotImplemented { call } => (C::NotImplemented, Some(ErrorData::Call { call })),
            M::ListUnavailable => (C::ListUnavailable, None),
            M::NotFound => (C::NotFound, None),
            M::AlreadyTracked => (C::AlreadyTracked, None),
            M::NotTracked => (C::NotTracked, None),
            M::NotEvonode => (C::NotEvonode, None),
            M::NoIdentity => (C::NoIdentity, None),
            M::PlatformUnavailable { .. } => (C::PlatformUnavailable, None),
            M::ProtxRefused { reason, .. } => (C::ProtxRefused, Some(ErrorData::Reason { reason })),
            M::CollateralUnavailable => (C::CollateralUnavailable, None),
            M::SignatureMismatch => (C::SignatureMismatch, None),
            M::OperationNotFound => (C::OperationNotFound, None),
            M::OperationState { state } => {
                (C::OperationState, Some(ErrorData::OperationState { state }))
            }
            M::InFlight => (C::InFlight, None),
            M::KeysExhausted { role } => (C::KeysExhausted, Some(ErrorData::KeyRole { role })),
            M::NoOwnerKey => (C::NoOwnerKey, None),
            M::UpdatePending { operation } => {
                (C::UpdatePending, Some(ErrorData::Operation { operation }))
            }
            M::UpdateSuperseded { operation } => (
                C::UpdateSuperseded,
                Some(ErrorData::Operation { operation }),
            ),
            M::SharedSessionNotFound => (C::SharedSessionNotFound, None),
            M::SharedStage { stage } => (C::SharedStage, Some(ErrorData::SharedStage { stage })),
            M::SharedMessage { reason, share } => (
                C::SharedMessage,
                Some(ErrorData::SharedMessage { reason, share }),
            ),
            M::SharedFundingMismatch { reason, needed } => (
                C::SharedFundingMismatch,
                Some(ErrorData::SharedFunding { reason, needed }),
            ),
            M::ShareNotMine => (C::ShareNotMine, None),
            M::SharedTooEarly => (C::SharedTooEarly, None),
            M::SharedRegistrationUnknown => (C::SharedRegistrationUnknown, None),
            M::SharedActorFee { fee } => {
                (C::SharedActorFee, Some(ErrorData::SharedActorFee { fee }))
            }
            M::WithdrawalKeyMissing => (C::WithdrawalKeyMissing, None),
            M::WithdrawalShared => (C::WithdrawalShared, None),
            M::WithdrawalNoPayoutKey { .. } => (C::WithdrawalNoPayoutKey, None),
            M::WithdrawalDestinationChecking { .. } => (C::WithdrawalDestinationChecking, None),
            M::WithdrawalPending => (C::WithdrawalPending, None),
            M::WithdrawalUnverified => (C::WithdrawalUnverified, None),
            M::WithdrawalRetryState => (C::WithdrawalRetryState, None),
            M::WithdrawTargetMismatch => (C::WithdrawTargetMismatch, None),
            M::WithdrawalMarkState => (C::WithdrawalMarkState, None),
            M::WithdrawalNotFound => (C::WithdrawalNotFound, None),
            M::WithdrawalAmountOutOfRange { .. } => (C::WithdrawalAmountOutOfRange, None),
            M::WithdrawalAmountInsufficient { .. } => (C::WithdrawalAmountInsufficient, None),
        };
        AppError {
            code: code.into(),
            message,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use dw_engine::EngineError;
    use dw_engine::masternodes::{AttachedKeyRole, WithdrawalCheckingReason};

    use super::*;

    fn every_variant() -> Vec<MasternodesError> {
        use MasternodesError as M;
        let s = || String::from("x");
        vec![
            M::ListUnavailable,
            M::NotFound,
            M::AlreadyTracked,
            M::NotTracked,
            M::KeyMismatch {
                role: AttachedKeyRole::Owner,
            },
            M::NotEvonode,
            M::NoIdentity,
            M::PlatformUnavailable { detail: s() },
            M::Unsupported {
                feature: s(),
                field: None,
            },
            M::NotImplemented { call: s() },
            M::ProtxRefused {
                reason: s(),
                operation: None,
            },
            M::CollateralUnavailable,
            M::SignatureMismatch,
            M::OperationNotFound,
            M::OperationState { state: s() },
            M::InFlight,
            M::KeysExhausted { role: s() },
            M::NoOwnerKey,
            M::UpdatePending { operation: s() },
            M::UpdateSuperseded { operation: s() },
            M::SharedSessionNotFound,
            M::SharedStage { stage: s() },
            M::SharedMessage {
                reason: s(),
                share: None,
            },
            M::SharedFundingMismatch {
                reason: s(),
                needed: None,
            },
            M::ShareNotMine,
            M::SharedTooEarly,
            M::SharedRegistrationUnknown,
            M::SharedActorFee { fee: 1 },
            M::WithdrawalKeyMissing,
            M::WithdrawalShared,
            M::WithdrawalNoPayoutKey { detail: s() },
            M::WithdrawalDestinationChecking {
                reason: WithdrawalCheckingReason::SpvUnsynced,
            },
            M::WithdrawalPending,
            M::WithdrawalUnverified,
            M::WithdrawalRetryState,
            M::WithdrawTargetMismatch,
            M::WithdrawalMarkState,
            M::WithdrawalNotFound,
            M::WithdrawalAmountOutOfRange { min: 1, max: 2 },
            M::WithdrawalAmountInsufficient {
                needed: 2,
                available: 1,
            },
            M::Engine(EngineError::NetworkNotOpen("regtest".into())),
            M::Platform(dw_engine::platform::PlatformError::GrantInvalid),
        ]
    }

    #[test]
    fn codes_match_the_engine() {
        let mut reached = std::collections::HashSet::new();
        for e in every_variant() {
            let code = e.code();
            assert_eq!(AppError::from(e).code_str(), code);
            reached.insert(code);
        }
        for c in MasternodesCode::ALL {
            assert!(
                reached.contains(c.as_str()),
                "{} is never produced",
                c.as_str()
            );
        }
    }

    #[test]
    fn codes_carry_their_fields() {
        let data = |e| serde_json::to_value(AppError::from(e)).unwrap()["data"].clone();
        assert_eq!(
            data(MasternodesError::KeyMismatch {
                role: AttachedKeyRole::Payout
            }),
            serde_json::json!({ "kind": "masternodeKey", "role": "payout" })
        );
        assert_eq!(
            data(MasternodesError::Unsupported {
                feature: "shared_masternodes".into(),
                field: None
            }),
            serde_json::json!({ "kind": "unsupported", "feature": "shared_masternodes", "field": null })
        );
        assert_eq!(
            data(MasternodesError::NotImplemented {
                call: "Masternodes.list".into()
            }),
            serde_json::json!({ "kind": "call", "call": "Masternodes.list" })
        );
        assert_eq!(
            data(MasternodesError::ProtxRefused {
                reason: "bad-protx-payee-reuse".into(),
                operation: None
            }),
            serde_json::json!({ "kind": "reason", "reason": "bad-protx-payee-reuse" })
        );
        assert_eq!(
            data(MasternodesError::OperationState {
                state: "sent".into()
            }),
            serde_json::json!({ "kind": "operationState", "state": "sent" })
        );
        assert_eq!(
            data(MasternodesError::KeysExhausted {
                role: "owner".into()
            }),
            serde_json::json!({ "kind": "keyRole", "role": "owner" })
        );
        assert_eq!(
            data(MasternodesError::UpdatePending {
                operation: "op2".into()
            }),
            serde_json::json!({ "kind": "operation", "operation": "op2" })
        );
        assert_eq!(
            data(MasternodesError::SharedMessage {
                reason: "bad_signature".into(),
                share: Some(2)
            }),
            serde_json::json!({ "kind": "sharedMessage", "reason": "bad_signature", "share": 2 })
        );
        assert_eq!(
            data(MasternodesError::SharedStage {
                stage: "replied".into()
            }),
            serde_json::json!({ "kind": "sharedStage", "stage": "replied" })
        );
        assert_eq!(
            data(MasternodesError::SharedFundingMismatch {
                reason: "fee_low".into(),
                needed: Some(120_000)
            }),
            serde_json::json!({ "kind": "sharedFunding", "reason": "fee_low", "needed": 120_000 })
        );
        assert_eq!(
            data(MasternodesError::SharedActorFee { fee: 1_000 }),
            serde_json::json!({ "kind": "sharedActorFee", "fee": 1_000 })
        );
        assert_eq!(
            data(MasternodesError::PlatformUnavailable {
                detail: "dapi".into()
            }),
            serde_json::Value::Null
        );
    }
}
