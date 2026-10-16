use super::*;
use crate::peer::{decode_inv, encode_inv};
use crate::store::ListEntry;
use crate::vote::{VoteOutcome, to_recoverable_compact};
use dashcore::OutPoint;
use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::{Message, PublicKey, SecretKey};
use dw_p2p::commands;
use dw_p2p::testing::MockPeer;
use std::collections::HashSet;

fn key_id(n: u8) -> [u8; 20] {
    let sk = SecretKey::from_secret_bytes([n; 32]).unwrap();
    hash160::Hash::hash(&PublicKey::from_secret_key(&sk).serialize()).to_byte_array()
}

fn vote(parent: [u8; 32], key: u8, outcome: VoteOutcome) -> GovernanceVote {
    let sk = SecretKey::from_secret_bytes([key; 32]).unwrap();
    let pk = PublicKey::from_secret_key(&sk);
    let mut v = GovernanceVote::funding(
        OutPoint::new(dashcore::Txid::from_byte_array([key; 32]), 0),
        parent,
        outcome,
        unix_now() - 100,
    );
    let d = v.signing_digest(Network::Regtest);
    let s = dashcore::secp256k1::ecdsa::sign(Message::from_digest(d), &sk);
    v.signature = to_recoverable_compact(&s.serialize_compact(), &pk, &d)
        .unwrap()
        .to_vec();
    v
}

fn proposal(name: &str) -> GovernanceObject {
    let data = format!(
        r#"{{"end_epoch":{},"name":"{name}","payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}}"#,
        unix_now() + 10_000
    );
    GovernanceObject::new_proposal(1, unix_now(), data.into_bytes())
}

/// How a fake full node behaves. Like Core it leaves out of vote invs the
/// votes the wallet announced on the connection, but counts them in `ssc`.
#[derive(Clone, Default)]
struct Node {
    objects: Vec<GovernanceObject>,
    votes: Vec<GovernanceVote>,
    /// Announced (and served) right after the full sync, as relays.
    relays: Vec<GovernanceVote>,
    /// Wait this long before answering a `getdata` for votes.
    vote_delay: Option<Duration>,
    /// Close the connection right after the `ssc(11)` of a vote sync.
    close_after_vote_ssc: bool,
    /// Close the connection on a `getdata` for votes (after its invs).
    close_on_vote_getdata: bool,
    /// Never answer `govsync` (Core while not synced).
    mute: bool,
    /// Announce a vote it does not have on every ping.
    chatter: bool,
    /// Answer every `getdata` with `notfound`.
    notfound: bool,
    /// Announce this many more objects in `ssc(10)` than it sends.
    missing: i32,
    /// Answer the full sync with this many unrequested votes only.
    unsolicited: usize,
    /// Stop reading (and answering pings) once synced.
    go_silent: bool,
}

/// What the wallet sent a fake node.
#[derive(Debug)]
enum Seen {
    Govsync([u8; 32]),
    Vote(Vec<u8>),
}

async fn serve(mock: MockPeer, node: Node, seen: mpsc::UnboundedSender<Seen>) {
    let mut conn = mock.accept().await.unwrap();
    let mut synced_votes = false;
    let mut known: HashSet<[u8; 32]> = HashSet::new();
    let mut chatter = 0u32;
    let mut votes = node.votes.clone();
    votes.extend(node.relays.iter().cloned());
    while let Ok(msg) = conn.recv().await {
        match msg.command.as_str() {
            "ping" => {
                let _ = conn.send("pong", &msg.payload).await;
                if node.chatter {
                    chatter += 1;
                    let mut hash = [0xcc; 32];
                    hash[..4].copy_from_slice(&chatter.to_le_bytes());
                    let item = InvItem {
                        kind: INV_GOVERNANCE_VOTE,
                        hash,
                    };
                    let _ = conn.send("inv", &encode_inv(&[item])).await;
                }
            }
            _ if node.mute && msg.command == commands::MNGOVERNANCESYNC => {}
            commands::MNGOVERNANCESYNC => {
                let hash: [u8; 32] = msg.payload[..32].try_into().unwrap();
                let _ = seen.send(Seen::Govsync(hash));
                if hash != [0; 32] && msg.payload[32..] == object_fetch_filter()[..] {
                    // One object, by inv.
                    if node.objects.iter().any(|o| o.hash() == hash) {
                        let item = InvItem {
                            kind: INV_GOVERNANCE_OBJECT,
                            hash,
                        };
                        conn.send("inv", &encode_inv(&[item])).await.unwrap();
                    }
                    continue;
                }
                if hash == [0; 32] && node.unsolicited > 0 {
                    let v = node.votes[0].encode();
                    for _ in 0..node.unsolicited {
                        if conn
                            .send(commands::MNGOVERNANCEOBJECTVOTE, &v)
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    continue;
                }
                if hash == [0; 32] {
                    let mut ssc = SSC_GOVOBJ.to_le_bytes().to_vec();
                    ssc.extend_from_slice(
                        &(node.objects.len() as i32 + node.missing).to_le_bytes(),
                    );
                    conn.send(commands::SYNCSTATUSCOUNT, &ssc).await.unwrap();
                    let items: Vec<InvItem> = node
                        .objects
                        .iter()
                        .map(|o| InvItem {
                            kind: INV_GOVERNANCE_OBJECT,
                            hash: o.hash(),
                        })
                        .collect();
                    conn.send("inv", &encode_inv(&items)).await.unwrap();
                    let relays: Vec<InvItem> = node
                        .relays
                        .iter()
                        .map(|v| InvItem {
                            kind: INV_GOVERNANCE_VOTE,
                            hash: v.hash(),
                        })
                        .collect();
                    if !relays.is_empty() {
                        conn.send("inv", &encode_inv(&relays)).await.unwrap();
                    }
                } else {
                    let all: Vec<InvItem> = votes
                        .iter()
                        .filter(|v| v.parent_hash == hash)
                        .map(|v| InvItem {
                            kind: INV_GOVERNANCE_VOTE,
                            hash: v.hash(),
                        })
                        .collect();
                    let mut ssc = SSC_GOVOBJ_VOTE.to_le_bytes().to_vec();
                    ssc.extend_from_slice(&(all.len() as i32).to_le_bytes());
                    conn.send(commands::SYNCSTATUSCOUNT, &ssc).await.unwrap();
                    if node.close_after_vote_ssc {
                        conn.shutdown().await;
                        return;
                    }
                    let items: Vec<InvItem> = all
                        .into_iter()
                        .filter(|i| !known.contains(&i.hash))
                        .collect();
                    if !items.is_empty() {
                        conn.send("inv", &encode_inv(&items)).await.unwrap();
                    }
                    synced_votes = true;
                }
            }
            "getdata" => {
                let items = decode_inv(&msg.payload).unwrap();
                if node.notfound {
                    conn.send("notfound", &encode_inv(&items)).await.unwrap();
                    continue;
                }
                if node.close_on_vote_getdata && items.iter().any(|i| i.kind == INV_GOVERNANCE_VOTE)
                {
                    conn.shutdown().await;
                    return;
                }
                if let Some(d) = node.vote_delay
                    && items.iter().any(|i| i.kind == INV_GOVERNANCE_VOTE)
                {
                    tokio::time::sleep(d).await;
                }
                let mut missing = Vec::new();
                for i in items {
                    if let Some(o) = node.objects.iter().find(|o| o.hash() == i.hash) {
                        conn.send(commands::MNGOVERNANCEOBJECT, &o.encode())
                            .await
                            .unwrap();
                    } else if let Some(v) = votes.iter().find(|v| v.hash() == i.hash) {
                        conn.send(commands::MNGOVERNANCEOBJECTVOTE, &v.encode())
                            .await
                            .unwrap();
                    } else {
                        missing.push(i);
                    }
                }
                if !missing.is_empty() {
                    conn.send("notfound", &encode_inv(&missing)).await.unwrap();
                }
                if node.go_silent && synced_votes {
                    // Hold the connection open without reading.
                    std::future::pending::<()>().await;
                }
            }
            "inv" => {
                for i in decode_inv(&msg.payload).unwrap() {
                    known.insert(i.hash);
                }
                conn.send("getdata", &msg.payload).await.unwrap();
            }
            commands::MNGOVERNANCEOBJECTVOTE => {
                let _ = seen.send(Seen::Vote(msg.payload));
            }
            _ => {}
        }
    }
}

struct Harness {
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), SyncError>>,
    seen: Vec<mpsc::UnboundedReceiver<Seen>>,
}

async fn start(nodes: Vec<Node>, tune: impl FnOnce(&mut SyncConfig)) -> Harness {
    start_with(nodes, tune, |_, _, _| {}).await
}

/// `prepare` sees the shared state, the history and the nodes' addresses
/// before the run starts.
async fn start_with(
    nodes: Vec<Node>,
    tune: impl FnOnce(&mut SyncConfig),
    prepare: impl FnOnce(&Shared, &mut SyncHistory, &[SocketAddr]),
) -> Harness {
    let shared = Arc::new(Shared::new(Network::Regtest, Box::new(|| {})));
    shared.store().set_masternode_list(&[
        ListEntry {
            voting_key_id: key_id(1),
            weight: Some(1),
            valid: true,
        },
        ListEntry {
            voting_key_id: key_id(2),
            weight: Some(4),
            valid: true,
        },
    ]);
    // One fake node cannot confirm an object by itself (D3).
    if nodes.len() == 1 {
        shared.store().set_confirmations(1);
    }
    let mut addrs = Vec::new();
    let mut seen = Vec::new();
    for node in nodes {
        let mock = MockPeer::bind(Network::Regtest).await.unwrap();
        addrs.push(mock.addr());
        let (tx, rx) = mpsc::unbounded_channel();
        seen.push(rx);
        tokio::spawn(serve(mock, node, tx));
    }
    let mut history = SyncHistory::default();
    prepare(&shared, &mut history, &addrs);
    let mut cfg = SyncConfig::new(PeerConfig::new(Network::Regtest, "/t/", 1));
    cfg.idle_timeout = Duration::from_secs(3);
    tune(&mut cfg);
    let (stop, stop_rx) = watch::channel(false);
    let task = tokio::spawn(run(
        Arc::clone(&shared),
        cfg,
        addrs,
        Arc::new(Mutex::new(history)),
        stop_rx,
    ));
    Harness {
        shared,
        stop,
        task,
        seen,
    }
}

async fn wait_for(shared: &Shared, what: impl Fn(&Progress) -> bool) -> Progress {
    for _ in 0..200 {
        let p = shared.progress().clone();
        if what(&p) {
            return p;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out: {:?}", shared.progress());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_qt_128_objects_from_every_peer_then_votes() {
    let obj = proposal("a");
    let h = obj.hash();
    let node = Node {
        objects: vec![obj],
        votes: vec![vote(h, 1, VoteOutcome::Yes), vote(h, 2, VoteOutcome::No)],
        ..Node::default()
    };
    let mut hx = start(vec![node.clone(), node], |_| {}).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    // Two announcers: confirmed. Each vote fetched once.
    assert_eq!((p.objects, p.unconfirmed_objects, p.votes), (1, 0, 2));
    assert!(p.bytes_received > 0);
    assert!(hx.shared.store().object(&h).is_some());
    // Both peers were full-synced; the one proposal's votes were asked of
    // one of them.
    let mut full = 0;
    let mut votes = 0;
    for rx in &mut hx.seen {
        while let Ok(s) = rx.try_recv() {
            match s {
                Seen::Govsync(x) if x == [0; 32] => full += 1,
                Seen::Govsync(_) => votes += 1,
                Seen::Vote(_) => {}
            }
        }
    }
    assert_eq!((full, votes), (2, 1));
    hx.stop.send(true).unwrap();
    assert_eq!(hx.task.await.unwrap(), Ok(()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_announcer_leaves_objects_unconfirmed() {
    let obj = proposal("a");
    let with = Node {
        objects: vec![obj],
        ..Node::default()
    };
    let hx = start(vec![with, Node::default()], |_| {}).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!((p.objects, p.unconfirmed_objects), (0, 1));
    hx.stop.send(true).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notfound_moves_to_the_next_announcer() {
    let obj = proposal("a");
    let good = Node {
        objects: vec![obj.clone()],
        ..Node::default()
    };
    let refusing = Node {
        notfound: true,
        ..good.clone()
    };
    let hx = start(vec![refusing, good], |_| {}).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!(p.objects, 1);
    hx.stop.send(true).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_short_phase_fails_and_is_never_synced() {
    let node = Node {
        objects: vec![proposal("a")],
        missing: 1,
        ..Node::default()
    };
    let hx = start(vec![node], |c| c.idle_timeout = Duration::from_secs(1)).await;
    let r = hx.task.await.unwrap();
    assert!(matches!(r, Err(SyncError::Incomplete(_))), "{r:?}");
    let p = hx.shared.progress().clone();
    assert_eq!(p.phase, Phase::Failed);
    assert!(p.last_synced_at.is_none());
    assert!(p.last_error.unwrap().contains("idle"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsolicited_items_are_dropped_and_close_the_peer() {
    let obj = proposal("a");
    let node = Node {
        objects: vec![obj.clone()],
        votes: vec![vote(obj.hash(), 1, VoteOutcome::Yes)],
        unsolicited: 6,
        ..Node::default()
    };
    let hx = start(vec![node], |c| c.max_misbehaviour = 5).await;
    let r = hx.task.await.unwrap();
    assert!(r.is_err(), "{r:?}");
    assert_eq!(hx.shared.store().vote_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dead_peer_is_detected_in_follow_mode() {
    let obj = proposal("a");
    let node = Node {
        objects: vec![obj.clone()],
        votes: vec![vote(obj.hash(), 1, VoteOutcome::Yes)],
        go_silent: true,
        ..Node::default()
    };
    let hx = start(vec![node], |c| {
        c.peer.session.ping_interval = Duration::from_millis(300);
        c.peer.session.idle_timeout = Duration::from_secs(2);
    })
    .await;
    wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    let r = tokio::time::timeout(Duration::from_secs(10), hx.task)
        .await
        .expect("the run ends once the silent peer idles out")
        .unwrap();
    assert_eq!(r, Err(SyncError::PeersLost));
    assert_eq!(hx.shared.progress().phase, Phase::Failed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_votes_are_announced_and_served() {
    let obj = proposal("a");
    let node = Node {
        objects: vec![obj.clone()],
        ..Node::default()
    };
    let mut hx = start(vec![node], |_| {}).await;
    wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    let v = vote(obj.hash(), 1, VoteOutcome::Yes);
    assert_eq!(hx.shared.relay_vote(&v), 1);
    let got = loop {
        match tokio::time::timeout(Duration::from_secs(5), hx.seen[0].recv())
            .await
            .unwrap()
            .unwrap()
        {
            Seen::Vote(p) => break p,
            Seen::Govsync(_) => {}
        }
    };
    assert_eq!(got, v.encode());
    hx.stop.send(true).unwrap();
}

fn trigger() -> GovernanceObject {
    let data = format!(
        r#"{{"event_block_height":100,"payment_addresses":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amounts":"1","proposal_hashes":"{}","type":2}}"#,
        "ab".repeat(32)
    );
    GovernanceObject::new_proposal(1, unix_now(), data.into_bytes())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn votes_core_relays_but_the_wallet_ignores_cost_no_penalty() {
    let obj = proposal("a");
    let h = obj.hash();
    let t = trigger();
    // Operators' BLS-signed votes on the trigger, and a delete vote on the
    // proposal, relayed; one funding vote to count.
    let mut relays = Vec::new();
    for key in 3..8u8 {
        let mut v = vote(t.hash(), key, VoteOutcome::Yes);
        v.signature = vec![key; crate::vote::BLS_SIG_SIZE];
        relays.push(v);
    }
    let mut delete = vote(h, 1, VoteOutcome::Yes);
    delete.signal = 3;
    relays.push(delete);
    let node = Node {
        objects: vec![obj, t],
        votes: vec![vote(h, 2, VoteOutcome::Yes)],
        relays,
        ..Node::default()
    };
    // Any misbehaviour closes the only peer.
    let hx = start(vec![node], |c| c.max_misbehaviour = 0).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!((p.objects, p.votes, p.peers), (2, 1, 1));
    hx.stop.send(true).unwrap();
    assert_eq!(hx.task.await.unwrap(), Ok(()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_delivery_after_a_timeout_is_accepted_without_penalty() {
    let obj = proposal("a");
    let h = obj.hash();
    let node = Node {
        objects: vec![obj],
        votes: vec![vote(h, 1, VoteOutcome::Yes)],
        vote_delay: Some(Duration::from_millis(1500)),
        ..Node::default()
    };
    // The vote's one announcer is asked again after 300 ms and given up on
    // 300 ms later; it still arrives (twice) and completes the phase.
    let hx = start(vec![node], |c| {
        c.request_timeout = Duration::from_millis(300);
        c.max_misbehaviour = 0;
    })
    .await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!((p.votes, p.peers), (1, 1));
    tokio::time::sleep(Duration::from_millis(1600)).await;
    assert_eq!(
        hx.shared.progress().peers,
        1,
        "the second copy is no offence"
    );
    hx.stop.send(true).unwrap();
    assert_eq!(hx.task.await.unwrap(), Ok(()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_given_up_sync_item_fails_the_phase() {
    let obj = proposal("a");
    let h = obj.hash();
    let node = Node {
        objects: vec![obj],
        votes: vec![vote(h, 1, VoteOutcome::Yes)],
        vote_delay: Some(Duration::from_secs(30)),
        ..Node::default()
    };
    let hx = start(vec![node], |c| {
        c.request_timeout = Duration::from_millis(300)
    })
    .await;
    let r = hx.task.await.unwrap();
    assert_eq!(
        r,
        Err(SyncError::Incomplete("1 items could not be fetched".into()))
    );
    assert!(hx.shared.progress().last_synced_at.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_closing_after_its_vote_count_is_replaced() {
    let obj = proposal("a");
    let h = obj.hash();
    let votes = vec![vote(h, 1, VoteOutcome::Yes), vote(h, 2, VoteOutcome::No)];
    let closing = Node {
        objects: vec![obj.clone()],
        votes: votes.clone(),
        close_after_vote_ssc: true,
        ..Node::default()
    };
    let good = Node {
        objects: vec![obj],
        votes,
        ..Node::default()
    };
    // The first peer is asked for the proposal's votes, counts them and
    // closes before its invs: the second one is asked instead.
    let hx = start(vec![closing, good], |_| {}).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!((p.votes, p.peers), (2, 1));
    hx.stop.send(true).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_closing_after_its_vote_invs_is_replaced() {
    let obj = proposal("a");
    let h = obj.hash();
    let votes = vec![vote(h, 1, VoteOutcome::Yes), vote(h, 2, VoteOutcome::No)];
    let closing = Node {
        objects: vec![obj.clone()],
        votes: votes.clone(),
        close_on_vote_getdata: true,
        ..Node::default()
    };
    let good = Node {
        objects: vec![obj],
        votes,
        ..Node::default()
    };
    // The first peer completes its counts, then closes on the `getdata`:
    // its votes have no other announcer, so the sync goes to the second.
    let hx = start(vec![closing, good], |_| {}).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!((p.votes, p.peers), (2, 1));
    hx.stop.send(true).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_object_the_wallet_refuses_neither_fails_the_sync_nor_costs_a_penalty() {
    let good = proposal("a");
    // Core keeps what `UniValue::read` parsed before trailing bytes; the
    // wallet refuses the object (D35).
    let data = format!(
        r#"{{"end_epoch":{},"name":"b","payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}} x"#,
        unix_now() + 10_000
    );
    let refused = GovernanceObject::new_proposal(1, unix_now(), data.into_bytes());
    let node = Node {
        objects: vec![good.clone(), refused.clone()],
        ..Node::default()
    };
    let hx = start(vec![node.clone(), node], |c| c.max_misbehaviour = 0).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!((p.objects, p.peers), (1, 2));
    {
        let store = hx.shared.store();
        assert!(store.object(&good.hash()).is_some());
        assert!(store.object(&refused.hash()).is_none());
        // Not fetched again; its votes are ignored.
        assert!(store.has_object(&refused.hash()));
    }
    hx.stop.send(true).unwrap();
    assert_eq!(hx.task.await.unwrap(), Ok(()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_votes_wait_until_synced_so_core_still_sends_every_vote() {
    let obj = proposal("a");
    let h = obj.hash();
    let own = vote(h, 1, VoteOutcome::Yes);
    let node = Node {
        objects: vec![obj],
        votes: vec![own.clone(), vote(h, 2, VoteOutcome::No)],
        ..Node::default()
    };
    // The wallet relayed `own` in an earlier run: it is in the outbox, and
    // the node counts it in the vote sync.
    let hx = start_with(
        vec![node],
        |_| {},
        |shared, _, _| {
            shared.relay_vote(&own);
        },
    )
    .await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!(p.votes, 2);
    hx.stop.send(true).unwrap();
    assert_eq!(hx.task.await.unwrap(), Ok(()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mute_peer_is_closed_while_another_chatters() {
    let obj = proposal("a");
    let h = obj.hash();
    let chatty = Node {
        objects: vec![obj.clone()],
        votes: vec![vote(h, 1, VoteOutcome::Yes)],
        chatter: true,
        ..Node::default()
    };
    let mute = Node {
        mute: true,
        ..Node::default()
    };
    let hx = start(vec![chatty, mute], |c| {
        c.peer.session.ping_interval = Duration::from_millis(200);
        c.answer_timeout = Duration::from_secs(1);
        c.idle_timeout = Duration::from_secs(5);
    })
    .await;
    hx.shared.store().set_confirmations(1);
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!((p.objects, p.votes, p.peers), (1, 1, 1));
    hx.stop.send(true).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phase_past_its_deadline_fails() {
    let node = Node {
        objects: vec![proposal("a")],
        missing: 1,
        ..Node::default()
    };
    let hx = start(vec![node], |c| {
        c.idle_timeout = Duration::from_secs(30);
        c.phase_deadline = Duration::from_secs(1);
    })
    .await;
    let r = hx.task.await.unwrap();
    assert_eq!(
        r,
        Err(SyncError::Incomplete(
            "the phase ran past its deadline".into()
        ))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn objects_one_peer_announced_are_confirmed_by_the_others() {
    let obj = proposal("a");
    let node = Node {
        objects: vec![obj.clone()],
        ..Node::default()
    };
    // The second peer was full-synced within the window: it is only asked
    // about the object one peer announced, and confirms it.
    let mut hx = start_with(
        vec![node.clone(), node],
        |_| {},
        |_, history, addrs| {
            history.full_synced.insert(addrs[1], Instant::now());
        },
    )
    .await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced && p.objects == 1).await;
    assert_eq!(p.unconfirmed_objects, 0);
    let mut asked = Vec::new();
    while let Ok(s) = hx.seen[1].try_recv() {
        asked.push(s);
    }
    assert!(
        matches!(&asked[..], [Seen::Govsync(x)] if *x == obj.hash()),
        "{asked:?}"
    );
    hx.stop.send(true).unwrap();
}
