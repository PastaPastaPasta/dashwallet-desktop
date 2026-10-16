//! Governance vectors from dashd v24:
//!
//! - testdata/governance_votes.json (regtest, gen_masternode_vectors.py):
//!   funding votes signed with `signmessage` by two masternodes' voting
//!   keys, accepted by `voteraw`, and `gobject getcurrentvotes`;
//! - testdata/governance_budget.json (regtest, same script) and
//!   testdata/governance_live.json (mainnet and testnet nodes,
//!   gen_governance_live.py): `getsuperblockbudget`, `getgovernanceinfo`,
//!   and testnet proposals with their current votes.

#![allow(non_snake_case)] // test names carry checklist ids

use std::str::FromStr;

use dashcore::hashes::Hash;
use dashcore::{Address, Network, OutPoint, Txid};
use dw_governance::clock::superblock_budget;
use dw_governance::object::{GovernanceObject, display_hex};
use dw_governance::params::governance_params;
use dw_governance::vote::{GovernanceVote, VoteOutcome, current_vote_string};
use serde_json::Value;

fn load(name: &str) -> Value {
    let path = format!("{}/../../../testdata/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).expect("golden file")).unwrap()
}

fn network_of(name: &str) -> Network {
    match name {
        "mainnet" => Network::Mainnet,
        "testnet" => Network::Testnet,
        "regtest" => Network::Regtest,
        other => panic!("unknown network {other}"),
    }
}

fn duffs(dash: &Value) -> u64 {
    (dash.as_f64().unwrap() * 1e8).round() as u64
}

/// Parses `outpoint:time:outcome:signal:weight` back into a funding vote.
fn vote_from_current(parent: [u8; 32], text: &str) -> (GovernanceVote, u32) {
    let parts: Vec<&str> = text.split(':').collect();
    assert_eq!(parts.len(), 5, "{text}");
    let (txid, n) = parts[0].split_once('-').unwrap();
    let outcome = match parts[2] {
        "yes" => VoteOutcome::Yes,
        "no" => VoteOutcome::No,
        "abstain" => VoteOutcome::Abstain,
        other => panic!("outcome {other}"),
    };
    assert_eq!(parts[3], "funding");
    let vote = GovernanceVote::funding(
        OutPoint::new(Txid::from_str(txid).unwrap(), n.parse().unwrap()),
        parent,
        outcome,
        parts[1].parse().unwrap(),
    );
    (vote, parts[4].parse().unwrap())
}

/// Every `getcurrentvotes` entry: the key is the vote hash of the value's
/// fields, and the value prints back the same.
fn check_current_votes(parent: [u8; 32], current: &Value) -> usize {
    let current = current.as_object().unwrap();
    for (hash, text) in current {
        let (vote, weight) = vote_from_current(parent, text.as_str().unwrap());
        assert_eq!(&display_hex(&vote.hash()), hash, "{text}");
        assert_eq!(current_vote_string(&vote, weight), text.as_str().unwrap());
    }
    current.len()
}

#[test]
fn test_QT_131_regtest_votes_equal_dashd() {
    let doc = load("governance_votes.json");
    assert_eq!(doc["network"], "regtest");
    let object_hash = doc["object_hash"].as_str().unwrap();
    let o = &doc["object"];
    let object = GovernanceObject::new_proposal(
        o["revision"].as_i64().unwrap() as i32,
        o["time"].as_i64().unwrap(),
        hex::decode(o["data_hex"].as_str().unwrap()).unwrap(),
    );
    assert_eq!(display_hex(&object.hash()), object_hash);
    let parent = object.hash();

    let votes = doc["votes"].as_array().unwrap();
    assert_eq!(votes.len(), 2);
    for v in votes {
        assert_eq!(v["voteraw"], "Voted successfully");
        let outpoint = OutPoint::new(
            Txid::from_str(v["collateral_hash"].as_str().unwrap()).unwrap(),
            v["collateral_index"].as_u64().unwrap() as u32,
        );
        let mut vote = GovernanceVote::funding(
            outpoint,
            parent,
            VoteOutcome::from_code(v["outcome"].as_i64().unwrap() as i32).unwrap(),
            v["time"].as_i64().unwrap(),
        );
        // The string the voting key signs off testnet.
        let message = v["message"].as_str().unwrap();
        assert_eq!(vote.signature_string(), message);
        // dashd's signature recovers the voting key…
        vote.signature = hex::decode(v["signature_hex"].as_str().unwrap()).unwrap();
        let voting = Address::from_str(v["voting_address"].as_str().unwrap())
            .unwrap()
            .require_network(Network::Regtest)
            .unwrap();
        let dashcore::address::Payload::PubkeyHash(key_id) = voting.payload() else {
            panic!("voting address is not P2PKH");
        };
        assert_eq!(
            vote.signer_key_id(Network::Regtest),
            Some(key_id.to_byte_array())
        );
        // …and signing the same string with that key gives dashd's bytes
        // (RFC 6979, no extra entropy).
        let ours = dw_message::sign_message_with_wif(
            v["voting_wif"].as_str().unwrap(),
            Network::Regtest,
            message.as_bytes(),
        )
        .unwrap();
        assert_eq!(
            dw_message::decode_base64_strict(&ours).unwrap(),
            vote.signature
        );
        // The vote's hash is the one getcurrentvotes lists.
        let listed = &doc["current_votes"][display_hex(&vote.hash())];
        assert_eq!(listed.as_str().unwrap(), current_vote_string(&vote, 1));
        // The `govobjvote` payload round-trips.
        assert_eq!(GovernanceVote::decode(&vote.encode()).unwrap(), vote);
    }
    assert_eq!(check_current_votes(parent, &doc["current_votes"]), 2);
}

#[test]
fn test_QT_131_testnet_current_votes_equal_dashd() {
    let doc = load("governance_live.json");
    let testnet = &doc["networks"]["testnet"];
    let proposals = testnet["proposals"].as_array().unwrap();
    assert!(!proposals.is_empty());
    let mut votes = 0;
    for p in proposals {
        let object_hash = p["object_hash"].as_str().unwrap();
        // Proposals are revision 1 with no masternode outpoint or signature.
        let object = GovernanceObject::new_proposal(
            1,
            p["creation_time"].as_i64().unwrap(),
            hex::decode(p["data_hex"].as_str().unwrap()).unwrap(),
        );
        assert_eq!(display_hex(&object.hash()), object_hash);
        votes += check_current_votes(object.hash(), &p["current_votes"]);
    }
    assert!(votes > 0, "no testnet vote to compare");
}

#[test]
fn test_QT_026_superblock_budgets_equal_dashd() {
    let regtest = load("governance_budget.json");
    let live = load("governance_live.json");
    let mut compared = 0;
    for doc in [&regtest, &live] {
        for (name, entry) in doc["networks"].as_object().unwrap() {
            let network = network_of(name);
            for (height, budget) in entry["budgets"].as_object().unwrap() {
                let height: u32 = height.parse().unwrap();
                assert_eq!(
                    superblock_budget(network, height),
                    duffs(budget),
                    "{name} {height}"
                );
                compared += 1;
            }
            let info = &entry["governanceinfo"];
            let p = governance_params(network);
            assert_eq!(
                u64::from(p.superblock_cycle),
                info["superblockcycle"].as_u64().unwrap()
            );
            assert_eq!(
                u64::from(p.maturity_window),
                info["superblockmaturitywindow"].as_u64().unwrap()
            );
            assert_eq!(
                u64::from(p.min_quorum),
                info["governanceminquorum"].as_u64().unwrap()
            );
            assert_eq!(p.proposal_fee, duffs(&info["proposalfee"]));
            let next = info["nextsuperblock"].as_u64().unwrap() as u32;
            assert_eq!(
                superblock_budget(network, next),
                duffs(&info["governancebudget"]),
                "{name} next superblock {next}"
            );
        }
    }
    assert!(compared >= 20);
}
