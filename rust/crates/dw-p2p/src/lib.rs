//! Minimal outbound Dash P2P sessions (DESIGN-opus §1.4 `dw-p2p`).
//!
//! dash-spv owns the block/filter/masternode-list sync and its peer pool.
//! CoinJoin and governance need their own connections: CoinJoin talks to the
//! session masternode (`dsa`/`dsi`/`dss`/…) and governance pulls objects and
//! votes from full-node peers (`govsync`). DESIGN.md R2 chose our own crate
//! over an upstream tap into the dash-spv pool.
//!
//! Owner: R1. Consumers: `dw-coinjoin` (R1) and `dw-governance` (R2).
//!
//! Module layout (each is R1's to implement):
//! - [`commands`]: the wire command names this crate passes through.
//! - `codec`: `RawNetworkMessage` framing from `dashcore`; messages without a
//!   typed `NetworkMessage` variant travel as `Unknown { command, payload }`.
//! - `session`: one outbound TCP session (version/verack handshake with
//!   `senddsq`, ping/pong, read/write tasks, rate limiting, clean close).
//! - `peers`: picking masternodes/full nodes from the SPV masternode list
//!   (service addresses, the "recently used" ring CoinJoin needs).
//! - `proxy`: SOCKS5 (only once the SPV client supports a proxy, U1).
//!
//! Planned public API (the contract R2 relies on, docs/contracts/m3-engine.md
//! §6): `Session::connect(addr, network, SessionConfig) -> Session`,
//! `Session::send(command, payload)`, `Session::subscribe(commands) ->
//! receiver of (command, payload)`, `Session::close()`, and
//! `PeerPicker::{masternodes, full_nodes}` over a snapshot of the list.

pub mod commands;
