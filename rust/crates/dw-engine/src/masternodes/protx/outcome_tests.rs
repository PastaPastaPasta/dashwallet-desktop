//! MG-10 §4.4 rule 3: a stored hand-off code reads `refused` exactly when
//! the attempt that wrote it was a definite `NotSent`.

use dashcore::OutPoint;

use super::ops::error_code;
use crate::EngineError;
use crate::platform::lease::Outcome;
use crate::send::{HandoffOutcome, SendFailure, attempt_outcome, outcome_of_code};

/// Every `SendFailure`: a new variant fails to compile here until it is
/// listed in [`failures`].
fn listed(f: &SendFailure) {
    use SendFailure as F;
    match f {
        F::NoRecipients
        | F::InvalidAddress { .. }
        | F::PlatformAddress { .. }
        | F::InvalidAmount { .. }
        | F::DustAmount { .. }
        | F::DuplicateAddress { .. }
        | F::AmountExceedsBalance { .. }
        | F::AmountWithFeeExceedsBalance { .. }
        | F::AmountTooSmallAfterFee { .. }
        | F::InsufficientMixedFunds { .. }
        | F::OutpointUnavailable(_)
        | F::AbsurdFee { .. }
        | F::TxTooLarge
        | F::InvalidChangeAddress
        | F::WatchOnly
        | F::Restored
        | F::VaultLocked
        | F::GrantInvalid
        | F::GrantExceeded { .. }
        | F::PreparedTxSpent
        | F::NoPeers
        | F::BroadcastRejected { .. }
        | F::BroadcastUnknown { .. }
        | F::Cancelled
        | F::HeldUntilRestart => {}
    }
}

fn failures() -> Vec<SendFailure> {
    use SendFailure as F;
    vec![
        F::NoRecipients,
        F::InvalidAddress { index: 0 },
        F::PlatformAddress { index: 0 },
        F::InvalidAmount { index: 0 },
        F::DustAmount { index: 0 },
        F::DuplicateAddress { index: 0 },
        F::AmountExceedsBalance { available: 1 },
        F::AmountWithFeeExceedsBalance {
            fee: 1,
            available: 1,
        },
        F::AmountTooSmallAfterFee { index: 0 },
        F::InsufficientMixedFunds { available: 1 },
        F::OutpointUnavailable(OutPoint::null()),
        F::AbsurdFee { fee: 1 },
        F::TxTooLarge,
        F::InvalidChangeAddress,
        F::WatchOnly,
        F::Restored,
        F::VaultLocked,
        F::GrantInvalid,
        F::GrantExceeded {
            max_duffs: 1,
            outflow: 2,
        },
        F::PreparedTxSpent,
        F::NoPeers,
        F::BroadcastRejected { reason: "r".into() },
        F::BroadcastUnknown { reason: "r".into() },
        F::Cancelled,
        F::HeldUntilRestart,
    ]
}

fn errors() -> Vec<EngineError> {
    let mut errors: Vec<EngineError> = failures().into_iter().map(EngineError::Send).collect();
    errors.extend([
        EngineError::NetworkNotOpen("n".into()),
        EngineError::WalletNotFound("w".into()),
        EngineError::SpvNotRunning,
        EngineError::NoPeers,
        EngineError::Internal("i".into()),
        EngineError::Storage("s".into()),
        EngineError::Spv("s".into()),
    ]);
    errors
}

#[test]
fn outcome_of_code_matches_attempt_outcome_for_every_hand_off_error() {
    let refused: Vec<String> = errors()
        .into_iter()
        .filter_map(|e| {
            if let EngineError::Send(f) = &e {
                listed(f);
            }
            let code = error_code(&e);
            let attempt = attempt_outcome(&Err(e));
            assert_ne!(attempt, Outcome::Sent);
            let expected = if attempt == Outcome::NotSent {
                HandoffOutcome::Refused
            } else {
                HandoffOutcome::Unknown
            };
            assert_eq!(outcome_of_code(&code), expected, "{code}");
            (expected == HandoffOutcome::Refused).then_some(code)
        })
        .collect();
    let mut inherited: Vec<String> = (dw_appdb::INHERITED_REFUSED_CODES.iter())
        .chain(crate::send::REFUSED_SINCE_INHERITED)
        .map(|c| (*c).to_string())
        .collect();
    inherited.sort();
    let mut refused = refused;
    refused.sort();
    assert_eq!(refused, inherited);
}

#[test]
fn the_resend_marker_and_the_unknown_code_read_unknown() {
    for code in [dw_appdb::RESEND_IN_PROGRESS, "send.broadcast_unknown", ""] {
        assert_eq!(outcome_of_code(code), HandoffOutcome::Unknown, "{code}");
    }
}
