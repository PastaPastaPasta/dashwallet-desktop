//! Governance vectors (BL-232): one engine record of each kind the
//! `governance_*` commands return, through dw-app's conversion, and every
//! `GovernanceError` through `AppError`, written to
//! `apps/desktop/src/fixture/vectors/governance.json`. The contested-name
//! results are MG-07's fixtures (`dw-engine/tests/fixtures/mg07`, held equal
//! to the facade's records there), read back as engine records, so the
//! renderer's fixture builds on them (MG-11). The renderer's
//! contract test (`apps/desktop/src/api/governance.test.ts`) holds them
//! against the records of mg-engine §3. This test fails when the committed file
//! differs from what dw-app produces now; regenerate it with
//! `DW_UPDATE_VECTORS=1 cargo test -p dw-app governance_vectors`.

use std::path::{Path, PathBuf};

use dw_engine::EngineError;
use dw_engine::governance::{
    ContestedNames, CurrentVoteRow, GovernanceClock, GovernanceError, GovernanceInfo,
    GovernanceSyncPhase, GovernanceSyncState, MasternodeType, MyProposal, MyProposalState,
    NameVoteCast, ProposalCounts, ProposalDetail, ProposalField, ProposalPreview, ProposalRow,
    ProposalStatus, SignalTallies, SubmitRefusal, SubmitWait, SuperblockDate, VoteCast,
    VoteOutcome, VoteSignal, VoteTally, VotingMasternode, WeightRange,
};
use serde_json::{Value, json};

use crate::dto::governance as dto;
use crate::error::AppError;

const REGENERATE: &str = "DW_UPDATE_VECTORS=1 cargo test -p dw-app governance_vectors";

fn vectors_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../apps/desktop/src/fixture/vectors/governance.json")
}

/// One of MG-07's fixtures, as the engine record it holds.
fn mg07<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../dw-engine/tests/fixtures/mg07")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn to_json<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap()
}

fn range(min: u32, max: u32, estimate: u32) -> WeightRange {
    WeightRange { min, max, estimate }
}

fn clock() -> GovernanceClock {
    GovernanceClock {
        tip_height: 2_350_100,
        cycle_progress: 0.75,
        last_superblock: 2_337_640,
        next_superblock: 2_354_256,
        blocks_to_superblock: 4_156,
        superblock_eta: 1_790_653_323,
        voting_cutoff_estimate: 1_790_400_000,
        voting_cutoff_height: 2_352_594,
        voting_open: true,
    }
}

fn row() -> ProposalRow {
    ProposalRow {
        hash: "5c".repeat(32),
        name: "dash-desktop-wallet".into(),
        url: "https://www.dashcentral.org/p/dash-desktop-wallet".into(),
        payment_address: "XpESxaUmonkq8RaLLp46Brx2K39ggQe226".into(),
        payment_amount: 1_250_000_000_000,
        start_epoch: 1_788_000_000,
        end_epoch: 1_793_000_000,
        payments_requested: 2,
        status: ProposalStatus::Passing,
        close_call: true,
        tally: VoteTally {
            yes: range(410, 422, 416),
            no: range(31, 31, 31),
            abstain: range(4, 4, 4),
            net_yes_estimate: 385,
            voters: 451,
            ranged_votes: 3,
        },
        funded_height: None,
    }
}

fn build() -> Value {
    let sync = GovernanceSyncState {
        phase: GovernanceSyncPhase::Synced,
        objects: 42,
        unconfirmed_objects: 1,
        votes: 12_345,
        orphan_votes: 2,
        peers: 3,
        bytes_received: 9_876_543,
        last_synced_at: Some(1_790_000_000),
        last_error: None,
        next_retry_at: None,
        objects_secs: Some(1.5),
        votes_secs: Some(12.25),
    };
    let info = GovernanceInfo {
        clock: clock(),
        superblock_cycle: 16_616,
        maturity_window: 1_662,
        target_spacing: 150,
        proposal_fee: 100_000_000,
        budget: 6_650_000_000_000,
        passing_threshold: 336,
        valid_masternodes: 2_880,
        valid_evonodes: 150,
        valid_weight: 3_480,
        proposals: Some(ProposalCounts {
            passing: 12,
            failing: 7,
            unfunded: 1,
            total: 20,
            close_call: 2,
        }),
        allocated_estimate: Some(5_900_000_000_000),
        unallocated_estimate: Some(750_000_000_000),
    };
    let detail = ProposalDetail {
        row: row(),
        collateral_txid: "7e".repeat(32),
        created_at: 1_787_900_000,
        data_json: r#"{"end_epoch":1793000000,"name":"dash-desktop-wallet","type":1}"#.into(),
        signals: SignalTallies {
            valid: VoteTally::default(),
            delete: VoteTally {
                no: range(2_400, 2_404, 2_402),
                voters: 2_380,
                ranged_votes: 1,
                net_yes_estimate: -2_402,
                ..VoteTally::default()
            },
            endorsed: VoteTally::default(),
        },
        deletion_marked_at: Some(1_790_000_600),
    };
    let votes = vec![
        CurrentVoteRow {
            outpoint: format!("{}-1", "11".repeat(32)),
            signal: VoteSignal::Funding,
            time: 1_789_000_000,
            outcome: VoteOutcome::Yes,
            weight: range(1, 1, 1),
            exact: true,
        },
        CurrentVoteRow {
            outpoint: format!("{}-0", "22".repeat(32)),
            signal: VoteSignal::Delete,
            time: 1_789_100_000,
            outcome: VoteOutcome::No,
            weight: range(1, 4, 1),
            exact: false,
        },
    ];
    let masternodes = vec![
        VotingMasternode {
            pro_tx_hash: "a1".repeat(32),
            kind: MasternodeType::Regular,
            weight: 1,
            valid: true,
            key_index: 0,
            current_vote: Some(VoteOutcome::Yes),
            vote_time: Some(1_789_000_000),
            key_holders: 1,
            key_last_vote: None,
            key_last_vote_time: None,
        },
        VotingMasternode {
            pro_tx_hash: "b2".repeat(32),
            kind: MasternodeType::Evonode,
            weight: 4,
            valid: false,
            key_index: 3,
            current_vote: None,
            vote_time: None,
            key_holders: 2,
            key_last_vote: Some(VoteOutcome::Abstain),
            key_last_vote_time: Some(1_789_200_000),
        },
    ];
    let cast = vec![
        VoteCast {
            pro_tx_hash: "a1".repeat(32),
            hash: Some("7e".repeat(32)),
            peers: 0,
            rejected: None,
        },
        VoteCast {
            pro_tx_hash: "b2".repeat(32),
            hash: None,
            peers: 0,
            rejected: Some("too_often".into()),
        },
    ];
    let superblocks = vec![
        SuperblockDate {
            height: 2_354_256,
            estimated_time: 1_790_653_323,
            budget: 6_650_000_000_000,
            voting_cutoff_height: 2_352_594,
            voting_cutoff_estimate: 1_790_400_000,
            cutoff_soon: true,
        },
        SuperblockDate {
            height: 2_370_872,
            estimated_time: 1_793_265_358,
            budget: 6_650_000_000_000,
            voting_cutoff_height: 2_369_210,
            voting_cutoff_estimate: 1_793_000_000,
            cutoff_soon: false,
        },
    ];
    let preview = ProposalPreview {
        hash: "c3".repeat(32),
        data_json: r#"[["proposal",{"end_epoch":1793265358,"name":"wallet-fund","payment_address":"yP8A3cbdxRtLRduy5mXDsBnJtMzHWs6ZXr","payment_amount":100,"start_epoch":1790653323,"type":1,"url":"https://example.org/wallet"}]]"#.into(),
        data_hex: "5b5b2270726f706f73616c222c7b7d5d5d".into(),
        start_epoch: 1_790_653_323,
        end_epoch: 1_793_265_358,
        fee: 100_000_000,
        network_fee: 2_260,
    };
    let my_proposal = |state| MyProposal {
        hash: "c3".repeat(32),
        name: "wallet-fund".into(),
        url: "https://example.org/wallet".into(),
        payment_address: "yP8A3cbdxRtLRduy5mXDsBnJtMzHWs6ZXr".into(),
        payment_amount: 10_000_000_000,
        payment_count: 2,
        start_epoch: 1_790_653_323,
        end_epoch: 1_793_265_358,
        fee_txid: "d4".repeat(32),
        created_at: 1_790_000_000,
        submitted_at: Some(1_790_000_120),
        state,
        last_error: None,
    };
    let submitted = my_proposal(MyProposalState::ReadyToSubmit {
        reason: SubmitWait::NotSynced,
    });
    let my_proposals = vec![
        my_proposal(MyProposalState::FeePaid { confirmations: 0 }),
        my_proposal(MyProposalState::ReadyToSubmit {
            reason: SubmitWait::NoPeerAccepted,
        }),
        my_proposal(MyProposalState::Confirming { confirmations: 3 }),
        my_proposal(MyProposalState::Public {
            status: Some(ProposalStatus::Voting),
        }),
        MyProposal {
            last_error: Some("the broadcast's outcome is unknown: no verdict yet".into()),
            ..my_proposal(MyProposalState::FeeUnsent)
        },
        my_proposal(MyProposalState::FeeFailed),
        my_proposal(MyProposalState::Expired),
    ];
    let errors: Vec<Value> = [
        GovernanceError::SyncDisabled,
        GovernanceError::NotSynced {
            phase: GovernanceSyncPhase::SyncingVotes,
        },
        GovernanceError::ProposalNotFound {
            hash: "5d".repeat(32),
        },
        GovernanceError::VoteRejected {
            reason: "too_often".into(),
        },
        GovernanceError::NoPeers,
        GovernanceError::ListUnavailable,
        GovernanceError::InvalidProposal {
            fields: vec![ProposalField::Name, ProposalField::PaymentAddress],
        },
        GovernanceError::SubmitRefused {
            reason: SubmitRefusal::Public,
        },
        GovernanceError::SubmitRefused {
            reason: SubmitRefusal::InFlight,
        },
        GovernanceError::ContestNotFound {
            label: "a11ce".into(),
        },
        GovernanceError::ContestClosed {
            label: "a11ce".into(),
        },
        GovernanceError::NotAContender {
            identity: "8ZtWrmgDwJHJnwyoNNsCs5CbE5LedD8eKzmTzpRzR97P".into(),
        },
        GovernanceError::NotImplemented {
            call: "Governance.vote".into(),
        },
        GovernanceError::Platform(dw_engine::platform::PlatformError::GrantInvalid),
        GovernanceError::Engine(EngineError::InvalidArgument(
            "not a 64-character hex hash".into(),
        )),
    ]
    .into_iter()
    .map(|e| to_json(AppError::from(e)))
    .collect();
    json!({
        "comment": format!(
            "Generated by dw-app (rust/crates/dw-app/src/governance_vectors_tests.rs) from dw-engine's governance records through dw-app's conversions; do not edit. Regenerate with {REGENERATE}."
        ),
        "results": {
            "governance_sync_enabled": true,
            "governance_set_sync_enabled": null,
            "governance_sync_state": to_json(dto::GovernanceSyncState::from(sync)),
            "governance_resync": null,
            "governance_clock": to_json(dto::GovernanceClock::from(clock())),
            "governance_info": to_json(dto::GovernanceInfo::from(info)),
            "governance_upcoming_superblocks": to_json(
                superblocks.into_iter().map(dto::SuperblockDate::from).collect::<Vec<_>>()
            ),
            "governance_proposals": to_json(vec![dto::ProposalRow::from(row())]),
            "governance_proposal": to_json(dto::ProposalDetail::from(detail)),
            "governance_current_votes": to_json(
                votes.into_iter().map(dto::CurrentVoteRow::from).collect::<Vec<_>>()
            ),
            "governance_vote": to_json(
                cast.into_iter().map(dto::VoteCast::from).collect::<Vec<_>>()
            ),
            "governance_preview_proposal": to_json(dto::ProposalPreview::from(preview)),
            "governance_create_proposal": to_json(dto::MyProposal::from(my_proposals[0].clone())),
            "governance_my_proposals": to_json(
                my_proposals.into_iter().map(dto::MyProposal::from).collect::<Vec<_>>()
            ),
            "governance_submit_proposal": to_json(dto::MyProposal::from(submitted)),
            "governance_voting_masternodes": to_json(
                masternodes.into_iter().map(dto::VotingMasternode::from).collect::<Vec<_>>()
            ),
            "governance_contested_names": to_json(dto::ContestedNames::from(
                mg07::<ContestedNames>("contested_names.json")
            )),
            "governance_vote_name": to_json(
                mg07::<Vec<NameVoteCast>>("name_vote_casts.json")
                    .into_iter()
                    .map(dto::NameVoteCast::from)
                    .collect::<Vec<_>>()
            ),
        },
        "errors": errors,
    })
}

#[test]
fn governance_vectors_are_current() {
    let text = serde_json::to_string_pretty(&build()).unwrap() + "\n";
    let path = vectors_path();
    if std::env::var_os("DW_UPDATE_VECTORS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == text,
        "apps/desktop/src/fixture/vectors/governance.json is stale; run {REGENERATE}"
    );
}

/// Every governance command has a result in the vectors, so the renderer's
/// contract test checks each one.
#[test]
fn every_command_has_a_vector() {
    let v = build();
    let results = v["results"].as_object().unwrap();
    let mut names: Vec<&str> = crate::api::governance::NAMES.to_vec();
    names.sort_unstable();
    let mut keys: Vec<&str> = results.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, names);
}

/// Every reason MG-07's fixture holds (all of §3's) crosses as itself, none
/// read as `refused` but `refused`; an unknown one reads as `refused`.
#[test]
fn every_name_vote_reason_crosses_as_itself() {
    let reasons: std::collections::BTreeSet<String> =
        mg07::<Vec<NameVoteCast>>("name_vote_casts.json")
            .into_iter()
            .filter_map(|c| c.rejected)
            .collect();
    assert_eq!(reasons.len(), 17, "{reasons:?}");
    for reason in &reasons {
        let crossed = dto::NameRejectionName::from_engine(reason);
        assert_eq!(to_json(crossed), Value::String(reason.clone()));
    }
    assert_eq!(
        dto::NameRejectionName::from_engine("new_reason"),
        dto::NameRejectionName::Unknown
    );
    // `unknown` is this build's word, never one of the engine's.
    assert!(!reasons.contains("unknown"));
}
