//! A minimal outbound Dash P2P session for governance: handshake,
//! ping/pong, and the governance messages (`govsync`, `inv`/`getdata` of
//! types 17/18, `govobj`, `govobjvote`, `ssc`).
//!
//! This is R2's own transport until `dw-p2p` (R1, docs/contracts/m3-engine.md
//! §6) ships `Session`; it uses the wire names of [`dw_p2p::commands`] and
//! keeps the same shape (connect, send, a receiver of events, close), so the
//! switch is local to this module.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dashcore::Network;
use dashcore::consensus::encode::{Decodable, Encodable, VarInt};
use dashcore::hashes::{Hash, sha256d};
use dashcore::network::address::Address as NetAddress;
use dashcore::network::constants::ServiceFlags;
use dashcore::network::message_network::VersionMessage;
use dw_p2p::commands;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// `MSG_GOVERNANCE_OBJECT` (Core `protocol.h`).
pub const INV_GOVERNANCE_OBJECT: u32 = 17;
/// `MSG_GOVERNANCE_OBJECT_VOTE`.
pub const INV_GOVERNANCE_VOTE: u32 = 18;
/// `MASTERNODE_SYNC_GOVOBJ` / `MASTERNODE_SYNC_GOVOBJ_VOTE` (`ssc` item ids).
pub const SSC_GOVOBJ: i32 = 10;
pub const SSC_GOVOBJ_VOTE: i32 = 11;

/// Largest payload read (Core `MAX_PROTOCOL_MESSAGE_LENGTH`, 3 MiB).
const MAX_PAYLOAD: usize = 3 * 1024 * 1024;
/// Most entries in one `inv` / `getdata` (Core `MAX_INV_SZ`).
pub const MAX_INV: usize = 50_000;

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
    /// A `govobj` payload.
    Object(Vec<u8>),
    /// A `govobjvote` payload.
    Vote(Vec<u8>),
    /// `ssc`: how many items of `item` the peer announces for a sync.
    SyncCount { item: i32, count: i32 },
    /// The connection ended; the reason.
    Closed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("connect {0}: {1}")]
    Connect(SocketAddr, String),
    #[error("handshake with {0}: {1}")]
    Handshake(String, String),
}

/// Session settings.
#[derive(Debug, Clone)]
pub struct PeerConfig {
    pub network: Network,
    pub user_agent: String,
    pub start_height: i32,
    pub connect_timeout: Duration,
    pub handshake_timeout: Duration,
}

impl PeerConfig {
    pub fn new(network: Network, user_agent: impl Into<String>, start_height: u32) -> Self {
        Self {
            network,
            user_agent: user_agent.into(),
            start_height: i32::try_from(start_height).unwrap_or(i32::MAX),
            connect_timeout: Duration::from_secs(10),
            handshake_timeout: Duration::from_secs(15),
        }
    }
}

/// One connected peer. Dropping it closes the connection.
#[derive(Debug)]
pub struct Peer {
    pub id: usize,
    pub addr: String,
    out: mpsc::UnboundedSender<Vec<u8>>,
    bytes_received: Arc<AtomicU64>,
    tasks: Vec<JoinHandle<()>>,
    magic: u32,
}

impl Drop for Peer {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Builds one wire frame.
fn frame(magic: u32, command: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(24 + payload.len());
    out.extend_from_slice(&magic.to_le_bytes());
    let mut cmd = [0u8; 12];
    cmd[..command.len()].copy_from_slice(command.as_bytes());
    out.extend_from_slice(&cmd);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    let check = sha256d::Hash::hash(payload).to_byte_array();
    out.extend_from_slice(&check[..4]);
    out.extend_from_slice(payload);
    out
}

fn encode_inv(items: &[InvItem]) -> Vec<u8> {
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

fn decode_inv(payload: &[u8]) -> Option<Vec<InvItem>> {
    let mut r = payload;
    let n = VarInt::consensus_decode(&mut r).ok()?.0 as usize;
    if n > MAX_INV || r.len() != n * 36 {
        return None;
    }
    Some(
        r.chunks_exact(36)
            .map(|c| InvItem {
                kind: u32::from_le_bytes(c[..4].try_into().expect("4 bytes")),
                hash: c[4..].try_into().expect("32 bytes"),
            })
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
/// vector.
pub fn object_fetch_filter() -> Vec<u8> {
    let mut buf = Vec::with_capacity(10);
    Vec::<u8>::new().consensus_encode(&mut buf).expect("Vec write");
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.push(0);
    buf
}

/// A nonce for the version message (detects connecting to ourselves).
fn nonce(addr: &str) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let h = sha256d::Hash::hash(format!("{addr}{now}").as_bytes()).to_byte_array();
    u64::from_le_bytes(h[..8].try_into().expect("8 bytes"))
}

impl Peer {
    /// Connects over TCP and completes the version handshake.
    pub async fn connect(
        id: usize,
        addr: SocketAddr,
        cfg: &PeerConfig,
        events: mpsc::UnboundedSender<(usize, PeerEvent)>,
    ) -> Result<Peer, NetError> {
        let stream = tokio::time::timeout(cfg.connect_timeout, tokio::net::TcpStream::connect(addr))
            .await
            .map_err(|_| NetError::Connect(addr, "timed out".into()))?
            .map_err(|e| NetError::Connect(addr, e.to_string()))?;
        let _ = stream.set_nodelay(true);
        Self::handshake(id, addr.to_string(), Some(addr), stream, cfg, events).await
    }

    /// Runs the handshake over any byte stream (tests use an in-memory
    /// pipe).
    pub async fn handshake<S>(
        id: usize,
        label: String,
        addr: Option<SocketAddr>,
        stream: S,
        cfg: &PeerConfig,
        events: mpsc::UnboundedSender<(usize, PeerEvent)>,
    ) -> Result<Peer, NetError>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let magic = cfg.network.magic();
        let (mut rd, mut wr) = tokio::io::split(stream);
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let writer = tokio::spawn(async move {
            while let Some(bytes) = out_rx.recv().await {
                if wr.write_all(&bytes).await.is_err() {
                    break;
                }
            }
            let _ = wr.shutdown().await;
        });

        let unspecified: SocketAddr = ([0, 0, 0, 0], 0).into();
        let receiver = NetAddress::new(&addr.unwrap_or(unspecified), ServiceFlags::NONE);
        let sender = NetAddress::new(&unspecified, ServiceFlags::NONE);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let version = VersionMessage::new(
            ServiceFlags::NONE,
            now,
            receiver,
            sender,
            nonce(&label),
            cfg.user_agent.clone(),
            cfg.start_height,
            false,
            [0u8; 32],
        );
        let mut payload = Vec::new();
        version
            .consensus_encode(&mut payload)
            .expect("writing to a Vec cannot fail");
        let _ = out_tx.send(frame(magic, "version", &payload));

        let bytes_received = Arc::new(AtomicU64::new(0));
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), String>>();
        let reader = {
            let out = out_tx.clone();
            let bytes = Arc::clone(&bytes_received);
            tokio::spawn(async move {
                let reason = read_loop(&mut rd, magic, id, &out, &bytes, &events, ready_tx).await;
                let _ = events.send((id, PeerEvent::Closed(reason)));
            })
        };
        let peer = Peer {
            id,
            addr: label.clone(),
            out: out_tx,
            bytes_received,
            tasks: vec![reader, writer],
            magic,
        };
        match tokio::time::timeout(cfg.handshake_timeout, ready_rx).await {
            Ok(Ok(Ok(()))) => Ok(peer),
            Ok(Ok(Err(e))) => Err(NetError::Handshake(label, e)),
            Ok(Err(_)) => Err(NetError::Handshake(label, "connection closed".into())),
            Err(_) => Err(NetError::Handshake(label, "timed out".into())),
        }
    }

    pub fn bytes_received(&self) -> u64 {
        self.bytes_received.load(Ordering::Relaxed)
    }

    /// Whether the writer is still running.
    pub fn is_open(&self) -> bool {
        !self.out.is_closed()
    }

    fn send(&self, command: &str, payload: &[u8]) {
        let _ = self.out.send(frame(self.magic, command, payload));
    }

    /// `govsync` for every object (`hash` zero, empty filter) or for the
    /// votes of one object (`hash`, [`empty_vote_filter`]).
    pub fn send_govsync(&self, hash: [u8; 32], filter: &[u8]) {
        let mut payload = Vec::with_capacity(32 + filter.len());
        payload.extend_from_slice(&hash);
        payload.extend_from_slice(filter);
        self.send(commands::MNGOVERNANCESYNC, &payload);
    }

    pub fn send_getdata(&self, items: &[InvItem]) {
        for chunk in items.chunks(MAX_INV) {
            self.send("getdata", &encode_inv(chunk));
        }
    }

    pub fn send_inv(&self, items: &[InvItem]) {
        for chunk in items.chunks(MAX_INV) {
            self.send("inv", &encode_inv(chunk));
        }
    }

    pub fn send_object(&self, payload: &[u8]) {
        self.send(commands::MNGOVERNANCEOBJECT, payload);
    }

    pub fn send_vote(&self, payload: &[u8]) {
        self.send(commands::MNGOVERNANCEOBJECTVOTE, payload);
    }
}

async fn read_exact_counted<R: AsyncRead + Unpin>(
    rd: &mut R,
    buf: &mut [u8],
    bytes: &AtomicU64,
) -> std::io::Result<()> {
    rd.read_exact(buf).await?;
    bytes.fetch_add(buf.len() as u64, Ordering::Relaxed);
    Ok(())
}

/// Reads frames until the connection ends; returns why.
async fn read_loop<R: AsyncRead + Unpin>(
    rd: &mut R,
    magic: u32,
    id: usize,
    out: &mpsc::UnboundedSender<Vec<u8>>,
    bytes: &AtomicU64,
    events: &mpsc::UnboundedSender<(usize, PeerEvent)>,
    ready: oneshot::Sender<Result<(), String>>,
) -> String {
    let mut ready = Some(ready);
    let (mut got_version, mut got_verack) = (false, false);
    let fail = |ready: &mut Option<oneshot::Sender<Result<(), String>>>, why: String| {
        if let Some(r) = ready.take() {
            let _ = r.send(Err(why.clone()));
        }
        why
    };
    loop {
        let mut header = [0u8; 24];
        if let Err(e) = read_exact_counted(rd, &mut header, bytes).await {
            return fail(&mut ready, format!("read: {e}"));
        }
        if u32::from_le_bytes(header[..4].try_into().expect("4 bytes")) != magic {
            return fail(&mut ready, "wrong network magic".into());
        }
        let command = String::from_utf8_lossy(&header[4..16])
            .trim_end_matches('\0')
            .to_string();
        let len = u32::from_le_bytes(header[16..20].try_into().expect("4 bytes")) as usize;
        if len > MAX_PAYLOAD {
            return fail(&mut ready, format!("{command} payload of {len} bytes"));
        }
        let mut payload = vec![0u8; len];
        if let Err(e) = read_exact_counted(rd, &mut payload, bytes).await {
            return fail(&mut ready, format!("read: {e}"));
        }
        if sha256d::Hash::hash(&payload).to_byte_array()[..4] != header[20..24] {
            return fail(&mut ready, format!("{command} checksum mismatch"));
        }
        let event = match command.as_str() {
            "version" => {
                let mut r = payload.as_slice();
                if VersionMessage::consensus_decode(&mut r).is_err() {
                    return fail(&mut ready, "undecodable version".into());
                }
                got_version = true;
                let _ = out.send(frame(magic, "verack", &[]));
                None
            }
            "verack" => {
                got_verack = true;
                None
            }
            "ping" => {
                let _ = out.send(frame(magic, "pong", &payload));
                None
            }
            "inv" => decode_inv(&payload)
                .map(|items| {
                    items
                        .into_iter()
                        .filter(|i| matches!(i.kind, INV_GOVERNANCE_OBJECT | INV_GOVERNANCE_VOTE))
                        .collect::<Vec<_>>()
                })
                .filter(|items| !items.is_empty())
                .map(PeerEvent::Inv),
            "getdata" => decode_inv(&payload)
                .map(|items| {
                    items
                        .into_iter()
                        .filter(|i| matches!(i.kind, INV_GOVERNANCE_OBJECT | INV_GOVERNANCE_VOTE))
                        .collect::<Vec<_>>()
                })
                .filter(|items| !items.is_empty())
                .map(PeerEvent::GetData),
            c if c == commands::MNGOVERNANCEOBJECT => Some(PeerEvent::Object(payload)),
            c if c == commands::MNGOVERNANCEOBJECTVOTE => Some(PeerEvent::Vote(payload)),
            c if c == commands::SYNCSTATUSCOUNT && payload.len() == 8 => Some(PeerEvent::SyncCount {
                item: i32::from_le_bytes(payload[..4].try_into().expect("4 bytes")),
                count: i32::from_le_bytes(payload[4..].try_into().expect("4 bytes")),
            }),
            _ => None,
        };
        if got_version
            && got_verack
            && let Some(r) = ready.take()
        {
            let _ = r.send(Ok(()));
        }
        if let Some(ev) = event
            && events.send((id, ev)).is_err()
        {
            return "event receiver gone".into();
        }
    }
}

/// An in-process peer for tests: answers the handshake and hands every
/// frame it receives to the test as `(command, payload)`.
#[cfg(any(test, feature = "test-peer"))]
pub mod test_peer {
    use super::*;

    pub struct FakePeer {
        pub received: mpsc::UnboundedReceiver<(String, Vec<u8>)>,
        pub send: mpsc::UnboundedSender<(String, Vec<u8>)>,
    }

    /// Serves one side of `stream` as a full node would for the handshake.
    pub fn spawn<S>(stream: S, network: Network) -> FakePeer
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let magic = network.magic();
        let (mut rd, mut wr) = tokio::io::split(stream);
        let (rx_tx, received) = mpsc::unbounded_channel();
        let (send, mut send_rx) = mpsc::unbounded_channel::<(String, Vec<u8>)>();
        tokio::spawn(async move {
            while let Some((cmd, payload)) = send_rx.recv().await {
                if wr.write_all(&frame(magic, &cmd, &payload)).await.is_err() {
                    break;
                }
            }
        });
        let reply = send.clone();
        tokio::spawn(async move {
            loop {
                let mut header = [0u8; 24];
                if rd.read_exact(&mut header).await.is_err() {
                    break;
                }
                let cmd = String::from_utf8_lossy(&header[4..16])
                    .trim_end_matches('\0')
                    .to_string();
                let len = u32::from_le_bytes(header[16..20].try_into().unwrap()) as usize;
                let mut payload = vec![0u8; len];
                if rd.read_exact(&mut payload).await.is_err() {
                    break;
                }
                if cmd == "version" {
                    let v = VersionMessage::new(
                        ServiceFlags::NETWORK,
                        0,
                        NetAddress::new(&([0, 0, 0, 0], 0).into(), ServiceFlags::NONE),
                        NetAddress::new(&([0, 0, 0, 0], 0).into(), ServiceFlags::NONE),
                        1,
                        "/fake:1/".into(),
                        0,
                        true,
                        [0; 32],
                    );
                    let mut p = Vec::new();
                    v.consensus_encode(&mut p).unwrap();
                    let _ = reply.send(("version".into(), p));
                    let _ = reply.send(("verack".into(), Vec::new()));
                }
                if rx_tx.send((cmd, payload)).is_err() {
                    break;
                }
            }
        });
        FakePeer { received, send }
    }

    pub fn inv_payload(items: &[InvItem]) -> Vec<u8> {
        encode_inv(items)
    }

    pub fn parse_inv(payload: &[u8]) -> Vec<InvItem> {
        decode_inv(payload).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handshake_ping_and_governance_frames() {
        let (a, b) = tokio::io::duplex(1 << 16);
        let mut fake = test_peer::spawn(b, Network::Regtest);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let cfg = PeerConfig::new(Network::Regtest, "/dwd-test/", 7);
        let peer = Peer::handshake(3, "fake".into(), None, a, &cfg, tx).await.unwrap();
        let (cmd, _) = fake.received.recv().await.unwrap();
        assert_eq!(cmd, "version");
        let (cmd, _) = fake.received.recv().await.unwrap();
        assert_eq!(cmd, "verack");

        fake.send.send(("ping".into(), 42u64.to_le_bytes().to_vec())).unwrap();
        let (cmd, payload) = fake.received.recv().await.unwrap();
        assert_eq!((cmd.as_str(), payload), ("pong", 42u64.to_le_bytes().to_vec()));

        peer.send_govsync([0; 32], &object_fetch_filter());
        let (cmd, payload) = fake.received.recv().await.unwrap();
        assert_eq!(cmd, "govsync");
        assert_eq!(payload.len(), 32 + 10);

        let items = [
            InvItem { kind: INV_GOVERNANCE_OBJECT, hash: [1; 32] },
            InvItem { kind: 1, hash: [2; 32] },
        ];
        fake.send.send(("inv".into(), test_peer::inv_payload(&items))).unwrap();
        let mut ssc = 10i32.to_le_bytes().to_vec();
        ssc.extend_from_slice(&5i32.to_le_bytes());
        fake.send.send(("ssc".into(), ssc)).unwrap();
        assert_eq!(rx.recv().await.unwrap(), (3, PeerEvent::Inv(vec![items[0]])));
        assert_eq!(
            rx.recv().await.unwrap(),
            (3, PeerEvent::SyncCount { item: 10, count: 5 })
        );
        assert!(peer.bytes_received() > 0);
        drop(fake);
    }

    #[test]
    fn filters_have_core_layout() {
        assert_eq!(empty_vote_filter(), vec![1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(object_fetch_filter(), vec![0; 10]);
    }
}
