//! `dw-protx::shared::session` against dash-qt's own `MnShareSession`
//! (Dash Core v24.0.0-rc.3 `src/qt/mnsharesession.cpp`, MG-04b M9). The
//! vectors in testdata/shared_envelopes.json come from
//! `testdata/oracle/dashqt/gen_envelopes.py`, which links that file
//! unmodified:
//!
//! - `cases`: the two registrations of shared_registration.json replayed
//!   through the session as dash-qt's creation dialog does, with
//!   `toJsonString()` and `fingerprint()` after every step;
//! - `corpus`: those envelopes with one change each, and `fromJson`'s
//!   verdict on each.
//!
//! The wallet must write dash-qt's text byte for byte at every step, read
//! back the session it wrote, and accept exactly what `fromJson` accepts,
//! except where `wallet_refuses` names the D-row that makes it stricter.

#![allow(non_snake_case)] // test names carry checklist ids

use dashcore::Network;
use dw_protx::shared::envelope::fingerprint_matches;
use dw_protx::shared::json::Json;
use dw_protx::shared::session::funding::{decode_frozen, rebuild_funding_tx};
use dw_protx::shared::session::merge::add_signature;
use dw_protx::shared::session::{
    Change, Contribution, Input, Session, Share, Signature, Stage, Terms,
};
use serde_json::Value;

/// The corpus texts gen_envelopes.py writes; a change there updates this.
const EXPECTED_CORPUS: usize = 220;

fn load() -> Value {
    let path = format!(
        "{}/../../../testdata/shared_envelopes.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("golden file")).unwrap();
    assert_eq!(doc["network"], "regtest");
    assert!(doc["source"].as_str().unwrap().contains("v24.0.0-rc.3"));
    doc
}

fn str_of(v: &Value, key: &str) -> String {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is missing"))
        .to_owned()
}

fn int_of(v: &Value, key: &str) -> i64 {
    v[key]
        .as_i64()
        .unwrap_or_else(|| panic!("{key} is missing"))
}

fn read(text: &str) -> Option<Json> {
    Json::read(text.as_bytes())
}

#[test]
fn test_QT_126_session_steps_equal_dash_qts() {
    let doc = load();
    let cases = doc["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    for c in cases {
        let name = str_of(c, "name");
        let mut steps = c["steps"].as_array().unwrap().iter();
        // Each step: our text and code are dash-qt's, and reading dash-qt's
        // text gives back this session.
        let mut check = |session: &Session, step: &str| {
            let want = steps
                .next()
                .unwrap_or_else(|| panic!("{name}: no step {step}"));
            assert_eq!(str_of(want, "step"), step, "{name}");
            let text = str_of(want, "text");
            assert_eq!(session.to_text(), text, "{name} {step}");
            assert_eq!(
                session.message_code(),
                str_of(want, "fingerprint"),
                "{name} {step}"
            );
            let json = read(&text).unwrap();
            assert!(fingerprint_matches(&json));
            assert_eq!(
                Session::from_json(&json, Network::Regtest).as_ref(),
                Ok(session),
                "{name} {step}"
            );
        };

        let mut session = Session::new(Network::Regtest, str_of(c, "sessionId"));
        check(&session, "new");

        for s in c["shares"].as_array().unwrap() {
            session.shares.push(Share {
                label: str_of(s, "label"),
                amount: int_of(s, "amount"),
                owner_address: str_of(s, "ownerAddress"),
                refund_address: str_of(s, "refundAddress"),
                reward_address: str_of(s, "rewardAddress"),
            });
        }
        let t = &c["terms"];
        session.terms = Terms {
            core_p2p_addrs: str_of(t, "coreP2PAddrs"),
            operator_pub_key: str_of(t, "operatorPubKey"),
            voting_address: str_of(t, "votingAddress"),
            operator_reward: int_of(t, "operatorReward") as i32,
            early_period_blocks: int_of(t, "earlyPeriodBlocks") as u32,
            early_penalty: int_of(t, "earlyPenalty"),
        };
        session.prepare_wallet = str_of(c, "prepareWallet");
        session.operator_secret_holder = str_of(c, "operatorSecretHolder");
        session.coordinator_label = str_of(c, "coordinatorLabel");
        session.revision += 1; // noteDraftChange
        check(&session, "terms");

        // addContribution: rebuilt funding template, one revision each.
        for k in c["contributions"].as_array().unwrap() {
            let label = str_of(k, "label");
            let inputs = k["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| Input {
                    txid: str_of(i, "txid"),
                    vout: int_of(i, "vout") as u32,
                    sequence: int_of(i, "sequence") as u32,
                })
                .collect();
            let change = &k["change"];
            session.contributions.push(Contribution {
                label: label.clone(),
                inputs,
                change: Some(Change {
                    address: str_of(change, "address"),
                    amount: int_of(change, "amount"),
                }),
                change_index: -1,
            });
            rebuild_funding_tx(&mut session).unwrap();
            session.revision += 1;
            check(&session, &format!("contribution {label}"));
        }

        session.protx = str_of(c, "protx");
        session.consent_hash = str_of(c, "consentHash");
        session.collateral_index = int_of(c, "collateralIndex") as i32;
        session.stage = Stage::Frozen;
        check(&session, "freeze");

        // Out of order; kept ascending.
        let frozen = decode_frozen(&session).unwrap();
        for sig in c["signatures"].as_array().unwrap() {
            let index = int_of(sig, "shareIndex") as i32;
            let added = add_signature(&mut session, &frozen, index, &str_of(sig, "signature"));
            assert!(matches!(added, Ok(true)), "{name} {index}");
            check(&session, &format!("signature {index}"));
        }
        assert!(session.sigs.is_sorted_by_key(|s: &Signature| s.share_index));

        session.protx = str_of(c, "combinedTx");
        session.stage = Stage::Combined;
        check(&session, "combine");
        session.protx = str_of(c, "fundingSignedTx");
        session.stage = Stage::FundingSigned;
        check(&session, "sign funding");
        session.stage = Stage::Broadcast;
        check(&session, "broadcast");
        assert!(steps.next().is_none(), "{name}: steps left over");
    }
}

#[test]
fn test_QT_126_envelope_verdicts_equal_dash_qts() {
    let doc = load();
    let corpus = doc["corpus"].as_array().unwrap();
    assert_eq!(corpus.len(), EXPECTED_CORPUS);
    let mut names = std::collections::BTreeSet::new();
    let mut wrong = Vec::new();
    for e in corpus {
        let name = str_of(e, "name");
        assert!(names.insert(name.clone()), "{name} is repeated");
        let text = str_of(e, "text");
        let json = read(&text);
        let ours = json
            .as_ref()
            .map(|j| Session::from_json(j, Network::Regtest));
        let theirs = e["ok"].as_bool().unwrap();
        if let Some(row) = e.get("wallet_refuses") {
            assert!(theirs, "{name}");
            if !matches!(ours, Some(Err(_))) {
                wrong.push(format!("{name}: the wallet must refuse it ({row})"));
            }
            continue;
        }
        match ours {
            Some(Ok(_)) if !theirs => {
                wrong.push(format!("{name}: dash-qt refuses it: {}", e["error"]))
            }
            Some(Ok(session)) => {
                assert_eq!(session.to_text(), str_of(e, "read_back"), "{name}");
                assert_eq!(
                    fingerprint_matches(json.as_ref().unwrap()),
                    e["fingerprint_ok"].as_bool().unwrap(),
                    "{name}"
                );
            }
            Some(Err(err)) if theirs => {
                wrong.push(format!("{name}: dash-qt reads it; we refuse: {err}"))
            }
            None if theirs => wrong.push(format!("{name}: dash-qt reads it; it isn't JSON to us")),
            _ => {}
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
