//! BL-187: the pinned dashcore and dash-spv read Dash Core v24 blocks.
//!
//! At rust-dashcore 8fe0a381, 7 of these 9 blocks did not decode (a version-3 ProRegTx
//! with payouts or extended net info, ProUpServTx version 3, and the special types
//! 10–12), and dash-spv drops a P2P message it can't decode, so wallet sync stalled at
//! the first such block. One more (a ProUpRegTx version 3) decoded to a misread payload
//! with a wrong merkle root (design note BL-187 D19). The vectors in
//! testdata/core_v24_payload_blocks.json are blocks dashd v24.0.0-rc.3 mined while
//! running Core's own functional tests (testdata/oracle/core_v24/).

use std::str::FromStr;

use dash_spv::network::Peer;
use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
use dashcore::blockdata::transaction::special_transaction::TransactionType;
use dashcore::blockdata::transaction::special_transaction::provider_registration::MasternodePayoutShare;
use dashcore::consensus::{deserialize, serialize};
use dashcore::hashes::{Hash, sha256d};
use dashcore::network::message::NetworkMessage;
use dashcore::sml::masternode_list_entry::MasternodeNetInfo;
use dashcore::sml::masternode_list_entry::net_info::{Bip155Network, NetInfoEntry, NetInfoPurpose};
use dashcore::{Address, Block, BlockHash, Network, PubkeyHash, ScriptBuf, Transaction, Txid};
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

/// RFC 4648 base32 (Core's lower-case Tor and I2P names), without padding.
fn base32(text: &str) -> Vec<u8> {
    let (mut bits, mut acc, mut out) = (0u32, 0u32, Vec::new());
    for c in text.bytes() {
        let v = match c {
            b'a'..=b'z' => c - b'a',
            b'2'..=b'7' => c - b'2' + 26,
            _ => panic!("not base32: {text}"),
        };
        acc = (acc << 5) | u32::from(v);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// A net info entry as Core's JSON writes it (`ToStringAddrPort`), except that a Tor
/// or I2P name is its key in hex: the Tor name also carries a SHA3 checksum.
fn entry_string(entry: &NetInfoEntry) -> String {
    match entry {
        NetInfoEntry::Service {
            network,
            addr,
            port,
        } => match network {
            Bip155Network::Ipv4 => format!(
                "{}:{port}",
                std::net::Ipv4Addr::from(<[u8; 4]>::try_from(&addr[..]).unwrap())
            ),
            Bip155Network::Ipv6 => format!(
                "[{}]:{port}",
                std::net::Ipv6Addr::from(<[u8; 16]>::try_from(&addr[..]).unwrap())
            ),
            Bip155Network::TorV3 => format!("tor {}:{port}", hex::encode(addr)),
            Bip155Network::I2p => format!("i2p {}:{port}", hex::encode(addr)),
            other => panic!("not in the vectors: {other:?}"),
        },
        NetInfoEntry::Domain { host, port } => format!("{host}:{port}"),
        NetInfoEntry::Invalid => panic!("an invalid entry"),
    }
}

/// Core's JSON address in [`entry_string`]'s form.
fn core_entry_string(address: &str) -> String {
    let (host, port) = address.rsplit_once(':').unwrap();
    if let Some(name) = host.strip_suffix(".onion") {
        // pubkey (32) ‖ checksum (2) ‖ version (1)
        format!("tor {}:{port}", hex::encode(&base32(name)[..32]))
    } else if let Some(name) = host.strip_suffix(".b32.i2p") {
        format!("i2p {}:{port}", hex::encode(base32(name)))
    } else {
        address.to_string()
    }
}

/// A version-3 payload's net info, per purpose, keyed as Core's JSON keys them.
fn addresses(net_info: &MasternodeNetInfo) -> Vec<(&'static str, Vec<String>)> {
    let MasternodeNetInfo::Extended(info) = net_info else {
        panic!("version 3 has extended net info: {net_info:?}");
    };
    let mut out: Vec<_> = info
        .purposes
        .iter()
        .map(|(purpose, entries)| {
            let key = match purpose {
                NetInfoPurpose::CoreP2P => "core_p2p",
                NetInfoPurpose::PlatformP2P => "platform_p2p",
                NetInfoPurpose::PlatformHttps => "platform_https",
            };
            (key, entries.iter().map(entry_string).collect())
        })
        .collect();
    out.sort();
    out
}

fn core_addresses(payload: &Value) -> Vec<(&str, Vec<String>)> {
    let mut out: Vec<_> = payload["addresses"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            let entries = v.as_array().unwrap();
            (
                k.as_str(),
                entries
                    .iter()
                    .map(|e| core_entry_string(e.as_str().unwrap()))
                    .collect(),
            )
        })
        .collect();
    out.sort();
    out
}

/// A key hash or a script as a regtest address, as Core's JSON writes them.
fn key_address(hash: &PubkeyHash) -> String {
    Address::new(
        Network::Regtest,
        dashcore::address::Payload::PubkeyHash(*hash),
    )
    .to_string()
}

fn script_address(script: &ScriptBuf) -> String {
    Address::from_script(script, Network::Regtest)
        .unwrap()
        .to_string()
}

/// Payout shares against Core's `payouts`.
fn check_payouts(ours: &[MasternodePayoutShare], core: &Value) {
    let core = core.as_array().unwrap();
    assert_eq!(ours.len(), core.len());
    for (ours, theirs) in ours.iter().zip(core) {
        assert_eq!(
            hex::encode(ours.script_payout.as_bytes()),
            str_of(theirs, "script")
        );
        assert_eq!(
            script_address(&ours.script_payout),
            str_of(theirs, "address")
        );
        assert_eq!(u64::from(ours.reward), theirs["reward"].as_u64().unwrap());
    }
}

/// A payload this crate kept as bytes.
fn is_raw(tx: &Transaction) -> bool {
    matches!(
        tx.special_transaction_payload,
        Some(TransactionPayload::RawPayloadType(..))
    )
}

/// The provider transaction `special` describes decodes to its typed payload, with
/// every field Core's JSON shows.
fn check_special(tx: &Transaction, special: &Value) {
    let core = &special["payload"];
    let version = core["version"].as_u64().unwrap() as u16;
    let pro_tx_hash = |h: &Txid| assert_eq!(h.to_string(), str_of(core, "proTxHash"));
    let inputs_hash = |h: &dyn ToString| assert_eq!(h.to_string(), str_of(core, "inputsHash"));
    let operator_key = |k: &[u8]| assert_eq!(hex::encode(k), str_of(core, "pubKeyOperator"));
    let voting = |h: &PubkeyHash| assert_eq!(key_address(h), str_of(core, "votingAddress"));
    // Core prints the uint160 reversed (`uint160::ToString`); a
    // `PlatformNodeId` holds that order already (its wire form is reversed).
    let platform_node_id = |id: Option<&[u8]>| {
        let ours = id.map(hex::encode);
        assert_eq!(ours.as_deref(), core["platformNodeID"].as_str());
    };
    match tx.special_transaction_payload.as_ref().unwrap() {
        TransactionPayload::ProviderRegistrationPayloadType(p) => {
            assert_eq!(special["type"], 1);
            assert_eq!(p.version, version);
            assert_eq!(addresses(&p.service_address), core_addresses(core));
            assert_eq!(
                p.collateral_outpoint.txid.to_string(),
                str_of(core, "collateralHash")
            );
            assert_eq!(
                u64::from(p.collateral_outpoint.vout),
                core["collateralIndex"].as_u64().unwrap()
            );
            operator_key(p.operator_public_key.as_ref());
            voting(&p.voting_key_hash);
            // Core prints the reward in percent (`nOperatorReward / 100`).
            assert_eq!(
                f64::from(p.operator_reward) / 100.0,
                core["operatorReward"].as_f64().unwrap()
            );
            inputs_hash(&p.inputs_hash);
            platform_node_id(p.platform_node_id.as_ref().map(|id| &id[..]));
            match core.get("shares") {
                Some(shares) => {
                    let shared = p.shared_collateral.as_ref().unwrap();
                    let shares = shares.as_array().unwrap();
                    assert_eq!(shared.shares.len(), shares.len());
                    for (ours, theirs) in shared.shares.iter().zip(shares) {
                        assert_eq!(ours.amount, theirs["amount"].as_i64().unwrap());
                        assert_eq!(
                            hex::encode(ours.script_refund.as_bytes()),
                            str_of(theirs, "refundScript")
                        );
                        // Core prints `RewardScript()`: an empty reward
                        // script pays the refund script (`providertx.h:80`).
                        let reward = if ours.script_reward.is_empty() {
                            &ours.script_refund
                        } else {
                            &ours.script_reward
                        };
                        assert_eq!(
                            hex::encode(reward.as_bytes()),
                            str_of(theirs, "rewardScript")
                        );
                        assert_eq!(
                            key_address(&ours.owner_key_hash),
                            str_of(theirs, "ownerAddress")
                        );
                    }
                    assert_eq!(
                        u64::from(shared.early_period_blocks),
                        core["earlyPeriodBlocks"].as_u64().unwrap()
                    );
                    assert_eq!(shared.early_penalty, core["earlyPenalty"].as_i64().unwrap());
                    // the shares carry the rewards; Core prints no `payouts`
                    assert_eq!(p.payouts.as_deref(), Some(&[][..]));
                }
                None => {
                    assert_eq!(key_address(&p.owner_key_hash), str_of(core, "ownerAddress"));
                    check_payouts(p.payouts.as_ref().unwrap(), &core["payouts"]);
                }
            }
        }
        TransactionPayload::ProviderUpdateServicePayloadType(p) => {
            assert_eq!(special["type"], 2);
            assert_eq!(p.version, version);
            pro_tx_hash(&p.pro_tx_hash);
            assert_eq!(p.mn_type.map(u64::from), core["type"].as_u64());
            assert_eq!(addresses(&p.service_address), core_addresses(core));
            match core.get("operatorPayoutAddress") {
                Some(a) => assert_eq!(script_address(&p.script_payout), a.as_str().unwrap()),
                None => assert!(p.script_payout.is_empty()),
            }
            inputs_hash(&p.inputs_hash);
            platform_node_id(p.platform_node_id.as_ref().map(|id| &id[..]));
        }
        TransactionPayload::ProviderUpdateRegistrarPayloadType(p) => {
            assert_eq!(special["type"], 3);
            assert_eq!(p.version, version);
            pro_tx_hash(&p.pro_tx_hash);
            operator_key(p.operator_public_key.as_ref());
            voting(&p.voting_key_hash);
            inputs_hash(&p.inputs_hash);
            check_payouts(p.payouts.as_ref().unwrap(), &core["payouts"]);
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
            assert_eq!(
                script_address(&p.script_reward),
                str_of(core, "rewardAddress")
            );
            inputs_hash(&p.inputs_hash);
        }
        TransactionPayload::ProviderUpdateSharedRegistrarPayloadType(p) => {
            assert_eq!(special["type"], 12);
            assert_eq!(p.version, version);
            pro_tx_hash(&p.pro_tx_hash);
            operator_key(p.operator_public_key.as_ref());
            voting(&p.voting_key_hash);
            inputs_hash(&p.inputs_hash);
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
            assert!(!is_raw(tx), "{source}: {} kept raw", tx.txid());
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

/// A Core block with one transaction's bytes rewritten.
struct Crafted {
    /// The block, its merkle root recomputed.
    block: Vec<u8>,
    /// The rewritten transaction.
    tx: Vec<u8>,
    /// Where its special payload starts in `tx`.
    payload_at: usize,
}

/// Core block `source` with its transaction of `tx_type` rewritten by `edit`, which
/// gets the transaction's bytes and where its payload starts.
fn crafted(source: &str, tx_type: TransactionType, edit: impl FnOnce(&mut [u8], usize)) -> Crafted {
    let v = vectors()
        .into_iter()
        .find(|v| str_of(v, "source") == source)
        .unwrap();
    let mut block: Block = deserialize(&bytes(&v)).unwrap();
    let index = block
        .txdata
        .iter()
        .position(|t| t.tx_type() == tx_type)
        .unwrap();
    let payload = serialize(
        block.txdata[index]
            .special_transaction_payload
            .as_ref()
            .unwrap(),
    );
    let mut tx = serialize(&block.txdata[index]);
    // The payload is last, after its length.
    let payload_at = tx.len() - payload.len();
    assert_eq!(tx[payload_at..], payload[..]);
    edit(&mut tx, payload_at);
    block.txdata[index] = deserialize(&tx).unwrap();
    block.header.merkle_root = block.compute_merkle_root().unwrap();
    Crafted {
        block: serialize(&block),
        tx,
        payload_at,
    }
}

/// Core's type 10 transaction with its `nType` set to `UNKNOWN_TYPE`.
fn with_unknown_type() -> Crafted {
    crafted(
        "feature_masternode_shares@292",
        TransactionType::ProviderDissolve,
        // A version-3 transaction's first four bytes are the version and the type, LE.
        |tx, _| tx[2..4].copy_from_slice(&UNKNOWN_TYPE.to_le_bytes()),
    )
}

/// Core's ProUpServTx version 3 relabelled version 4, a layout no build knows yet.
fn with_unknown_version() -> Crafted {
    crafted(
        "feature_masternode_payout_shares@125",
        TransactionType::ProviderUpdateService,
        // A payload's first two bytes are its version, LE.
        |tx, at| tx[at..at + 2].copy_from_slice(&4u16.to_le_bytes()),
    )
}

/// `crafted`'s block decodes, re-encodes to the same bytes with the same merkle root,
/// and keeps the rewritten transaction's payload raw, byte for byte, as `tx_type`;
/// it is the only raw payload in the block.
fn check_kept_raw(crafted: &Crafted, tx_type: u16) {
    let block: Block = deserialize(&crafted.block).unwrap();
    assert_eq!(serialize(&block), crafted.block);
    assert!(block.check_merkle_root());
    let raw: Vec<&Transaction> = block.txdata.iter().filter(|t| is_raw(t)).collect();
    assert_eq!(raw.len(), 1);
    assert_eq!(
        serialize(raw[0]),
        crafted.tx,
        "the transaction re-encodes as written"
    );
    match &raw[0].special_transaction_payload {
        Some(TransactionPayload::RawPayloadType(t, bytes)) => {
            assert_eq!(*t, tx_type);
            assert_eq!(bytes[..], crafted.tx[crafted.payload_at..]);
        }
        other => panic!("not raw: {other:?}"),
    }
}

/// A special type this crate doesn't know (crafted from a Core type 10 transaction by
/// rewriting its `nType`) keeps its payload raw.
#[test]
fn an_unknown_special_type_is_kept_raw() {
    let crafted = with_unknown_type();
    check_kept_raw(&crafted, UNKNOWN_TYPE);
    let block: Block = deserialize(&crafted.block).unwrap();
    assert!(
        block
            .txdata
            .iter()
            .any(|t| t.tx_type() == TransactionType::Unknown(UNKNOWN_TYPE))
    );
}

/// A known type in a layout this crate doesn't know (rust-dashcore #1154, why BL-187
/// takes that rev) keeps its payload raw instead of failing the block.
#[test]
fn a_known_type_in_an_unknown_layout_is_kept_raw() {
    check_kept_raw(&with_unknown_version(), 2);
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
/// receives every block message from a peer, the two crafted ones too. At the old pin
/// the decode of 7 of the 9 Core blocks failed and the peer loop resynced past the frame,
/// so the block never arrived.
#[tokio::test]
async fn dash_spv_receives_core_v24_block_messages() {
    let network = Network::Regtest;
    let blocks = vectors();
    let mut sent: Vec<Vec<u8>> = blocks.iter().map(bytes).collect();
    sent.push(with_unknown_type().block);
    sent.push(with_unknown_version().block);
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
