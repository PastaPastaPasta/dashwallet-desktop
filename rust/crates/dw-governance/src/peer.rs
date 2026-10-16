//! One full-node peer for govsync, over a [`dw_p2p::Session`]: the
//! governance messages of Dash Core's `governance/net_governance.cpp`
//! (`govsync`, `ssc`, `govobj`, `govobjvote`) and the `inv` / `getdata` /
//! `notfound` entries of types 17 and 18, turned into [`PeerEvent`]s on one
//! channel shared by every peer of a run (MG-02 §2.1).
//!
//! The session answers pings, closes after its idle timeout (5 minutes here,
//! with a ping every minute, so a dead peer ends its stream; MG-01 D17) and
//! counts the bytes it reads. Sends never wait: one that finds the session's
//! queue full closes the session (the peer stopped reading), so a stuck
//! peer cannot hold up the sync task.

use std::net::SocketAddr;
use std::time::Duration;

use dashcore::Network;
use dashcore::consensus::encode::{Decodable, Encodable, VarInt};
use dw_p2p::{Message, P2pError, Session, SessionConfig, commands};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// `MSG_GOVERNANCE_OBJECT` (Core `protocol.h`).
pub const INV_GOVERNANCE_OBJECT: u32 = 17;
/// `MSG_GOVERNANCE_OBJECT_VOTE`.
pub const INV_GOVERNANCE_VOTE: u32 = 18;
/// `MASTERNODE_SYNC_GOVOBJ` / `MASTERNODE_SYNC_GOVOBJ_VOTE` (`ssc` item ids).
pub const SSC_GOVOBJ: i32 = 10;
pub const SSC_GOVOBJ_VOTE: i32 = 11;
/// Most entries in one `inv`, `getdata` or `notfound` (Core `MAX_INV_SZ`).
pub const MAX_INV: usize = 50_000;

/// Frames queued per session: a vote sync of every current proposal plus
/// the `getdata`s in flight fit with room to spare.
const SEND_QUEUE: usize = 4096;

const INV: &str = "inv";
const GETDATA: &str = "getdata";
const NOTFOUND: &str = "notfound";

/// One inventory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InvItem {
    pub kind: u32,
    /// Internal order.
    pub hash: [u8; 32],
}

/// What a peer sent that governance cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvent {
    /// Governance entries of an `inv`.
    Inv(Vec<InvItem>),
    /// Governance entries of a `getdata`.
    GetData(Vec<InvItem>),
    /// Governance entries of a `notfound`.
    NotFound(Vec<InvItem>),
    /// A `govobj` payload.
    Object(Vec<u8>),
    /// A `govobjvote` payload.
    Vote(Vec<u8>),
    /// `ssc`: how many items of `item` the peer announces for a sync.
    SyncCount { item: i32, count: i32 },
    /// A governance message that does not decode.
    Malformed,
    /// The session ended.
    Closed,
}

/// Session settings for governance peers.
#[derive(Debug, Clone)]
pub struct PeerConfig {
    pub network: Network,
    pub session: SessionConfig,
}

impl PeerConfig {
    pub fn new(network: Network, user_agent: impl Into<String>, start_height: u32) -> Self {
        Self {
            network,
            session: SessionConfig {
                user_agent: user_agent.into(),
                start_height: i32::try_from(start_height).unwrap_or(i32::MAX),
                ping_interval: Duration::from_secs(60),
                idle_timeout: Duration::from_secs(5 * 60),
                send_queue: SEND_QUEUE,
                ..SessionConfig::default()
            },
        }
    }
}

/// One connected peer. Dropping it closes the session.
#[derive(Debug)]
pub struct Peer {
    pub id: usize,
    pub addr: SocketAddr,
    session: Session,
    forwarder: JoinHandle<()>,
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.forwarder.abort();
    }
}

pub(crate) fn encode_inv(items: &[InvItem]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(9 + items.len() * 36);
    VarInt(items.len() as u64)
        .consensus_encode(&mut buf)
        .expect("writing to a Vec cannot fail");
    for i in items {
        buf.extend_from_slice(&i.kind.to_le_bytes());
        buf.extend_from_slice(&i.hash);
    }
    buf
}

/// The governance entries of an `inv`-shaped payload; `None` if it does not
/// decode or holds more than [`MAX_INV`] entries.
pub(crate) fn decode_inv(payload: &[u8]) -> Option<Vec<InvItem>> {
    let mut r = payload;
    let n = VarInt::consensus_decode(&mut r).ok()?.0;
    if n > MAX_INV as u64 || r.len() as u64 != n * 36 {
        return None;
    }
    Some(
        r.as_chunks::<36>()
            .0
            .iter()
            .map(|c| InvItem {
                kind: u32::from_le_bytes(c[..4].try_into().expect("4 bytes")),
                hash: c[4..].try_into().expect("32 bytes"),
            })
            .filter(|i| matches!(i.kind, INV_GOVERNANCE_OBJECT | INV_GOVERNANCE_VOTE))
            .collect(),
    )
}

/// A Core `CBloomFilter` that matches nothing: one zero byte, one hash
/// function. A vote `govsync` needs a non-empty filter (an empty one asks
/// for the object itself), and this one leaves every vote to be sent.
pub fn empty_vote_filter() -> Vec<u8> {
    let mut buf = Vec::with_capacity(11);
    vec![0u8].consensus_encode(&mut buf).expect("Vec write");
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.push(0);
    buf
}

/// The filter Core reads as "send me the object, not its votes": an empty
/// vector. With a zero hash it asks for every object.
pub fn object_fetch_filter() -> Vec<u8> {
    let mut buf = Vec::with_capacity(10);
    Vec::<u8>::new()
        .consensus_encode(&mut buf)
        .expect("Vec write");
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.push(0);
    buf
}

fn to_event(msg: Message) -> PeerEvent {
    let inv =
        |f: fn(Vec<InvItem>) -> PeerEvent| decode_inv(&msg.payload).map_or(PeerEvent::Malformed, f);
    match msg.command.as_str() {
        INV => inv(PeerEvent::Inv),
        GETDATA => inv(PeerEvent::GetData),
        NOTFOUND => inv(PeerEvent::NotFound),
        commands::MNGOVERNANCEOBJECT => PeerEvent::Object(msg.payload),
        commands::MNGOVERNANCEOBJECTVOTE => PeerEvent::Vote(msg.payload),
        commands::SYNCSTATUSCOUNT => match msg.payload.as_slice() {
            [a, b, c, d, e, f, g, h] => PeerEvent::SyncCount {
                item: i32::from_le_bytes([*a, *b, *c, *d]),
                count: i32::from_le_bytes([*e, *f, *g, *h]),
            },
            _ => PeerEvent::Malformed,
        },
        _ => PeerEvent::Malformed,
    }
}

impl Peer {
    /// Connects and runs the handshake; from then on every governance
    /// message of the peer arrives on `events` as `(id, event)`, ending with
    /// [`PeerEvent::Closed`].
    pub async fn connect(
        id: usize,
        addr: SocketAddr,
        cfg: &PeerConfig,
        events: mpsc::UnboundedSender<(usize, PeerEvent)>,
    ) -> Result<Peer, P2pError> {
        let session = Session::connect(addr, cfg.network, cfg.session.clone()).await?;
        let mut rx = session.subscribe(&[
            INV,
            GETDATA,
            NOTFOUND,
            commands::MNGOVERNANCEOBJECT,
            commands::MNGOVERNANCEOBJECTVOTE,
            commands::SYNCSTATUSCOUNT,
        ]);
        let forwarder = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if events.send((id, to_event(msg))).is_err() {
                    return;
                }
            }
            let _ = events.send((id, PeerEvent::Closed));
        });
        Ok(Peer {
            id,
            addr,
            session,
            forwarder,
        })
    }

    pub fn bytes_received(&self) -> u64 {
        self.session.bytes_received()
    }

    pub fn is_open(&self) -> bool {
        !self.session.is_closed()
    }

    /// Ends the session (its stream then ends with `Closed`).
    pub fn close(&self) {
        self.session.close();
    }

    fn send(&self, command: &str, payload: Vec<u8>) {
        if let Err(e) = self.session.try_send(command, payload) {
            tracing::debug!(peer = %self.addr, error = %e, "governance send failed");
        }
    }

    /// `govsync` for every object (`hash` zero, [`object_fetch_filter`]),
    /// for the votes of one object (`hash`, [`empty_vote_filter`]), or for
    /// one object (`hash`, [`object_fetch_filter`]).
    pub fn send_govsync(&self, hash: [u8; 32], filter: &[u8]) {
        let mut payload = Vec::with_capacity(32 + filter.len());
        payload.extend_from_slice(&hash);
        payload.extend_from_slice(filter);
        self.send(commands::MNGOVERNANCESYNC, payload);
    }

    pub fn send_getdata(&self, items: &[InvItem]) {
        for chunk in items.chunks(MAX_INV) {
            self.send(GETDATA, encode_inv(chunk));
        }
    }

    pub fn send_inv(&self, items: &[InvItem]) {
        for chunk in items.chunks(MAX_INV) {
            self.send(INV, encode_inv(chunk));
        }
    }

    pub fn send_object(&self, payload: Vec<u8>) {
        self.send(commands::MNGOVERNANCEOBJECT, payload);
    }

    pub fn send_vote(&self, payload: Vec<u8>) {
        self.send(commands::MNGOVERNANCEOBJECTVOTE, payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dw_p2p::testing::MockPeer;

    #[test]
    fn inv_keeps_governance_entries_and_refuses_oversize() {
        let items = [
            InvItem {
                kind: 1,
                hash: [1; 32],
            },
            InvItem {
                kind: INV_GOVERNANCE_OBJECT,
                hash: [2; 32],
            },
            InvItem {
                kind: INV_GOVERNANCE_VOTE,
                hash: [3; 32],
            },
        ];
        assert_eq!(
            decode_inv(&encode_inv(&items)).unwrap(),
            items[1..].to_vec()
        );
        let mut short = encode_inv(&items);
        short.pop();
        assert_eq!(decode_inv(&short), None);
        let mut huge = Vec::new();
        VarInt(MAX_INV as u64 + 1)
            .consensus_encode(&mut huge)
            .unwrap();
        huge.resize(huge.len() + (MAX_INV + 1) * 36, 0);
        assert_eq!(decode_inv(&huge), None);
    }

    #[tokio::test]
    async fn events_from_a_session() {
        let mock = MockPeer::bind(Network::Regtest).await.unwrap();
        let addr = mock.addr();
        let server = tokio::spawn(async move {
            let mut conn = mock.accept().await.unwrap();
            let sync = conn.recv_command(commands::MNGOVERNANCESYNC).await.unwrap();
            assert_eq!(&sync.payload[..32], &[0; 32]);
            assert_eq!(&sync.payload[32..], &object_fetch_filter()[..]);
            let mut ssc = SSC_GOVOBJ.to_le_bytes().to_vec();
            ssc.extend_from_slice(&2i32.to_le_bytes());
            conn.send(commands::SYNCSTATUSCOUNT, &ssc).await.unwrap();
            let item = InvItem {
                kind: INV_GOVERNANCE_OBJECT,
                hash: [7; 32],
            };
            conn.send(NOTFOUND, &encode_inv(&[item])).await.unwrap();
            conn.send(INV, &[0xff]).await.unwrap();
            conn.send(commands::MNGOVERNANCEOBJECT, &[1, 2])
                .await
                .unwrap();
            conn
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let cfg = PeerConfig::new(Network::Regtest, "/t/", 1);
        let peer = Peer::connect(4, addr, &cfg, tx).await.unwrap();
        peer.send_govsync([0; 32], &object_fetch_filter());
        let mut got = Vec::new();
        for _ in 0..4 {
            got.push(rx.recv().await.unwrap());
        }
        assert_eq!(
            got,
            vec![
                (
                    4,
                    PeerEvent::SyncCount {
                        item: SSC_GOVOBJ,
                        count: 2
                    }
                ),
                (
                    4,
                    PeerEvent::NotFound(vec![InvItem {
                        kind: INV_GOVERNANCE_OBJECT,
                        hash: [7; 32]
                    }])
                ),
                (4, PeerEvent::Malformed),
                (4, PeerEvent::Object(vec![1, 2])),
            ]
        );
        assert!(peer.bytes_received() > 0);
        server.await.unwrap().shutdown().await;
        assert_eq!(rx.recv().await.unwrap(), (4, PeerEvent::Closed));
        assert!(!peer.is_open());
    }
}
