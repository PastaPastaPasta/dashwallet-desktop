//! `dw-protx::shared` against Dash Core v24.0.0-rc.3's own shared-masternode
//! RPCs (MG-04b §12). The vectors come from
//! `testdata/oracle/dashd/gen_shared_vectors.py` on a v24 regtest:
//!
//! - testdata/shared_registration.json: `shared_register_prepare`,
//!   `shared_sign`, `shared_combine` and the funded registration, with
//!   dashd's verdict on it and on mutated join signatures;
//! - testdata/shared_maintenance.json: `shared_update_share`,
//!   `shared_update_registrar_prepare`, `shared_dissolve` (both
//!   `payPenalty`) and `shared_dissolve_prepare`, each with `testmempoolaccept`
//!   at its height, on it and on mutations, and `protx info` at that time;
//! - testdata/shared_dissolve_shapes.json (`--dissolve-shapes`): a third
//!   registration and `shared_dissolve` standbys with the actor holding the
//!   last share and its output omitted or dust.
//!
//! The wallet's codecs must read and re-write dashd's bytes, its digests
//! must equal dashd's sign hashes, its signatures over them must equal
//! dashd's (both sign with RFC 6979 nonces), and `shared::rules` must give
//! dashd's verdict and reason for every transaction and mutation.

#![allow(non_snake_case)] // test names carry checklist ids

use std::str::FromStr;

use dashcore::hashes::Hash;
use dashcore::{Address, Network, OutPoint, ScriptBuf, Txid};
use dw_protx::params::MASTERNODE_COLLATERAL;
use dw_protx::payloads::sign_hash_compact;
use dw_protx::rules::{ListedMasternode, signed_by};
use dw_protx::shared::dissolve::build_dissolution;
use dw_protx::shared::envelope::encode_base64;
use dw_protx::shared::netinfo::ExtNetInfo;
use dw_protx::shared::rawtx::SpecialTx;
use dw_protx::shared::registration::{NULL_JOIN_SIGNATURE, ProRegTxV3, SHARED_COLLATERAL_SCRIPT};
use dw_protx::shared::rules::{
    SharedMn, check_dissolution, check_registration, check_update_share,
    check_update_shared_registrar, verify_hash_canonical,
};
use dw_protx::shared::session::funding::{Frozen, verify_consent_signature};
use dw_protx::shared::standby::{self, Variant};
use dw_protx::shared::{
    CollateralShare, ProDisTx, ProUpShareTx, ProUpSharedRegTx, TRANSACTION_PROVIDER_DISSOLVE,
    TRANSACTION_PROVIDER_UPDATE_SHARE, TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR, payload_bytes,
};
use serde_json::Value;

fn load(name: &str) -> Value {
    let path = format!("{}/../../../testdata/{name}", env!("CARGO_MANIFEST_DIR"));
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("golden file")).unwrap();
    assert_eq!(doc["network"], "regtest");
    assert!(doc["source"].as_str().unwrap().contains("v24.0.0-rc.3"));
    doc
}

fn str_of<'a>(c: &'a Value, key: &str) -> &'a str {
    c[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is missing"))
}

fn tx_of(hex_str: &str) -> SpecialTx {
    let bytes = hex::decode(hex_str).unwrap();
    let tx = SpecialTx::from_bytes(&bytes).expect("dashd's transaction decodes");
    assert_eq!(tx.to_bytes(), bytes, "and re-encodes to the same bytes");
    tx
}

fn script_of(address: &str) -> ScriptBuf {
    Address::from_str(address)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap()
        .script_pubkey()
}

fn key_id_of(address: &str) -> [u8; 20] {
    let script = script_of(address);
    assert!(script.is_p2pkh(), "{address} is P2PKH");
    script.as_bytes()[3..23].try_into().unwrap()
}

fn secret_of(wif: &str) -> [u8; 32] {
    let secret = dw_uri::keyio::decode_secret(wif, Network::Regtest).expect("regtest WIF");
    assert!(secret.compressed);
    *secret.key
}

/// A `uint256::ToString()` as the 32 bytes it shows.
fn display(hex_str: &str) -> [u8; 32] {
    let mut h: [u8; 32] = hex::decode(hex_str).unwrap().try_into().unwrap();
    h.reverse();
    h
}

/// Padded standard base64, as `EncodeBase64` writes it.
fn base64(s: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.trim_end_matches('=').bytes() {
        let v = ALPHABET.iter().position(|&a| a == c).expect("base64") as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn sig_of(entry: &Value) -> [u8; 65] {
    base64(str_of(entry, "signature"))
        .try_into()
        .expect("65 bytes")
}

/// `protx shared_sign`'s entries, in share order.
fn signatures(c: &Value) -> Vec<[u8; 65]> {
    let entries = c["signatures"].as_array().unwrap();
    for (i, e) in entries.iter().enumerate() {
        assert_eq!(e["shareIndex"].as_u64(), Some(i as u64));
    }
    entries.iter().map(sig_of).collect()
}

fn operator_key(hex_str: &str) -> [u8; 48] {
    hex::decode(hex_str).unwrap().try_into().unwrap()
}

/// `testmempoolaccept`'s verdict as `shared::rules` returns it.
fn verdict(accept: &Value) -> Result<(), String> {
    if accept["allowed"].as_bool().unwrap() {
        Ok(())
    } else {
        Err(str_of(accept, "reject_reason").to_owned())
    }
}

// --- registration -------------------------------------------------------

fn registrations() -> Vec<Value> {
    let doc = load("shared_registration.json");
    let cases = doc["cases"].as_array().unwrap().clone();
    let names: Vec<&str> = cases.iter().map(|c| str_of(c, "name")).collect();
    assert_eq!(names, ["three_shares_early", "two_shares_no_early"]);
    cases
}

fn payload_of(tx: &SpecialTx) -> ProRegTxV3 {
    let p = ProRegTxV3::from_payload(&tx.payload).expect("a version-3 payload");
    assert_eq!(p.to_payload().unwrap(), tx.payload);
    p
}

/// The share table `shared_register_prepare` was given: an omitted reward
/// is an empty script, a given one (even the refund) its script.
fn shares_given(c: &Value) -> Vec<CollateralShare> {
    c["prepare_args"][1]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| CollateralShare {
            amount: s["amount"].as_u64().unwrap(),
            refund_script: script_of(str_of(s, "refundAddress")),
            reward_script: s
                .get("rewardAddress")
                .map_or_else(ScriptBuf::new, |a| script_of(a.as_str().unwrap())),
            owner_key_id: key_id_of(str_of(s, "ownerAddress")),
        })
        .collect()
}

#[test]
fn test_QT_126_prepared_registration_equals_dashds() {
    for c in registrations() {
        let name = str_of(&c, "name");
        let funding = tx_of(str_of(&c, "funding_tx_hex"));
        let prep = &c["prepare"];
        let tx = tx_of(str_of(prep, "tx"));
        let p = payload_of(&tx);

        // The template, as given, with the collateral appended last.
        assert_eq!((tx.version, tx.tx_type), (3, 1), "{name}");
        assert_eq!(tx.input, funding.input);
        assert_eq!(tx.lock_time, funding.lock_time);
        assert_eq!(tx.output[..funding.output.len()], funding.output[..]);
        let collateral = tx.output.last().unwrap();
        assert_eq!(collateral.value, MASTERNODE_COLLATERAL);
        assert_eq!(
            collateral.script_pubkey.as_bytes(),
            SHARED_COLLATERAL_SCRIPT
        );
        assert_eq!(tx.output.len(), funding.output.len() + 1);
        assert_eq!(
            prep["collateralIndex"].as_u64(),
            Some(funding.output.len() as u64)
        );

        // The payload is the terms given.
        let args = c["prepare_args"].as_array().unwrap();
        assert_eq!(p.shares, shares_given(&c), "{name}");
        assert_eq!(
            p.collateral,
            OutPoint::new(Txid::all_zeros(), funding.output.len() as u32)
        );
        let service = args[2].as_str().unwrap();
        let net_info = if service.is_empty() {
            ExtNetInfo::default()
        } else {
            ExtNetInfo::core_p2p(&[service.parse().unwrap()]).unwrap()
        };
        assert_eq!(p.net_info, net_info);
        assert_eq!(p.owner_key_id, [0; 20]);
        assert_eq!(
            p.operator_public_key,
            operator_key(str_of(&c, "operator_public"))
        );
        assert_eq!(p.voting_key_id, key_id_of(str_of(&c, "voting_address")));
        let reward = (args[5].as_str().unwrap().parse::<f64>().unwrap() * 100.0).round() as u16;
        assert_eq!(p.operator_reward, reward);
        assert!(p.payouts.is_empty());
        assert_eq!(p.early_period_blocks, args[6].as_u64().unwrap() as u32);
        assert_eq!(p.early_penalty, args[7].as_i64().unwrap());
        assert_eq!(p.inputs_hash, tx.inputs_hash());
        assert_eq!(p.inputs_hash, display(str_of(&prep["terms"], "inputsHash")));
        assert_eq!(p.join_sigs, vec![NULL_JOIN_SIGNATURE; p.shares.len()]);
        assert!(p.sig.is_empty());
        assert_eq!(p.platform_node_id, None);

        // dashd's consent digest, and its zero-penalty warning.
        assert_eq!(
            ProRegTxV3::display_hash(&p.consent_hash(&tx)),
            str_of(prep, "consentHash"),
            "{name}"
        );
        assert_eq!(
            prep.get("warning").is_some(),
            p.early_penalty == 0,
            "{name}"
        );
    }
}

#[test]
fn test_QT_126_join_signatures_equal_dashds() {
    for c in registrations() {
        let name = str_of(&c, "name");
        let prepared = tx_of(str_of(&c["prepare"], "tx"));
        let consent = payload_of(&prepared).consent_hash(&prepared);
        let sigs = signatures(&c);
        let parties = c["parties"].as_array().unwrap();
        assert_eq!(sigs.len(), parties.len());
        for ((party, sig), share) in parties.iter().zip(&sigs).zip(&payload_of(&prepared).shares) {
            let wallet = str_of(party, "wallet");
            // Each party's `shared_sign` signed its own share only.
            let own = c["shared_sign"][wallet]["signatures"].as_array().unwrap();
            assert_eq!(own.len(), 1, "{name} {wallet}");
            assert_eq!(str_of(&c["shared_sign"][wallet], "type"), "registration");
            assert_eq!(&sig_of(&own[0]), sig);
            let secret = secret_of(str_of(party, "owner_wif"));
            assert_eq!(
                &sign_hash_compact(&secret, &consent).unwrap(),
                sig,
                "{name} {wallet}"
            );
            assert!(verify_hash_canonical(consent, share.owner_key_id, sig));
        }

        // `shared_combine` fills the join signatures and nothing else; the
        // funding inputs' scriptSigs change nothing the payload covers.
        let combined = tx_of(str_of(&c, "combined_tx_hex"));
        let mut want = payload_of(&prepared);
        want.join_sigs = sigs.clone();
        assert_eq!(payload_of(&combined), want);
        assert_eq!(
            (combined.input.clone(), combined.output.clone()),
            (prepared.input.clone(), prepared.output.clone())
        );
        let funded = tx_of(str_of(&c, "tx_hex"));
        assert_eq!(payload_of(&funded), want);
        assert_eq!(funded.prevouts(), prepared.prevouts());
        assert_eq!(funded.output, prepared.output);
        assert!(funded.input.iter().all(|i| !i.script_sig.is_empty()));
        assert_eq!(verdict(&c["accept"]), Ok(()));
        // §5's rules accept what dashd accepted, and refuse the prepared
        // registration, whose join signatures are still null.
        let check = |tx: &SpecialTx| {
            check_registration(tx, &payload_of(tx), &[], true, Network::Regtest, true)
        };
        assert_eq!(check(&funded), Ok(()), "{name}");
        assert_eq!(check(&prepared), Err("bad-protx-shares-sig"), "{name}");
        assert_eq!(
            ProRegTxV3::pro_tx_hash(&funded).to_string(),
            str_of(&c, "pro_tx_hash")
        );
        assert_eq!(funded.txid().to_string(), str_of(&c, "pro_tx_hash"));
    }
}

#[test]
fn test_QT_126_mutated_join_signatures_fail_as_in_dashd() {
    for c in registrations() {
        let funded = tx_of(str_of(&c, "tx_hex"));
        let consent = payload_of(&funded).consent_hash(&funded);
        let mutations = c["mutations"].as_array().unwrap();
        assert_eq!(mutations.len(), 3);
        for m in mutations {
            let tx = tx_of(str_of(m, "tx_hex"));
            let p = payload_of(&tx);
            // The join signatures are outside the digest.
            assert_eq!(p.consent_hash(&tx), consent);
            let all_valid = p
                .shares
                .iter()
                .zip(&p.join_sigs)
                .all(|(s, sig)| verify_hash_canonical(consent, s.owner_key_id, sig));
            assert!(!all_valid, "{}", str_of(m, "mutation"));
            assert_eq!(verdict(m), Err("bad-protx-shares-sig".into()));
            // §5's rules refuse it with dashd's reason (review r1 of 1a, M9).
            assert_eq!(
                check_registration(&tx, &p, &[], true, Network::Regtest, true)
                    .map_err(str::to_owned),
                verdict(m),
                "{}",
                str_of(m, "mutation")
            );
        }
    }
}

// --- maintenance --------------------------------------------------------

/// The masternode as `protx info` shows it: Core's current share table
/// (the effective reward), terms and voting key.
fn shares_listed(info: &Value) -> Vec<CollateralShare> {
    info["state"]["shares"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| CollateralShare {
            amount: s["amount"].as_u64().unwrap(),
            refund_script: ScriptBuf::from_bytes(hex::decode(str_of(s, "refundScript")).unwrap()),
            reward_script: ScriptBuf::from_bytes(hex::decode(str_of(s, "rewardScript")).unwrap()),
            owner_key_id: key_id_of(str_of(s, "ownerAddress")),
        })
        .collect()
}

fn shared_mn<'a>(info: &Value, shares: &'a [CollateralShare]) -> SharedMn<'a> {
    let st = &info["state"];
    SharedMn {
        pro_tx_hash: Txid::from_str(str_of(info, "proTxHash")).unwrap(),
        collateral: OutPoint::new(
            Txid::from_str(str_of(info, "collateralHash")).unwrap(),
            info["collateralIndex"].as_u64().unwrap() as u32,
        ),
        shares,
        registered_height: st["registeredHeight"].as_u64().unwrap() as u32,
        early_period_blocks: st["earlyPeriodBlocks"].as_u64().unwrap() as u32,
        early_penalty: st["earlyPenalty"].as_i64().unwrap(),
        voting_key_id: key_id_of(str_of(st, "votingAddress")),
    }
}

fn listed(info: &Value) -> ListedMasternode {
    ListedMasternode {
        pro_tx_hash: Txid::from_str(str_of(info, "proTxHash"))
            .unwrap()
            .to_byte_array(),
        net_info: Vec::new(),
        operator_public_key: operator_key(str_of(&info["state"], "pubKeyOperator")),
        voting_key_id: key_id_of(str_of(&info["state"], "votingAddress")),
        platform_node_id: None,
        collateral: None,
    }
}

struct Maintenance {
    cases: Vec<Value>,
    /// Party wallet → owner secret, from registration A.
    owners: Vec<[u8; 32]>,
    /// Registration B's list entry: another masternode in the list.
    other: ListedMasternode,
}

fn maintenance() -> Maintenance {
    let regs = registrations();
    let doc = load("shared_maintenance.json");
    assert_eq!(doc["pro_tx_hash"], regs[0]["pro_tx_hash"]);
    let cases = doc["cases"].as_array().unwrap().clone();
    let names: Vec<&str> = cases.iter().map(|c| str_of(c, "name")).collect();
    assert_eq!(
        names,
        [
            "standby_penalty_early",
            "standby_no_penalty_early",
            "unanimous_early",
            "reward_new",
            "reward_refund",
            "operator_and_voting",
            "standby_no_penalty_late",
            "standby_no_early",
        ]
    );
    let owners = regs[0]["parties"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| secret_of(str_of(p, "owner_wif")))
        .collect();
    Maintenance {
        cases,
        owners,
        other: listed(&regs[1]["protx_info"]),
    }
}

/// `shared::rules` on `tx` as dashd's `testmempoolaccept` saw it: the
/// masternode from `protx info` at that time, mined at the next height.
fn rules_verdict(c: &Value, tx: &SpecialTx, list: &[ListedMasternode]) -> Result<(), String> {
    let info = &c["protx_info"];
    let shares = shares_listed(info);
    let mn = shared_mn(info, &shares);
    let spend_height = c["accept"]["tip_height"].as_u64().unwrap() as u32 + 1;
    let r = match tx.tx_type {
        TRANSACTION_PROVIDER_DISSOLVE => {
            let p: ProDisTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
            check_dissolution(tx, &p, Some(&mn), spend_height, true, true)
        }
        TRANSACTION_PROVIDER_UPDATE_SHARE => {
            let p: ProUpShareTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
            check_update_share(tx, &p, Some(&mn), true, true)
        }
        TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR => {
            let p: ProUpSharedRegTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
            check_update_shared_registrar(tx, &p, Some(&mn), list, true, true)
        }
        t => panic!("type {t}"),
    };
    r.map_err(str::to_owned)
}

#[test]
fn test_QT_127_maintenance_payloads_round_trip() {
    for c in maintenance().cases {
        let tx = tx_of(str_of(&c, "tx_hex"));
        assert_eq!(tx.version, 3);
        let bytes = match tx.tx_type {
            TRANSACTION_PROVIDER_DISSOLVE => {
                payload_bytes(&dashcore::consensus::deserialize::<ProDisTx>(&tx.payload).unwrap())
            }
            TRANSACTION_PROVIDER_UPDATE_SHARE => payload_bytes(
                &dashcore::consensus::deserialize::<ProUpShareTx>(&tx.payload).unwrap(),
            ),
            TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR => payload_bytes(
                &dashcore::consensus::deserialize::<ProUpSharedRegTx>(&tx.payload).unwrap(),
            ),
            t => panic!("type {t}"),
        };
        assert_eq!(bytes, tx.payload, "{}", str_of(&c, "name"));
    }
}

#[test]
fn test_QT_127_maintenance_digests_and_signatures_equal_dashds() {
    let m = maintenance();
    for c in &m.cases {
        let name = str_of(c, "name");
        let tx = tx_of(str_of(c, "tx_hex"));
        let shares = shares_listed(&c["protx_info"]);
        match str_of(c, "kind") {
            "dissolve_unilateral" => {
                let p: ProDisTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
                let actor = c["actor_index"].as_u64().unwrap() as usize;
                assert_eq!(usize::from(p.actor_index), actor);
                assert_eq!(p.sigs.len(), 1);
                let hash = p.sign_hash(tx.version, tx.lock_time, &tx.prevouts(), &tx.output, 1);
                if name != "standby_no_early" {
                    // Registration B's owners aren't in `owners`.
                    let mine = sign_hash_compact(&m.owners[actor], &hash).unwrap();
                    assert_eq!(mine, p.sigs[0], "{name}");
                }
                assert!(verify_hash_canonical(
                    hash,
                    shares[actor].owner_key_id,
                    &p.sigs[0]
                ));
            }
            "dissolve_unanimous" => {
                let prepared = tx_of(str_of(&c["prepare"], "tx"));
                let unsigned: ProDisTx =
                    dashcore::consensus::deserialize(&prepared.payload).unwrap();
                assert!(unsigned.sigs.is_empty());
                let n = shares.len() as u8;
                let hash = unsigned.sign_hash(
                    prepared.version,
                    prepared.lock_time,
                    &prepared.prevouts(),
                    &prepared.output,
                    n,
                );
                assert_eq!(hash, display(str_of(&c["prepare"], "signHash")), "{name}");
                let sigs = signatures(c);
                for (i, sig) in sigs.iter().enumerate() {
                    assert_eq!(&sign_hash_compact(&m.owners[i], &hash).unwrap(), sig);
                }
                let p: ProDisTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
                assert_eq!(p.sigs, sigs, "combined in share order");
                assert_eq!((&tx.input, &tx.output), (&prepared.input, &prepared.output));
            }
            "update_share" => {
                let p: ProUpShareTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
                let index = c["share_index"].as_u64().unwrap() as usize;
                assert_eq!(usize::from(p.share_index), index);
                // D31: the refund written out is its script, never empty.
                assert_eq!(p.reward_script, script_of(str_of(c, "reward_address")));
                assert_eq!(p.inputs_hash, tx.inputs_hash());
                let mine = sign_hash_compact(&m.owners[index], &p.sign_hash()).unwrap();
                assert_eq!(mine[..], p.sig[..], "{name}");
                // Core's list shows the new reward after a block.
                let after = shares_listed(&c["protx_info_after"]);
                assert_eq!(after[index].reward_script, p.reward_script);
            }
            "update_registrar" => {
                let prepared = tx_of(str_of(&c["prepare"], "tx"));
                let unsigned: ProUpSharedRegTx =
                    dashcore::consensus::deserialize(&prepared.payload).unwrap();
                assert_eq!(unsigned.sigs, vec![[0u8; 65]; shares.len()]);
                assert_eq!(unsigned.inputs_hash, prepared.inputs_hash());
                assert_eq!(
                    unsigned.operator_public_key,
                    operator_key(str_of(c, "operator_public"))
                );
                assert_eq!(
                    unsigned.voting_key_id,
                    key_id_of(str_of(c, "voting_address"))
                );
                let hash = unsigned.sign_hash();
                assert_eq!(hash, display(str_of(&c["prepare"], "signHash")), "{name}");
                let sigs = signatures(c);
                for (i, sig) in sigs.iter().enumerate() {
                    assert_eq!(&sign_hash_compact(&m.owners[i], &hash).unwrap(), sig);
                }
                // Combining fills the signatures; the fee inputs are
                // re-signed, the prevouts and outputs kept.
                let p: ProUpSharedRegTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
                assert_eq!(p, ProUpSharedRegTx { sigs, ..unsigned });
                assert_eq!(tx.prevouts(), prepared.prevouts());
                assert_eq!(tx.output, prepared.output);
                let after = &c["protx_info_after"]["state"];
                assert_eq!(str_of(after, "votingAddress"), str_of(c, "voting_address"));
                assert_eq!(
                    str_of(after, "pubKeyOperator"),
                    str_of(c, "operator_public")
                );
            }
            k => panic!("kind {k}"),
        }
    }
}

#[test]
fn test_QT_127_build_dissolution_equals_dashds_shared_dissolve() {
    let m = maintenance();
    let mut rebuilt = 0;
    for c in &m.cases {
        if str_of(c, "kind") != "dissolve_unilateral" {
            continue;
        }
        let name = str_of(c, "name");
        let tx = tx_of(str_of(c, "tx_hex"));
        let shares = shares_listed(&c["protx_info"]);
        let mn = shared_mn(&c["protx_info"], &shares);
        let actor = c["actor_index"].as_u64().unwrap() as u16;
        // `DissolveShared`: `pay_penalty` takes `nEarlyPenalty`, else 0.
        let penalty = if c["pay_penalty"].as_bool().unwrap() {
            mn.early_penalty as u64
        } else {
            0
        };
        let built = build_dissolution(&mn, actor, penalty, c["fee"].as_u64().unwrap()).unwrap();
        let p: ProDisTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
        assert_eq!(built.signed(p.sigs.clone()), tx, "{name}");
        if name != "standby_no_early" {
            // Registration B's owners aren't in `owners`.
            let sig =
                sign_hash_compact(&m.owners[usize::from(actor)], &built.sign_hash(1)).unwrap();
            assert_eq!(built.signed(vec![sig]).to_bytes(), tx.to_bytes(), "{name}");
        }
        rebuilt += 1;
    }
    assert_eq!(rebuilt, 4);
}

/// The shapes the maintenance file lacks (MG-04b 2a review r1 n-3): the
/// actor holding the last share, and its output omitted (the penalty and
/// fee take its share) or left dust. Rebuilt and signed byte for byte, and
/// Core's rules give dashd's verdicts, except that dashd refuses the dust
/// one by relay policy (`dust`), which the rules don't check (D83).
#[test]
fn test_QT_127_build_dissolution_equals_dashds_in_every_shape() {
    let doc = load("shared_dissolve_shapes.json");
    let owners: Vec<[u8; 32]> = doc["registration"]["parties"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| secret_of(str_of(p, "owner_wif")))
        .collect();
    let cases = doc["cases"].as_array().unwrap();
    let names: Vec<&str> = cases.iter().map(|c| str_of(c, "name")).collect();
    assert_eq!(
        names,
        [
            "last_actor_penalty",
            "last_actor_omitted",
            "middle_actor_omitted",
            "last_actor_dust",
        ]
    );
    for c in cases {
        let name = str_of(c, "name");
        let tx = tx_of(str_of(c, "tx_hex"));
        let shares = shares_listed(&c["protx_info"]);
        let mn = shared_mn(&c["protx_info"], &shares);
        let actor = c["actor_index"].as_u64().unwrap() as u16;
        assert!(c["pay_penalty"].as_bool().unwrap(), "{name}");
        let built = build_dissolution(
            &mn,
            actor,
            mn.early_penalty as u64,
            c["fee"].as_u64().unwrap(),
        )
        .unwrap();
        let sig = sign_hash_compact(&owners[usize::from(actor)], &built.sign_hash(1)).unwrap();
        assert_eq!(built.signed(vec![sig]).to_bytes(), tx.to_bytes(), "{name}");
        let (kept, back) = match name {
            "last_actor_penalty" => (true, 40_000),
            "last_actor_dust" => (true, 100),
            _ => (false, 0),
        };
        assert_eq!(built.actor_output, back, "{name}");
        assert_eq!(tx.output.len(), shares.len() - usize::from(!kept), "{name}");
        if kept {
            let last = tx.output.last().unwrap();
            assert_eq!(last.value, back, "{name}");
            assert_eq!(last.script_pubkey, shares[usize::from(actor)].refund_script);
        }
        let mutations = c["mutations"].as_array().unwrap();
        if name == "last_actor_dust" {
            assert_eq!(rules_verdict(c, &tx, &[]), Ok(()));
            assert_eq!(verdict(&c["accept"]), Err("dust".to_owned()));
            assert!(
                mutations
                    .iter()
                    .all(|m| verdict(m) == Err("dust".to_owned()))
            );
            continue;
        }
        assert_eq!(rules_verdict(c, &tx, &[]), verdict(&c["accept"]), "{name}");
        for m in mutations {
            let mut at = c.clone();
            at["accept"] = m.clone();
            let mutated = tx_of(str_of(m, "tx_hex"));
            assert_eq!(
                rules_verdict(&at, &mutated, &[]),
                verdict(m),
                "{name} {}",
                str_of(m, "mutation")
            );
        }
    }
}

#[test]
fn test_QT_127_rules_give_dashds_verdicts_and_reasons() {
    let m = maintenance();
    let mut checked = 0;
    for c in &m.cases {
        let name = str_of(c, "name");
        let own = listed(&c["protx_info"]);
        let list = [own, m.other.clone()];
        let tx = tx_of(str_of(c, "tx_hex"));
        assert_eq!(
            rules_verdict(c, &tx, &list),
            verdict(&c["accept"]),
            "{name}"
        );
        checked += 1;
        for mutation in c
            .get("mutations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let what = str_of(mutation, "mutation");
            let mut at = c.clone();
            at["accept"] = mutation.clone();
            let mutated = tx_of(str_of(mutation, "tx_hex"));
            assert_eq!(
                rules_verdict(&at, &mutated, &list),
                verdict(mutation),
                "{name} {what}"
            );
            checked += 1;
        }
    }
    // Eight transactions and their 30 mutations.
    assert_eq!(checked, 38);
}

#[test]
fn test_QT_127_zero_penalty_standby_valid_from_the_early_periods_end() {
    let m = maintenance();
    let early = &m.cases[1];
    let late = &m.cases[6];
    assert_eq!(late["tx_hex"], early["tx_hex"]);
    let registered = early["protx_info"]["state"]["registeredHeight"]
        .as_u64()
        .unwrap();
    let period = early["protx_info"]["state"]["earlyPeriodBlocks"]
        .as_u64()
        .unwrap();
    assert!(early["accept"]["tip_height"].as_u64().unwrap() + 1 < registered + period);
    assert_eq!(
        verdict(&early["accept"]),
        Err("bad-prodis-penalty-floor".into())
    );
    assert_eq!(
        late["accept"]["tip_height"].as_u64().unwrap() + 1,
        registered + period
    );
    assert_eq!(verdict(&late["accept"]), Ok(()));
    // Mined, it takes the masternode out of the list.
    assert_eq!(late["listed_after"], false);
}

/// Every unilateral `shared_dissolve` dashd signed (with `submit=false`,
/// the standby `createStandby` writes) passes the import's check (MG-04b
/// D89) as the variant its `payPenalty` chose, and is the matching half of
/// [`standby::pair`] for its actor and fee, signed; the full variant is
/// valid where dashd accepts it. Every mutation dashd refuses, the check
/// refuses. The dust shape passes: relay policy is the send's (D83).
#[test]
fn test_QT_127_standby_pair_equals_dashds_standby() {
    let shapes = load("shared_dissolve_shapes.json");
    let cases = maintenance()
        .cases
        .into_iter()
        .chain(shapes["cases"].as_array().unwrap().iter().cloned())
        .filter(|c| c.get("kind").is_none_or(|k| k == "dissolve_unilateral"));
    let mut checked = 0;
    let mut mutations = 0;
    for c in cases {
        let name = str_of(&c, "name");
        let tx = tx_of(str_of(&c, "tx_hex"));
        let shares = shares_listed(&c["protx_info"]);
        let mn = shared_mn(&c["protx_info"], &shares);
        let spend_height = c["accept"]["tip_height"].as_u64().unwrap() as u32 + 1;
        let actor = c["actor_index"].as_u64().unwrap() as u16;
        let fee = c["fee"].as_u64().unwrap();
        let variant = if c["pay_penalty"].as_bool().unwrap() && mn.early_penalty > 0 {
            Variant::Immediate
        } else {
            Variant::Full
        };
        let got =
            standby::check(&tx, &mn, spend_height).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(
            (got.variant, got.actor, got.fee),
            (variant, actor, fee),
            "{name}"
        );

        let p: ProDisTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
        let pair = standby::pair(&mn, actor, fee).unwrap();
        assert_eq!(pair.immediate.is_none(), mn.early_penalty == 0, "{name}");
        let half = match variant {
            Variant::Full => &pair.full,
            Variant::Immediate => pair.immediate.as_ref().unwrap(),
        };
        assert_eq!(half.signed(p.sigs.clone()), tx, "{name}");
        assert_eq!(got.actor_output, half.actor_output, "{name}");
        if variant == Variant::Full {
            assert_eq!(
                standby::full_valid(&mn, spend_height),
                verdict(&c["accept"]).is_ok(),
                "{name}"
            );
        }
        checked += 1;

        for m in c
            .get("mutations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if verdict(m).is_ok() || name == "last_actor_dust" {
                continue;
            }
            let mutated = tx_of(str_of(m, "tx_hex"));
            assert!(
                standby::check(&mutated, &mn, spend_height).is_err(),
                "{name} {}",
                str_of(m, "mutation")
            );
            mutations += 1;
        }
    }
    assert_eq!(checked, 8);
    assert_eq!(mutations, 24);
    println!("{checked} standbys, {mutations} refused mutations");
}

/// dashd's own funding signatures on each registration: every input's
/// scriptSig (`<sig> <pubkey>`, P2PKH) verifies against
/// `SpecialTx::signature_hash_all`, the payload included, and is
/// `SIGHASH_ALL`. A changed payload byte breaks every one.
#[test]
fn test_QT_126_funding_signatures_verify_against_the_special_tx_sighash() {
    use dashcore::secp256k1::{Message, PublicKey, ecdsa};
    let mut checked = 0;
    for c in registrations() {
        let tx = tx_of(str_of(&c, "tx_hex"));
        let mut tampered = tx.clone();
        *tampered.payload.last_mut().unwrap() ^= 1;
        for (i, input) in tx.input.iter().enumerate() {
            let pushes: Vec<Vec<u8>> = input
                .script_sig
                .instructions()
                .map(|ins| match ins.unwrap() {
                    dashcore::blockdata::script::Instruction::PushBytes(b) => b.as_bytes().to_vec(),
                    op => panic!("not a push: {op:?}"),
                })
                .collect();
            let [sig, pubkey] = &pushes[..] else {
                panic!("P2PKH scriptSig")
            };
            let (hash_type, der) = sig.split_last().unwrap();
            assert_eq!(u32::from(*hash_type), dw_protx::shared::rawtx::SIGHASH_ALL);
            let pubkey = PublicKey::from_slice(pubkey).unwrap();
            let script_code = ScriptBuf::new_p2pkh(&dashcore::PublicKey::new(pubkey).pubkey_hash());
            let sig = ecdsa::Signature::from_der(der).unwrap();
            let digest = |t: &SpecialTx| {
                Message::from_digest(t.signature_hash_all(i, &script_code).unwrap())
            };
            ecdsa::verify(&sig, digest(&tx), &pubkey).unwrap_or_else(|e| panic!("input {i}: {e}"));
            assert!(ecdsa::verify(&sig, digest(&tampered), &pubkey).is_err());
            checked += 1;
        }
        assert_eq!(
            tx.signature_hash_all(tx.input.len(), &ScriptBuf::new()),
            None
        );
    }
    assert!(checked >= 5, "{checked}");
}

/// `(r, n − s)` with the other recovery parity: the same key recovers
/// (Core's legacy `VerifyHash` takes it), but S is high.
fn high_s(sig: &[u8; 65]) -> [u8; 65] {
    const N: [u8; 32] = [
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36,
        0x41, 0x41,
    ];
    let mut high = *sig;
    let mut borrow = 0i16;
    for i in (0..32).rev() {
        let d = i16::from(N[i]) - i16::from(sig[33 + i]) - borrow;
        high[33 + i] = d.rem_euclid(256) as u8;
        borrow = i16::from(d < 0);
    }
    high[0] = 27 + ((sig[0] - 27) ^ 1);
    high
}

/// The forms `CHashSigner::VerifyHashCanonical` (rc.3
/// `messagesigner.cpp:58-88`) refuses: high S, a header aliased past 34
/// (`RecoverCompact` masks it, so the same key recovers) and one below 27.
fn non_canonical(sig: &[u8; 65], hash: [u8; 32], key_id: [u8; 20]) -> Vec<[u8; 65]> {
    let high = high_s(sig);
    assert!(signed_by(&high, hash, key_id), "high S recovers the key");
    let mut alias = *sig;
    alias[0] += 8;
    let mut below = *sig;
    below[0] = 26;
    vec![high, alias, below]
}

/// Every v24 shared-masternode signature path checks canonically: dashd's
/// own signatures pass, and their high-S and out-of-range-header forms
/// are refused with Core's reason (join `specialtxman.cpp:1305-1317`,
/// ProUpShareTx, ProUpSharedRegTx, ProDisTx), and as a session's consent.
#[test]
fn test_QT_126_shared_signatures_are_checked_canonically() {
    let mut refused = 0;
    for c in registrations() {
        let name = str_of(&c, "name");
        let funded = tx_of(str_of(&c, "tx_hex"));
        let p = payload_of(&funded);
        let consent = p.consent_hash(&funded);
        let check = |q: &ProRegTxV3| {
            let mut t = funded.clone();
            t.payload = q.to_payload().unwrap();
            check_registration(&t, q, &[], true, Network::Regtest, true)
        };
        assert_eq!(check(&p), Ok(()), "{name}");
        let frozen = Frozen {
            tx: funded.clone(),
            payload: p.clone(),
            consent_hash: consent,
        };
        for (i, sig) in p.join_sigs.iter().enumerate() {
            let owner = p.shares[i].owner_key_id;
            assert!(verify_consent_signature(
                &frozen,
                i as i32,
                &encode_base64(sig)
            ));
            for bad in non_canonical(sig, consent, owner) {
                let mut q = p.clone();
                q.join_sigs[i] = bad;
                assert_eq!(check(&q), Err("bad-protx-shares-sig"), "{name} {i}");
                assert!(!verify_consent_signature(
                    &frozen,
                    i as i32,
                    &encode_base64(&bad)
                ));
                refused += 1;
            }
        }
    }

    let m = maintenance();
    let mut kinds = std::collections::BTreeSet::new();
    for c in &m.cases {
        if verdict(&c["accept"]).is_err() {
            continue;
        }
        let name = str_of(c, "name");
        let own = listed(&c["protx_info"]);
        let list = [own, m.other.clone()];
        let tx = tx_of(str_of(c, "tx_hex"));
        let shares = shares_listed(&c["protx_info"]);
        assert_eq!(rules_verdict(c, &tx, &list), Ok(()), "{name}");
        let with = |payload: Vec<u8>| {
            let mut t = tx.clone();
            t.payload = payload;
            rules_verdict(c, &t, &list)
        };
        match tx.tx_type {
            TRANSACTION_PROVIDER_UPDATE_SHARE => {
                let p: ProUpShareTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
                let sig: [u8; 65] = p.sig[..].try_into().unwrap();
                let owner = shares[usize::from(p.share_index)].owner_key_id;
                for bad in non_canonical(&sig, p.sign_hash(), owner) {
                    let mut q = p.clone();
                    q.sig = bad.to_vec();
                    assert_eq!(
                        with(payload_bytes(&q)),
                        Err("bad-proupshare-sig".into()),
                        "{name}"
                    );
                    refused += 1;
                }
            }
            TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR => {
                let p: ProUpSharedRegTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
                for (i, sig) in p.sigs.iter().enumerate() {
                    for bad in non_canonical(sig, p.sign_hash(), shares[i].owner_key_id) {
                        let mut q = p.clone();
                        q.sigs[i] = bad;
                        assert_eq!(
                            with(payload_bytes(&q)),
                            Err("bad-proupsharedreg-sig".into()),
                            "{name} {i}"
                        );
                        refused += 1;
                    }
                }
            }
            TRANSACTION_PROVIDER_DISSOLVE => {
                let p: ProDisTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
                let n = p.sigs.len() as u8;
                let hash = p.sign_hash(tx.version, tx.lock_time, &tx.prevouts(), &tx.output, n);
                for (i, sig) in p.sigs.iter().enumerate() {
                    let signer = if n == 1 {
                        usize::from(p.actor_index)
                    } else {
                        i
                    };
                    for bad in non_canonical(sig, hash, shares[signer].owner_key_id) {
                        let mut q = p.clone();
                        q.sigs[i] = bad;
                        assert_eq!(
                            with(payload_bytes(&q)),
                            Err("bad-prodis-sig".into()),
                            "{name} {i}"
                        );
                        refused += 1;
                    }
                }
            }
            t => panic!("type {t}"),
        }
        kinds.insert(tx.tx_type);
    }
    assert_eq!(
        kinds.into_iter().collect::<Vec<_>>(),
        [
            TRANSACTION_PROVIDER_DISSOLVE,
            TRANSACTION_PROVIDER_UPDATE_SHARE,
            TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR
        ],
        "every maintenance path has a valid dashd case"
    );
    assert!(refused > 0);
}
