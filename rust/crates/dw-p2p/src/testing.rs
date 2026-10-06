//! An in-process mock peer for tests of this crate and its consumers
//! (CoinJoin and governance sessions): it listens on loopback, answers the
//! handshake like Dash Core, and lets the test read and write raw messages.

use std::net::SocketAddr;

use dashcore::Network;
use dashcore::consensus::{Decodable, Encodable};
use dashcore::network::address::Address as NetAddress;
use dashcore::network::constants::ServiceFlags;
use dashcore::network::message_network::VersionMessage;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::codec::{self, HEADER_LEN};
use crate::session::Message;

/// Version the mock peer announces (Dash Core v24, `src/version.h:14`).
pub const MOCK_PEER_VERSION: u32 = 70242;

/// A listening mock peer.
pub struct MockPeer {
    listener: TcpListener,
    magic: u32,
    version: u32,
}

/// One accepted connection after the handshake.
pub struct MockConn {
    stream: TcpStream,
    magic: u32,
    /// The `version` the client sent.
    pub client_version: VersionMessage,
}

impl MockPeer {
    pub async fn bind(network: Network) -> std::io::Result<Self> {
        Self::bind_with_version(network, MOCK_PEER_VERSION).await
    }

    /// A mock peer announcing `version` (to test the client's minimum).
    pub async fn bind_with_version(network: Network, version: u32) -> std::io::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind("127.0.0.1:0").await?,
            magic: network.magic(),
            version,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.listener.local_addr().expect("bound listener")
    }

    /// Accepts one connection and answers its handshake: reads `version`,
    /// sends `version` and `verack`, reads the client's `verack`.
    pub async fn accept(&self) -> std::io::Result<MockConn> {
        let (stream, _) = self.listener.accept().await?;
        let mut conn = MockConn {
            stream,
            magic: self.magic,
            client_version: VersionMessage::new(
                ServiceFlags::NONE,
                0,
                NetAddress::new(&self.addr(), ServiceFlags::NONE),
                NetAddress::new(&self.addr(), ServiceFlags::NONE),
                0,
                String::new(),
                0,
                false,
                [0; 32],
            ),
        };
        let first = conn.recv_raw().await?;
        if first.command != "version" {
            return Err(std::io::Error::other(format!(
                "expected version, got {}",
                first.command
            )));
        }
        conn.client_version = VersionMessage::consensus_decode(&mut &first.payload[..])
            .map_err(std::io::Error::other)?;
        let mut ours = VersionMessage::new(
            ServiceFlags::NETWORK,
            0,
            NetAddress::new(&self.addr(), ServiceFlags::NONE),
            NetAddress::new(&self.addr(), ServiceFlags::NETWORK),
            7,
            "/mock-dashd:24.0.0/".into(),
            100,
            true,
            [0; 32],
        );
        ours.version = self.version;
        let mut payload = Vec::new();
        ours.consensus_encode(&mut payload)?;
        conn.send("version", &payload).await?;
        // Dash Core sends feature negotiation before verack.
        conn.send("sendaddrv2", &[]).await?;
        conn.send("verack", &[]).await?;
        let ack = conn.recv_raw().await?;
        if ack.command != "verack" {
            return Err(std::io::Error::other(format!(
                "expected verack, got {}",
                ack.command
            )));
        }
        Ok(conn)
    }
}

impl MockConn {
    async fn recv_raw(&mut self) -> std::io::Result<Message> {
        let mut raw = [0u8; HEADER_LEN];
        self.stream.read_exact(&mut raw).await?;
        let header = codec::decode_header(self.magic, &raw).map_err(std::io::Error::other)?;
        let mut payload = vec![0u8; header.length];
        self.stream.read_exact(&mut payload).await?;
        codec::verify_payload(&header, &payload).map_err(std::io::Error::other)?;
        Ok(Message {
            command: header.command,
            payload,
        })
    }

    /// The next message from the client, answering pings on the way.
    pub async fn recv(&mut self) -> std::io::Result<Message> {
        loop {
            let msg = self.recv_raw().await?;
            if msg.command == "ping" {
                let nonce = msg.payload.clone();
                self.send("pong", &nonce).await?;
                continue;
            }
            return Ok(msg);
        }
    }

    /// The next message with `command`, skipping others.
    pub async fn recv_command(&mut self, command: &str) -> std::io::Result<Message> {
        loop {
            let msg = self.recv().await?;
            if msg.command == command {
                return Ok(msg);
            }
        }
    }

    pub async fn send(&mut self, command: &str, payload: &[u8]) -> std::io::Result<()> {
        let framed = codec::encode(self.magic, command, payload).map_err(std::io::Error::other)?;
        self.stream.write_all(&framed).await
    }

    /// Closes the connection.
    pub async fn shutdown(mut self) {
        let _ = self.stream.shutdown().await;
    }
}
