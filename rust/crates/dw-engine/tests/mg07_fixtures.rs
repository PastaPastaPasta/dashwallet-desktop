//! The contested-name fixtures MG-11's UI starts from
//! (`tests/fixtures/mg07/`, MG-07 §5): each reads back as the facade's
//! record and writes out identically, so they cannot drift from the
//! records. `DW_BLESS=1` rewrites them from the samples below (refused
//! under `CI`; the bless run itself fails).

use std::path::PathBuf;

use dw_engine::governance::{
    ContestedName, ContestedNames, MasternodeType, NameChoice, NameContender, NameVoteCast,
    NameVoter, NameVoterState, NameVotingKey,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/mg07/{name}"))
}

const ID1: &str = "8ZtWrmgDwJHJnwyoNNsCs5CbE5LedD8eKzmTzpRzR97P";
const ID2: &str = "4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF";

/// Two contests, three voters with mixed keys and choices.
fn contested_names() -> ContestedNames {
    let state = |label: &str, choice, votes_used| NameVoterState {
        label: label.into(),
        choice,
        votes_used,
    };
    ContestedNames {
        contests: vec![
            ContestedName {
                label: "a11ce".into(),
                ends_at: 1_800_003_600,
                contenders: vec![
                    NameContender {
                        identity: ID1.into(),
                        votes: Some(9),
                    },
                    NameContender {
                        identity: ID2.into(),
                        votes: Some(4),
                    },
                ],
                lock_votes: Some(1),
                abstain_votes: Some(0),
                contenders_complete: true,
            },
            ContestedName {
                label: "dash-pay".into(),
                ends_at: 1_800_090_000,
                contenders: vec![NameContender {
                    identity: ID2.into(),
                    votes: Some(0),
                }],
                lock_votes: Some(0),
                abstain_votes: Some(0),
                contenders_complete: true,
            },
        ],
        voters: vec![
            NameVoter {
                pro_tx_hash: "a1".repeat(32),
                kind: MasternodeType::Regular,
                weight: 1,
                valid: true,
                key: NameVotingKey::Keychain { index: 0 },
                current: vec![state(
                    "a11ce",
                    Some(NameChoice::TowardsIdentity {
                        identity: ID1.into(),
                    }),
                    2,
                )],
            },
            NameVoter {
                pro_tx_hash: "b2".repeat(32),
                kind: MasternodeType::Evonode,
                weight: 4,
                valid: true,
                key: NameVotingKey::Keychain { index: 3 },
                current: vec![
                    state("a11ce", Some(NameChoice::Lock), 5),
                    state("dash-pay", Some(NameChoice::Abstain), 1),
                ],
            },
            NameVoter {
                pro_tx_hash: "c3".repeat(32),
                kind: MasternodeType::Regular,
                weight: 1,
                valid: false,
                key: NameVotingKey::Attached,
                current: vec![],
            },
        ],
        votes_allowed: 5,
    }
}

/// Every `rejected` reason of mg-engine §2.6 once, after one counted vote.
fn name_vote_casts() -> Vec<NameVoteCast> {
    let mut casts = vec![NameVoteCast {
        pro_tx_hash: format!("{:064x}", 1),
        transition: Some("5e".repeat(32)),
        rejected: None,
    }];
    let reasons = [
        ("unknown_masternode", false),
        ("no_voting_key", false),
        ("cancelled", false),
        ("signer_unavailable", false),
        ("in_flight", false),
        ("vote_limit", false),
        ("platform_unavailable", false),
        ("unconfirmed", true),
        ("already_voted", true),
        ("contest_closed", true),
        ("contest_not_found", true),
        ("masternode_not_found", true),
        ("wrong_voting_key", true),
        ("choice_not_allowed", true),
        ("no_voter_identity", true),
        ("voting_key_disabled", true),
        ("refused", true),
    ];
    for (n, (reason, broadcast)) in reasons.into_iter().enumerate() {
        casts.push(NameVoteCast {
            pro_tx_hash: format!("{:064x}", n + 2),
            transition: broadcast.then(|| format!("{:064x}", 0x5e00 + n)),
            rejected: Some(reason.into()),
        });
    }
    casts
}

fn check<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(name: &str, sample: T) {
    let path = fixture(name);
    let want = serde_json::to_string_pretty(&sample).unwrap() + "\n";
    if std::env::var_os("DW_BLESS").is_some() {
        assert!(
            std::env::var_os("CI").is_none(),
            "refusing DW_BLESS under CI"
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &want).unwrap();
        panic!("blessed {name}; review it and re-run without DW_BLESS");
    }
    let have = std::fs::read_to_string(&path).unwrap();
    let read: T = serde_json::from_str(&have).unwrap();
    assert_eq!(read, sample, "{name} no longer matches its sample");
    assert_eq!(
        serde_json::to_string_pretty(&read).unwrap() + "\n",
        have,
        "{name} does not write out identically"
    );
}

#[test]
fn contested_names_fixture_is_the_record() {
    check("contested_names.json", contested_names());
}

#[test]
fn name_vote_casts_fixture_is_the_record() {
    check("name_vote_casts.json", name_vote_casts());
}
