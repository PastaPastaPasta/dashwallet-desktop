//! MG-04b D2, D62: dw-protx's shared-masternode codecs and the dashcore
//! pin's (BL-187: `ProviderRegistrationPayload` v3 with its
//! `SharedCollateral`, `ProviderDissolvePayload`,
//! `ProviderUpdateSharePayload`, `ProviderUpdateSharedRegistrarPayload`)
//! read every transaction dashd made for the shared vectors alike: the
//! same bytes back, the same txid, the same payload bytes, and the same
//! fields. The wallet keeps its own codecs (D2); this is what lets 1b hand
//! a `SpecialTx` to the network as a `dashcore::Transaction`.

#![allow(non_snake_case)]

use dashcore::consensus::{deserialize, serialize};
use dashcore::hashes::Hash;
use dashcore::sml::masternode_list_entry::MasternodeNetInfo;
use dashcore::transaction::special_transaction::TransactionPayload;
use dw_protx::shared::netinfo::{NetInfoEntry, PURPOSE_CORE_P2P};
use dw_protx::shared::{ProDisTx, ProRegTxV3, ProUpShareTx, ProUpSharedRegTx, SpecialTx};
use serde_json::Value;

fn load(name: &str) -> Value {
    let path = format!("{}/../../../testdata/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).expect("golden file")).unwrap()
}

/// Every transaction hex in a vector file: the templates, the combined and
/// final registrations, the maintenance transactions and their mutations.
fn transactions(doc: &Value) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for case in doc["cases"].as_array().unwrap() {
        let mut hexes = vec![
            &case["tx_hex"],
            &case["combined_tx_hex"],
            &case["prepare"]["tx"],
        ];
        if let Some(mutations) = case["mutations"].as_array() {
            hexes.extend(mutations.iter().map(|m| &m["tx_hex"]));
        }
        out.extend(
            hexes
                .into_iter()
                .filter_map(Value::as_str)
                .map(|h| hex::decode(h).unwrap()),
        );
    }
    out
}

fn sigs<T: AsRef<[u8]>>(sigs: &[T]) -> Vec<Vec<u8>> {
    sigs.iter().map(|s| s.as_ref().to_vec()).collect()
}

#[test]
fn test_QT_126_dashcore_reads_the_shared_transactions_as_dw_protx_does() {
    let mut seen = [0usize; 4];
    for name in ["shared_registration.json", "shared_maintenance.json"] {
        for bytes in transactions(&load(name)) {
            let ours = SpecialTx::from_bytes(&bytes).unwrap();
            let theirs: dashcore::Transaction = deserialize(&bytes).unwrap();
            assert_eq!(serialize(&ours), bytes);
            assert_eq!(serialize(&theirs), bytes);
            assert_eq!(theirs.txid(), ours.txid());
            let payload = theirs
                .special_transaction_payload
                .expect("a special transaction");
            match payload {
                TransactionPayload::ProviderRegistrationPayloadType(p) => {
                    let q = ProRegTxV3::from_payload(&ours.payload).unwrap();
                    assert_eq!(serialize(&p), ours.payload);
                    assert_eq!(p.version, 3);
                    assert_eq!(p.collateral_outpoint, q.collateral);
                    let MasternodeNetInfo::Extended(net) = &p.service_address else {
                        panic!("a version-3 registration has extended net info");
                    };
                    assert_eq!(net.version, q.net_info.version);
                    let counts: Vec<usize> = net.purposes.iter().map(|(_, e)| e.len()).collect();
                    let ours: Vec<usize> =
                        q.net_info.entries.iter().map(|(_, e)| e.len()).collect();
                    assert_eq!(counts, ours);
                    let primary = match q.net_info.purpose(PURPOSE_CORE_P2P).first() {
                        Some(NetInfoEntry::Service(s)) => s.socket_addr(),
                        _ => None,
                    };
                    assert_eq!(p.service_address.primary_service_address(), primary);
                    assert_eq!(p.owner_key_hash.to_byte_array(), q.owner_key_id);
                    assert_eq!(p.operator_public_key.as_ref(), &q.operator_public_key);
                    assert_eq!(p.voting_key_hash.to_byte_array(), q.voting_key_id);
                    assert_eq!(p.inputs_hash.to_byte_array(), q.inputs_hash);
                    let shared = p.shared_collateral.expect("a shared registration");
                    assert_eq!(shared.shares.len(), q.shares.len());
                    for (a, b) in shared.shares.iter().zip(&q.shares) {
                        assert_eq!(u64::try_from(a.amount).unwrap(), b.amount);
                        assert_eq!(a.script_refund, b.refund_script);
                        assert_eq!(a.script_reward, b.reward_script);
                        assert_eq!(a.owner_key_hash.to_byte_array(), b.owner_key_id);
                    }
                    assert_eq!(sigs(&shared.join_sigs), sigs(&q.join_sigs));
                    assert_eq!(shared.early_period_blocks, q.early_period_blocks);
                    assert_eq!(shared.early_penalty, q.early_penalty);
                    seen[0] += 1;
                }
                TransactionPayload::ProviderDissolvePayloadType(p) => {
                    let q: ProDisTx = deserialize(&ours.payload).unwrap();
                    assert_eq!(serialize(&p), ours.payload);
                    assert_eq!(
                        (p.version, p.pro_tx_hash, p.actor_index),
                        (q.version, q.pro_tx_hash, q.actor_index)
                    );
                    assert_eq!(sigs(&p.signatures), sigs(&q.sigs));
                    seen[1] += 1;
                }
                TransactionPayload::ProviderUpdateSharePayloadType(p) => {
                    let q: ProUpShareTx = deserialize(&ours.payload).unwrap();
                    assert_eq!(serialize(&p), ours.payload);
                    assert_eq!(
                        (p.version, p.pro_tx_hash, p.share_index),
                        (q.version, q.pro_tx_hash, q.share_index)
                    );
                    assert_eq!(p.script_reward, q.reward_script);
                    assert_eq!(p.inputs_hash.to_byte_array(), q.inputs_hash);
                    assert_eq!(p.payload_sig, q.sig);
                    seen[2] += 1;
                }
                TransactionPayload::ProviderUpdateSharedRegistrarPayloadType(p) => {
                    let q: ProUpSharedRegTx = deserialize(&ours.payload).unwrap();
                    assert_eq!(serialize(&p), ours.payload);
                    assert_eq!((p.version, p.pro_tx_hash), (q.version, q.pro_tx_hash));
                    assert_eq!(p.operator_public_key.as_ref(), &q.operator_public_key);
                    assert_eq!(p.voting_key_hash.to_byte_array(), q.voting_key_id);
                    assert_eq!(p.inputs_hash.to_byte_array(), q.inputs_hash);
                    assert_eq!(sigs(&p.signatures), sigs(&q.sigs));
                    seen[3] += 1;
                }
                other => panic!("dashcore read type {} otherwise: {other:?}", ours.tx_type),
            }
        }
    }
    assert!(
        seen.iter().all(|n| *n > 0),
        "every payload type is covered: {seen:?}"
    );
}
