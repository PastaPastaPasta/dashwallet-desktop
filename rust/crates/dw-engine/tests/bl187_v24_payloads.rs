//! BL-187: the pinned dashcore and dash-spv read Dash Core v24 blocks.
//!
//! At rust-dashcore 8fe0a381, a block holding a version-3 provider transaction or one of
//! the special types 10–12 did not decode, and dash-spv drops a P2P message it can't
//! decode, so wallet sync stalled at the first such block. The vectors in
//! testdata/core_v24_payload_blocks.json are blocks dashd v24.0.0-rc.3 mined while
//! running Core's own functional tests (testdata/oracle/core_v24/).

use std::str::FromStr;

use dash_spv::network::Peer;
use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
use dashcore::blockdata::transaction::special_transaction::TransactionType;
use dashcore::consensus::{deserialize, serialize};
use dashcore::hashes::{Hash, sha256d};
use dashcore::network::message::NetworkMessage;
use dashcore::sml::masternode_list_entry::MasternodeNetInfo;
use dashcore::sml::masternode_list_entry::net_info::NetInfoPurpose;
use dashcore::{Block, BlockHash, Network, Transaction, Txid};
use serde_json::Value;
use tokio::io::AsyncWriteExt;

/// A type Core v24 doesn't define (its last is 12).
const UNKNOWN_TYPE: u16 = 13;

fn vectors() -> Vec<Value> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../testdata/core_v24_payload_blocks.json"
    );
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(doc["generator"].as_str().unwrap().contains("v24.0.0-rc.3"));
    doc["blocks"].as_array().unwrap().clone()
}

fn bytes(v: &Value) -> Vec<u8> {
    hex::decode(v["hex"].as_str().unwrap()).unwrap()
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_else(|| panic!("no {key} in {v}"))
}

/// Entries per purpose of a version-3 payload's net info, keyed as Core's JSON keys them.
fn address_counts(net_info: &MasternodeNetInfo) -> Vec<(&'static str, usize)> {
    let MasternodeNetInfo::Extended(info) = net_info else {
        panic!("version 3 has extended net info: {net_info:?}");
    };
    let mut counts: Vec<_> = info
        .purposes
        .iter()
        .map(|(purpose, entries)| {
            let key = match purpose {
                NetInfoPurpose::CoreP2P => "core_p2p",
                NetInfoPurpose::PlatformP2P => "platform_p2p",
                NetInfoPurpose::PlatformHttps => "platform_https",
            };
            (key, entries.len())
        })
        .collect();
    counts.sort();
    counts
}

fn core_address_counts(payload: &Value) -> Vec<(&str, usize)> {
    let mut counts: Vec<_> = payload["addresses"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_array().unwrap().len()))
        .collect();
    counts.sort();
    counts
}

/// The provider transaction `special` describes decodes to its typed payload, with
/// the fields Core's JSON shows.
fn check_special(tx: &Transaction, special: &Value) {
    let core = &special["payload"];
    let version = core["version"].as_u64().unwrap() as u16;
    let pro_tx_hash = |h: &Txid| assert_eq!(h.to_string(), str_of(core, "proTxHash"));
    match tx.special_transaction_payload.as_ref().unwrap() {
        TransactionPayload::ProviderRegistrationPayloadType(p) => {
            assert_eq!(special["type"], 1);
            assert_eq!(p.version, version);
            assert_eq!(
                address_counts(&p.service_address),
                core_address_counts(core)
            );
            match core.get("shares") {
                Some(shares) => {
                    let shared = p.shared_collateral.as_ref().unwrap();
                    assert_eq!(shared.shares.len(), shares.as_array().unwrap().len());
                    for (ours, theirs) in shared.shares.iter().zip(shares.as_array().unwrap()) {
                        assert_eq!(ours.amount, theirs["amount"].as_i64().unwrap());
                        assert_eq!(
                            hex::encode(ours.script_refund.as_bytes()),
                            str_of(theirs, "refundScript")
                        );
                    }
                }
                None => {
                    let payouts = p.payouts.as_ref().unwrap();
                    let core_payouts = core["payouts"].as_array().unwrap();
                    assert_eq!(payouts.len(), core_payouts.len());
                    for (ours, theirs) in payouts.iter().zip(core_payouts) {
                        assert_eq!(
                            hex::encode(ours.script_payout.as_bytes()),
                            str_of(theirs, "script")
                        );
                        assert_eq!(u64::from(ours.reward), theirs["reward"].as_u64().unwrap());
                    }
                }
            }
        }
        TransactionPayload::ProviderUpdateServicePayloadType(p) => {
            assert_eq!(special["type"], 2);
            assert_eq!(p.version, version);
            pro_tx_hash(&p.pro_tx_hash);
            assert_eq!(
                address_counts(&p.service_address),
                core_address_counts(core)
            );
        }
        TransactionPayload::ProviderUpdateRegistrarPayloadType(p) => {
            assert_eq!(special["type"], 3);
            assert_eq!(p.version, version);
            pro_tx_hash(&p.pro_tx_hash);
            let scripts: Vec<String> = p
                .payouts
                .as_ref()
                .unwrap()
                .iter()
                .map(|s| hex::encode(s.script_payout.as_bytes()))
                .collect();
            let core_scripts: Vec<&str> = core["payouts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| str_of(s, "script"))
                .collect();
            assert_eq!(scripts, core_scripts);
        }
        TransactionPayload::ProviderDissolvePayloadType(p) => {
            assert_eq!(special["type"], 10);
            assert_eq!(p.version, version);
            pro_tx_hash(&p.pro_tx_hash);
            assert_eq!(
                u64::from(p.actor_index),
                core["actorIndex"].as_u64().unwrap()
            );
            assert_eq!(
                p.signatures.len() as u64,
                core["sigCount"].as_u64().unwrap()
            );
        }
        TransactionPayload::ProviderUpdateSharePayloadType(p) => {
            assert_eq!(special["type"], 11);
            assert_eq!(p.version, version);
            pro_tx_hash(&p.pro_tx_hash);
            assert_eq!(
                u64::from(p.share_index),
                core["shareIndex"].as_u64().unwrap()
            );
        }
        TransactionPayload::ProviderUpdateSharedRegistrarPayloadType(p) => {
            assert_eq!(special["type"], 12);
            assert_eq!(p.version, version);
            pro_tx_hash(&p.pro_tx_hash);
            assert_eq!(
                hex::encode(p.operator_public_key.as_ref()),
                str_of(core, "pubKeyOperator")
            );
            assert_eq!(
                p.signatures.len() as u64,
                core["sigCount"].as_u64().unwrap()
            );
        }
        other => panic!("{} kept as {other:?}", tx.txid()),
    }
}

/// Each block decodes, re-encodes to Core's bytes and has Core's hash, txids and merkle
/// root; no payload is left raw, and each provider transaction reads as Core reads it.
#[test]
fn core_v24_blocks_decode_with_typed_payloads() {
    let blocks = vectors();
    assert_eq!(blocks.len(), 9);
    let mut types = std::collections::BTreeSet::new();
    for v in &blocks {
        let source = str_of(v, "source");
        let raw = bytes(v);
        let block: Block = deserialize(&raw).unwrap_or_else(|e| panic!("{source}: {e}"));
        assert_eq!(serialize(&block), raw, "{source}");
        assert_eq!(
            block.block_hash(),
            BlockHash::from_str(str_of(v, "hash")).unwrap()
        );
        assert!(block.check_merkle_root(), "{source}");
        let txids: Vec<String> = block.txdata.iter().map(|t| t.txid().to_string()).collect();
        let core_txids: Vec<&str> = v["txids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert_eq!(txids, core_txids, "{source}");
        for tx in &block.txdata {
            assert!(
                !matches!(
                    tx.special_transaction_payload,
                    Some(TransactionPayload::RawPayloadType(..))
                ),
                "{source}: {} kept raw",
                tx.txid()
            );
        }
        for special in v["special"].as_array().unwrap() {
            let txid = Txid::from_str(str_of(special, "txid")).unwrap();
            let tx = block.txdata.iter().find(|t| t.txid() == txid).unwrap();
            check_special(tx, special);
            types.insert((
                special["type"].as_u64().unwrap(),
                special["payload"]["version"].as_u64().unwrap(),
            ));
        }
    }
    assert_eq!(
        types.into_iter().collect::<Vec<_>>(),
        [(1, 3), (2, 3), (3, 3), (10, 1), (11, 1), (12, 1)],
        "every v24 provider layout BL-187 needs"
    );
}

/// `block`'s Core bytes with its type 10 transaction's `nType` set to `UNKNOWN_TYPE`,
/// and the payload bytes it carries.
fn with_unknown_type(v: &Value) -> (Vec<u8>, Vec<u8>) {
    let raw = bytes(v);
    let block: Block = deserialize(&raw).unwrap();
    let index = block
        .txdata
        .iter()
        .position(|t| t.tx_type() == TransactionType::ProviderDissolve)
        .unwrap();
    let mut txs = block.txdata.clone();
    let payload = serialize(txs[index].special_transaction_payload.as_ref().unwrap());
    let mut tx_bytes = serialize(&txs[index]);
    // A version-3 transaction's first four bytes are the version and the type, both LE.
    tx_bytes[2..4].copy_from_slice(&UNKNOWN_TYPE.to_le_bytes());
    txs[index] = deserialize(&tx_bytes).unwrap();
    let mut header = block.header;
    header.merkle_root = Block {
        header,
        txdata: txs.clone(),
    }
    .compute_merkle_root()
    .unwrap();
    (
        serialize(&Block {
            header,
            txdata: txs,
        }),
        payload,
    )
}

/// A special type this crate doesn't know (crafted from a Core type 10 transaction by
/// rewriting its `nType`) keeps its payload raw: the block decodes, the transaction
/// re-encodes to the same bytes and txid, and the rest of the block is typed.
#[test]
fn an_unknown_special_type_is_kept_raw() {
    let v = vectors()
        .into_iter()
        .find(|v| str_of(v, "source") == "feature_masternode_shares@292")
        .unwrap();
    let (raw, payload) = with_unknown_type(&v);
    let block: Block = deserialize(&raw).unwrap();
    assert_eq!(serialize(&block), raw);
    assert!(block.check_merkle_root());
    let unknown: Vec<&Transaction> = block
        .txdata
        .iter()
        .filter(|t| t.tx_type() == TransactionType::Unknown(UNKNOWN_TYPE))
        .collect();
    assert_eq!(unknown.len(), 1);
    match &unknown[0].special_transaction_payload {
        Some(TransactionPayload::RawPayloadType(t, bytes)) => {
            assert_eq!(*t, UNKNOWN_TYPE);
            assert_eq!(*bytes, payload, "the payload's bytes, as Core wrote them");
        }
        other => panic!("not raw: {other:?}"),
    }
    assert!(
        block
            .txdata
            .iter()
            .filter(|t| t.tx_type() != TransactionType::Unknown(UNKNOWN_TYPE))
            .all(|t| !matches!(
                t.special_transaction_payload,
                Some(TransactionPayload::RawPayloadType(..))
            )),
    );
}

/// A P2P `block` message: Core's block bytes framed as dashd sends them.
fn block_message(network: Network, block: &[u8]) -> Vec<u8> {
    let mut command = [0u8; 12];
    command[..5].copy_from_slice(b"block");
    let checksum = sha256d::Hash::hash(block);
    let mut frame = network.magic().to_le_bytes().to_vec();
    frame.extend_from_slice(&command);
    frame.extend_from_slice(&(block.len() as u32).to_le_bytes());
    frame.extend_from_slice(&checksum.as_byte_array()[..4]);
    frame.extend_from_slice(block);
    frame
}

/// The SPV path: dash-spv's `Peer`, which every sync manager reads the network through,
/// receives every block message from a peer. At the old pin its decode failed and the
/// peer loop resynced past the frame, so the block never arrived.
#[tokio::test]
async fn dash_spv_receives_core_v24_block_messages() {
    let network = Network::Regtest;
    let blocks = vectors();
    let unknown = with_unknown_type(
        blocks
            .iter()
            .find(|v| str_of(v, "source") == "feature_masternode_shares@292")
            .unwrap(),
    )
    .0;
    let mut sent: Vec<Vec<u8>> = blocks.iter().map(bytes).collect();
    sent.push(unknown);
    let expected: Vec<BlockHash> = sent
        .iter()
        .map(|b| deserialize::<Block>(b).unwrap().block_hash())
        .collect();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        for block in &sent {
            stream
                .write_all(&block_message(network, block))
                .await
                .unwrap();
        }
        stream.flush().await.unwrap();
        // Hold the connection until the client has read everything.
        let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut [0u8; 1]).await;
    });

    let mut peer = Peer::connect(address, 10, network).await.unwrap();
    let mut received = Vec::new();
    while received.len() < expected.len() {
        let message =
            tokio::time::timeout(std::time::Duration::from_secs(30), peer.receive_message())
                .await
                .expect("dash-spv received no message")
                .unwrap();
        if let Some(message) = message {
            match message.inner() {
                NetworkMessage::Block(block) => received.push(block.block_hash()),
                other => panic!("not a block: {other:?}"),
            }
        }
    }
    assert_eq!(received, expected);
    peer.disconnect().await.unwrap();
    server.await.unwrap();
}
