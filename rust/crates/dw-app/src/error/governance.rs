//! The governance facade's error (mg-engine §4, BL-232). The strings are the
//! engine's `code()`s; `tests::codes_match_the_engine` holds them equal. The
//! common codes (`network_not_open`, `invalid_argument`, …) come through
//! `Engine` as every domain's do, and a `vote` grant's refusals through
//! `Platform` as `platform.*` (MG-05 §5).

use dw_engine::governance::GovernanceError;

use super::{AppError, ErrorData, codes};

codes! {
    GovernanceCode {
        /// Governance sync is off and the call needs it (`resync`).
        SyncDisabled = "governance.sync_disabled",
        /// No sync run reached `synced` yet, or no tip is known: `data`
        /// gives the phase.
        NotSynced = "governance.not_synced",
        /// Unknown, unconfirmed or not a proposal: `data` gives the hash.
        ProposalNotFound = "governance.proposal_not_found",
        /// A relayed vote failed a check: `data` gives the reason.
        VoteRejected = "governance.vote_rejected",
        /// No full node to talk to.
        NoPeers = "governance.no_peers",
        /// The call needs the masternode list and SPV has none yet.
        ListUnavailable = "governance.list_unavailable",
        /// A contract stub: `data` names the call.
        NotImplemented = "governance.not_implemented",
    }
}

impl From<GovernanceError> for AppError {
    fn from(e: GovernanceError) -> Self {
        use GovernanceError as G;
        let message = e.to_string();
        let (code, data) = match e {
            G::Engine(e) => return e.into(),
            G::Platform(e) => return e.into(),
            G::SyncDisabled => (GovernanceCode::SyncDisabled, None),
            G::NotSynced { phase } => (
                GovernanceCode::NotSynced,
                Some(ErrorData::GovernancePhase {
                    phase: phase.into(),
                }),
            ),
            G::ProposalNotFound { hash } => (
                GovernanceCode::ProposalNotFound,
                Some(ErrorData::ProposalHash { hash }),
            ),
            G::VoteRejected { reason } => (
                GovernanceCode::VoteRejected,
                Some(ErrorData::Reason { reason }),
            ),
            G::NoPeers => (GovernanceCode::NoPeers, None),
            G::ListUnavailable => (GovernanceCode::ListUnavailable, None),
            G::NotImplemented { call } => (
                GovernanceCode::NotImplemented,
                Some(ErrorData::Call { call }),
            ),
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
    use dw_engine::governance::GovernanceSyncPhase;

    use super::*;

    fn every_variant() -> Vec<GovernanceError> {
        let s = || String::from("x");
        vec![
            GovernanceError::SyncDisabled,
            GovernanceError::NotSynced {
                phase: GovernanceSyncPhase::SyncingVotes,
            },
            GovernanceError::ProposalNotFound { hash: s() },
            GovernanceError::VoteRejected { reason: s() },
            GovernanceError::NoPeers,
            GovernanceError::ListUnavailable,
            GovernanceError::NotImplemented { call: s() },
            GovernanceError::Engine(EngineError::NetworkNotOpen("regtest".into())),
            GovernanceError::Platform(dw_engine::platform::PlatformError::GrantInvalid),
        ]
    }

    /// Every variant's code equals the engine's `code()`, and every
    /// `GovernanceCode` is reached: the strings here are copies of the
    /// contract's, and must not drift from it.
    #[test]
    fn codes_match_the_engine() {
        let mut reached = std::collections::HashSet::new();
        for e in every_variant() {
            let code = e.code();
            let app = AppError::from(e);
            assert_eq!(app.code_str(), code);
            reached.insert(code);
        }
        for c in GovernanceCode::ALL {
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
            data(GovernanceError::NotSynced {
                phase: GovernanceSyncPhase::SyncingObjects
            }),
            serde_json::json!({ "kind": "governancePhase", "phase": "syncingObjects" })
        );
        assert_eq!(
            data(GovernanceError::ProposalNotFound {
                hash: "ab".repeat(32)
            }),
            serde_json::json!({ "kind": "proposalHash", "hash": "ab".repeat(32) })
        );
        assert_eq!(
            data(GovernanceError::VoteRejected {
                reason: "too_often".into()
            }),
            serde_json::json!({ "kind": "reason", "reason": "too_often" })
        );
        assert_eq!(
            data(GovernanceError::NotImplemented {
                call: "Governance.vote".into()
            }),
            serde_json::json!({ "kind": "call", "call": "Governance.vote" })
        );
        assert_eq!(data(GovernanceError::NoPeers), serde_json::Value::Null);
    }
}
