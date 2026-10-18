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
use std::sync::atomic::AtomicBool;

fn key_id(n: u8) -> [u8; 20] {
    let sk = SecretKey::from_secret_bytes([n; 32]).unwrap();
    hash160::Hash::hash(&PublicKey::from_secret_key(&sk).serialize()).to_byte_array()
}

/// `v` with its signature made by key `key` instead: same hash.
fn resigned(v: &GovernanceVote, key: u8) -> GovernanceVote {
    let sk = SecretKey::from_secret_bytes([key; 32]).unwrap();
    let pk = PublicKey::from_secret_key(&sk);
    let mut f = v.clone();
    let d = f.signing_digest(Network::Regtest);
    let s = dashcore::secp256k1::ecdsa::sign(Message::from_digest(d), &sk);
    f.signature = to_recoverable_compact(&s.serialize_compact(), &pk, &d)
        .unwrap()
        .to_vec();
    assert_eq!(f.hash(), v.hash());
    f
}

fn vote(parent: [u8; 32], key: u8, outcome: VoteOutcome) -> GovernanceVote {
    let v = GovernanceVote::funding(
        OutPoint::new(dashcore::Txid::from_byte_array([key; 32]), 0),
        parent,
        outcome,
        unix_now() - 100,
    );
    resigned(&v, key)
}

fn proposal(name: &str) -> GovernanceObject {
    let data = format!(
        r#"{{"end_epoch":{},"name":"{name}","payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}}"#,
        unix_now() + 10_000
    );
    GovernanceObject::new_proposal(1, unix_now(), data.into_bytes())
}

/// How a fake full node behaves. Like Core it leaves out of vote invs the
/// votes already announced on the connection, by either side, but counts
/// them in `ssc`.
#[derive(Clone, Default)]
struct Node {
    objects: Vec<GovernanceObject>,
    votes: Vec<GovernanceVote>,
    /// Announced (and served) right after the full sync, as relays.
    relays: Vec<GovernanceVote>,
    /// Wait this long before answering a `getdata` for votes.
    vote_delay: Option<Duration>,
    /// Wait this long before answering the full sync.
    object_delay: Option<Duration>,
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
    /// Answer every `getdata`, after the delay, with a `notfound` of this
    /// kind instead.
    notfound_kind: Option<(u32, Duration)>,
    /// Answer one vote sync with invs of other proposals' votes, then close.
    relay_instead: Option<RelayInstead>,
    /// Announced (and served) on the first inv the wallet sends, which it
    /// does only once synced: relays in the follow phase.
    follow_objects: Vec<GovernanceObject>,
    /// Served and announced on a one-object `govsync`, but left out of the
    /// full sync.
    hidden_objects: Vec<GovernanceObject>,
    /// Served on `getdata`, never announced.
    hidden_votes: Vec<GovernanceVote>,
}

/// A vote sync of `asked` (the first one, over every node sharing `once`)
/// answered, after `delay`, with its `ssc(11)` and then invs of `votes`
/// only, as Core relaying other votes on the connection before it trickles
/// the sync's own; the node then closes.
#[derive(Clone)]
struct RelayInstead {
    asked: [u8; 32],
    votes: Vec<GovernanceVote>,
    delay: Duration,
    once: Arc<AtomicBool>,
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
    let mut held = votes.clone();
    held.extend(node.hidden_votes.iter().cloned());
    let mut served = node.objects.clone();
    served.extend(node.follow_objects.iter().cloned());
    served.extend(node.hidden_objects.iter().cloned());
    let mut followed = false;
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
                    if served.iter().any(|o| o.hash() == hash) {
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
                }
                if hash == [0; 32] {
                    if let Some(d) = node.object_delay {
                        tokio::time::sleep(d).await;
                    }
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
                        .filter(|i| known.insert(i.hash))
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
                    if let Some(r) = &node.relay_instead
                        && r.asked == hash
                        && !r.once.swap(true, std::sync::atomic::Ordering::SeqCst)
                    {
                        tokio::time::sleep(r.delay).await;
                        conn.send(commands::SYNCSTATUSCOUNT, &ssc).await.unwrap();
                        let items: Vec<InvItem> = r
                            .votes
                            .iter()
                            .map(|v| InvItem {
                                kind: INV_GOVERNANCE_VOTE,
                                hash: v.hash(),
                            })
                            .collect();
                        conn.send("inv", &encode_inv(&items)).await.unwrap();
                        conn.shutdown().await;
                        return;
                    }
                    conn.send(commands::SYNCSTATUSCOUNT, &ssc).await.unwrap();
                    if node.close_after_vote_ssc {
                        conn.shutdown().await;
                        return;
                    }
                    let items: Vec<InvItem> =
                        all.into_iter().filter(|i| known.insert(i.hash)).collect();
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
                if let Some((kind, delay)) = node.notfound_kind {
                    tokio::time::sleep(delay).await;
                    let items: Vec<InvItem> =
                        items.iter().map(|i| InvItem { kind, ..*i }).collect();
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
                    let object = (i.kind == INV_GOVERNANCE_OBJECT)
                        .then(|| served.iter().find(|o| o.hash() == i.hash))
                        .flatten();
                    let vote = (i.kind == INV_GOVERNANCE_VOTE)
                        .then(|| held.iter().find(|v| v.hash() == i.hash))
                        .flatten();
                    if let Some(o) = object {
                        conn.send(commands::MNGOVERNANCEOBJECT, &o.encode())
                            .await
                            .unwrap();
                    } else if let Some(v) = vote {
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
                if !followed && !node.follow_objects.is_empty() {
                    followed = true;
                    let items: Vec<InvItem> = node
                        .follow_objects
                        .iter()
                        .map(|o| InvItem {
                            kind: INV_GOVERNANCE_OBJECT,
                            hash: o.hash(),
                        })
                        .collect();
                    conn.send("inv", &encode_inv(&items)).await.unwrap();
                }
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
    history: Arc<Mutex<SyncHistory>>,
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
    let history = Arc::new(Mutex::new(history));
    let task = tokio::spawn(run(
        Arc::clone(&shared),
        cfg,
        addrs,
        Arc::clone(&history),
        stop_rx,
    ));
    Harness {
        shared,
        stop,
        task,
        seen,
        history,
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
    // both, as Core asks up to three peers (D56).
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
    assert_eq!((full, votes), (2, 2));
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
    let hx = start(vec![refusing, good.clone()], |_| {}).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!(p.objects, 1);
    hx.stop.send(true).unwrap();

    // Sol r1 Major 3: a `notfound` naming the hash as a vote neither turns
    // the object's retry into a vote request nor settles it. The good node
    // answers its full sync late, so the mistyping one is asked first, and
    // the mistyping one answers after the good one announced the object.
    let mistyping = Node {
        notfound_kind: Some((INV_GOVERNANCE_VOTE, Duration::from_millis(900))),
        ..good.clone()
    };
    let late = Node {
        object_delay: Some(Duration::from_millis(300)),
        ..good
    };
    let hx = start(vec![mistyping, late], |_| {}).await;
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
async fn unsolicited_items_are_dropped_without_closing_the_peer() {
    // Core drops an item it did not ask for, with no penalty (D59): the
    // burst before the sync answer costs the peer nothing.
    let obj = proposal("a");
    let node = Node {
        objects: vec![obj.clone()],
        votes: vec![vote(obj.hash(), 1, VoteOutcome::Yes)],
        unsolicited: 6,
        ..Node::default()
    };
    let hx = start(vec![node], |c| c.max_misbehaviour = 5).await;
    wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!(hx.shared.store().vote_count(), 1);
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
    let mut t = GovernanceObject::new_proposal(1, unix_now(), data.into_bytes());
    t.object_type = crate::object::OBJECT_TYPE_TRIGGER;
    t
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
    // closes before its invs: the second one is asked instead (one peer per
    // proposal, so only the replacement asks it).
    let hx = start(vec![closing, good], |c| c.vote_peers_per_proposal = 1).await;
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
    // its votes have no other announcer, so the sync goes to the second
    // (one peer per proposal, so only the replacement asks it).
    let hx = start(vec![closing, good], |c| c.vote_peers_per_proposal = 1).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!((p.votes, p.peers), (2, 1));
    hx.stop.send(true).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn votes_relayed_before_their_sync_are_credited_to_its_count() {
    let obj = proposal("a");
    let h = obj.hash();
    // The relay reaches the wallet before it asks for the proposal's votes:
    // the sync counts both votes but announces only the other one (D36).
    let node = Node {
        objects: vec![obj],
        votes: vec![vote(h, 2, VoteOutcome::No)],
        relays: vec![vote(h, 1, VoteOutcome::Yes)],
        ..Node::default()
    };
    let hx = start(vec![node.clone(), node], |_| {}).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!((p.votes, p.peers), (2, 2));
    hx.stop.send(true).unwrap();
    assert_eq!(hx.task.await.unwrap(), Ok(()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_fetched_before_its_sync_is_asked_is_credited_when_asked() {
    let obj = proposal("a");
    let h = obj.hash();
    // The slow peer holds the objects phase open, so the fast peer's relay
    // is fetched before any peer is asked for the proposal's votes (D36).
    let fast = Node {
        objects: vec![obj],
        votes: vec![vote(h, 2, VoteOutcome::No)],
        relays: vec![vote(h, 1, VoteOutcome::Yes)],
        ..Node::default()
    };
    let slow = Node {
        object_delay: Some(Duration::from_millis(1500)),
        ..fast.clone()
    };
    let hx = start(vec![fast, slow], |_| {}).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!((p.votes, p.peers), (2, 2));
    hx.stop.send(true).unwrap();
    assert_eq!(hx.task.await.unwrap(), Ok(()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_synced_run_turns_the_trigger_time_window_on() {
    let mut old = trigger();
    old.time = unix_now() - 30 * 86_400;
    // Before any sync, Core skips the window (`MasternodeRateCheck`).
    assert!(
        GovernanceStore::new(Network::Regtest)
            .add_object(old.clone(), None, unix_now())
            .is_ok()
    );
    let node = Node {
        objects: vec![proposal("a")],
        ..Node::default()
    };
    let hx = start(vec![node], |_| {}).await;
    wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!(
        hx.shared.store().add_object(old, None, unix_now()),
        Err(crate::validate::ObjectRejection::TriggerTimeOutOfRange)
    );
    hx.stop.send(true).unwrap();
    assert_eq!(hx.task.await.unwrap(), Ok(()));
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
async fn a_mistyped_copy_is_not_remembered_so_the_genuine_one_still_comes() {
    // The wire type is outside the hash: a peer relabelling a proposal a
    // trigger must not make the wallet refuse its hash for good (D42).
    let good = proposal("a");
    let mut mistyped = good.clone();
    mistyped.object_type = crate::object::OBJECT_TYPE_TRIGGER;
    assert_eq!(mistyped.hash(), good.hash());
    let node = Node {
        objects: vec![mistyped],
        ..Node::default()
    };
    let hx = start(vec![node], |_| {}).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!(p.objects, 0);
    {
        let mut store = hx.shared.store();
        assert!(!store.has_object(&good.hash()));
        assert_eq!(store.add_object(good.clone(), None, unix_now()), Ok(true));
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
    // Two chatty peers: a vote sync needs two completions once the run had
    // two peers (D68).
    let hx = start(vec![chatty.clone(), chatty, mute], |c| {
        c.peer.session.ping_interval = Duration::from_millis(200);
        c.answer_timeout = Duration::from_secs(1);
        c.idle_timeout = Duration::from_secs(5);
    })
    .await;
    hx.shared.store().set_confirmations(1);
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!((p.objects, p.votes, p.peers), (1, 1, 2));
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
    // about the object one peer announced, and confirms it, then for its
    // votes (D56).
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
    // The mock may log its vote sync after answering it.
    let mut asked = Vec::new();
    let until = Instant::now() + Duration::from_secs(5);
    while asked.len() < 2 && Instant::now() < until {
        match hx.seen[1].try_recv() {
            Ok(s) => asked.push(s),
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    assert!(
        matches!(&asked[..], [Seen::Govsync(x), Seen::Govsync(y)] if *x == obj.hash() && *y == obj.hash()),
        "{asked:?}"
    );
    hx.stop.send(true).unwrap();
}

/// Sol r1 Major 4: while a peer's vote sync of A is open, an inv of a vote
/// on B (already held) does not count towards A's `ssc(11)`; when the peer
/// then closes, A's vote sync goes to the other peer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_proposals_relay_does_not_settle_a_vote_sync() {
    let (a, b) = (proposal("a"), proposal("b"));
    let (ha, hb) = (a.hash(), b.hash());
    let (va, vb) = (vote(ha, 1, VoteOutcome::Yes), vote(hb, 2, VoteOutcome::No));
    let node = Node {
        objects: vec![a, b],
        votes: vec![va, vb.clone()],
        relay_instead: Some(RelayInstead {
            asked: ha,
            votes: vec![vb],
            // B's vote sync, at the other node, delivers B's vote first.
            delay: Duration::from_millis(500),
            once: Arc::new(AtomicBool::new(false)),
        }),
        ..Node::default()
    };
    // Three: the one that closes leaves two completions (D68).
    let hx = start(vec![node.clone(), node.clone(), node], |_| {}).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!((p.objects, p.votes), (2, 2));
    hx.stop.send(true).unwrap();
}

/// Validator M1 on D47: honest objects arrive first and carry no vote yet;
/// two peers then confirm enough fakes to pass the cap. Evicting the honest
/// ones must not end in `Synced`: the run fails as the store is full.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flood_past_the_object_cap_fails_the_run() {
    let (a, b) = (proposal("a"), proposal("b"));
    let honest = Node {
        votes: vec![vote(a.hash(), 1, VoteOutcome::Yes)],
        objects: vec![a, b],
        ..Node::default()
    };
    let flooding = Node {
        objects: (0..4).map(|i| proposal(&format!("fake{i}"))).collect(),
        object_delay: Some(Duration::from_millis(300)),
        ..Node::default()
    };
    let hx = start_with(
        vec![honest.clone(), honest, flooding.clone(), flooding],
        |_| {},
        |shared, _, _| shared.store().set_max_objects(4),
    )
    .await;
    let p = wait_for(&hx.shared, |p| {
        matches!(p.phase, Phase::Synced | Phase::Failed)
    })
    .await;
    assert_eq!(p.phase, Phase::Failed, "{p:?}");
    assert!(p.last_synced_at.is_none());
    let r = hx.task.await.unwrap();
    assert!(
        matches!(&r, Err(SyncError::Incomplete(e)) if e.contains("store is full")),
        "{r:?}"
    );
}

/// Validator M1 on D47: unvoted triggers left from earlier runs, which the
/// wallet cannot tell from forged ones, make room for this run's objects.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_triggers_make_room_for_this_runs_objects() {
    let honest = Node {
        objects: vec![proposal("a"), proposal("b")],
        ..Node::default()
    };
    let hx = start_with(
        vec![honest.clone(), honest],
        |_| {},
        |shared, _, _| {
            let mut store = shared.store();
            store.set_max_objects(3);
            for height in 100..103 {
                let t = big_trigger(height, 1);
                store.add_object(t, None, unix_now()).unwrap();
            }
        },
    )
    .await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!(p.objects, 3);
    assert_eq!(hx.shared.store().current_proposals(unix_now()).len(), 2);
    hx.stop.send(true).unwrap();
    assert_eq!(hx.task.await.unwrap(), Ok(()));
}

fn big_trigger(height: i32, payees: usize) -> GovernanceObject {
    let addresses = vec!["yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf"; payees].join("|");
    let amounts = vec!["1"; payees].join("|");
    let hash = "ab".repeat(32);
    let hashes = vec![hash.as_str(); payees].join("|");
    let data = format!(
        r#"{{"event_block_height":{height},"payment_addresses":"{addresses}","payment_amounts":"{amounts}","proposal_hashes":"{hashes}","type":2}}"#
    );
    let mut t = GovernanceObject::new_proposal(1, unix_now(), data.into_bytes());
    t.object_type = crate::object::OBJECT_TYPE_TRIGGER;
    t
}

/// Fills the unconfirmed byte bound with forged triggers from addresses
/// no node of the run has, each up to its share, then with short
/// proposals: one with a longer name than `p9999` no longer fits.
fn fill_unconfirmed(shared: &Shared) {
    let mut store = shared.store();
    let (max, share) = (
        crate::store::MAX_UNCONFIRMED_BYTES,
        crate::store::MAX_UNCONFIRMED_BYTES_PER_PEER,
    );
    let (mut total, mut height) = (0usize, 100);
    for n in 1..=(max / share + 1) as u8 {
        let from: SocketAddr = format!("10.9.9.{n}:9999").parse().unwrap();
        let mut mine = 0usize;
        loop {
            let room = (max - total).min(share - mine);
            if room <= 150 {
                break;
            }
            let t = big_trigger(height, ((room - 120) / 102).clamp(1, 28_000));
            if t.data.len() > room {
                break;
            }
            total += t.data.len();
            mine += t.data.len();
            store.add_object(t, Some(from), unix_now()).unwrap();
            height += 1;
        }
    }
    let pad: SocketAddr = "10.9.8.1:9999".parse().unwrap();
    let mut i = 0;
    while store
        .add_object(proposal(&format!("p{i}")), Some(pad), unix_now())
        .is_ok()
    {
        i += 1;
    }
}

/// Validator r2 MJ1: peers' forged triggers fill the unconfirmed bound;
/// proposals two other peers announce are confirmed on arrival and stored
/// regardless, rather than refused unseen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unconfirmed_bytes_do_not_hide_what_two_peers_announce() {
    let honest = Node {
        objects: vec![proposal("a"), proposal("b")],
        ..Node::default()
    };
    let hx = start_with(
        vec![honest.clone(), honest],
        |_| {},
        |shared, _, _| fill_unconfirmed(shared),
    )
    .await;
    let p = wait_for(&hx.shared, |p| {
        matches!(p.phase, Phase::Synced | Phase::Failed)
    })
    .await;
    assert_eq!(p.phase, Phase::Synced, "{p:?}");
    assert_eq!(hx.shared.store().current_proposals(unix_now()).len(), 2);
    hx.stop.send(true).unwrap();
    assert_eq!(hx.task.await.unwrap(), Ok(()));
}

/// Two nodes serving `honest` and one that relays `fakes` after the
/// wallet's first inv; returns once synced, with that inv sent.
async fn follow_flood(
    honest: Vec<GovernanceObject>,
    fakes: usize,
    prepare: impl FnOnce(&Shared, &mut SyncHistory, &[SocketAddr]),
) -> Harness {
    let own = vote(honest[0].hash(), 1, VoteOutcome::Yes);
    let honest = Node {
        objects: honest,
        ..Node::default()
    };
    let flooding = Node {
        follow_objects: (0..fakes).map(|i| proposal(&format!("fake{i}"))).collect(),
        ..Node::default()
    };
    let hx = start_with(vec![honest.clone(), honest, flooding], |_| {}, prepare).await;
    wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert_eq!(hx.shared.relay_vote(&own), 3);
    hx
}

/// Validator r2 m4: the cap overflowing in the follow phase fails the run
/// as it does while syncing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flood_after_synced_fails_the_follow_phase() {
    let hx = follow_flood(vec![proposal("a"), proposal("b")], 3, |shared, _, _| {
        shared.store().set_max_objects(4)
    })
    .await;
    let p = wait_for(&hx.shared, |p| p.phase != Phase::Synced).await;
    assert_eq!(p.phase, Phase::Failed, "{p:?}");
    let r = hx.task.await.unwrap();
    assert!(
        matches!(&r, Err(SyncError::Incomplete(e)) if e.contains("store is full")),
        "{r:?}"
    );
}

/// Validator r2 m1: a run with no object phase (every peer full-synced
/// within its window) keeps the last run's objects current, so a flood in
/// its follow phase that pushes them out fails the run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_without_an_object_phase_keeps_the_last_runs_objects_current() {
    let (a, b) = (proposal("a"), proposal("b"));
    let held = vec![a.clone(), b.clone()];
    let hx = follow_flood(held, 2, move |shared, history, addrs| {
        shared.progress().last_synced_at = Some(unix_now() as u64);
        for addr in addrs {
            history.full_synced.insert(*addr, Instant::now());
        }
        let mut store = shared.store();
        store.set_max_objects(3);
        store.add_object(a, None, unix_now()).unwrap();
        store.add_object(b, None, unix_now()).unwrap();
    })
    .await;
    let p = wait_for(&hx.shared, |p| p.phase != Phase::Synced).await;
    assert_eq!(p.phase, Phase::Failed, "{p:?}");
}

/// Validator r2 m2: an orphan vote's object is asked of every peer, as
/// Core's scheduler does, so it comes back confirmed and shown.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_orphan_votes_object_is_asked_of_every_peer() {
    let x = proposal("x");
    let hidden = Node {
        hidden_objects: vec![x.clone()],
        ..Node::default()
    };
    let relaying = Node {
        relays: vec![vote(x.hash(), 1, VoteOutcome::Yes)],
        ..hidden.clone()
    };
    let hx = start(vec![relaying, hidden], |_| {}).await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced && p.objects == 1).await;
    assert_eq!((p.unconfirmed_objects, p.votes), (0, 1));
    assert!(hx.shared.store().object(&x.hash()).is_some());
    hx.stop.send(true).unwrap();
}

/// Answers a `govsync` with its sync status count.
async fn send_status(conn: &mut dw_p2p::testing::MockConn, kind: i32, count: i32) {
    let mut ssc = kind.to_le_bytes().to_vec();
    ssc.extend_from_slice(&count.to_le_bytes());
    conn.send(commands::SYNCSTATUSCOUNT, &ssc).await.unwrap();
}

/// Answers the full sync at once with `h`, holds the `getdata` for it
/// while the honest nodes announce it, then (if `flood`) announces and
/// `notfound`s 150,000 junk objects before sending `h`.
async fn announcer_clearing_peer(mock: MockPeer, h: GovernanceObject, flood: bool, hold: Duration) {
    let mut conn = mock.accept().await.unwrap();
    let mut sent = false;
    while let Ok(msg) = conn.recv().await {
        match msg.command.as_str() {
            commands::MNGOVERNANCESYNC if msg.payload[..32] == [0; 32] => {
                send_status(&mut conn, SSC_GOVOBJ, 1).await;
                let item = InvItem {
                    kind: INV_GOVERNANCE_OBJECT,
                    hash: h.hash(),
                };
                conn.send("inv", &encode_inv(&[item])).await.unwrap();
            }
            // A vote sync: it holds no votes.
            commands::MNGOVERNANCESYNC if msg.payload[32..] != object_fetch_filter()[..] => {
                send_status(&mut conn, SSC_GOVOBJ_VOTE, 0).await;
            }
            "getdata" if !sent => {
                let items = decode_inv(&msg.payload).unwrap();
                if !items.iter().any(|i| i.hash == h.hash()) {
                    continue;
                }
                sent = true;
                tokio::time::sleep(hold).await;
                for round in 0u8..3 {
                    if !flood {
                        break;
                    }
                    let junk: Vec<InvItem> = (0u32..50_000)
                        .map(|i| {
                            let mut hash = [0xee; 32];
                            hash[0] = round;
                            hash[1..5].copy_from_slice(&i.to_le_bytes());
                            InvItem {
                                kind: INV_GOVERNANCE_OBJECT,
                                hash,
                            }
                        })
                        .collect();
                    conn.send("inv", &encode_inv(&junk)).await.unwrap();
                    conn.send("notfound", &encode_inv(&junk)).await.unwrap();
                }
                conn.send(commands::MNGOVERNANCEOBJECT, &h.encode())
                    .await
                    .unwrap();
            }
            _ => {}
        }
    }
}

/// A fake peer written for one test.
type CustomPeer = Box<
    dyn FnOnce(MockPeer) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send,
>;

fn custom<Fut>(f: impl FnOnce(MockPeer) -> Fut + Send + 'static) -> CustomPeer
where
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    Box::new(move |mock| Box::pin(f(mock)))
}

/// Runs a sync whose first peers (asked first) are served by `first`, the
/// others by `nodes`; `prepare` sees the store first. Returns once the run
/// synced or failed, with the shared state and the stop handle.
async fn start_custom(
    first: Vec<CustomPeer>,
    nodes: Vec<Node>,
    tune: impl FnOnce(&mut SyncConfig),
    prepare: impl FnOnce(&Shared),
) -> (Arc<Shared>, Progress, watch::Sender<bool>) {
    let shared = Arc::new(Shared::new(Network::Regtest, Box::new(|| {})));
    prepare(&shared);
    let mut addrs = Vec::new();
    for f in first {
        let mock = MockPeer::bind(Network::Regtest).await.unwrap();
        addrs.push(mock.addr());
        tokio::spawn(f(mock));
    }
    for node in nodes {
        let mock = MockPeer::bind(Network::Regtest).await.unwrap();
        addrs.push(mock.addr());
        let (tx, rx) = mpsc::unbounded_channel();
        std::mem::forget(rx);
        tokio::spawn(serve(mock, node, tx));
    }
    let mut cfg = SyncConfig::new(PeerConfig::new(Network::Regtest, "/t/", 1));
    cfg.idle_timeout = Duration::from_secs(60);
    cfg.max_peers = addrs.len();
    tune(&mut cfg);
    let (stop, stop_rx) = watch::channel(false);
    tokio::spawn(run(
        Arc::clone(&shared),
        cfg,
        addrs,
        Arc::new(Mutex::new(SyncHistory::default())),
        stop_rx,
    ));
    for _ in 0..1200 {
        let p = shared.progress().clone();
        if matches!(p.phase, Phase::Synced | Phase::Failed) {
            return (shared, p, stop);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the run never ended its sync: {:?}", shared.progress());
}

/// With the unconfirmed bound full, a peer asked first for `h` holds it
/// while two honest nodes announce it; returns the run's end state and
/// whether `h` is shown.
async fn announcer_clearing_run(flood: bool) -> (Progress, bool) {
    let h = proposal("a-longer-honest-name");
    let honest = Node {
        objects: vec![h.clone()],
        object_delay: Some(Duration::from_millis(300)),
        ..Node::default()
    };
    let held = h.clone();
    let (shared, end, stop) = start_custom(
        vec![custom(move |mock| {
            announcer_clearing_peer(mock, held, flood, Duration::from_millis(1500))
        })],
        vec![honest.clone(), honest],
        |_| {},
        fill_unconfirmed,
    )
    .await;
    let shown = shared.store().object(&h.hash()).is_some();
    let _ = stop.send(true);
    (end, shown)
}

/// Validator r3 MJ1: the peer asked first for an object cannot clear the
/// record of its other announcers (by flooding junk announcements) to make
/// its copy arrive unconfirmed and be refused unseen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_clearing_announcer_records_does_not_hide_an_object() {
    for flood in [false, true] {
        let (p, shown) = announcer_clearing_run(flood).await;
        assert_eq!(p.phase, Phase::Synced, "flood {flood}: {p:?}");
        assert!(shown, "flood {flood}");
    }
}

/// Validator r3 m1 and r4 MJ2: with the unconfirmed bound full of earlier
/// runs' objects, an object only a fresh peer announces in a run whose
/// other peer was full-synced within its window (D25) still makes room,
/// is asked about and confirmed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_object_only_a_fresh_peer_announces_makes_room_in_a_full_bound() {
    let obj = proposal("a-longer-honest-name");
    let h = obj.hash();
    let node = Node {
        objects: vec![obj],
        ..Node::default()
    };
    let hx = start_with(
        vec![node.clone(), node],
        |_| {},
        |shared, history, addrs| {
            history.full_synced.insert(addrs[1], Instant::now());
            fill_unconfirmed(shared);
        },
    )
    .await;
    let p = wait_for(&hx.shared, |p| {
        matches!(p.phase, Phase::Synced | Phase::Failed)
    })
    .await;
    assert_eq!(p.phase, Phase::Synced, "{p:?}");
    let shown = || hx.shared.store().object(&h).is_some();
    for _ in 0..100 {
        if shown() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(shown());
    hx.stop.send(true).unwrap();
}

/// Validator r3 m3: a copy refused by its sender's unconfirmed quota keeps
/// the sender as an announcer, so the object is confirmed once another
/// peer announces and sends it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_copys_sender_counts_once_another_peer_announces_it() {
    let h = proposal("h");
    let first = Node {
        objects: vec![h.clone()],
        ..Node::default()
    };
    let later = Node {
        object_delay: Some(Duration::from_millis(700)),
        ..first.clone()
    };
    let hx = start_with(
        vec![first, later],
        |_| {},
        |shared, _, addrs| {
            let mut store = shared.store();
            for i in 0..crate::store::MAX_UNCONFIRMED_PER_PEER {
                store
                    .add_object(proposal(&format!("seed{i}")), Some(addrs[0]), unix_now())
                    .unwrap();
            }
        },
    )
    .await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced).await;
    assert!(hx.shared.store().object(&h.hash()).is_some(), "{p:?}");
    hx.stop.send(true).unwrap();
}

/// Validator r3 m2: orphan votes from one peer spend that peer's parent
/// fetches only, so another peer's orphan still has its object asked for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_peers_orphans_do_not_spend_another_peers_parent_fetches() {
    let x = proposal("x");
    let fakes: Vec<GovernanceVote> = (0..3)
        .map(|i| vote(proposal(&format!("gone{i}")).hash(), 1, VoteOutcome::Yes))
        .collect();
    let orphaning = Node {
        relays: fakes,
        ..Node::default()
    };
    let hidden = Node {
        hidden_objects: vec![x.clone()],
        object_delay: Some(Duration::from_millis(500)),
        ..Node::default()
    };
    let relaying = Node {
        relays: vec![vote(x.hash(), 2, VoteOutcome::No)],
        ..hidden.clone()
    };
    let hx = start(vec![orphaning, relaying, hidden], |c| {
        c.max_parent_fetches = 3;
    })
    .await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced && p.objects == 1).await;
    assert!(hx.shared.store().object(&x.hash()).is_some(), "{p:?}");
    hx.stop.send(true).unwrap();
}

/// Validator r3 m1: in a run whose other peer was full-synced within its
/// window, one fresh peer's 500 unconfirmed objects do not use up the
/// confirmation asks another fresh peer's object needs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirmation_asks_are_shared_between_first_announcers() {
    let h = proposal("h");
    let crowding = Node {
        objects: (0..crate::store::MAX_UNCONFIRMED_PER_PEER)
            .map(|i| proposal(&format!("crowd{i}")))
            .collect(),
        ..Node::default()
    };
    // `h` comes last; earlier runs' objects, older still, from 300
    // senders, are not this run's to ask about.
    let honest = Node {
        objects: vec![h.clone()],
        object_delay: Some(Duration::from_millis(1500)),
        ..Node::default()
    };
    let hx = start_with(
        vec![crowding, honest.clone(), honest],
        |_| {},
        |shared, history, addrs| {
            history.full_synced.insert(addrs[2], Instant::now());
            let mut store = shared.store();
            for i in 0..300u16 {
                let from: SocketAddr = format!("10.7.{}.{}:9999", i / 250, i % 250)
                    .parse()
                    .unwrap();
                store
                    .add_object(proposal(&format!("old{i}")), Some(from), unix_now() - 60)
                    .unwrap();
            }
        },
    )
    .await;
    let p = wait_for(&hx.shared, |p| p.phase == Phase::Synced && p.objects >= 1).await;
    assert!(hx.shared.store().object(&h.hash()).is_some(), "{p:?}");
    hx.stop.send(true).unwrap();
}

/// Announces `h` as a vote first, holds the `getdata` of that kind until
/// the honest nodes announced `h` as an object, then answers `notfound` of
/// the asked kind.
async fn kind_lying_peer(mock: MockPeer, h: [u8; 32]) {
    let mut conn = mock.accept().await.unwrap();
    while let Ok(msg) = conn.recv().await {
        match msg.command.as_str() {
            commands::MNGOVERNANCESYNC if msg.payload[..32] == [0; 32] => {
                send_status(&mut conn, SSC_GOVOBJ, 0).await;
                let item = InvItem {
                    kind: INV_GOVERNANCE_VOTE,
                    hash: h,
                };
                conn.send("inv", &encode_inv(&[item])).await.unwrap();
            }
            commands::MNGOVERNANCESYNC if msg.payload[32..] != object_fetch_filter()[..] => {
                send_status(&mut conn, SSC_GOVOBJ_VOTE, 0).await;
            }
            "getdata" => {
                let items = decode_inv(&msg.payload).unwrap();
                tokio::time::sleep(Duration::from_millis(1500)).await;
                conn.send("notfound", &encode_inv(&items)).await.unwrap();
            }
            _ => {}
        }
    }
}

/// Validator r4 MJ1: a peer announcing an object's hash as a vote first
/// does not turn every request for it into a vote request: once the vote
/// announcers fail, the object announcers are asked for the object.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hash_first_announced_as_the_other_kind_is_still_fetched() {
    let (h, g) = (proposal("h-two-honest-announcers"), proposal("g-control"));
    let honest = Node {
        objects: vec![h.clone(), g.clone()],
        object_delay: Some(Duration::from_millis(300)),
        ..Node::default()
    };
    let lied = h.hash();
    let (shared, p, stop) = start_custom(
        vec![custom(move |mock| kind_lying_peer(mock, lied))],
        vec![honest.clone(), honest],
        |_| {},
        |_| {},
    )
    .await;
    let s = shared.store();
    assert!(s.object(&g.hash()).is_some(), "control {p:?}");
    assert_eq!(p.phase, Phase::Synced, "{p:?}");
    assert!(s.object(&h.hash()).is_some(), "{p:?}");
    drop(s);
    let _ = stop.send(true);
}

/// Announces `count` invented objects in its full sync and answers every
/// `getdata` with `notfound`: their late-copy records stay, filling it.
async fn junk_filler(mock: MockPeer, count: u32) {
    let mut conn = mock.accept().await.unwrap();
    while let Ok(msg) = conn.recv().await {
        match msg.command.as_str() {
            commands::MNGOVERNANCESYNC if msg.payload[..32] == [0; 32] => {
                send_status(&mut conn, SSC_GOVOBJ, count as i32).await;
                let items: Vec<InvItem> = (0..count)
                    .map(|i| {
                        let mut hash = [0xdd; 32];
                        hash[..4].copy_from_slice(&i.to_le_bytes());
                        InvItem {
                            kind: INV_GOVERNANCE_OBJECT,
                            hash,
                        }
                    })
                    .collect();
                for chunk in items.chunks(50_000) {
                    conn.send("inv", &encode_inv(chunk)).await.unwrap();
                }
            }
            commands::MNGOVERNANCESYNC if msg.payload[32..] != object_fetch_filter()[..] => {
                send_status(&mut conn, SSC_GOVOBJ_VOTE, 0).await;
            }
            "getdata" => {
                conn.send("notfound", &msg.payload).await.unwrap();
            }
            _ => {}
        }
    }
}

/// Validator r4 m1: once one peer filled the late-copy record, the run's
/// request record still counts the announcers of an object held by the
/// peer asked first, so it arrives confirmed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_late_copy_record_does_not_lose_an_objects_announcers() {
    let h = proposal("h");
    let honest = Node {
        objects: vec![h.clone()],
        object_delay: Some(Duration::from_millis(500)),
        ..Node::default()
    };
    let held = h.clone();
    let (shared, p, stop) = start_custom(
        vec![
            custom(|mock| junk_filler(mock, 50)),
            custom(move |mock| {
                announcer_clearing_peer(mock, held, false, Duration::from_millis(1500))
            }),
        ],
        vec![honest.clone(), honest],
        |c| c.max_late_announcers = 50,
        |_| {},
    )
    .await;
    assert_eq!(p.phase, Phase::Synced, "{p:?}");
    assert!(shared.store().object(&h.hash()).is_some(), "{p:?}");
    let _ = stop.send(true);
}

/// Validator r4 m3: parent fetches are charged to the orphan vote's sender
/// (two here), and a parent is asked of each peer once whoever sent its
/// orphans.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parent_fetches_are_charged_to_the_sender_and_asked_once() {
    let gone: Vec<[u8; 32]> = (0..3)
        .map(|i| proposal(&format!("gone{i}")).hash())
        .collect();
    let sender = Node {
        relays: gone.iter().map(|g| vote(*g, 1, VoteOutcome::Yes)).collect(),
        ..Node::default()
    };
    let other = Node {
        relays: vec![vote(gone[0], 2, VoteOutcome::No)],
        object_delay: Some(Duration::from_millis(500)),
        ..Node::default()
    };
    let mut hx = start(vec![sender, other], |c| c.max_parent_fetches = 2).await;
    wait_for(&hx.shared, |p| {
        p.phase == Phase::Synced && p.orphan_votes == 4
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut asked = HashMap::new();
    while let Ok(s) = hx.seen[1].try_recv() {
        if let Seen::Govsync(h) = s
            && gone.contains(&h)
        {
            *asked.entry(h).or_insert(0) += 1;
        }
    }
    assert_eq!(asked, HashMap::from([(gone[0], 1), (gone[1], 1)]));
    hx.stop.send(true).unwrap();
}

/// Announces `junk` invented vote hashes and then `h` as `h_kind` before
/// any `govsync`, answers its syncs with nothing, and never answers a
/// `getdata` (validator r5).
async fn withholding_peer(mock: MockPeer, junk: u32, h: [u8; 32], h_kind: u32) {
    let mut conn = mock.accept().await.unwrap();
    let mut items: Vec<InvItem> = (0..junk)
        .map(|i| {
            let mut hash = [0xab; 32];
            hash[..4].copy_from_slice(&i.to_le_bytes());
            InvItem {
                kind: INV_GOVERNANCE_VOTE,
                hash,
            }
        })
        .collect();
    items.push(InvItem {
        kind: h_kind,
        hash: h,
    });
    conn.send("inv", &encode_inv(&items)).await.unwrap();
    while let Ok(msg) = conn.recv().await {
        match msg.command.as_str() {
            commands::MNGOVERNANCESYNC if msg.payload[..32] == [0; 32] => {
                send_status(&mut conn, SSC_GOVOBJ, 0).await;
            }
            commands::MNGOVERNANCESYNC if msg.payload[32..] != object_fetch_filter()[..] => {
                send_status(&mut conn, SSC_GOVOBJ_VOTE, 0).await;
            }
            _ => {}
        }
    }
}

/// Two honest nodes hold `h` and `g` (one vote on `g`); the withholding
/// peer announces `h` as `h_kind` first.
async fn withheld_run(
    junk: u32,
    h_kind: u32,
    tune: impl FnOnce(&mut SyncConfig),
) -> (Arc<Shared>, Progress, watch::Sender<bool>, [u8; 32]) {
    let (h, g) = (proposal("h-two-honest-announcers"), proposal("g-control"));
    let honest = Node {
        objects: vec![h.clone(), g.clone()],
        votes: vec![vote(g.hash(), 1, VoteOutcome::Yes)],
        object_delay: Some(Duration::from_millis(300)),
        ..Node::default()
    };
    let held = h.hash();
    let (shared, p, stop) = start_custom(
        vec![custom(move |mock| {
            withholding_peer(mock, junk, held, h_kind)
        })],
        vec![honest.clone(), honest],
        tune,
        |shared| {
            shared.store().set_masternode_list(&[ListEntry {
                voting_key_id: key_id(1),
                weight: Some(1),
                valid: true,
            }]);
        },
    )
    .await;
    assert!(shared.store().object(&g.hash()).is_some(), "control {p:?}");
    (shared, p, stop, held)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_vote_announcement_of_an_objects_hash_does_not_hide_it() {
    // Validator r5 MJ1: announced as a vote first, the hash was asked only
    // as a vote and the honest object announcements waited behind it.
    let (shared, p, stop, h) = withheld_run(0, INV_GOVERNANCE_VOTE, |_| {}).await;
    let shown = shared.store().object(&h).is_some();
    let _ = stop.send(true);
    assert_eq!(p.phase, Phase::Synced, "{p:?}");
    assert!(shown, "Synced without the honest proposal: {p:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_queued_behind_full_slots_moves_to_another_announcer() {
    // Validator r5 MJ2: a queued request never timed out, so one peer
    // holding its slots failed every run. Its 20 junk requests hold its one
    // slot for 20 s of timeouts, past the idle timeout, unless `h` moves.
    let (shared, p, stop, h) = withheld_run(20, INV_GOVERNANCE_OBJECT, |c| {
        c.max_in_flight = 1;
        c.request_timeout = Duration::from_millis(500);
        c.idle_timeout = Duration::from_secs(5);
    })
    .await;
    let shown = shared.store().object(&h).is_some();
    let _ = stop.send(true);
    assert_eq!(p.phase, Phase::Synced, "{p:?}");
    assert!(shown, "{p:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_peer_cannot_withhold_a_proposals_votes() {
    // Validator r5 m3: the one peer asked for a proposal's votes answered
    // `ssc(11, 0)`.
    let (_shared, p, stop, _h) = withheld_run(0, INV_GOVERNANCE_VOTE, |_| {}).await;
    let _ = stop.send(true);
    assert_eq!((p.phase, p.votes), (Phase::Synced, 1), "{p:?}");
}

/// Answers its full sync with `h` and closes right after.
async fn announce_and_close(mock: MockPeer, h: [u8; 32]) {
    let mut conn = mock.accept().await.unwrap();
    while let Ok(msg) = conn.recv().await {
        if msg.command == commands::MNGOVERNANCESYNC && msg.payload[..32] == [0; 32] {
            // After the junk peer's announcements.
            tokio::time::sleep(Duration::from_millis(200)).await;
            send_status(&mut conn, SSC_GOVOBJ, 1).await;
            let item = InvItem {
                kind: INV_GOVERNANCE_OBJECT,
                hash: h,
            };
            conn.send("inv", &encode_inv(&[item])).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            conn.shutdown().await;
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_announcer_that_closes_still_confirms_the_object() {
    // Validator r5 m1: with the late-copy record full, a closed
    // announcer was forgotten and the object arrived unconfirmed.
    let h = proposal("h");
    let honest = Node {
        objects: vec![h.clone()],
        object_delay: Some(Duration::from_millis(800)),
        ..Node::default()
    };
    let hh = h.hash();
    let (shared, p, stop) = start_custom(
        vec![
            custom(|mock| junk_filler(mock, 50)),
            custom(move |mock| announce_and_close(mock, hh)),
        ],
        vec![honest],
        |c| c.max_late_announcers = 50,
        |_| {},
    )
    .await;
    let shown = shared.store().object(&h.hash()).is_some();
    let _ = stop.send(true);
    // The objects phase completes. Whether the votes phase settles depends
    // on how many of the peers left complete a vote sync (two needed, D68),
    // which is not this test's subject.
    let votes_short = p
        .last_error
        .as_deref()
        .is_some_and(|e| e.contains("too few peers"));
    assert!(p.phase == Phase::Synced || votes_short, "{p:?}");
    assert!(p.objects_secs.is_some(), "{p:?}");
    assert!(shown, "two honest peers announced it: {p:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delivery_asks_for_the_items_queued_behind_it() {
    // Validator r5 MJ2 on an honest network: more items than the in-flight
    // limit, and a freed slot did not send the next.
    let obj = proposal("a");
    let h = obj.hash();
    let node = Node {
        objects: vec![obj],
        votes: vec![vote(h, 1, VoteOutcome::Yes), vote(h, 2, VoteOutcome::No)],
        ..Node::default()
    };
    let hx = start(vec![node.clone(), node], |c| c.max_in_flight = 1).await;
    let p = wait_for(&hx.shared, |p| {
        matches!(p.phase, Phase::Synced | Phase::Failed)
    })
    .await;
    let _ = hx.stop.send(true);
    assert_eq!((p.phase, p.votes), (Phase::Synced, 2), "{p:?}");
}

/// A proposal that decodes but fails validation (a bad address): refused
/// without penalty (D35).
fn bad_proposal(i: u32) -> GovernanceObject {
    let data = format!(
        r#"{{"end_epoch":{},"name":"bad{i}","payment_address":"nope","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}}"#,
        unix_now() + 10_000
    );
    GovernanceObject::new_proposal(1, unix_now(), data.into_bytes())
}

/// Announces `h` as `h_kind`, never answers that `getdata`, and keeps
/// delivering: every 200 ms it announces and sends a fresh invalid object
/// (refused without penalty, but a delivery).
async fn keepalive_peer(mock: MockPeer, h: [u8; 32], h_kind: u32) {
    let mut conn = mock.accept().await.unwrap();
    let item = InvItem {
        kind: h_kind,
        hash: h,
    };
    conn.send("inv", &encode_inv(&[item])).await.unwrap();
    let mut n = 0u32;
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    loop {
        tokio::select! {
            m = conn.recv() => {
                let Ok(msg) = m else { return };
                match msg.command.as_str() {
                    commands::MNGOVERNANCESYNC if msg.payload[..32] == [0; 32] => {
                        send_status(&mut conn, SSC_GOVOBJ, 0).await;
                    }
                    commands::MNGOVERNANCESYNC if msg.payload[32..] != object_fetch_filter()[..] => {
                        send_status(&mut conn, SSC_GOVOBJ_VOTE, 0).await;
                    }
                    "getdata" => {
                        for i in decode_inv(&msg.payload).unwrap() {
                            for k in n.saturating_sub(5)..n {
                                let b = bad_proposal(k);
                                if b.hash() == i.hash {
                                    let _ = conn.send(commands::MNGOVERNANCEOBJECT, &b.encode()).await;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ = tick.tick() => {
                let b = bad_proposal(n);
                n += 1;
                let item = InvItem { kind: INV_GOVERNANCE_OBJECT, hash: b.hash() };
                let _ = conn.send("inv", &encode_inv(&[item])).await;
            }
        }
    }
}

async fn keepalive_run(h_kind: u32) {
    let (h, g) = (proposal("h-two-honest-announcers"), proposal("g-control"));
    let honest = Node {
        objects: vec![h.clone(), g.clone()],
        object_delay: Some(Duration::from_millis(300)),
        ..Node::default()
    };
    let held = h.hash();
    let (shared, p, stop) = start_custom(
        vec![custom(move |mock| keepalive_peer(mock, held, h_kind))],
        vec![honest.clone(), honest],
        |c| {
            c.request_timeout = Duration::from_millis(500);
            c.max_request_age = Duration::from_secs(2);
        },
        |_| {},
    )
    .await;
    let shown = shared.store().object(&held).is_some();
    let _ = stop.send(true);
    assert_eq!(p.phase, Phase::Synced, "{p:?}");
    assert!(shared.store().object(&g.hash()).is_some(), "control {p:?}");
    assert!(shown, "{p:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_that_keeps_delivering_cannot_hold_a_request() {
    // Validator r5 MJ1: deliveries of anything restarted the timeout of
    // every request of the peer, so it held `h` for good.
    keepalive_run(INV_GOVERNANCE_OBJECT).await;
    keepalive_run(INV_GOVERNANCE_VOTE).await;
}

/// A connected peer with id `id` that reads and drops everything.
async fn sink_peer(id: usize, cfg: &SyncConfig) -> PeerState {
    let mock = MockPeer::bind(Network::Regtest).await.unwrap();
    let addr = mock.addr();
    tokio::spawn(async move {
        let mut conn = mock.accept().await.unwrap();
        while conn.recv().await.is_ok() {}
    });
    let (tx, _rx) = mpsc::channel(EVENTS_QUEUE);
    PeerState::new(Peer::connect(id, addr, &cfg.peer, tx).await.unwrap())
}

/// A run with one connected peer (id 0) that reads and drops everything,
/// driven by calling its handlers.
async fn one_peer_run(tune: impl FnOnce(&mut SyncConfig, &mut PeerState)) -> Run {
    let mut cfg = SyncConfig::new(PeerConfig::new(Network::Regtest, "/t/", 1));
    let mut state = sink_peer(0, &cfg).await;
    tune(&mut cfg, &mut state);
    Run {
        shared: Arc::new(Shared::new(Network::Regtest, Box::new(|| {}))),
        cfg,
        history: Arc::new(Mutex::new(SyncHistory::default())),
        peers: vec![state],
        requests: Requests::default(),
        base_bytes: 0,
        votes_open: true,
        announce_open: false,
        vote_targets: HashSet::new(),
        completed: HashSet::new(),
        met: HashSet::new(),
        refetches: Refetches::default(),
        vote_given_up: false,
        lost: 0,
        recovered: 0,
        full_syncs_done: 0,
        vote_parents: HashMap::new(),
        vote_parent_order: VecDeque::new(),
        last_progress: Instant::now(),
    }
}

/// `count` invented vote hashes, distinct per `round`.
fn junk_votes(round: u8, count: u32) -> Vec<InvItem> {
    (0..count)
        .map(|i| {
            let mut hash = [0x99; 32];
            hash[0] = round;
            hash[2..6].copy_from_slice(&i.to_le_bytes());
            InvItem {
                kind: INV_GOVERNANCE_VOTE,
                hash,
            }
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peers_queue_holds_no_more_than_it_may_have_queued() {
    // Validator r5 MJ3: inv + notfound pairs for a hash queued behind a
    // held slot each left a queue entry.
    let mut run = one_peer_run(|c, _| c.max_in_flight = 1).await;
    // Requests ended while queued (moved, or the peer closed) leave their
    // entries behind.
    let x = ([0x22; 32], INV_GOVERNANCE_OBJECT);
    for _ in 0..10_000 {
        run.assign(x, 0, Taken::default());
        run.release(&x);
    }
    let p = &run.peers[0];
    assert!(
        p.queue.len() < 100,
        "queue {} entries, announced {}",
        p.queue.len(),
        p.announced
    );
}

/// Answers its full sync with nothing, its vote syncs with `ssc(11, n)`
/// and, after `delay` (so after the honest nodes), invs of `votes`; every
/// `getdata` gets a `notfound`.
async fn late_notfound_peer(mock: MockPeer, votes: Vec<[u8; 32]>, delay: Duration) {
    let mut conn = mock.accept().await.unwrap();
    while let Ok(msg) = conn.recv().await {
        match msg.command.as_str() {
            commands::MNGOVERNANCESYNC if msg.payload[..32] == [0; 32] => {
                send_status(&mut conn, SSC_GOVOBJ, 0).await;
            }
            commands::MNGOVERNANCESYNC if msg.payload[32..] != object_fetch_filter()[..] => {
                send_status(&mut conn, SSC_GOVOBJ_VOTE, votes.len() as i32).await;
                tokio::time::sleep(delay).await;
                let items: Vec<InvItem> = votes
                    .iter()
                    .map(|h| InvItem {
                        kind: INV_GOVERNANCE_VOTE,
                        hash: *h,
                    })
                    .collect();
                conn.send("inv", &encode_inv(&items)).await.unwrap();
            }
            "getdata" => {
                conn.send("notfound", &msg.payload).await.unwrap();
            }
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_moved_queued_request_leaves_its_peer_askable() {
    backlog_run(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backlogged_honest_votes_all_arrive() {
    backlog_run(false).await;
}

async fn backlog_run(liar_announces: bool) {
    // Validator r6 MJ1. Two honest nodes hold P and N votes on it; each
    // answers one vote `getdata` per 300 ms (a backlog). The liar announces
    // the same votes last and answers `notfound`. Queued 600 ms at A, a
    // vote moves to B; queued 600 ms at B, it moves to the liar. A and B
    // were never sent it, so they stay askable after its `notfound`.
    const N: u8 = 10;
    let p = proposal("p-backlogged");
    let votes: Vec<GovernanceVote> = (1..=N)
        .map(|k| vote(p.hash(), k, VoteOutcome::Yes))
        .collect();
    let hashes: Vec<[u8; 32]> = if liar_announces {
        votes.iter().map(|v| v.hash()).collect()
    } else {
        Vec::new()
    };
    let honest = Node {
        objects: vec![p.clone()],
        votes: votes.clone(),
        vote_delay: Some(Duration::from_millis(300)),
        ..Node::default()
    };
    let (shared, end, stop) = start_custom(
        vec![custom(move |mock| {
            late_notfound_peer(mock, hashes, Duration::from_millis(200))
        })],
        vec![honest.clone(), honest],
        |c| {
            c.max_in_flight = 1;
            c.request_timeout = Duration::from_millis(600);
        },
        |shared| {
            let list: Vec<ListEntry> = (1..=N)
                .map(|k| ListEntry {
                    voting_key_id: key_id(k),
                    weight: Some(1),
                    valid: true,
                })
                .collect();
            shared.store().set_masternode_list(&list);
        },
    )
    .await;
    let stored = shared.store().votes_on(&p.hash()).count();
    let _ = stop.send(true);
    assert!(shared.store().object(&p.hash()).is_some(), "{end:?}");
    assert!(
        end.phase != Phase::Synced || stored == N as usize,
        "Synced with {stored} of {N} votes both honest peers hold: {end:?}"
    );
}

/// Full sync: nothing. Vote sync: waits `wait`, then announces `v` as a
/// vote (a phase item, its own sync still open), answers its `getdata`
/// with `notfound`, and only then sends `ssc(11, 1)`.
async fn reannouncing_liar(mock: MockPeer, v: [u8; 32], wait: Duration, announce: bool) {
    let mut conn = mock.accept().await.unwrap();
    while let Ok(msg) = conn.recv().await {
        match msg.command.as_str() {
            commands::MNGOVERNANCESYNC if msg.payload[..32] == [0; 32] => {
                send_status(&mut conn, SSC_GOVOBJ, 0).await;
            }
            commands::MNGOVERNANCESYNC if msg.payload[32..] != object_fetch_filter()[..] => {
                tokio::time::sleep(wait).await;
                if !announce {
                    send_status(&mut conn, SSC_GOVOBJ_VOTE, 0).await;
                    continue;
                }
                let item = InvItem {
                    kind: INV_GOVERNANCE_VOTE,
                    hash: v,
                };
                conn.send("inv", &encode_inv(&[item])).await.unwrap();
            }
            "getdata" => {
                conn.send("notfound", &msg.payload).await.unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
                send_status(&mut conn, SSC_GOVOBJ_VOTE, 1).await;
            }
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_vote_stays_lost_after_a_liars_notfound() {
    lost_vote_run(true, Duration::from_millis(3500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_vote_fails_the_run() {
    lost_vote_run(false, Duration::from_millis(3500)).await;
}

async fn lost_vote_run(announce: bool, wait: Duration) {
    // Validator r6 MJ2. The honest node is slow: its one vote is given up
    // and counted lost, so the phase must fail (D13) unless the vote still
    // arrives. The liar then announces the same vote and answers
    // `notfound`: that replaced the late record uncounted, the run
    // reported Synced, and the honest late copy was refused.
    let p = proposal("p-slow-honest");
    let v = vote(p.hash(), 1, VoteOutcome::Yes);
    let vh = v.hash();
    let honest = Node {
        objects: vec![p.clone()],
        votes: vec![v],
        vote_delay: Some(Duration::from_secs(6)),
        ..Node::default()
    };
    let (shared, end, stop) = start_custom(
        vec![custom(move |mock| {
            reannouncing_liar(mock, vh, wait, announce)
        })],
        vec![honest],
        |c| c.request_timeout = Duration::from_millis(400),
        |shared| {
            shared.store().set_confirmations(1);
            shared.store().set_masternode_list(&[ListEntry {
                voting_key_id: key_id(1),
                weight: Some(1),
                valid: true,
            }]);
        },
    )
    .await;
    let at_end = shared.store().votes_on(&p.hash()).count();
    let _ = stop.send(true);
    assert!(
        end.phase != Phase::Synced || at_end == 1,
        "Synced without the honest vote: {end:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_late_records_stay_bounded() {
    // Validator r6 m1: a peer whose vote sync count is never met (it
    // answered a huge `ssc(11)`) makes every vote it announces a phase
    // item; given up by timeout, each was a lost late record that
    // `remember_late` never dropped, rescanning them all on each insert.
    // (Once synced no item is a phase item; this holds the bound while a
    // phase runs.)
    let mut run = one_peer_run(|c, p| {
        c.max_in_flight = 1_000_000;
        c.request_timeout = Duration::ZERO;
        p.vote_asked.push([0x77; 32]);
        p.vote_expected.push(i64::from(i32::MAX));
    })
    .await;
    for round in 0u8..3 {
        run.handle(0, PeerEvent::Inv(junk_votes(round, 50_000)));
        run.expire(); // asked again
        run.expire(); // given up, lost
    }
    assert!(run.lost >= 100_000, "lost {}", run.lost);
    assert!(
        run.requests.late.len() <= MAX_LATE,
        "late record {} past {MAX_LATE}",
        run.requests.late.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_item_is_lost_once_synced() {
    // Validator r6 m1: after Synced, a peer whose vote count is never met
    // made each vote it relays a phase item, lost when given up.
    let mut run = one_peer_run(|c, p| {
        c.request_timeout = Duration::ZERO;
        p.vote_asked.push([0x77; 32]);
        p.vote_expected.push(i64::from(i32::MAX));
    })
    .await;
    run.announce_open = true;
    run.handle(0, PeerEvent::Inv(junk_votes(0, 100)));
    run.expire();
    run.expire();
    assert_eq!((run.lost, run.requests.pending.len()), (0, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_notfound_for_a_request_not_sent_is_ignored() {
    // Validator r6 nit (D50): a `notfound` for a request still queued at
    // the peer ended it.
    let mut run = one_peer_run(|c, _| c.max_in_flight = 1).await;
    let items = junk_votes(0, 2);
    run.handle(0, PeerEvent::Inv(items.clone()));
    let sent = |run: &Run, i: &InvItem| run.requests.pending[&(i.hash, i.kind)].sent.is_some();
    let queued = items.iter().find(|i| !sent(&run, i)).copied().unwrap();
    run.handle(0, PeerEvent::NotFound(vec![queued]));
    assert!(
        run.requests
            .pending
            .contains_key(&(queued.hash, queued.kind))
    );
    assert_eq!(run.peers[0].misbehaviour, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_timed_out_vote_stays_lost_after_a_later_notfound() {
    // Validator r6 MJ1: the honest node's request times out and moves to
    // the liar, which answers `notfound`; the honest copy may still come,
    // so the vote is lost, not settled.
    lost_vote_run(true, Duration::from_millis(200)).await;
}
/// A run with two connected peers (ids 0 and 1) that read and drop
/// everything, a stored current proposal and key 1 listed.
async fn two_peer_run(p: &GovernanceObject) -> Run {
    let mut run = one_peer_run(|c, _| c.max_in_flight = 5000).await;
    run.peers.push(sink_peer(1, &run.cfg).await);
    list_key_1(&run);
    let mut s = run.shared.store();
    s.set_confirmations(1);
    s.add_object(p.clone(), None, unix_now()).unwrap();
    drop(s);
    run
}

/// The voting keys `keys` listed, one masternode each, of weight 1.
fn list_keys(run: &Run, keys: impl IntoIterator<Item = u8>) {
    let entries: Vec<ListEntry> = keys
        .into_iter()
        .map(|k| ListEntry {
            voting_key_id: key_id(k),
            weight: Some(1),
            valid: true,
        })
        .collect();
    run.shared.store().set_masternode_list(&entries);
}

/// Key 1 as the one listed voting key.
fn list_key_1(run: &Run) {
    list_keys(run, [1]);
}

/// A run with one peer whose vote sync is open and whose requests time out
/// at once, and key 1 listed: its votes are phase items.
async fn phase_vote_run() -> Run {
    let run = one_peer_run(|c, p| {
        c.request_timeout = Duration::ZERO;
        open_vote_sync(p, [0x77; 32]);
    })
    .await;
    list_key_1(&run);
    run
}

/// Opens `p`'s vote sync, whose count is far off, so its votes are phase
/// items.
fn open_vote_sync(p: &mut PeerState, parent: [u8; 32]) {
    p.vote_asked.push(parent);
    p.vote_expected.push(1_000);
}

fn vote_inv(v: &GovernanceVote) -> InvItem {
    InvItem {
        kind: INV_GOVERNANCE_VOTE,
        hash: v.hash(),
    }
}

/// A vote of key 1 on `parent` dated `age` + 1 seconds before a `vote` made
/// at the first call of the run: older than any `vote` made since, a
/// distinct hash per `age`.
fn aged_vote(parent: [u8; 32], age: u32) -> GovernanceVote {
    // From one start time, so votes made a second apart do not collide.
    static START: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let mut x = vote(parent, 1, VoteOutcome::Yes);
    x.time = START.get_or_init(unix_now) - 100 - i64::from(age) - 1;
    resigned(&x, 1)
}

/// `v` re-timed `by` seconds earlier, signed by key 1: obsolete once `v` is
/// held, whatever the clock did since `v` was made (`aged_vote` is not, for
/// a `v` made before its first call).
fn older_than(v: &GovernanceVote, by: i64) -> GovernanceVote {
    let mut x = v.clone();
    x.time -= by;
    resigned(&x, 1)
}

/// Peer 0 floods 150,000 junk vote invs, each answered `notfound`: more
/// late records than the bound keeps.
fn flood_late_record(run: &mut Run) {
    for round in 0u8..3 {
        let junk = junk_votes(round, 50_000);
        run.handle(0, PeerEvent::Inv(junk.clone()));
        run.handle(0, PeerEvent::NotFound(junk));
    }
}

/// Peer 0 announces `item`, times out twice and is given up on (a late
/// record: it may still send one copy, D60); then peer 1 announces it.
fn late_record_then_second_announcer(run: &mut Run, item: InvItem) {
    let timeout = std::mem::replace(&mut run.cfg.request_timeout, Duration::ZERO);
    run.handle(0, PeerEvent::Inv(vec![item]));
    run.expire(); // asked again
    run.expire(); // given up
    run.cfg.request_timeout = timeout;
    run.handle(1, PeerEvent::Inv(vec![item]));
}

/// Times out every request out, once, whatever the configured timeout.
fn expire_now(run: &mut Run) {
    let timeout = std::mem::replace(&mut run.cfg.request_timeout, Duration::ZERO);
    run.expire();
    run.cfg.request_timeout = timeout;
}

/// Peers 0 and 1 announce `item`; peer 0's request times out and moves to
/// peer 1.
fn moved_to_peer_1(run: &mut Run, item: InvItem) {
    run.handle(0, PeerEvent::Inv(vec![item]));
    run.handle(1, PeerEvent::Inv(vec![item]));
    expire_now(run);
}

/// Peer 0 is asked for the phase vote `x`, times out twice and is given
/// up on: lost.
fn lose_vote(run: &mut Run, x: &GovernanceVote) {
    run.handle(0, PeerEvent::Inv(vec![vote_inv(x)]));
    run.expire(); // asked again
    run.expire(); // given up, lost
    assert_eq!(run.lost, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_late_copy_leaves_another_peers_request_alone() {
    // Validator r7 MJ1: A announces X and times out (a late record of A);
    // honest H announces X (a request sent to H); A sends X
    // re-signed by an unlisted key (same hash, refused). The retry
    // reassigned X to H, overwriting its sent request: a leaked slot, a
    // second `getdata` and H charged for its second copy.
    let p = proposal("p-r7");
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    let item = vote_inv(&x);
    let mut run = two_peer_run(&p).await;
    // H's vote sync still open: its announcement is a phase item.
    open_vote_sync(&mut run.peers[1], p.hash());
    late_record_then_second_announcer(&mut run, item);
    let key = (item.hash, item.kind);
    assert!(run.requests.pending[&key].phase);
    assert_eq!(run.peers[1].in_flight, 1);
    run.handle(0, PeerEvent::Vote(resigned(&x, 2).encode()));
    let after = (
        run.peers[1].in_flight,
        run.requests.pending.get(&key).map(|r| r.phase),
    );
    assert_eq!(after, (1, Some(true)), "H's request was overwritten");
    run.handle(1, PeerEvent::Vote(x.encode()));
    let h = &run.peers[1];
    assert_eq!((h.in_flight, h.misbehaviour), (0, 0));
    assert!(run.shared.store().has_vote(&x.hash()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forged_late_copies_cannot_get_an_honest_peer_closed() {
    // Validator r7 MJ1, repeated past the misbehaviour bound.
    let p = proposal("p-r7-close");
    let mut run = two_peer_run(&p).await;
    let max = run.cfg.max_misbehaviour;
    for t in 0..=max {
        let x = aged_vote(p.hash(), t);
        late_record_then_second_announcer(&mut run, vote_inv(&x));
        run.handle(0, PeerEvent::Vote(resigned(&x, 2).encode()));
        run.handle(1, PeerEvent::Vote(x.encode()));
    }
    let h = &run.peers[1];
    assert_eq!((h.misbehaviour, h.in_flight), (0, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_uncheckable_copy_does_not_end_a_loss() {
    // Validator r7 m1: a phase vote on an object the wallet lacks is given
    // up (lost); its asked peer then sends the vote with a 96-byte junk
    // signature, which the wallet cannot check and drops: that ended the
    // loss with no copy held.
    let mut run = phase_vote_run().await;
    let x = vote(proposal("p-r7-unknown").hash(), 1, VoteOutcome::Yes);
    lose_vote(&mut run, &x);
    let mut junk = x.clone();
    junk.signature = vec![0x55; 96];
    run.handle(0, PeerEvent::Vote(junk.encode()));
    assert_eq!(run.lost, 1, "a junk copy the wallet dropped ended the loss");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copy_past_the_senders_orphan_bound_does_not_end_a_loss() {
    // Validator r7 m1: a sender at its orphan bound got `Orphan` back for
    // a vote not kept, which ended the request and its loss.
    let mut run = phase_vote_run().await;
    let addr = run.peers[0].peer.addr;
    let now = unix_now();
    for i in 0..crate::store::MAX_ORPHANS_PER_PEER {
        let mut parent = [0x44; 32];
        parent[..4].copy_from_slice(&i.to_le_bytes());
        let v = vote(parent, 1, VoteOutcome::Yes);
        let outcome = run.shared.store().precheck(&v, now).unwrap();
        let checked = recover(v, outcome, Network::Regtest).unwrap();
        let r = run.shared.store().add_checked(checked, Some(addr), now);
        assert_eq!(r, Err(VoteRejection::Orphan));
    }
    let x = vote([0x45; 32], 1, VoteOutcome::Yes);
    lose_vote(&mut run, &x);
    run.handle(0, PeerEvent::Vote(x.encode()));
    assert!(!run.shared.store().has_vote(&x.hash()));
    assert_eq!(
        run.lost, 1,
        "a copy the orphan bound refused ended the loss"
    );
    // Refused before its signature is recovered, at no cost (D62).
    assert_eq!(run.recovered, 0);
    assert_eq!(run.peers[0].suspect, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copy_taken_late_ends_the_request_still_out() {
    // Validator r7 nit1: a late copy taken while another peer's phase
    // request for the item was out left that request, whose timeout then
    // counted a loss for a vote the store held.
    let p = proposal("p-r7-late");
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    let mut run = two_peer_run(&p).await;
    run.cfg.request_timeout = Duration::ZERO;
    open_vote_sync(&mut run.peers[1], p.hash());
    late_record_then_second_announcer(&mut run, vote_inv(&x));
    run.handle(0, PeerEvent::Vote(x.encode()));
    assert!(run.shared.store().has_vote(&x.hash()));
    assert!(run.requests.pending.is_empty(), "H's request still out");
    run.expire();
    run.expire();
    assert_eq!(run.lost, 0);
    // H's copy still comes: late, not unsolicited.
    run.handle(1, PeerEvent::Vote(x.encode()));
    assert_eq!(run.peers[1].misbehaviour, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_earlier_peers_copy_after_the_next_peers_is_no_offence() {
    // Validator r8 MJ1: peer 0's request times out and moves to peer 1,
    // whose copy is taken first; peer 0's copy then comes (Core answers
    // its queue in order) and was unsolicited, closing peer 0 past the
    // misbehaviour bound. Core drops it with no penalty (D59).
    let p = proposal("p-r8-next");
    let mut run = two_peer_run(&p).await;
    let max = run.cfg.max_misbehaviour;
    for t in 0..=max {
        let x = aged_vote(p.hash(), t);
        let item = vote_inv(&x);
        moved_to_peer_1(&mut run, item);
        assert_eq!(run.requests.pending[&(item.hash, item.kind)].peer, 1);
        run.handle(1, PeerEvent::Vote(x.encode()));
        run.handle(0, PeerEvent::Vote(x.encode()));
    }
    assert_eq!(run.peers[0].misbehaviour, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_copy_from_a_peer_asked_before_leaves_the_live_request_alone() {
    // Validator r8 m2: peer 0 timed out, so it is in the live request's
    // `asked`; its re-signed copy released peer 1's request before being
    // refused, and the retry sent it to peer 1 again.
    let p = proposal("p-r8-asked");
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    let item = vote_inv(&x);
    let key = (item.hash, item.kind);
    let mut run = two_peer_run(&p).await;
    moved_to_peer_1(&mut run, item);
    let before = run.requests.pending[&key].sent;
    assert_eq!(
        (run.requests.pending[&key].peer, run.peers[1].in_flight),
        (1, 1)
    );
    std::thread::sleep(Duration::from_millis(2));
    run.handle(0, PeerEvent::Vote(resigned(&x, 2).encode()));
    assert_eq!(run.requests.pending.get(&key).and_then(|r| r.sent), before);
    assert_eq!(run.peers[1].in_flight, 1);
    // Its one `getdata` answered, peer 0's genuine copy is dropped (D60);
    // peer 1's request is still out.
    run.handle(0, PeerEvent::Vote(x.encode()));
    assert!(!run.shared.store().has_vote(&x.hash()));
    assert_eq!(run.requests.pending[&key].peer, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_late_record_does_not_make_an_honest_copy_an_offence() {
    // Validator r8 MJ2: peer 0's junk inv + `notfound` filled the late
    // record, whose bound dropped honest peer 1's record for its second
    // copy (asked twice, D12), which was then unsolicited.
    let p = proposal("p-r8-flood");
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    let item = vote_inv(&x);
    let mut run = two_peer_run(&p).await;
    run.cfg.max_in_flight = 1_000_000;
    run.cfg.request_timeout = Duration::ZERO;
    run.handle(1, PeerEvent::Inv(vec![item]));
    run.expire();
    assert!(run.requests.pending[&(item.hash, item.kind)].asked_again);
    run.handle(1, PeerEvent::Vote(x.encode()));
    assert!(run.shared.store().has_vote(&x.hash()));
    flood_late_record(&mut run);
    run.handle(1, PeerEvent::Vote(x.encode()));
    assert_eq!(
        (run.peers[0].misbehaviour, run.peers[1].misbehaviour),
        (0, 0)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forged_copies_and_a_late_record_flood_cannot_close_an_honest_peer() {
    // Validator r8 MJ2 chained with m2: each forged copy had honest peer 1
    // asked twice; the flood then dropped the record for its second copy,
    // and 1,001 of them closed it.
    let p = proposal("p-r8-chain");
    let mut run = two_peer_run(&p).await;
    run.cfg.max_in_flight = 1_000_000;
    let max = run.cfg.max_misbehaviour;
    let mut xs = Vec::new();
    for t in 0..=max {
        let x = aged_vote(p.hash(), t);
        let item = vote_inv(&x);
        run.handle(0, PeerEvent::Inv(vec![item]));
        run.handle(1, PeerEvent::Inv(vec![item]));
        run.handle(0, PeerEvent::NotFound(vec![item]));
        run.handle(0, PeerEvent::Vote(resigned(&x, 2).encode()));
        xs.push(x);
    }
    flood_late_record(&mut run);
    for x in &xs {
        run.handle(1, PeerEvent::Vote(x.encode()));
        run.handle(1, PeerEvent::Vote(x.encode()));
    }
    assert_eq!(run.peers[1].misbehaviour, 0);
    assert!(run.peers[1].peer.is_open());
}

/// Keys 1 and 2 listed, and key 2 at its limit on `parent` (D11) with four
/// votes of outpoints `base..base + 4`.
fn fill_key_2(run: &Run, parent: [u8; 32], base: u8) {
    list_keys(run, [1, 2]);
    let now = unix_now();
    for i in 0..4u8 {
        let f = resigned(&vote(parent, base + i, VoteOutcome::No), 2);
        run.shared.store().add_vote(f, None, now).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copy_past_its_keys_limit_does_not_hide_the_honest_vote() {
    // Validator r8 MJ3: a listed key's holder re-signs the honest vote x
    // with its own key (same hash). Past that key's limit on the proposal
    // the copy was dropped as `KeyLimit` but taken, ending the request:
    // the honest announcer was never asked.
    let p = proposal("p-r8-limit");
    let mut run = two_peer_run(&p).await;
    fill_key_2(&run, p.hash(), 0x60);
    let now = unix_now();
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    let item = vote_inv(&x);
    run.handle(0, PeerEvent::Inv(vec![item]));
    run.handle(1, PeerEvent::Inv(vec![item]));
    let forged = resigned(&x, 2);
    run.handle(0, PeerEvent::Vote(forged.encode()));
    let key = (item.hash, item.kind);
    assert_eq!(run.requests.pending.get(&key).map(|r| r.peer), Some(1));
    // Not charged, and remembered: not recovered again (D62).
    assert_eq!(run.peers[0].suspect, 0);
    assert_eq!(
        run.shared.store().precheck(&forged, now).map(|_| ()),
        Err(VoteRejection::KeyLimit)
    );
    run.handle(1, PeerEvent::Vote(x.encode()));
    assert!(run.shared.store().has_vote(&x.hash()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remembered_key_limit_copy_is_still_refused() {
    // D62: a forged copy past its key's limit, sent again by the next
    // announcer, gets the remembered verdict: refused, so the honest
    // announcer after it is still asked.
    let p = proposal("p-d62-limit");
    let mut run = two_peer_run(&p).await;
    run.peers.push(sink_peer(2, &run.cfg).await);
    fill_key_2(&run, p.hash(), 0x70);
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    let item = vote_inv(&x);
    let key = (item.hash, item.kind);
    for peer in 0..3 {
        run.handle(peer, PeerEvent::Inv(vec![item]));
    }
    let forged = resigned(&x, 2);
    run.handle(0, PeerEvent::Vote(forged.encode()));
    let recovered = run.recovered;
    run.handle(1, PeerEvent::Vote(forged.encode()));
    assert_eq!(run.recovered, recovered);
    assert_eq!(run.requests.pending.get(&key).map(|r| r.peer), Some(2));
    run.handle(2, PeerEvent::Vote(x.encode()));
    assert!(run.shared.store().has_vote(&x.hash()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vote_sync_history_drops_entries_past_the_window() {
    // Validator r8 m3: the session's record of vote syncs gained an entry
    // per (peer, object) asked and was never pruned.
    let mut run = one_peer_run(|c, _| c.repeat_window = Duration::ZERO).await;
    run.shared.store().set_confirmations(1);
    for round in 0..3 {
        let hashes: Vec<[u8; 32]> = (0..500)
            .map(|i| {
                let o = proposal(&format!("p-r8-history-{round}-{i}"));
                run.shared
                    .store()
                    .add_object(o.clone(), None, unix_now())
                    .unwrap();
                o.hash()
            })
            .collect();
        run.ask_votes(hashes);
    }
    let n = run.history.lock().unwrap().vote_synced.len();
    assert!(
        n <= MIN_HISTORY_PRUNE,
        "{n} entries kept, all past a zero window"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_earlier_peers_copy_ends_the_request_still_out() {
    // D59: peer 0's request timed out and moved to peer 1; peer 0's copy,
    // taken, ends peer 1's request, freeing its slot.
    let p = proposal("p-d60-earlier");
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    let item = vote_inv(&x);
    let mut run = two_peer_run(&p).await;
    moved_to_peer_1(&mut run, item);
    assert_eq!(run.requests.pending[&(item.hash, item.kind)].peer, 1);
    run.handle(0, PeerEvent::Vote(x.encode()));
    assert!(run.shared.store().has_vote(&x.hash()));
    assert!(run.requests.pending.is_empty());
    assert_eq!(run.peers[1].in_flight, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_notfound_answers_the_peers_getdata() {
    // Validator r9 MJ1: a peer that answered `notfound` was kept in the
    // item's late record, so it could replay a copy (an old, superseded
    // vote, refused as `Obsolete` without a penalty) without end, each a
    // signature recovery. Core spends a request's authorization on its
    // answer (D60).
    let p = proposal("p-d60-notfound");
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    let item = vote_inv(&x);
    let mut run = two_peer_run(&p).await;
    run.handle(0, PeerEvent::Inv(vec![item]));
    run.handle(0, PeerEvent::NotFound(vec![item]));
    // Owed no copy and not lost: no late record is kept for it.
    assert!(run.requests.late.is_empty());
    assert!(run.take(0, &(item.hash, item.kind)).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_copy_answers_one_getdata() {
    // Validator r9 MJ1: a timed-out peer's late copy left its late record
    // in place, so the same copy could be replayed without end (D60).
    let p = proposal("p-d60-late");
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    let key = (x.hash(), INV_GOVERNANCE_VOTE);
    let mut run = two_peer_run(&p).await;
    run.cfg.request_timeout = Duration::ZERO;
    run.handle(0, PeerEvent::Inv(vec![vote_inv(&x)]));
    run.expire(); // asked again
    run.expire(); // given up: it may still send a copy
    assert!(run.take(0, &key).is_some());
    assert!(run.take(0, &key).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_earlier_peers_copies_answer_one_getdata() {
    // Validator r9 MJ1, through a request still out: peer 0's request
    // moved to peer 1, and each copy peer 0 sent cost a signature
    // recovery while peer 1's request stayed (D60).
    let p = proposal("p-d60-replay");
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    let item = vote_inv(&x);
    let mut run = two_peer_run(&p).await;
    moved_to_peer_1(&mut run, item);
    for _ in 0..5 {
        run.handle(0, PeerEvent::Vote(resigned(&x, 2).encode()));
    }
    assert_eq!(run.peers[0].suspect, 1, "each replayed copy was recovered");
    assert_eq!(run.requests.pending[&(item.hash, item.kind)].peer, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_copy_from_the_timed_out_peer_ends_its_loss() {
    // Validator r9 m1: with no penalty for unsolicited items (D59), a late
    // copy wrongly dropped no longer shows as misbehaviour; the timed-out
    // peer's late copy must still be taken and end the loss (D57).
    let mut run = phase_vote_run().await;
    let p = proposal("p-d60-loss");
    {
        let mut s = run.shared.store();
        s.set_confirmations(1);
        s.add_object(p.clone(), None, unix_now()).unwrap();
    }
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    lose_vote(&mut run, &x);
    run.handle(0, PeerEvent::Vote(x.encode()));
    assert!(run.shared.store().has_vote(&x.hash()));
    assert_eq!(run.lost, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reannounced_obsolete_copy_is_recovered_once() {
    // Validator r10 MJ2: an obsolete copy ends its request (taken), so its
    // peer could announce it again and be asked again: an `inv` and a copy,
    // each a signature recovery, without end. Core rejects an obsolete vote
    // before checking its signature; the wallet remembers the copy it
    // recovered instead, at no charge: honest peers relay such copies
    // (validator r11 MJ2, D62).
    let p = proposal("p-d62-reannounce");
    let mut run = two_peer_run(&p).await;
    let now = unix_now();
    let current = vote(p.hash(), 1, VoteOutcome::Yes);
    let old = older_than(&current, 1);
    run.shared.store().add_vote(current, None, now).unwrap();
    let item = vote_inv(&old);
    for _ in 0..5 {
        run.handle(0, PeerEvent::Inv(vec![item]));
        run.handle(0, PeerEvent::Vote(old.encode()));
        assert!(run.requests.pending.is_empty());
    }
    assert_eq!(run.recovered, 1);
    assert_eq!(run.peers[0].suspect, 0);
    assert_eq!(run.peers[0].misbehaviour, 0);
    assert_eq!(
        run.shared.store().precheck(&old, now).map(|_| ()),
        Err(VoteRejection::Obsolete)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_late_copies_do_not_multiply_grants() {
    // Validator r10 MJ1: a refused copy taken from a late record carried
    // the record's other grants into the retry, whose give-up merged them
    // back into the record: each refused copy doubled them, without bound,
    // and they never ran out (D61).
    let mut run = one_peer_run(|_, _| {}).await;
    let x = vote([0x5a; 32], 1, VoteOutcome::Yes);
    let item = vote_inv(&x);
    let key = (item.hash, item.kind);
    // A 96-byte signature on a vote whose object is unknown: refused,
    // free.
    let mut junk = x.clone();
    junk.signature = vec![0x55; 96];
    run.cfg.request_timeout = Duration::ZERO;
    for _ in 0..3 {
        run.handle(0, PeerEvent::Inv(vec![item]));
        run.expire(); // asked again
        run.expire(); // given up: one copy owed
    }
    let owed = |run: &Run| run.requests.late.get(&key).map_or(0, |l| l.owed.len());
    assert_eq!(owed(&run), 3);
    let mut left = Vec::new();
    for _ in 0..6 {
        run.handle(0, PeerEvent::Vote(junk.encode()));
        left.push(owed(&run));
    }
    assert_eq!(left, [2, 1, 0, 0, 0, 0]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copy_for_a_request_not_sent_is_not_taken() {
    // Validator r10 m1: a copy for a request still queued behind full
    // slots, with no `getdata` sent, was taken (D61).
    let p = proposal("p-d61-queued");
    let mut run = two_peer_run(&p).await;
    run.cfg.max_in_flight = 1;
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    run.handle(0, PeerEvent::Inv(junk_votes(7, 1)));
    run.handle(0, PeerEvent::Inv(vec![vote_inv(&x)]));
    let key = (x.hash(), INV_GOVERNANCE_VOTE);
    assert!(run.requests.pending[&key].sent.is_none());
    run.handle(0, PeerEvent::Vote(x.encode()));
    assert!(!run.shared.store().has_vote(&x.hash()));
    assert!(run.requests.pending[&key].sent.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_notfound_spends_a_copy_still_owed() {
    // Validator r10 nit: a peer asked twice whose first copy arrived kept
    // its grant after a `notfound` answering the second `getdata` (D61).
    let p = proposal("p-d61-notfound-owed");
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    let item = vote_inv(&x);
    let key = (item.hash, item.kind);
    let mut run = two_peer_run(&p).await;
    run.handle(0, PeerEvent::Inv(vec![item]));
    expire_now(&mut run); // asked again: two getdatas
    assert!(run.take(0, &key).is_some());
    run.handle(0, PeerEvent::NotFound(vec![item]));
    assert!(run.take(0, &key).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grants_and_losses_stay_consistent_under_random_events() {
    // Validator r10: random announcements, `notfound`s, copies (genuine
    // and re-signed) and timeouts from two peers over four phase votes.
    // `lost` always counts the lost late records, and the copies owed grow
    // only with what can grant one: a timeout (at most one per request out)
    // or a copy taken (one for a request it ends or a second `getdata`).
    let p = proposal("p-d61-random");
    let xs: Vec<GovernanceVote> = (0..4).map(|a| aged_vote(p.hash(), 10 - a)).collect();
    for mut seed in [0x9e37_79b9_7f4a_7c15u64, 1, 2, 3, 4, 5, 6, 7] {
        let mut run = two_peer_run(&p).await;
        open_vote_sync(&mut run.peers[0], p.hash());
        open_vote_sync(&mut run.peers[1], p.hash());
        let mut next = |m: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % m
        };
        let mut granting = 0;
        for step in 0..5_000 {
            let peer = next(2) as usize;
            let x = &xs[next(4) as usize];
            match next(7) {
                0 | 1 => run.handle(peer, PeerEvent::Inv(vec![vote_inv(x)])),
                2 => run.handle(peer, PeerEvent::NotFound(vec![vote_inv(x)])),
                3 => {
                    granting += 1;
                    run.handle(peer, PeerEvent::Vote(x.encode()));
                }
                4 => run.handle(peer, PeerEvent::Vote(resigned(x, 2).encode())),
                _ => {
                    granting += run.requests.pending.len();
                    expire_now(&mut run);
                }
            }
            let recorded = run.requests.late.values().filter(|l| l.lost).count();
            assert_eq!(run.lost, recorded, "step {step}");
            let owed: usize = (run.requests.late.values().map(|l| l.owed.len()))
                .chain(run.requests.pending.values().map(|r| r.owed.len()))
                .sum();
            assert!(
                owed <= granting,
                "step {step}: {owed} copies owed, {granting} granting events"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_that_becomes_a_phase_item_holds_the_phase() {
    // D61: the count of pending phase requests follows a request that
    // turns into one when a peer whose vote sync is open announces it too.
    let p = proposal("p-d61-phase");
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    let item = vote_inv(&x);
    let mut run = two_peer_run(&p).await;
    run.handle(0, PeerEvent::Inv(vec![item]));
    assert!(run.phase_requests_done());
    open_vote_sync(&mut run.peers[1], p.hash());
    run.handle(1, PeerEvent::Inv(vec![item]));
    assert!(run.requests.pending[&(item.hash, item.kind)].phase);
    assert!(!run.phase_requests_done());
    run.handle(0, PeerEvent::Vote(x.encode()));
    assert!(run.phase_requests_done());
}

/// A sink peer that counts the `getdata` items it receives, by (hash, kind).
async fn counting_peer(
    id: usize,
    cfg: &SyncConfig,
) -> (PeerState, Arc<Mutex<HashMap<Item, usize>>>) {
    let mock = MockPeer::bind(Network::Regtest).await.unwrap();
    let addr = mock.addr();
    let counts: Arc<Mutex<HashMap<Item, usize>>> = Arc::default();
    let c = Arc::clone(&counts);
    tokio::spawn(async move {
        let mut conn = mock.accept().await.unwrap();
        while let Ok(m) = conn.recv().await {
            if m.command == "getdata" {
                for i in decode_inv(&m.payload).unwrap() {
                    *c.lock().unwrap().entry((i.hash, i.kind)).or_default() += 1;
                }
            }
        }
    });
    let (tx, _rx) = mpsc::channel(EVENTS_QUEUE);
    let state = PeerState::new(Peer::connect(id, addr, &cfg.peer, tx).await.unwrap());
    (state, counts)
}

/// Copies `peer` may still send for `item` in the run's records: its live
/// sent request (two once asked again), each grant it holds, and a refetch
/// asked of it (D77).
fn capacity(run: &Run, peer: usize, item: &Item) -> usize {
    let refetch = item.1 == INV_GOVERNANCE_VOTE
        && run
            .refetches
            .out
            .get(&item.0)
            .is_some_and(|r| r.peer == peer);
    let live = run.requests.pending.get(item).map_or(0, |r| {
        let mine = r.peer == peer && r.sent.is_some();
        usize::from(mine) * (1 + usize::from(r.asked_again))
            + r.owed.iter().filter(|p| **p == peer).count()
    });
    live + usize::from(refetch)
        + run
            .requests
            .late
            .get(item)
            .map_or(0, |l| l.owed.iter().filter(|p| **p == peer).count())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn copies_taken_never_exceed_getdatas_sent() {
    // Validator r11 (property): per peer and item, copies taken +
    // notfounds that spent a grant never exceed the getdatas sent (counted
    // from the run's state and cross-checked against the wire), and what
    // the run still accepts is within what is left.
    let p = proposal("p-r11-prop");
    let orphan_parent = [0x6b; 32];
    let mut xs: Vec<GovernanceVote> = (0..4).map(|a| aged_vote(p.hash(), 6 - a)).collect();
    xs.push(vote(orphan_parent, 1, VoteOutcome::Yes));
    xs.push(vote(p.hash(), 2, VoteOutcome::No)); // unlisted key: suspect
    for mut seed in [0x9e37_79b9_7f4a_7c15u64, 11, 12, 13, 14, 15] {
        let mut run = two_peer_run(&p).await;
        run.cfg.max_in_flight = 3;
        run.cfg.max_suspect = u32::MAX - 1;
        run.cfg.max_misbehaviour = u32::MAX - 1;
        run.peers.clear();
        let mut wire = Vec::new();
        for id in 0..3 {
            let (s, c) = counting_peer(id, &run.cfg).await;
            run.peers.push(s);
            wire.push(c);
        }
        open_vote_sync(&mut run.peers[0], p.hash());
        open_vote_sync(&mut run.peers[1], p.hash());
        let mut next = |m: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % m
        };
        let mut items: Vec<Item> = Vec::new();
        for x in &xs {
            items.push((x.hash(), INV_GOVERNANCE_VOTE));
            items.push((x.hash(), INV_GOVERNANCE_OBJECT));
        }
        // (peer, item) → getdatas sent, copies taken, notfounds that spent.
        let mut sent: HashMap<(usize, Item), usize> = HashMap::new();
        let mut taken: HashMap<(usize, Item), usize> = HashMap::new();
        let mut spent: HashMap<(usize, Item), usize> = HashMap::new();
        let long_ago = Instant::now() - Duration::from_secs(10_000);
        for step in 0..4_000 {
            let peer = next(3) as usize;
            let xi = next(xs.len() as u64) as usize;
            let x = xs[xi].clone();
            let kind = if next(5) == 0 {
                INV_GOVERNANCE_OBJECT
            } else {
                INV_GOVERNANCE_VOTE
            };
            let inv = InvItem {
                kind,
                hash: x.hash(),
            };
            let before: HashMap<Item, (usize, Option<Instant>)> = run
                .requests
                .pending
                .iter()
                .map(|(i, r)| (*i, (r.peer, r.sent)))
                .collect();
            let refetch_before: HashMap<[u8; 32], (usize, Instant)> = run
                .refetches
                .out
                .iter()
                .map(|(h, r)| (*h, (r.peer, r.sent)))
                .collect();
            let cap_before: Vec<usize> = items.iter().map(|i| capacity(&run, peer, i)).collect();
            let mut copy = false;
            let mut notfound = false;
            match next(10) {
                0..=2 => run.handle(peer, PeerEvent::Inv(vec![inv])),
                3 => {
                    notfound = true;
                    run.handle(peer, PeerEvent::NotFound(vec![inv]));
                }
                4..=5 => {
                    copy = true;
                    run.peers[peer].last_delivery = long_ago;
                    run.handle(peer, PeerEvent::Vote(x.encode()));
                }
                6 => {
                    copy = true;
                    run.peers[peer].last_delivery = long_ago;
                    run.handle(peer, PeerEvent::Vote(resigned(&x, 3).encode()));
                }
                7 => {
                    copy = true;
                    run.peers[peer].last_delivery = long_ago;
                    let mut j = x.clone();
                    j.signature = vec![0x55; 96];
                    run.handle(peer, PeerEvent::Vote(j.encode()));
                }
                _ => {
                    let t = std::mem::replace(&mut run.cfg.request_timeout, Duration::ZERO);
                    run.expire();
                    run.cfg.request_timeout = t;
                }
            }
            if copy && run.peers[peer].last_delivery != long_ago {
                *taken
                    .entry((peer, (x.hash(), INV_GOVERNANCE_VOTE)))
                    .or_default() += 1;
            }
            if notfound {
                for (k, i) in items.iter().enumerate() {
                    if i.0 == x.hash() && capacity(&run, peer, i) < cap_before[k] {
                        *spent.entry((peer, *i)).or_default() += 1;
                        break;
                    }
                }
            }
            for (i, r) in &run.requests.pending {
                if let Some(s) = r.sent
                    && before.get(i) != Some(&(r.peer, Some(s)))
                {
                    *sent.entry((r.peer, *i)).or_default() += 1;
                }
            }
            for (h, r) in &run.refetches.out {
                if refetch_before.get(h) != Some(&(r.peer, r.sent)) {
                    *sent.entry((r.peer, (*h, INV_GOVERNANCE_VOTE))).or_default() += 1;
                }
            }
            assert!(run.phase_requests_done() || run.requests.phase_pending > 0);
            for q in 0..3 {
                for i in &items {
                    let g = sent.get(&(q, *i)).copied().unwrap_or(0);
                    let c = taken.get(&(q, *i)).copied().unwrap_or(0);
                    let n = spent.get(&(q, *i)).copied().unwrap_or(0);
                    assert!(
                        c + n <= g,
                        "seed {seed} step {step}: peer {q} answered {c}+{n} of {g}"
                    );
                    let cap = capacity(&run, q, i);
                    assert!(
                        cap + c + n <= g,
                        "seed {seed} step {step}: peer {q} may send {cap} more after {c}+{n} of {g}"
                    );
                }
            }
        }
        // The wire agrees with the state-derived count.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let ok = (0..3).all(|q| {
                let w = wire[q].lock().unwrap();
                items.iter().all(|i| {
                    w.get(i).copied().unwrap_or(0) == sent.get(&(q, *i)).copied().unwrap_or(0)
                })
            });
            if ok {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "wire getdata counts differ from the state's"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let total: usize = taken.values().sum();
        assert!(total > 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_voter_planting_a_newer_vote_gets_no_relayer_charged() {
    // Validator r11 MJ2: a masternode's own key signs V2 newer than its
    // vote V1 that Core holds, and hands V2 only to the wallet (as a peer,
    // once; the store keeps it). Every honest vote sync then serves V1,
    // obsolete for the wallet: its honest sender paid a suspect each run,
    // per (outpoint, object) the voter planted (D62).
    let p = proposal("p-r11-plant");
    let mut run = two_peer_run(&p).await;
    run.cfg.max_suspect = 3;
    // V1: what Core holds and serves; V2: newer, given to the wallet only.
    let v1 = aged_vote(p.hash(), 100);
    let v2 = vote(p.hash(), 1, VoteOutcome::No);
    run.handle(0, PeerEvent::Inv(vec![vote_inv(&v2)]));
    run.handle(0, PeerEvent::Vote(v2.encode()));
    assert!(run.shared.store().has_vote(&v2.hash()));
    run.handle(1, PeerEvent::Inv(vec![vote_inv(&v1)]));
    run.handle(1, PeerEvent::Vote(v1.encode()));
    assert_eq!(
        run.peers[1].suspect, 0,
        "an honest peer relaying Core's current vote was charged"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_limit_fillers_charge_no_honest_announcer() {
    // Validator r11 MJ2: the key's holder fills its 2n + 2 slots on an
    // object with outpoints Core would never relay (sent by itself as a
    // peer); its real masternode's vote, which Core relays, then hit the
    // limit at each honest announcer in turn (refused, so retried), each
    // charged (D62).
    let p = proposal("p-r11-fill");
    let mut run = two_peer_run(&p).await;
    run.peers.push(sink_peer(2, &run.cfg).await);
    for o in 0..4u8 {
        let mut f = GovernanceVote::funding(
            OutPoint::new(dashcore::Txid::from_byte_array([0xf0 + o; 32]), 0),
            p.hash(),
            VoteOutcome::Yes,
            unix_now() - 50,
        );
        f = resigned(&f, 1);
        run.handle(0, PeerEvent::Inv(vec![vote_inv(&f)]));
        run.handle(0, PeerEvent::Vote(f.encode()));
        assert!(run.shared.store().has_vote(&f.hash()));
    }
    let real = vote(p.hash(), 1, VoteOutcome::Yes);
    run.handle(1, PeerEvent::Inv(vec![vote_inv(&real)]));
    run.handle(2, PeerEvent::Inv(vec![vote_inv(&real)]));
    run.handle(1, PeerEvent::Vote(real.encode()));
    run.handle(2, PeerEvent::Vote(real.encode()));
    assert_eq!(
        (run.peers[1].suspect, run.peers[2].suspect),
        (0, 0),
        "honest announcers of a listed masternode's vote were charged"
    );
}

/// Key 1's vote for outpoint `o` on `parent` at `time`.
fn outpoint_vote(parent: [u8; 32], o: u32, outcome: VoteOutcome, time: i64) -> GovernanceVote {
    let mut txid = [0xa0; 32];
    txid[..4].copy_from_slice(&o.to_le_bytes());
    let v = GovernanceVote::funding(
        OutPoint::new(dashcore::Txid::from_byte_array(txid), 0),
        parent,
        outcome,
        time,
    );
    resigned(&v, 1)
}

/// Core's vote file after a masternode voted YES, NO then ABSTAIN with
/// one timestamp, an hour apart (each accepted: same time, higher
/// outcome; `RemoveOldVotes` drops only older times), in the order Core's
/// vote sync announces them (`push_front`: newest first).
fn ladder(parent: [u8; 32], o: u32, time: i64) -> Vec<GovernanceVote> {
    [VoteOutcome::Abstain, VoteOutcome::No, VoteOutcome::Yes]
        .into_iter()
        .map(|out| outpoint_vote(parent, o, out, time))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_honest_vote_sync_of_a_same_time_ladder_costs_no_suspect() {
    // Validator r11 MJ1: an honest Core peer serving its vote file of one
    // masternode's same-timestamp ladder: the wallet keeps ABSTAIN and
    // recovers NO and YES, SameTimestamp. Core sent them as it holds them
    // (D62).
    let p = proposal("p-r11-ladder");
    let mut run = two_peer_run(&p).await;
    let votes = ladder(p.hash(), 0, unix_now() - 7_200);
    run.handle(1, PeerEvent::Inv(votes.iter().map(vote_inv).collect()));
    for v in &votes {
        run.handle(1, PeerEvent::Vote(v.encode()));
    }
    assert!(run.shared.store().has_vote(&votes[0].hash()));
    assert_eq!(
        run.peers[1].suspect, 0,
        "an honest peer serving Core's vote file was charged"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_time_ladders_do_not_close_an_honest_peer() {
    // Validator r11 MJ1 at scale: 100 masternodes of one key, a ladder
    // each on 51 current proposals, all relayed by the network (no
    // connection to the wallet). One honest peer's vote syncs cost it 2
    // suspects per (masternode, proposal): 10,200 > 10,000, closed (D62).
    let mut run = one_peer_run(|c, _| c.max_in_flight = 5_000).await;
    run.peers.push(sink_peer(1, &run.cfg).await);
    let max = run.cfg.max_suspect;
    list_keys(&run, [1; 100]);
    let time = unix_now() - 7_200;
    let mut all = Vec::new();
    for i in 0..51 {
        let p = proposal(&format!("p-r11-ladder-{i}"));
        {
            let mut s = run.shared.store();
            s.set_confirmations(1);
            s.add_object(p.clone(), None, unix_now()).unwrap();
        }
        for o in 0..100 {
            all.extend(ladder(p.hash(), o, time));
        }
    }
    for chunk in all.chunks(3_000) {
        run.handle(1, PeerEvent::Inv(chunk.iter().map(vote_inv).collect()));
        for v in chunk {
            run.handle(1, PeerEvent::Vote(v.encode()));
        }
    }
    let s = run.peers[1].suspect;
    assert!(
        s <= max,
        "an honest peer reached {s} suspects (bound {max}) serving Core's vote files"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ladder_is_recovered_once_whoever_announces_it() {
    // Validator r11 MJ1: the not-kept hashes were not remembered, so each
    // peer whose vote sync announced them was asked for them in turn and
    // paid. Each copy is now recovered once (D62).
    let p = proposal("p-r11-ladder-each");
    let mut run = two_peer_run(&p).await;
    let votes = ladder(p.hash(), 0, unix_now() - 7_200);
    for peer in [0, 1] {
        run.handle(peer, PeerEvent::Inv(votes.iter().map(vote_inv).collect()));
        for v in &votes {
            run.handle(peer, PeerEvent::Vote(v.encode()));
        }
    }
    assert_eq!(
        (run.peers[0].suspect, run.peers[1].suspect),
        (0, 0),
        "both honest announcers were charged"
    );
    assert_eq!(run.recovered, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keyless_peer_gets_one_recovery_from_one_public_vote() {
    // Validator r12 MJ1: a peer holding no key re-sent one public obsolete
    // vote under the 64 signatures that recover the same listed key, each
    // recovered at no cost (D63).
    let p = proposal("p-d63-malleate");
    let mut run = two_peer_run(&p).await;
    let now = unix_now();
    let current = vote(p.hash(), 1, VoteOutcome::Yes);
    let old = older_than(&current, 1);
    run.shared.store().add_vote(current, None, now).unwrap();
    let item = vote_inv(&old);
    for c in crate::vote::malleations(&old) {
        run.handle(0, PeerEvent::Inv(vec![item]));
        run.handle(0, PeerEvent::Vote(c.encode()));
        assert!(run.requests.pending.is_empty());
    }
    assert_eq!((run.peers[0].suspect, run.peers[0].misbehaviour), (0, 0));
    assert_eq!(run.recovered, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_past_its_unkept_budget_is_refused_before_recovery() {
    // Validator r12 MJ1: a key's holder signs more obsolete votes than the
    // store remembers and replays them in a cycle, each recovered again.
    // Past `max_unkept` wasted recoveries the peer's copies go to the next
    // announcer unrecovered, at no penalty (D63).
    let p = proposal("p-d63-budget");
    let mut run = two_peer_run(&p).await;
    run.cfg.max_unkept = 3;
    let now = unix_now();
    let current = vote(p.hash(), 1, VoteOutcome::Yes);
    run.shared
        .store()
        .add_vote(current.clone(), None, now)
        .unwrap();
    for age in 1..=5 {
        let old = older_than(&current, age);
        run.handle(0, PeerEvent::Inv(vec![vote_inv(&old)]));
        run.handle(0, PeerEvent::Vote(old.encode()));
    }
    assert_eq!(run.recovered, 3);
    assert_eq!((run.peers[0].suspect, run.peers[0].misbehaviour), (0, 0));
    assert!(run.peers[0].peer.is_open());
    // A vote the store keeps goes to an announcer within its budget.
    let x = vote(p.hash(), 2, VoteOutcome::No);
    list_keys(&run, [1, 2]);
    let item = vote_inv(&x);
    run.handle(0, PeerEvent::Inv(vec![item]));
    run.handle(1, PeerEvent::Inv(vec![item]));
    run.handle(0, PeerEvent::Vote(x.encode()));
    assert_eq!(run.recovered, 3);
    let key = (item.hash, item.kind);
    assert_eq!(run.requests.pending.get(&key).map(|r| r.peer), Some(1));
    run.handle(1, PeerEvent::Vote(x.encode()));
    assert!(run.shared.store().has_vote(&x.hash()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_vote_past_the_orphan_bound_still_asks_for_its_parent() {
    // Validator r12 m1: a vote refused at the sender's orphan bound before
    // recovery skipped `fetch_parent`, the way an evicted object comes
    // back (D52, D63).
    let mut run = phase_vote_run().await;
    let addr = run.peers[0].peer.addr;
    let now = unix_now();
    let held = [0x46; 32];
    for o in 0..crate::store::MAX_ORPHANS_PER_PEER {
        let v = outpoint_vote(held, o, VoteOutcome::Yes, now - 100);
        let outcome = run.shared.store().precheck(&v, now).unwrap();
        let checked = recover(v, outcome, Network::Regtest).unwrap();
        let r = run.shared.store().add_checked(checked, Some(addr), now);
        assert_eq!(r, Err(VoteRejection::Orphan));
    }
    let parent = [0x47; 32];
    let x = vote(parent, 1, VoteOutcome::Yes);
    run.handle(0, PeerEvent::Inv(vec![vote_inv(&x)]));
    run.handle(0, PeerEvent::Vote(x.encode()));
    assert_eq!(run.recovered, 0);
    assert!(run.peers[0].parents_asked.contains(&parent));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kept_votes_do_not_spend_the_unkept_budget() {
    // D63: only recoveries the store does not keep count.
    let p = proposal("p-d63-kept");
    let mut run = two_peer_run(&p).await;
    run.cfg.max_unkept = 1;
    list_keys(&run, 1..=4);
    for key in 1..=4 {
        let x = vote(p.hash(), key, VoteOutcome::Yes);
        run.handle(0, PeerEvent::Inv(vec![vote_inv(&x)]));
        run.handle(0, PeerEvent::Vote(x.encode()));
        assert!(run.shared.store().has_vote(&x.hash()));
    }
    // Nor do orphans held for their object.
    for i in 0..3u8 {
        let x = vote([0x90 + i; 32], 1, VoteOutcome::Yes);
        run.handle(0, PeerEvent::Inv(vec![vote_inv(&x)]));
        run.handle(0, PeerEvent::Vote(x.encode()));
        assert!(run.shared.store().has_vote(&x.hash()));
    }
    assert_eq!(run.recovered, 7);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_budget_refusal_after_a_timeout_keeps_the_loss() {
    // Validator r13: a copy refused for its sender's budget, after an
    // earlier announcer's request timed out, ends no loss (D57), so the
    // phase fails (D13); D63 had said "not lost".
    let p = proposal("p-d63-loss");
    let x = vote(p.hash(), 1, VoteOutcome::Yes);
    let item = vote_inv(&x);
    let mut run = two_peer_run(&p).await;
    open_vote_sync(&mut run.peers[0], p.hash());
    open_vote_sync(&mut run.peers[1], p.hash());
    run.peers[1].unkept = run.cfg.max_unkept;
    moved_to_peer_1(&mut run, item);
    run.handle(1, PeerEvent::Vote(x.encode()));
    assert!(!run.shared.store().has_vote(&x.hash()));
    assert_eq!(run.recovered, 0);
    assert_eq!(run.lost, 1);
}

/// Sol r2 Major 3: a failed run asked H1 and H2 for A's votes, which never
/// came. Retried within the window, only the fresh M may be asked, and it
/// withholds them (`ssc(11, 0)`): A's votes were synced with too few peers,
/// not `Synced`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retry_that_may_only_ask_a_withholder_is_not_synced() {
    let a = proposal("a-r2-f3-retry");
    let ha = a.hash();
    let honest = Node {
        objects: vec![a.clone()],
        votes: vec![vote(ha, 1, VoteOutcome::Yes)],
        ..Node::default()
    };
    let withholder = Node {
        objects: vec![a.clone()],
        ..Node::default()
    };
    let hx = start_with(
        vec![honest.clone(), honest, withholder],
        |_| {},
        move |shared, history, addrs| {
            // The failed run confirmed A and asked H1 and H2 for its votes.
            shared.store().add_object(a, None, unix_now()).unwrap();
            for addr in &addrs[..2] {
                history.full_synced.insert(*addr, Instant::now());
                history.vote_synced.insert((*addr, ha), Instant::now());
            }
        },
    )
    .await;
    let p = wait_for(&hx.shared, |p| {
        matches!(p.phase, Phase::Failed | Phase::Synced)
    })
    .await;
    let _ = hx.stop.send(true);
    assert_eq!(p.phase, Phase::Failed, "{p:?}");
    assert!(p.last_error.unwrap().contains("too few peers"));
    assert!(p.last_synced_at.is_none());
}

/// Serves `a`; asked for its votes, counts one but announces only `v`, a
/// vote on an object the run does not know, which it then lacks.
async fn unrelated_notfound_peer(mock: MockPeer, a: GovernanceObject, v: [u8; 32]) {
    let mut conn = mock.accept().await.unwrap();
    while let Ok(msg) = conn.recv().await {
        match msg.command.as_str() {
            commands::MNGOVERNANCESYNC if msg.payload[..32] == [0; 32] => {
                send_status(&mut conn, SSC_GOVOBJ, 1).await;
                let item = InvItem {
                    kind: INV_GOVERNANCE_OBJECT,
                    hash: a.hash(),
                };
                conn.send("inv", &encode_inv(&[item])).await.unwrap();
            }
            commands::MNGOVERNANCESYNC => {
                send_status(&mut conn, SSC_GOVOBJ_VOTE, 1).await;
                let item = InvItem {
                    kind: INV_GOVERNANCE_VOTE,
                    hash: v,
                };
                conn.send("inv", &encode_inv(&[item])).await.unwrap();
            }
            "getdata" => {
                let items = decode_inv(&msg.payload).unwrap();
                if items.iter().any(|i| i.kind == INV_GOVERNANCE_OBJECT) {
                    conn.send(commands::MNGOVERNANCEOBJECT, &a.encode())
                        .await
                        .unwrap();
                }
                let votes: Vec<InvItem> = items
                    .into_iter()
                    .filter(|i| i.kind == INV_GOVERNANCE_VOTE)
                    .collect();
                if !votes.is_empty() {
                    conn.send("notfound", &encode_inv(&votes)).await.unwrap();
                }
            }
            _ => {}
        }
    }
}

/// Sol r2 Major 4: the one peer asked for A's votes counts one, announces
/// a vote on another object instead and answers its `getdata` with
/// `notfound`. Given up on, that vote lets the phase end, but completes no
/// vote sync of A.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_votes_notfound_does_not_settle_another_vote_sync() {
    let a = proposal("a-r2-f4-unrelated");
    let (_shared, p, stop) = start_custom(
        vec![custom(move |mock| {
            unrelated_notfound_peer(mock, a, [0x5f; 32])
        })],
        vec![],
        |_| {},
        |shared| shared.store().set_confirmations(1),
    )
    .await;
    let _ = stop.send(true);
    assert_eq!(p.phase, Phase::Failed, "{p:?}");
    assert!(p.last_error.unwrap().contains("too few peers"));
}

/// Sol r2 Major 4: cleared when full, the record of decoded votes' objects
/// lost the only known object of a vote, whose next inv then counted as
/// unknown. It forgets its oldest entries only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vote_parent_record_forgets_only_its_oldest() {
    let mut run = one_peer_run(|_, _| {}).await;
    let vote = |i: usize| {
        let mut h = [0x31; 32];
        h[..8].copy_from_slice(&(i as u64).to_le_bytes());
        h
    };
    for i in 0..=MAX_LATE {
        run.credit_relays(vote(i), [0x32; 32]);
    }
    assert_eq!(run.vote_parents.len(), MAX_LATE);
    assert!(!run.vote_parents.contains_key(&vote(0)));
    assert_eq!(run.vote_parents.get(&vote(MAX_LATE - 1)), Some(&[0x32; 32]));
}

/// Sol r2 Major 5: a copy of key 2's vote re-signed by key 1 arrives first
/// and, of the same hash, keeps the honest copy out. Once the outpoint
/// resolves to key 2, the copy is dropped and the hash asked for again: the
/// re-signed copy is refused, and the next peer's honest copy is kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resolved_outpoint_brings_back_the_honest_copy() {
    let p = proposal("p-r2-f5-refetch");
    let mut run = two_peer_run(&p).await;
    list_keys(&run, [1, 2]);
    let honest = vote(p.hash(), 2, VoteOutcome::Yes);
    let forged = resigned(&honest, 1);
    let item = vote_inv(&honest);
    let keys = |run: &Run| -> Vec<[u8; 20]> {
        run.shared
            .store()
            .votes_on(&p.hash())
            .map(|v| v.key_id)
            .collect()
    };
    run.handle(0, PeerEvent::Inv(vec![item]));
    run.handle(0, PeerEvent::Vote(forged.encode()));
    assert_eq!(keys(&run), vec![key_id(1)]);
    run.handle(1, PeerEvent::Inv(vec![item]));
    assert!(run.requests.pending.is_empty());

    run.shared.store().set_resolved(HashMap::from([(
        honest.masternode_outpoint,
        crate::store::ResolvedMasternode {
            pro_tx_hash: [9; 32],
            voting_key_id: key_id(2),
            weight: 1,
        },
    )]));
    assert!(keys(&run).is_empty());
    run.expire();
    run.handle(0, PeerEvent::Vote(forged.encode()));
    assert!(keys(&run).is_empty());
    run.handle(1, PeerEvent::Vote(honest.encode()));
    assert_eq!(keys(&run), vec![key_id(2)]);
    assert!(run.requests.pending.is_empty());
}

/// Validator r18 Minor 5: a refetched vote every peer answers `notfound`
/// for (its voter's key changed, D64) is given up on without marking the
/// run's vote syncs as possibly short.
#[tokio::test]
async fn a_refetch_given_up_leaves_vote_syncs_recorded() {
    let p = proposal("p-r18-refetch-notfound");
    let mut run = two_peer_run(&p).await;
    list_keys(&run, [1, 2]);
    let honest = vote(p.hash(), 2, VoteOutcome::Yes);
    let forged = resigned(&honest, 1);
    let item = vote_inv(&honest);
    run.handle(0, PeerEvent::Inv(vec![item]));
    run.handle(0, PeerEvent::Vote(forged.encode()));
    run.handle(1, PeerEvent::Inv(vec![item]));
    run.announce_open = true;
    run.shared.store().set_resolved(HashMap::from([(
        honest.masternode_outpoint,
        crate::store::ResolvedMasternode {
            pro_tx_hash: [9; 32],
            voting_key_id: key_id(3),
            weight: 1,
        },
    )]));
    run.expire();
    assert_eq!(run.refetches.out[&item.hash].peer, 0);
    run.handle(0, PeerEvent::NotFound(vec![item]));
    assert_eq!(run.refetches.out[&item.hash].peer, 1);
    run.handle(1, PeerEvent::NotFound(vec![item]));
    assert!(run.refetches.out.is_empty());
    assert!(!run.vote_given_up);
}

/// Validator r18 Major 2: a retry whose peers were all full-synced within
/// the window still checks the votes phase. Every peer was asked for A's
/// votes in a run that failed before completing them: the retry cannot
/// ask again, so it fails rather than reading Synced with A at no votes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retry_with_no_fresh_peer_still_needs_completed_vote_syncs() {
    let a = proposal("a-r18-nofresh");
    let ha = a.hash();
    let honest = Node {
        objects: vec![a.clone()],
        votes: vec![vote(ha, 1, VoteOutcome::Yes)],
        ..Node::default()
    };
    let hx = start_with(
        vec![honest.clone(), honest.clone(), honest],
        |_| {},
        move |shared, history, addrs| {
            shared.progress().last_synced_at = Some(unix_now() as u64 - 4_000);
            shared.store().add_object(a, None, unix_now()).unwrap();
            for addr in addrs {
                history.full_synced.insert(*addr, Instant::now());
                history.vote_synced.insert((*addr, ha), Instant::now());
            }
        },
    )
    .await;
    let p = wait_for(&hx.shared, |p| {
        matches!(p.phase, Phase::Failed | Phase::Synced)
    })
    .await;
    let _ = hx.stop.send(true);
    assert_eq!(p.phase, Phase::Failed, "{p:?}");
    assert!(
        p.last_error
            .as_deref()
            .unwrap_or("")
            .contains("too few peers"),
        "{p:?}"
    );
}

/// Validator r18 Moderate 3: two peers that never answer `govsync` (a
/// Core node still syncing) are closed at the answer timeout; the one
/// left withholds A's votes (`ssc(11, 0)`). Gone peers still count toward
/// the peers a proposal needs, so one withholder cannot settle it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peers_that_leave_do_not_let_one_withholder_settle_a_proposal() {
    let a = proposal("a-r18-mute");
    let ha = a.hash();
    let mute = Node {
        objects: vec![a.clone()],
        votes: vec![vote(ha, 1, VoteOutcome::Yes)],
        mute: true,
        ..Node::default()
    };
    let withholder = Node {
        objects: vec![a.clone()],
        ..Node::default()
    };
    let hx = start_with(
        vec![mute.clone(), mute, withholder],
        |cfg| {
            cfg.answer_timeout = Duration::from_secs(1);
        },
        move |shared, _history, _addrs| {
            shared.store().add_object(a, None, unix_now()).unwrap();
        },
    )
    .await;
    let p = wait_for(&hx.shared, |p| {
        matches!(p.phase, Phase::Failed | Phase::Synced)
    })
    .await;
    let _ = hx.stop.send(true);
    assert_eq!(p.phase, Phase::Failed, "{p:?}");
    assert!(
        p.last_error
            .as_deref()
            .unwrap_or("")
            .contains("too few peers"),
        "{p:?}"
    );
}

/// Validator r19 Moderate 2: a reconnect within the window to peers that
/// all completed A's vote sync, or two of three (the third asked but gone
/// before completing, so not askable again), reaches Synced on that
/// history; a peer that cannot be asked again does not keep A short.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reconnect_within_the_window_is_synced_on_completed_vote_syncs() {
    for completed in [3usize, 2] {
        let a = proposal(&format!("a-r19-reconnect-{completed}"));
        let ha = a.hash();
        let honest = Node {
            objects: vec![a.clone()],
            votes: vec![vote(ha, 1, VoteOutcome::Yes)],
            ..Node::default()
        };
        let hx = start_with(
            vec![honest.clone(), honest.clone(), honest],
            |_| {},
            move |shared, history, addrs| {
                shared.progress().last_synced_at = Some(unix_now() as u64 - 4_000);
                shared.store().add_object(a, None, unix_now()).unwrap();
                for (i, addr) in addrs.iter().enumerate() {
                    history.full_synced.insert(*addr, Instant::now());
                    history.vote_synced.insert((*addr, ha), Instant::now());
                    if i < completed {
                        history.vote_completed.insert((*addr, ha), Instant::now());
                    }
                }
            },
        )
        .await;
        let p = wait_for(&hx.shared, |p| {
            matches!(p.phase, Phase::Failed | Phase::Synced)
        })
        .await;
        let _ = hx.stop.send(true);
        assert_eq!(p.phase, Phase::Synced, "{completed}: {p:?}");
    }
}

/// Sol r3 Major 1: key 2's holder filled its 2n + 2 unresolved slots on A
/// with ABSTAIN votes for invented outpoints, carried from an earlier
/// connection; its masternode's YES, served by every vote-sync peer, hits
/// the per-key cap at each. The run reads Synced, so the refused vote
/// stays in A's tally as uncertainty: up to 4 more YES, a close call
/// (D71).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_limit_fillers_leave_the_refused_vote_uncertain_when_synced() {
    let a = proposal("a-r3-sol-fill");
    let ha = a.hash();
    let genuine = vote(ha, 2, VoteOutcome::Yes);
    let honest = Node {
        objects: vec![a.clone()],
        votes: vec![genuine.clone()],
        ..Node::default()
    };
    let hx = start_with(
        vec![honest.clone(), honest.clone(), honest],
        |_| {},
        move |shared, _, _| {
            let mut store = shared.store();
            store.add_object(a, None, unix_now()).unwrap();
            for o in 0..4u8 {
                let f = GovernanceVote::funding(
                    OutPoint::new(dashcore::Txid::from_byte_array([0xe0 + o; 32]), 0),
                    ha,
                    VoteOutcome::Abstain,
                    unix_now() - 50,
                );
                store.add_vote(resigned(&f, 2), None, unix_now()).unwrap();
            }
        },
    )
    .await;
    let p = wait_for(&hx.shared, |p| {
        matches!(p.phase, Phase::Failed | Phase::Synced)
    })
    .await;
    let _ = hx.stop.send(true);
    assert_eq!(p.phase, Phase::Synced, "{p:?}");
    let store = hx.shared.store();
    assert!(!store.has_vote(&genuine.hash()));
    let t = store.tally(&ha);
    assert_eq!((t.yes.min, t.yes.max), (0, 4));
    assert_eq!(t.uncounted, 1, "a refused vote vanished while Synced");
}

/// Sol r3 Major 2: key 1 re-signed key 2's honest NO, and its copy keeps
/// the hash. Once the outpoint resolves to key 2 the copy is dropped and
/// the hash asked for again; every peer answers `notfound`. The NO stays
/// up to its masternode's weight. A reconnect within the window to peers
/// whose vote syncs completed reads Synced without asking them again; the
/// NO is asked for again (D77), and stays uncertain unless a peer serves it
/// (D72).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_refetch_leaves_the_vote_uncertain_across_a_reconnect() {
    let p = proposal("p-r3-sol-refetch");
    let ha = p.hash();
    let honest = vote(ha, 2, VoteOutcome::No);
    let forged = resigned(&honest, 1);
    let item = vote_inv(&honest);
    let resolved = HashMap::from([(
        honest.masternode_outpoint,
        crate::store::ResolvedMasternode {
            pro_tx_hash: [9; 32],
            voting_key_id: key_id(2),
            weight: 4,
        },
    )]);
    let uncertain = |shared: &Shared| {
        let t = shared.store().tally(&ha);
        assert_eq!((t.no.min, t.no.max), (0, 4));
        assert_eq!(t.uncounted, 1);
    };
    let mut run = two_peer_run(&p).await;
    list_keys(&run, [1, 2]);
    run.handle(0, PeerEvent::Inv(vec![item]));
    run.handle(0, PeerEvent::Vote(forged.encode()));
    run.handle(1, PeerEvent::Inv(vec![item]));
    run.shared.store().set_resolved(resolved.clone());
    run.expire();
    assert!(run.refetches.out.contains_key(&item.hash));
    run.handle(0, PeerEvent::NotFound(vec![item]));
    run.handle(1, PeerEvent::NotFound(vec![item]));
    assert!(run.refetches.out.is_empty());
    uncertain(&run.shared);

    for served in [false, true] {
        let node = Node {
            objects: vec![p.clone()],
            hidden_votes: if served { vec![honest.clone()] } else { vec![] },
            ..Node::default()
        };
        let (p, forged, resolved) = (p.clone(), forged.clone(), resolved.clone());
        let hx = start_with(
            vec![node.clone(), node.clone(), node],
            |_| {},
            move |shared, history, addrs| {
                shared.progress().last_synced_at = Some(unix_now() as u64 - 4_000);
                let mut store = shared.store();
                store.add_object(p, None, unix_now()).unwrap();
                store.add_vote(forged, None, unix_now()).unwrap();
                store.set_resolved(resolved);
                // The earlier run's refetch was spent.
                assert_eq!(store.take_refetch(), vec![item.hash]);
                for addr in addrs {
                    history.full_synced.insert(*addr, Instant::now());
                    history.vote_synced.insert((*addr, ha), Instant::now());
                    history.vote_completed.insert((*addr, ha), Instant::now());
                }
            },
        )
        .await;
        let progress = wait_for(&hx.shared, |p| {
            matches!(p.phase, Phase::Failed | Phase::Synced)
        })
        .await;
        assert_eq!(progress.phase, Phase::Synced, "{progress:?}");
        if served {
            for _ in 0..100 {
                if hx.shared.store().has_vote(&item.hash) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let _ = hx.stop.send(true);
            let t = hx.shared.store().tally(&ha);
            assert_eq!((t.no.min, t.no.max, t.uncounted), (4, 4, 0));
        } else {
            // Every node lacks it: give the refetch time to be answered.
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ = hx.stop.send(true);
            uncertain(&hx.shared);
        }
    }
}

/// Sol r3 Moderate: with no proposal, no vote sync prunes the record of
/// full syncs; adding one drops the entries past the window once the
/// record doubles, keeping those within it (D73).
#[test]
fn full_sync_history_drops_entries_past_the_window() {
    let addr = |i: usize| SocketAddr::from(([10, 0, (i >> 8) as u8, i as u8], 9999));
    let mut h = SyncHistory::default();
    for i in 0..3 * MIN_HISTORY_PRUNE {
        h.note_full_synced(addr(i), Duration::ZERO);
    }
    assert!(
        h.full_synced.len() <= MIN_HISTORY_PRUNE,
        "{}",
        h.full_synced.len()
    );
    let mut h = SyncHistory::default();
    for i in 0..3 * MIN_HISTORY_PRUNE {
        h.note_full_synced(addr(i), Duration::from_secs(3_600));
    }
    assert_eq!(h.full_synced.len(), 3 * MIN_HISTORY_PRUNE);
}

/// D73: an object-only run records its full syncs through the bound: the
/// stale addresses of earlier runs leave the history.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_object_only_run_prunes_stale_full_syncs() {
    let hx = start_with(
        vec![Node::default()],
        |cfg| cfg.repeat_window = Duration::from_secs(1),
        |_, history, _| {
            let then = Instant::now() - Duration::from_secs(5);
            for i in 0..MIN_HISTORY_PRUNE {
                let addr = SocketAddr::from(([10, 1, (i >> 8) as u8, i as u8], 9999));
                history.full_synced.insert(addr, then);
            }
        },
    )
    .await;
    let p = wait_for(&hx.shared, |p| {
        matches!(p.phase, Phase::Failed | Phase::Synced)
    })
    .await;
    let _ = hx.stop.send(true);
    assert_eq!(p.phase, Phase::Synced, "{p:?}");
    assert_eq!(hx.history.lock().unwrap().full_synced.len(), 1);
}

/// Key 1's NO on `p` re-signed by key 2 and announced first by peer 0,
/// then replaced by key 2's newer vote in its slot; peer 1's inv of the
/// NO was skipped as known. Returns the honest NO's inv.
fn lose_honest_no(run: &mut Run, p: &GovernanceObject) -> InvItem {
    list_keys(run, [1, 2]);
    let honest = vote(p.hash(), 1, VoteOutcome::No);
    let item = vote_inv(&honest);
    run.handle(0, PeerEvent::Inv(vec![item]));
    run.handle(0, PeerEvent::Vote(resigned(&honest, 2).encode()));
    if run.peers.len() > 1 {
        run.handle(1, PeerEvent::Inv(vec![item]));
    }
    let own = GovernanceVote::funding(
        honest.masternode_outpoint,
        p.hash(),
        VoteOutcome::Abstain,
        unix_now() - 50,
    );
    let own = resigned(&own, 2);
    run.handle(0, PeerEvent::Inv(vec![vote_inv(&own)]));
    run.handle(0, PeerEvent::Vote(own.encode()));
    assert!(!run.shared.store().has_vote(&honest.hash()));
    assert_eq!(run.shared.store().tally(&p.hash()).uncounted, 1);
    item
}

/// Sol r4 Major 1 (DEC-257, D77): of three peers asked for the lost NO
/// again, two answer `notfound` and leave; the third, which may hold it,
/// leaves too. No answer proves no node holds it: the NO stays uncounted,
/// with or without the peers leaving.
#[tokio::test]
async fn forged_notfounds_and_disconnects_leave_a_lost_vote_uncounted() {
    for leave in [false, true] {
        let p = proposal(&format!("p-d77-notfound-{leave}"));
        let mut run = two_peer_run(&p).await;
        run.peers.push(sink_peer(2, &run.cfg).await);
        let item = lose_honest_no(&mut run, &p);
        run.expire();
        assert_eq!(run.refetches.out[&item.hash].peer, 0);
        run.handle(0, PeerEvent::NotFound(vec![item]));
        assert_eq!(run.refetches.out[&item.hash].peer, 1);
        run.handle(1, PeerEvent::NotFound(vec![item]));
        assert_eq!(run.refetches.out[&item.hash].peer, 2);
        if leave {
            run.handle(1, PeerEvent::Closed);
            run.handle(2, PeerEvent::Closed);
            assert_eq!(run.peers.len(), 1);
        } else {
            run.handle(2, PeerEvent::NotFound(vec![item]));
        }
        assert!(run.refetches.out.is_empty());
        let t = run.shared.store().tally(&p.hash());
        assert_eq!((t.uncounted, t.no.max), (1, 4), "leave: {leave}");
    }
}

/// D76: with one peer, its `notfound` does not settle the lost NO; the
/// next run asks for it again.
#[tokio::test]
async fn a_lost_vote_one_peer_lacks_stays_uncertain() {
    let p = proposal("p-d76-one");
    let mut run = one_peer_run(|_, _| {}).await;
    {
        let mut s = run.shared.store();
        s.set_confirmations(1);
        s.add_object(p.clone(), None, unix_now()).unwrap();
    }
    let item = lose_honest_no(&mut run, &p);
    run.expire();
    run.handle(0, PeerEvent::NotFound(vec![item]));
    assert!(run.refetches.out.is_empty());
    let mut store = run.shared.store();
    let t = store.tally(&p.hash());
    assert_eq!((t.uncounted, t.no.max), (1, 4));
    store.requeue_lost();
    assert_eq!(store.take_refetch(), vec![item.hash]);
}

/// D76: a lost NO the last run did not fetch is asked for again by the
/// next run's votes phase; a node that holds it but does not announce it
/// serves it, and the NO is no longer lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_vote_is_asked_for_again_by_the_next_run() {
    let p = proposal("p-d76-next-run");
    let ha = p.hash();
    let honest = vote(ha, 1, VoteOutcome::No);
    let node = Node {
        objects: vec![p.clone()],
        hidden_votes: vec![honest.clone()],
        ..Node::default()
    };
    let lost = honest.hash();
    let hx = start_with(
        vec![node.clone(), node],
        |_| {},
        move |shared, _, _| {
            let mut store = shared.store();
            store.add_object(p, None, unix_now()).unwrap();
            store
                .add_vote(resigned(&honest, 2), None, unix_now())
                .unwrap();
            let own = GovernanceVote::funding(
                honest.masternode_outpoint,
                ha,
                VoteOutcome::Abstain,
                unix_now() - 50,
            );
            store.add_vote(resigned(&own, 2), None, unix_now()).unwrap();
            // The last run asked for it and gave up.
            assert_eq!(store.take_refetch(), vec![lost]);
            assert_eq!(store.tally(&ha).uncounted, 1);
        },
    )
    .await;
    let p = wait_for(&hx.shared, |p| {
        matches!(p.phase, Phase::Failed | Phase::Synced)
    })
    .await;
    assert_eq!(p.phase, Phase::Synced, "{p:?}");
    for _ in 0..100 {
        if hx.shared.store().has_vote(&lost) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = hx.stop.send(true);
    let store = hx.shared.store();
    assert!(store.has_vote(&lost));
    assert_eq!(store.tally(&ha).uncounted, 0);
}

/// Validator r23 Major 1 (D76): two peers announce the lost NO before it
/// is asked for again and answer `notfound`. Their request was theirs:
/// it is given up as any relay, and the NO stays uncertain.
#[tokio::test]
async fn a_lost_vote_its_announcers_lack_stays_uncertain() {
    let p = proposal("p-d76-announced");
    let mut run = two_peer_run(&p).await;
    run.peers.push(sink_peer(2, &run.cfg).await);
    let item = lose_honest_no(&mut run, &p);
    let key = (item.hash, item.kind);
    run.handle(0, PeerEvent::Inv(vec![item]));
    run.handle(1, PeerEvent::Inv(vec![item]));
    run.expire();
    assert!(run.refetches.out.is_empty());
    assert_eq!(run.requests.pending[&key].peer, 0);
    run.handle(0, PeerEvent::NotFound(vec![item]));
    assert_eq!(run.requests.pending[&key].peer, 1);
    run.handle(1, PeerEvent::NotFound(vec![item]));
    assert!(!run.requests.pending.contains_key(&key));
    let t = run.shared.store().tally(&p.hash());
    assert_eq!((t.uncounted, t.no.max), (1, 4));
}

/// Sol r4 Major 2 (DEC-257, D77): with one announcement allowed per peer,
/// asking every peer for a lost vote again takes none of it: the honest
/// peer's next announced vote is still fetched.
#[tokio::test]
async fn a_refetch_takes_no_announcement_allowance() {
    let p = proposal("p-d77-allowance");
    let mut run = two_peer_run(&p).await;
    run.cfg.max_announcements = 1;
    let item = lose_honest_no(&mut run, &p);
    run.expire();
    assert_eq!(run.refetches.out[&item.hash].peer, 0);
    run.handle(0, PeerEvent::NotFound(vec![item]));
    assert_eq!(run.refetches.out[&item.hash].peer, 1);
    assert!(run.peers.iter().all(|s| s.announced == 0));
    assert!(run.requests.announced.is_empty());
    open_vote_sync(&mut run.peers[1], p.hash());
    let yes = vote(p.hash(), 1, VoteOutcome::Yes);
    let yes = GovernanceVote::funding(
        OutPoint::new(dashcore::Txid::from_byte_array([0x5e; 32]), 0),
        p.hash(),
        VoteOutcome::Yes,
        yes.time,
    );
    let yes_item = vote_inv(&resigned(&yes, 1));
    run.handle(1, PeerEvent::Inv(vec![yes_item]));
    let r = &run.requests.pending[&(yes_item.hash, yes_item.kind)];
    assert_eq!(r.peer, 1);
    assert!(r.phase);
}

/// Sol r4 Major 2 (DEC-257, D77): past its allowance, a phase vote the
/// peer announces is not fetched, so it completes no vote sync: the count
/// it makes up still lets the peer's sync end, as D68's unattributable
/// votes do, but not as complete for the proposal.
#[tokio::test]
async fn a_dropped_phase_vote_completes_no_sync() {
    let p = proposal("p-d77-dropped");
    let h = p.hash();
    for limit in [1, 2] {
        let mut run = one_peer_run(|c, s| {
            c.max_announcements = limit;
            s.vote_asked.push(h);
            s.vote_expected.push(2);
            s.vote_announced.insert(h, 0);
        })
        .await;
        list_key_1(&run);
        let (a, b) = (aged_vote(h, 1), aged_vote(h, 2));
        for v in [&a, &b] {
            run.vote_parents.insert(v.hash(), h);
        }
        run.handle(0, PeerEvent::Inv(vec![vote_inv(&a), vote_inv(&b)]));
        let state = &run.peers[0];
        assert!(state.votes_done());
        let completed: Vec<[u8; 32]> = state.votes_completed().collect();
        let fetched = run.requests.pending.len();
        assert_eq!(fetched, limit);
        assert_eq!(completed.contains(&h), limit == 2, "limit {limit}");
    }
}

/// D77: a run asks for at most `max_refetches` lost votes again; the rest
/// stay lost for a later run. A refetch that times out moves to the next
/// peer, apart from any announcement.
#[tokio::test]
async fn refetches_stay_within_the_run_budget() {
    let p = proposal("p-d77-budget");
    let mut run = two_peer_run(&p).await;
    run.cfg.max_refetches = 1;
    let item = lose_honest_no(&mut run, &p);
    run.expire();
    assert_eq!(run.refetches.out.len(), 1);
    assert_eq!(run.refetches.out[&item.hash].peer, 0);
    expire_now(&mut run);
    assert_eq!(run.refetches.out[&item.hash].peer, 1);
    expire_now(&mut run);
    assert!(run.refetches.out.is_empty());
    // Spent: asked again only by a later run.
    run.shared.store().requeue_lost();
    run.expire();
    assert!(run.refetches.out.is_empty());
    assert_eq!(run.refetches.started, 1);
    assert!(run.peers.iter().all(|s| s.announced == 0));
    assert_eq!(run.shared.store().tally(&p.hash()).uncounted, 1);
}

/// D77: a lost vote asked for again is answered only by the peer asked.
/// Another peer's copy is unsolicited and dropped (D59); a copy the store
/// refuses moves the ask to the next peer; the asked peer's copy is stored
/// and settles it.
#[tokio::test]
async fn a_refetched_vote_is_taken_only_from_the_peer_asked() {
    let p = proposal("p-d77-asked");
    let mut run = two_peer_run(&p).await;
    let item = lose_honest_no(&mut run, &p);
    let honest = vote(p.hash(), 1, VoteOutcome::No);
    assert_eq!(honest.hash(), item.hash);
    run.expire();
    assert_eq!(run.refetches.out[&item.hash].peer, 0);
    run.handle(1, PeerEvent::Vote(honest.encode()));
    assert!(!run.shared.store().has_vote(&item.hash));
    assert_eq!(run.refetches.out[&item.hash].peer, 0);
    let mut junk = honest.clone();
    junk.signature = vec![0x55; 96];
    run.handle(0, PeerEvent::Vote(junk.encode()));
    assert_eq!(run.refetches.out[&item.hash].peer, 1);
    run.handle(1, PeerEvent::Vote(honest.encode()));
    assert!(run.shared.store().has_vote(&item.hash));
    assert!(run.refetches.out.is_empty());
    assert_eq!(run.shared.store().tally(&p.hash()).uncounted, 0);
}

/// Validator r26 (D78): a vote a peer relays before its object's vote sync
/// is credited to that sync without a fetch when the store holds it. Gone
/// by the sync (its copy displaced or expired, its lost record trimmed
/// with the object unstored), it may be a vote Core counts that the wallet
/// lacks: it completes no sync, though the peer's may still end.
#[tokio::test]
async fn a_relayed_vote_gone_by_its_sync_completes_none() {
    for gone in [false, true] {
        let p = proposal(&format!("p-d78-relayed-{gone}"));
        let h = p.hash();
        let mut run = one_peer_run(|_, _| {}).await;
        list_key_1(&run);
        let v = aged_vote(h, 1);
        let now = unix_now();
        let r = run.shared.store().add_vote(v.clone(), None, now);
        assert_eq!(r, Err(VoteRejection::Orphan));
        run.handle(0, PeerEvent::Inv(vec![vote_inv(&v)]));
        assert!(run.requests.pending.is_empty());
        {
            let mut store = run.shared.store();
            if gone {
                store.expire_orphans(now + crate::store::ORPHAN_EXPIRY_SECS + 1);
            }
            store.add_object(p.clone(), None, now).unwrap();
            assert_eq!(store.has_vote(&v.hash()), !gone);
        }
        run.ask_votes_of(0, h, &[]);
        let state = &mut run.peers[0];
        state.vote_expected.push(1);
        assert!(state.votes_done());
        let completed: Vec<[u8; 32]> = state.votes_completed().collect();
        assert_eq!(completed.contains(&h), !gone, "gone {gone}");
        assert_eq!(state.unattributable, i64::from(gone));
    }
}

/// Validator r26 (D77): an awaited (D72) vote the run did not ask for again,
/// past its budget or with no peer connected, is asked for by a later run.
#[tokio::test]
async fn an_awaited_vote_left_unasked_is_asked_by_a_later_run() {
    for no_peers in [false, true] {
        let p = proposal(&format!("p-d77-awaited-{no_peers}"));
        let ha = p.hash();
        let honest = vote(ha, 2, VoteOutcome::No);
        let item = vote_inv(&honest);
        let resolved = HashMap::from([(
            honest.masternode_outpoint,
            crate::store::ResolvedMasternode {
                pro_tx_hash: [9; 32],
                voting_key_id: key_id(2),
                weight: 4,
            },
        )]);
        let mut run = two_peer_run(&p).await;
        list_keys(&run, [1, 2]);
        run.handle(0, PeerEvent::Inv(vec![item]));
        run.handle(0, PeerEvent::Vote(resigned(&honest, 1).encode()));
        if no_peers {
            run.handle(0, PeerEvent::Closed);
            run.handle(1, PeerEvent::Closed);
        } else {
            run.cfg.max_refetches = 0;
        }
        run.shared.store().set_resolved(resolved);
        run.expire();
        assert!(run.refetches.out.is_empty());
        let t = run.shared.store().tally(&ha);
        assert_eq!((t.uncounted, t.no.max), (1, 4));
        run.shared.store().requeue_lost();
        let queued = run.shared.store().take_refetch();
        assert!(queued.contains(&item.hash), "no peers {no_peers}");
    }
}
