use super::*;
use crate::peer::{decode_inv, encode_inv};
use crate::store::ListEntry;
use crate::vote::{VoteOutcome, to_recoverable_compact};
use dashcore::OutPoint;
use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::{Message, PublicKey, SecretKey};
use dw_p2p::commands;
use dw_p2p::testing::MockPeer;

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

/// How a fake full node behaves.
#[derive(Clone, Default)]
struct Node {
    objects: Vec<GovernanceObject>,
    votes: Vec<GovernanceVote>,
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
    while let Ok(msg) = conn.recv().await {
        match msg.command.as_str() {
            commands::MNGOVERNANCESYNC => {
                let hash: [u8; 32] = msg.payload[..32].try_into().unwrap();
                let _ = seen.send(Seen::Govsync(hash));
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
                } else {
                    let items: Vec<InvItem> = node
                        .votes
                        .iter()
                        .filter(|v| v.parent_hash == hash)
                        .map(|v| InvItem {
                            kind: INV_GOVERNANCE_VOTE,
                            hash: v.hash(),
                        })
                        .collect();
                    let mut ssc = SSC_GOVOBJ_VOTE.to_le_bytes().to_vec();
                    ssc.extend_from_slice(&(items.len() as i32).to_le_bytes());
                    conn.send(commands::SYNCSTATUSCOUNT, &ssc).await.unwrap();
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
                for i in items {
                    if let Some(o) = node.objects.iter().find(|o| o.hash() == i.hash) {
                        conn.send(commands::MNGOVERNANCEOBJECT, &o.encode())
                            .await
                            .unwrap();
                    }
                    if let Some(v) = node.votes.iter().find(|v| v.hash() == i.hash) {
                        conn.send(commands::MNGOVERNANCEOBJECTVOTE, &v.encode())
                            .await
                            .unwrap();
                    }
                }
                if node.go_silent && synced_votes {
                    // Hold the connection open without reading.
                    std::future::pending::<()>().await;
                }
            }
            "inv" => conn.send("getdata", &msg.payload).await.unwrap(),
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
    let shared = Arc::new(Shared::new(Network::Regtest, Box::new(|| {})));
    shared.store().set_masternode_list(&[
        ListEntry {
            voting_key_id: key_id(1),
            weight: 1,
            valid: true,
        },
        ListEntry {
            voting_key_id: key_id(2),
            weight: 4,
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
    let mut cfg = SyncConfig::new(PeerConfig::new(Network::Regtest, "/t/", 1));
    cfg.idle_timeout = Duration::from_secs(3);
    tune(&mut cfg);
    let (stop, stop_rx) = watch::channel(false);
    let task = tokio::spawn(run(
        Arc::clone(&shared),
        cfg,
        addrs,
        Arc::new(Mutex::new(SyncHistory::default())),
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
