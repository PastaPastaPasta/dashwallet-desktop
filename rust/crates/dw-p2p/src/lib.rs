//! Minimal outbound Dash P2P sessions (DESIGN-opus §1.4 `dw-p2p`).
//!
//! dash-spv owns the block/filter/masternode-list sync and its peer pool.
//! CoinJoin needs its own connections: it talks to the session masternode
//! (`dsa`/`dsi`/`dss`/…) and listens for `dsq` queues. DESIGN.md R2 chose our
//! own crate over an upstream tap into the dash-spv pool.
//!
//! Owner: R1. Consumer: `dw-coinjoin` through the engine.
//!
//! Modules:
//! - [`commands`]: the wire command names this crate passes through.
//! - [`codec`]: message framing; payloads stay bytes.
//! - [`session`]: one outbound TCP session (version/verack handshake with
//!   optional `senddsq`, ping/pong, reader/writer tasks, send rate limit,
//!   clean close).
//! - [`peers`]: picking masternodes/full nodes from a masternode-list
//!   snapshot, with the "recently used" ring CoinJoin needs.
//! - [`testing`]: an in-process mock peer for tests.
//!
//! Not built: `proxy` (SOCKS5) waits for SPV proxy support (U1); until then
//! these sessions connect directly, as dash-spv does.
//!
//! Public API (docs/contracts/m3-engine.md §6):
//! [`Session::connect`]`(addr, network, SessionConfig)`, [`Session::send`]`(command,
//! payload)`, [`Session::subscribe`]`(commands)` → a receiver of
//! [`Message`]`{ command, payload }`, [`Session::close`], and
//! [`PeerPicker`]`::masternodes` over a snapshot of the list.

pub mod codec;
pub mod commands;
pub mod peers;
pub mod session;
pub mod testing;

pub use peers::{PeerEntry, PeerPicker};
pub use session::{
    COINJOIN_PROTOCOL_VERSION, MIN_PEER_PROTO_VERSION, Message, P2pError, PeerVersion, Session,
    SessionConfig, serialize,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockPeer;
    use dashcore::Network;
    use std::time::Duration;

    #[tokio::test]
    async fn handshake_send_subscribe_and_close() {
        let peer = MockPeer::bind(Network::Regtest).await.unwrap();
        let addr = peer.addr();
        let server = tokio::spawn(async move {
            let mut conn = peer.accept().await.unwrap();
            assert_eq!(conn.client_version.version, COINJOIN_PROTOCOL_VERSION);
            // senddsq true right after the handshake.
            let dsq = conn.recv().await.unwrap();
            assert_eq!(dsq.command, commands::SENDDSQUEUE);
            assert_eq!(dsq.payload, vec![1]);
            let dsa = conn.recv().await.unwrap();
            assert_eq!(dsa.command, "dsa");
            conn.send("ping", &42u64.to_le_bytes()).await.unwrap();
            conn.send("dssu", &[9, 9]).await.unwrap();
            // The client answers the ping without the subscriber seeing it.
            let pong = conn.recv_command("pong").await.unwrap();
            assert_eq!(pong.payload, 42u64.to_le_bytes());
            conn
        });
        let config = SessionConfig {
            want_dsq: true,
            ..SessionConfig::default()
        };
        let session = Session::connect(addr, Network::Regtest, config)
            .await
            .unwrap();
        assert_eq!(session.peer_version().version, testing::MOCK_PEER_VERSION);
        assert_eq!(session.common_version(), COINJOIN_PROTOCOL_VERSION);
        let mut rx = session.subscribe(&["dssu"]);
        session.send("dsa", vec![1, 2, 3]).await.unwrap();
        let got = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.command, "dssu");
        assert_eq!(got.payload, vec![9, 9]);
        let conn = server.await.unwrap();
        session.close();
        assert!(session.is_closed());
        assert!(rx.recv().await.is_none());
        assert!(matches!(
            session.send("dsa", vec![]).await,
            Err(P2pError::Closed(_))
        ));
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn peer_close_ends_subscriptions() {
        let peer = MockPeer::bind(Network::Regtest).await.unwrap();
        let addr = peer.addr();
        let server = tokio::spawn(async move { peer.accept().await.unwrap() });
        let session = Session::connect(addr, Network::Regtest, SessionConfig::default())
            .await
            .unwrap();
        let mut rx = session.subscribe(&[]);
        server.await.unwrap().shutdown().await;
        tokio::time::timeout(Duration::from_secs(5), session.closed())
            .await
            .unwrap();
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn refuses_old_peers_and_wrong_networks() {
        let peer = MockPeer::bind_with_version(Network::Regtest, 70220)
            .await
            .unwrap();
        let addr = peer.addr();
        tokio::spawn(async move {
            let _ = peer.accept().await;
        });
        let err = Session::connect(addr, Network::Regtest, SessionConfig::default())
            .await
            .unwrap_err();
        assert!(matches!(err, P2pError::PeerTooOld { version: 70220, .. }));

        let peer = MockPeer::bind(Network::Testnet).await.unwrap();
        let addr = peer.addr();
        tokio::spawn(async move {
            let _ = peer.accept().await;
        });
        let config = SessionConfig {
            handshake_timeout: Duration::from_secs(2),
            ..SessionConfig::default()
        };
        let err = Session::connect(addr, Network::Regtest, config)
            .await
            .unwrap_err();
        assert!(matches!(err, P2pError::Handshake { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn connect_failure_is_reported() {
        // Bind and drop to get a port nothing listens on.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let err = Session::connect(addr, Network::Regtest, SessionConfig::default())
            .await
            .unwrap_err();
        assert!(matches!(err, P2pError::Connect { .. }));
    }
}
