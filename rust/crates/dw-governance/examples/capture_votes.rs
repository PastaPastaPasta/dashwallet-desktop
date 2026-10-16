//! Captures raw governance votes for the testnet vote-signature vector
//! (MG-01 D12, MG-02 §7): one per-object `govsync` to one peer, then a
//! `getdata` of the announced votes. Prints a JSON document with each
//! vote's payload, hash and recovered voting key id.
//!
//! Usage: `capture_votes <peer ip:port> <object hash> [max votes]` (testnet).
//! Pure reads at concurrency 1: one peer, one `govsync`, one `getdata`.

use std::net::SocketAddr;
use std::time::Duration;

use dashcore::Network;
use dw_governance::object::{display_hex, outpoint_short, parse_display_hex};
use dw_governance::peer::{INV_GOVERNANCE_VOTE, Peer, PeerConfig, PeerEvent, empty_vote_filter};
use dw_governance::vote::GovernanceVote;
use tokio::sync::mpsc;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let [_, peer, object, rest @ ..] = &args[..] else {
        return Err("usage: capture_votes <peer ip:port> <object hash> [max votes]".into());
    };
    let addr: SocketAddr = peer.parse().map_err(|e| format!("peer: {e}"))?;
    let hash = parse_display_hex(object).ok_or("object hash: 64 hex")?;
    let max: usize = rest
        .first()
        .map_or(Ok(40), |m| m.parse())
        .map_err(|e| format!("{e}"))?;

    let network = Network::Testnet;
    let cfg = PeerConfig::new(network, "/dw-capture-votes:0.1/", 0);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let p = Peer::connect(0, addr, &cfg, tx)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    p.send_govsync(hash, &empty_vote_filter());

    // Core sends `ssc` at once and trickles the invs after it.
    let mut announced = Vec::new();
    let mut votes = Vec::new();
    let mut expected = None;
    let mut asked = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv()).await;
        let Ok(Some((_, ev))) = ev else { break };
        match ev {
            PeerEvent::Inv(items) => {
                announced.extend(items.into_iter().filter(|i| i.kind == INV_GOVERNANCE_VOTE))
            }
            PeerEvent::SyncCount { item: 11, count } => {
                eprintln!("ssc(11, {count})");
                expected = Some(usize::try_from(count).unwrap_or(0));
            }
            PeerEvent::Vote(payload) => {
                votes.push(payload);
                if votes.len() >= announced.len() {
                    break;
                }
            }
            PeerEvent::Closed => break,
            _ => {}
        }
        if !asked && expected.is_some_and(|n| announced.len() >= n) {
            announced.truncate(max);
            p.send_getdata(&announced);
            asked = true;
        }
    }
    p.close();

    let rows: Vec<serde_json::Value> = votes
        .iter()
        .filter_map(|payload| {
            let v = GovernanceVote::decode(payload).ok()?;
            Some(serde_json::json!({
                "payload_hex": hex::encode(payload),
                "vote_hash": display_hex(&v.hash()),
                "outpoint": outpoint_short(&v.masternode_outpoint),
                "parent": display_hex(&v.parent_hash),
                "signal": v.signal,
                "outcome": v.outcome,
                "time": v.time,
                "signature_hash": display_hex(&v.signature_hash()),
                "voting_key_id": v.signer_key_id(network).map(hex::encode),
            }))
        })
        .collect();
    let doc = serde_json::json!({
        "network": "testnet",
        "peer": addr.to_string(),
        "object": object,
        "announced": announced.len(),
        "votes": rows,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?
    );
    Ok(())
}
