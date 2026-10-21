use serde_json::Value;

use super::funding::{check_frozen, rebuild_funding_tx};
use super::*;
use crate::shared::envelope::{
    EnvelopeError, SharedMessage, decode_message, encode_base64, without_fingerprint,
};

const REGISTRATION: &str = include_str!("../../../../../../testdata/shared_registration.json");

fn case(name: &str) -> Value {
    let doc: Value = serde_json::from_str(REGISTRATION).unwrap();
    doc["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()
        .clone()
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// dashd's `three_shares_early` registration as a dash-qt draft: one share
/// and one contribution per party, labelled with the party's wallet name,
/// the funding coin's change read from dashd's funding transaction.
fn draft() -> (Session, Value) {
    let c = case("three_shares_early");
    let args = c["prepare_args"].as_array().unwrap();
    let mut session = Session::new(Network::Regtest, "ab".repeat(32));
    for (row, party) in args[1]
        .as_array()
        .unwrap()
        .iter()
        .zip(c["parties"].as_array().unwrap())
    {
        session.shares.push(Share {
            label: s(&party["wallet"]),
            amount: row["amount"].as_i64().unwrap(),
            owner_address: s(&row["ownerAddress"]),
            refund_address: s(&row["refundAddress"]),
            reward_address: s(&row["rewardAddress"]),
        });
    }
    session.terms = Terms {
        core_p2p_addrs: s(&args[2]),
        operator_pub_key: s(&args[3]),
        voting_address: s(&args[4]),
        operator_reward: 525,
        early_period_blocks: 20,
        early_penalty: 100_000_000,
    };
    let funding =
        crate::shared::SpecialTx::from_bytes(&hex::decode(s(&c["funding_tx_hex"])).unwrap())
            .unwrap();
    for (party, out) in c["parties"].as_array().unwrap().iter().zip(&funding.output) {
        let coin = &party["funding_coin"];
        session.contributions.push(Contribution {
            label: s(&party["wallet"]),
            inputs: vec![Input {
                txid: s(&coin["txid"]),
                vout: coin["vout"].as_u64().unwrap() as u32,
                sequence: u32::MAX,
            }],
            change: Some(Change {
                address: s(&party["change_address"]),
                amount: out.value as i64,
            }),
            change_index: -1,
        });
    }
    rebuild_funding_tx(&mut session).unwrap();
    (session, c)
}

fn at(stage: Stage) -> Session {
    let (mut session, c) = draft();
    session.stage = stage;
    if stage == Stage::Draft {
        return session;
    }
    session.protx = s(&c["prepare"]["tx"]);
    session.consent_hash = s(&c["prepare"]["consentHash"]);
    session.collateral_index = c["prepare"]["collateralIndex"].as_i64().unwrap() as i32;
    if stage >= Stage::Signing {
        session.sigs = c["signatures"]
            .as_array()
            .unwrap()
            .iter()
            .map(|sig| Signature {
                share_index: sig["shareIndex"].as_i64().unwrap() as i32,
                signature: s(&sig["signature"]),
            })
            .collect();
    }
    if stage >= Stage::Combined {
        session.protx = s(&c["combined_tx_hex"]);
    }
    if stage >= Stage::FundingSigned {
        session.protx = s(&c["tx_hex"]);
    }
    session
}

fn reparse(session: &Session) -> Result<Session, EnvelopeError> {
    Session::from_json(&session.to_json(), Network::Regtest)
}

#[test]
fn test_QT_126_funding_template_is_dashds() {
    let (session, c) = draft();
    assert_eq!(session.funding_tx, s(&c["funding_tx_hex"]));
    assert_eq!(
        session
            .contributions
            .iter()
            .map(|c| c.change_index)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
}

#[test]
fn test_QT_126_every_stage_round_trips_and_checks_as_dash_qt() {
    for stage in Stage::ALL {
        let session = at(stage);
        assert_eq!(reparse(&session).as_ref(), Ok(&session), "{stage:?}");
        let text = session.to_text();
        let SharedMessage::Session {
            session: read,
            received,
        } = decode_message(&text, Network::Regtest).unwrap()
        else {
            panic!("a session")
        };
        assert_eq!(*read, session);
        assert!(!received.edited);
        assert_eq!(received.code, session.message_code());
    }
    assert_eq!(at(Stage::Draft).session_code(), "ABABAB");
}

/// dash-qt's envelope shape, written by `gen_vectors.py` and rendered by
/// UniValue: read and written back, it is byte-identical without its code.
#[test]
fn test_QT_126_dash_qt_envelope_reads_and_writes_back_identically() {
    let doc: Value = serde_json::from_str(include_str!(
        "../../../../../../testdata/oracle/univalue/univalue_vectors.json"
    ))
    .unwrap();
    let envelope = hex::decode(doc["cases"][0]["input"].as_str().unwrap()).unwrap();
    let json = Json::read(&envelope).unwrap();
    let session = Session::from_json(&json, Network::Regtest).unwrap();
    assert_eq!(session.coordinator_label, "Alice");
    assert_eq!(session.operator_secret_holder, "Carol's VPS");
    assert_eq!(
        session.contributions[0].change.as_ref().unwrap().amount,
        123_456
    );
    assert_eq!(
        session.to_json_body().write(2),
        without_fingerprint(&json).write(2)
    );
    // dash-qt's own message code of that body, from UniValue.
    assert_eq!(
        session.message_code(),
        doc["cases"][0]["fingerprint"].as_str().unwrap()
    );
    assert!(matches!(
        Session::from_json(&json, Network::Testnet),
        Err(EnvelopeError::Network)
    ));
}

fn edit(session: &Session, f: impl FnOnce(&mut Json)) -> Result<Session, EnvelopeError> {
    let mut json = session.to_json();
    f(&mut json);
    Session::from_json(&json, Network::Regtest)
}

fn member<'a>(json: &'a mut Json, path: &[&str]) -> &'a mut Json {
    path.iter().fold(json, |j, key| match j {
        Json::Obj(entries) => {
            &mut entries
                .iter_mut()
                .find(|(k, _)| k == key.as_bytes())
                .unwrap()
                .1
        }
        Json::Arr(items) => &mut items[key.parse::<usize>().unwrap()],
        _ => panic!("{key}"),
    })
}

fn set(path: &'static [&'static str], value: Json) -> impl FnOnce(&mut Json) {
    move |j| *member(j, path) = value
}

#[test]
fn test_QT_126_reader_refuses_as_dash_qt_and_the_wallet() {
    use EnvelopeError::*;
    let draft = at(Stage::Draft);
    let cases: Vec<(&str, Box<dyn FnOnce(&mut Json)>, EnvelopeError)> = vec![
        ("type", Box::new(set(&["type"], Json::str("x"))), NotShared),
        (
            "version 2",
            Box::new(set(&["version"], Json::int(2))),
            Parse("version"),
        ),
        (
            "version 1.0",
            Box::new(set(&["version"], Json::Num("1.0".into()))),
            Parse("version"),
        ),
        (
            "version string",
            Box::new(set(&["version"], Json::str("1"))),
            Parse("version"),
        ),
        (
            "network",
            Box::new(set(&["network"], Json::str("test"))),
            Network,
        ),
        (
            "network number",
            Box::new(set(&["network"], Json::int(1))),
            Network,
        ),
        (
            "sessionId empty",
            Box::new(set(&["sessionId"], Json::str(""))),
            Parse("sessionId"),
        ),
        (
            "sessionId upper",
            Box::new(set(&["sessionId"], Json::str(&"AB".repeat(32)))),
            Parse("sessionId"),
        ),
        (
            "sessionId short",
            Box::new(set(&["sessionId"], Json::str("abcd"))),
            Parse("sessionId"),
        ),
        (
            "revision 0",
            Box::new(set(&["revision"], Json::int(0))),
            Parse("revision"),
        ),
        (
            "revision 2^31",
            Box::new(set(&["revision"], Json::uint(1 << 31))),
            Parse("revision"),
        ),
        (
            "stage",
            Box::new(set(&["stage"], Json::str("Draft"))),
            Parse("stage"),
        ),
        (
            "amount over",
            Box::new(set(&["shares", "0", "amount"], Json::int(MAX_MONEY + 1))),
            Parse("shares"),
        ),
        (
            "amount negative",
            Box::new(set(&["shares", "0", "amount"], Json::int(-1))),
            Parse("shares"),
        ),
        (
            "amount 1.5",
            Box::new(set(&["shares", "0", "amount"], Json::Num("1.5".into()))),
            Parse("shares"),
        ),
        (
            "owner missing",
            Box::new(set(&["shares", "0", "ownerAddress"], Json::Null)),
            Parse("shares"),
        ),
        (
            "period",
            Box::new(set(&["terms", "earlyPeriodBlocks"], Json::int(1 << 32))),
            Parse("terms"),
        ),
        (
            "penalty",
            Box::new(set(&["terms", "earlyPenalty"], Json::int(-1))),
            Parse("terms"),
        ),
        (
            "reward 2^31",
            Box::new(set(&["terms", "operatorReward"], Json::uint(1 << 31))),
            Parse("terms"),
        ),
        (
            "txid",
            Box::new(set(
                &["contributions", "0", "inputs", "0", "txid"],
                Json::str("00"),
            )),
            Parse("contributions"),
        ),
        (
            "vout",
            Box::new(set(
                &["contributions", "0", "inputs", "0", "vout"],
                Json::int(1 << 32),
            )),
            Parse("contributions"),
        ),
        (
            "sequence",
            Box::new(set(
                &["contributions", "0", "inputs", "0", "sequence"],
                Json::int(-1),
            )),
            Parse("contributions"),
        ),
        (
            "change",
            Box::new(set(
                &["contributions", "0", "change", "amount"],
                Json::str("1"),
            )),
            Parse("contributions"),
        ),
        (
            "consentHash",
            Box::new(set(&["consentHash"], Json::str("ab"))),
            Parse("consentHash"),
        ),
        (
            "label empty",
            Box::new(set(&["shares", "1", "label"], Json::str(" "))),
            Labels,
        ),
        (
            "label repeated",
            Box::new(set(&["shares", "1", "label"], Json::str(" s0\t"))),
            Labels,
        ),
        (
            "contribution unknown",
            Box::new(set(&["contributions", "1", "label"], Json::str("s9"))),
            Labels,
        ),
        (
            "contribution twice",
            Box::new(set(&["contributions", "1", "label"], Json::str("s0"))),
            Labels,
        ),
        (
            "frozen without protx",
            Box::new(set(&["stage"], Json::str("frozen"))),
            Parse("frozen"),
        ),
    ];
    for (name, f, want) in cases {
        assert_eq!(edit(&draft, f), Err(want), "{name}");
    }
    // Nine shares, and 65 inputs in one contribution.
    let mut many = draft.clone();
    many.shares = vec![many.shares[0].clone(); 9];
    assert_eq!(reparse(&many), Err(Parse("shares")));
    let mut inputs = draft.clone();
    inputs.contributions[0].inputs = vec![inputs.contributions[0].inputs[0].clone(); 65];
    assert_eq!(reparse(&inputs), Err(Parse("contributions")));
    // A duplicate share index in `sigs`.
    let mut dup = at(Stage::Signing);
    dup.sigs.push(dup.sigs[0].clone());
    assert_eq!(reparse(&dup), Err(Parse("sigs")));
}

#[test]
fn test_QT_126_reader_takes_what_dash_qt_takes() {
    let draft = at(Stage::Draft);
    // Members of the wrong type are skipped, as `if (v.isStr())` skips them.
    let read = edit(&draft, |j| {
        *member(j, &["fundingTx"]) = Json::int(1);
        *member(j, &["terms", "votingAddress"]) = Json::int(1);
        *member(j, &["shares", "0", "label"]) = Json::str("s0");
        *member(j, &["shares", "0", "rewardAddress"]) = Json::Null;
        *member(j, &["terms"]) = Json::Arr(Vec::new());
    })
    .unwrap();
    assert_eq!(read.funding_tx, "");
    assert_eq!(read.terms, Terms::default());
    assert_eq!(read.shares[0].reward_address, "");
    // An upper-case consent hash is stored lower-cased; an unknown key and
    // an edited code are read.
    let mut frozen = at(Stage::Frozen);
    frozen.consent_hash = frozen.consent_hash.to_ascii_uppercase();
    let read = edit(&frozen, |j| j.push_kv("extra", Json::Bool(true))).unwrap();
    assert_eq!(read, at(Stage::Frozen));
}

#[test]
fn test_QT_126_frozen_registration_must_be_what_the_envelope_shows() {
    use EnvelopeError::Parse;
    type Edit = fn(&mut Session);
    let mismatches: &[(&str, Edit, EnvelopeError)] = &[
        (
            "consent hash",
            |s| s.consent_hash = "00".repeat(32),
            Parse("consentHash"),
        ),
        (
            "protx not a registration",
            |s| s.protx = s.funding_tx.clone(),
            Parse("protx"),
        ),
        (
            "amount",
            |s| {
                s.shares[0].amount -= 1;
                s.shares[1].amount += 1;
            },
            Parse("protx"),
        ),
        (
            "owner",
            |s| s.shares[0].owner_address = s.shares[1].owner_address.clone(),
            Parse("protx"),
        ),
        (
            "refund",
            |s| s.shares[0].refund_address = s.shares[2].refund_address.clone(),
            Parse("protx"),
        ),
        (
            "reward",
            |s| s.shares[1].reward_address = s.shares[1].refund_address.clone(),
            Parse("protx"),
        ),
        (
            "reward cleared",
            |s| s.shares[0].reward_address.clear(),
            Parse("protx"),
        ),
        (
            "voting",
            |s| s.terms.voting_address = s.shares[0].owner_address.clone(),
            Parse("protx"),
        ),
        (
            "operator",
            |s| s.terms.operator_pub_key.replace_range(..2, "00"),
            Parse("protx"),
        ),
        (
            "operator reward",
            |s| s.terms.operator_reward = 500,
            Parse("protx"),
        ),
        (
            "period",
            |s| s.terms.early_period_blocks = 21,
            Parse("protx"),
        ),
        ("penalty", |s| s.terms.early_penalty += 1, Parse("protx")),
        (
            "service",
            |s| s.terms.core_p2p_addrs = "127.0.0.1:19902".into(),
            Parse("protx"),
        ),
        (
            "service twice",
            |s| s.terms.core_p2p_addrs = "127.0.0.1:19901,127.0.0.1:19901".into(),
            Parse("protx"),
        ),
        (
            "service empty",
            |s| s.terms.core_p2p_addrs.clear(),
            Parse("protx"),
        ),
        (
            "change address",
            |s| {
                s.contributions[1].change.as_mut().unwrap().address =
                    s.shares[1].owner_address.clone()
            },
            Parse("protx"),
        ),
        (
            "change amount",
            |s| s.contributions[1].change.as_mut().unwrap().amount += 1,
            Parse("protx"),
        ),
        (
            "sequence",
            |s| s.contributions[2].inputs[0].sequence = 0,
            Parse("protx"),
        ),
        (
            "contribution order",
            |s| s.contributions.swap(1, 2),
            Parse("protx"),
        ),
        (
            "collateral index",
            |s| s.collateral_index = 2,
            Parse("protx"),
        ),
        (
            "signature out of range",
            |s| {
                s.sigs.push(Signature {
                    share_index: 3,
                    signature: String::new(),
                })
            },
            Parse("sigs"),
        ),
    ];
    for (name, f, want) in mismatches {
        let mut session = at(Stage::Frozen);
        f(&mut session);
        assert_eq!(reparse(&session), Err(*want), "{name}");
    }
    // Case and separators of the terms don't matter where dash-qt's
    // comparison is canonical.
    let mut session = at(Stage::Frozen);
    session.terms.operator_pub_key = session.terms.operator_pub_key.to_ascii_uppercase();
    session.terms.core_p2p_addrs = " 127.0.0.1:19901 ,".into();
    session.contributions[0].inputs[0].txid =
        session.contributions[0].inputs[0].txid.to_ascii_uppercase();
    assert!(reparse(&session).is_ok());
}

#[test]
fn test_QT_126_combined_and_signed_stages_check_signatures_and_inputs() {
    // Combined: every share signed, each verifying.
    let mut missing = at(Stage::Combined);
    missing.sigs.pop();
    assert_eq!(reparse(&missing), Err(EnvelopeError::Parse("sigs")));
    let mut wrong = at(Stage::Combined);
    wrong.sigs[1].signature = wrong.sigs[0].signature.clone();
    assert_eq!(
        reparse(&wrong),
        Err(EnvelopeError::BadSignature { share: 1 })
    );
    let mut garbled = at(Stage::Combined);
    garbled.sigs[2].signature = encode_base64(&[1; 65]);
    assert_eq!(
        reparse(&garbled),
        Err(EnvelopeError::BadSignature { share: 2 })
    );
    // A signing copy stores signatures as they are.
    let mut signing = at(Stage::Signing);
    signing.sigs[1].signature = "x".into();
    assert!(reparse(&signing).is_ok());
    // fundingSigned: every input carries a scriptSig.
    let mut unsigned = at(Stage::FundingSigned);
    unsigned.protx = at(Stage::Combined).protx;
    assert_eq!(reparse(&unsigned), Err(EnvelopeError::Parse("scriptSig")));
    assert!(check_frozen(&at(Stage::Broadcast)).is_ok());
}
