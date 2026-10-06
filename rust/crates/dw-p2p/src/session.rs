//! One outbound P2P session: TCP connect, the `version`/`verack` handshake
//! (plus `senddsq` when the caller wants CoinJoin queues), ping/pong, a
//! reader task that fans messages out to subscribers, a rate-limited writer
//! task, and a clean close.
//!
//! Handshake as Dash Core's `PeerManagerImpl::ProcessMessage`
//! (`src/net_processing.cpp`): each side sends `version`, answers the
//! other's with `verack`; nothing else is processed before that. `ping`
//! (`src/net_processing.cpp` "ping") is answered with `pong` carrying the
//! same nonce, here inside the reader so subscribers never see it.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashcore::Network;
use dashcore::consensus::{Decodable, Encodable};
use dashcore::network::address::Address as NetAddress;
use dashcore::network::constants::ServiceFlags;
use dashcore::network::message_network::VersionMessage;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::codec::{self, HEADER_LEN};
use crate::commands;

/// The protocol version CoinJoin sessions announce: one below Dash Core's
/// `DSQ_INV_VERSION` (70234, `src/version.h:47`), so a peer pushes `dsq`
/// messages directly instead of announcing them by `inv`
/// (`src/net_processing.cpp` SENDDSQUEUE handler: `WantsDSQ::ALL` below
/// 70234), and below `COINJOIN_REBALANCE_VERSION` (70241, `version.h:73`),
/// so `dsa` carries no flags byte (`CCoinJoinAccept` serialization,
/// `src/coinjoin/coinjoin.h`). Standard (1:1) mixing needs neither.
pub const COINJOIN_PROTOCOL_VERSION: u32 = 70233;

/// Oldest peer Dash Core v24 talks to (`MIN_PEER_PROTO_VERSION`,
/// `src/version.h:20`); older peers are refused here too.
pub const MIN_PEER_PROTO_VERSION: u32 = 70221;

/// Session settings.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Version announced in `version`.
    pub protocol_version: u32,
    pub user_agent: String,
    /// Our best height, announced in `version`.
    pub start_height: i32,
    /// `fRelay`: whether the peer should announce transactions to us.
    pub relay: bool,
    /// Send `senddsq true` after the handshake (CoinJoin queue relay).
    pub want_dsq: bool,
    pub connect_timeout: Duration,
    pub handshake_timeout: Duration,
    /// How often a `ping` is sent while the session is open.
    pub ping_interval: Duration,
    /// Close when nothing arrived for this long (Dash Core
    /// `TIMEOUT_INTERVAL`, 20 minutes).
    pub idle_timeout: Duration,
    /// Outbound messages per second (token bucket, burst of the same size).
    pub send_rate_per_sec: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            protocol_version: COINJOIN_PROTOCOL_VERSION,
            user_agent: format!("/dw-p2p:{}/", env!("CARGO_PKG_VERSION")),
            start_height: 0,
            relay: false,
            want_dsq: false,
            connect_timeout: Duration::from_secs(10),
            handshake_timeout: Duration::from_secs(15),
            ping_interval: Duration::from_secs(120),
            idle_timeout: Duration::from_secs(20 * 60),
            send_rate_per_sec: 50,
        }
    }
}

/// Why a session could not be opened or used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum P2pError {
    #[error("connect to {addr} failed: {detail}")]
    Connect { addr: SocketAddr, detail: String },
    #[error("handshake with {addr} failed: {detail}")]
    Handshake { addr: SocketAddr, detail: String },
    #[error("peer {addr} speaks protocol {version}, below {min}")]
    PeerTooOld {
        addr: SocketAddr,
        version: u32,
        min: u32,
    },
    #[error("session with {0} is closed")]
    Closed(SocketAddr),
    #[error(transparent)]
    Codec(#[from] codec::CodecError),
}

/// One message received from the peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub command: String,
    pub payload: Vec<u8>,
}

/// What the peer said about itself in `version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerVersion {
    pub version: u32,
    pub services: u64,
    pub user_agent: String,
    pub start_height: i32,
}

struct Subscriber {
    /// Empty: every command.
    commands: Vec<String>,
    tx: mpsc::UnboundedSender<Message>,
}

struct Shared {
    subscribers: Mutex<Vec<Subscriber>>,
    closed: watch::Sender<bool>,
}

impl Shared {
    fn close(&self) {
        self.closed.send_replace(true);
        // Dropping the senders ends every subscriber's stream.
        self.subscribers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    fn dispatch(&self, msg: Message) {
        let mut subs = self.subscribers.lock().unwrap_or_else(|p| p.into_inner());
        subs.retain(|s| {
            if s.commands.is_empty() || s.commands.contains(&msg.command) {
                s.tx.send(msg.clone()).is_ok()
            } else {
                !s.tx.is_closed()
            }
        });
    }
}

/// An open session. Dropping it closes the connection.
pub struct Session {
    addr: SocketAddr,
    our_version: u32,
    peer: PeerVersion,
    magic: u32,
    out: mpsc::Sender<Vec<u8>>,
    shared: Arc<Shared>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("addr", &self.addr)
            .field("peer", &self.peer)
            .finish_non_exhaustive()
    }
}

async fn read_frame(
    reader: &mut OwnedReadHalf,
    magic: u32,
) -> Result<Message, Box<dyn std::error::Error + Send + Sync>> {
    let mut raw = [0u8; HEADER_LEN];
    reader.read_exact(&mut raw).await?;
    let header = codec::decode_header(magic, &raw)?;
    let mut payload = vec![0u8; header.length];
    reader.read_exact(&mut payload).await?;
    codec::verify_payload(&header, &payload)?;
    Ok(Message {
        command: header.command,
        payload,
    })
}

fn version_payload(addr: SocketAddr, config: &SessionConfig) -> Vec<u8> {
    let unspecified = SocketAddr::from(([0, 0, 0, 0], 0));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut msg = VersionMessage::new(
        ServiceFlags::NONE,
        now,
        NetAddress::new(&addr, ServiceFlags::NONE),
        NetAddress::new(&unspecified, ServiceFlags::NONE),
        rand::random(),
        config.user_agent.clone(),
        config.start_height,
        config.relay,
        [0u8; 32],
    );
    msg.version = config.protocol_version;
    let mut out = Vec::new();
    msg.consensus_encode(&mut out)
        .expect("writing to a Vec cannot fail");
    out
}

impl Session {
    /// Connects to `addr`, runs the handshake and starts the session's
    /// tasks. Must run inside a tokio runtime.
    pub async fn connect(
        addr: SocketAddr,
        network: Network,
        config: SessionConfig,
    ) -> Result<Session, P2pError> {
        let magic = network.magic();
        let stream = tokio::time::timeout(config.connect_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| P2pError::Connect {
                addr,
                detail: "timed out".into(),
            })?
            .map_err(|e| P2pError::Connect {
                addr,
                detail: e.to_string(),
            })?;
        let _ = stream.set_nodelay(true);
        let (mut reader, mut writer) = stream.into_split();
        let handshake_err = |detail: String| P2pError::Handshake { addr, detail };

        let hello = codec::encode(magic, "version", &version_payload(addr, &config))?;
        let peer = tokio::time::timeout(config.handshake_timeout, async {
            writer
                .write_all(&hello)
                .await
                .map_err(|e| handshake_err(e.to_string()))?;
            let mut peer: Option<PeerVersion> = None;
            let mut verack = false;
            while peer.is_none() || !verack {
                let msg = read_frame(&mut reader, magic)
                    .await
                    .map_err(|e| handshake_err(e.to_string()))?;
                match msg.command.as_str() {
                    "version" => {
                        let v = VersionMessage::consensus_decode(&mut &msg.payload[..])
                            .map_err(|e| handshake_err(format!("bad version message: {e}")))?;
                        if v.version < MIN_PEER_PROTO_VERSION {
                            return Err(P2pError::PeerTooOld {
                                addr,
                                version: v.version,
                                min: MIN_PEER_PROTO_VERSION,
                            });
                        }
                        peer = Some(PeerVersion {
                            version: v.version,
                            services: v.services.as_u64(),
                            user_agent: v.user_agent,
                            start_height: v.start_height,
                        });
                        let ack = codec::encode(magic, "verack", &[])?;
                        writer
                            .write_all(&ack)
                            .await
                            .map_err(|e| handshake_err(e.to_string()))?;
                    }
                    "verack" => verack = true,
                    // Feature negotiation (sendaddrv2, …) may precede verack.
                    _ => {}
                }
            }
            if config.want_dsq {
                let dsq = codec::encode(magic, commands::SENDDSQUEUE, &[1])?;
                writer
                    .write_all(&dsq)
                    .await
                    .map_err(|e| handshake_err(e.to_string()))?;
            }
            Ok::<_, P2pError>(peer.expect("loop ends with the peer's version"))
        })
        .await
        .map_err(|_| handshake_err("timed out".into()))??;

        let (closed, _) = watch::channel(false);
        let shared = Arc::new(Shared {
            subscribers: Mutex::new(Vec::new()),
            closed,
        });
        let (out, out_rx) = mpsc::channel::<Vec<u8>>(256);
        let tasks = vec![
            tokio::spawn(write_loop(
                writer,
                out_rx,
                config.send_rate_per_sec.max(1),
                Arc::clone(&shared),
            )),
            tokio::spawn(read_loop(
                reader,
                magic,
                out.clone(),
                config.idle_timeout,
                Arc::clone(&shared),
            )),
            tokio::spawn(ping_loop(
                magic,
                out.clone(),
                config.ping_interval,
                Arc::clone(&shared),
            )),
        ];
        tracing::debug!(%addr, version = peer.version, agent = %peer.user_agent, "p2p session open");
        Ok(Session {
            addr,
            our_version: config.protocol_version,
            peer,
            magic,
            out,
            shared,
            tasks: Mutex::new(tasks),
        })
    }

    pub fn peer_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn peer_version(&self) -> &PeerVersion {
        &self.peer
    }

    /// The version both sides serialize with (`CNode::GetCommonVersion`).
    pub fn common_version(&self) -> u32 {
        self.our_version.min(self.peer.version)
    }

    pub fn is_closed(&self) -> bool {
        *self.shared.closed.borrow()
    }

    /// Resolves once the session closed (peer, error or [`Self::close`]).
    pub async fn closed(&self) {
        let mut rx = self.shared.closed.subscribe();
        let _ = rx.wait_for(|c| *c).await;
    }

    /// Queues `payload` as `command`. Fails once the session is closed.
    pub async fn send(&self, command: &str, payload: Vec<u8>) -> Result<(), P2pError> {
        if self.is_closed() {
            return Err(P2pError::Closed(self.addr));
        }
        let framed = codec::encode(self.magic, command, &payload)?;
        self.out
            .send(framed)
            .await
            .map_err(|_| P2pError::Closed(self.addr))
    }

    /// Messages whose command is in `commands` (every message when empty),
    /// from now on, until the session closes (the stream then ends).
    pub fn subscribe(&self, commands: &[&str]) -> mpsc::UnboundedReceiver<Message> {
        let (tx, rx) = mpsc::unbounded_channel();
        if !self.is_closed() {
            self.shared
                .subscribers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(Subscriber {
                    commands: commands.iter().map(|c| c.to_string()).collect(),
                    tx,
                });
        }
        rx
    }

    /// Closes the connection and ends every subscription. Idempotent.
    pub fn close(&self) {
        self.shared.close();
        for task in self
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain(..)
        {
            task.abort();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

async fn write_loop(
    mut writer: OwnedWriteHalf,
    mut rx: mpsc::Receiver<Vec<u8>>,
    rate: u32,
    shared: Arc<Shared>,
) {
    let mut tokens = rate;
    let mut refill = tokio::time::Instant::now() + Duration::from_secs(1);
    while let Some(frame) = rx.recv().await {
        if tokens == 0 {
            tokio::time::sleep_until(refill).await;
        }
        let now = tokio::time::Instant::now();
        if now >= refill {
            tokens = rate;
            refill = now + Duration::from_secs(1);
        }
        tokens = tokens.saturating_sub(1);
        if let Err(e) = writer.write_all(&frame).await {
            tracing::debug!(error = %e, "p2p write failed; closing");
            break;
        }
    }
    let _ = writer.shutdown().await;
    shared.close();
}

async fn read_loop(
    mut reader: OwnedReadHalf,
    magic: u32,
    out: mpsc::Sender<Vec<u8>>,
    idle: Duration,
    shared: Arc<Shared>,
) {
    loop {
        let msg = match tokio::time::timeout(idle, read_frame(&mut reader, magic)).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(e)) => {
                tracing::debug!(error = %e, "p2p read ended; closing");
                break;
            }
            Err(_) => {
                tracing::debug!("p2p session idle; closing");
                break;
            }
        };
        if msg.command == "ping" {
            match codec::encode(magic, "pong", &msg.payload) {
                Ok(pong) => {
                    if out.send(pong).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
            continue;
        }
        shared.dispatch(msg);
    }
    shared.close();
}

async fn ping_loop(magic: u32, out: mpsc::Sender<Vec<u8>>, every: Duration, shared: Arc<Shared>) {
    let mut closed = shared.closed.subscribe();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(every) => {}
            _ = closed.wait_for(|c| *c) => return,
        }
        let nonce: u64 = rand::random();
        let Ok(ping) = codec::encode(magic, "ping", &nonce.to_le_bytes()) else {
            return;
        };
        if out.send(ping).await.is_err() {
            return;
        }
    }
}

/// Encodes a value in Dash Core's wire format.
pub fn serialize<T: Encodable + ?Sized>(value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    value
        .consensus_encode(&mut out)
        .expect("writing to a Vec cannot fail");
    out
}
