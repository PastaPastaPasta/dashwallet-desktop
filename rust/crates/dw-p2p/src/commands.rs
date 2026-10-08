//! Wire command names (Dash Core `src/protocol.cpp`) that CoinJoin sends or
//! receives. dashcore's `NetworkMessage` types `senddsq`;
//! the rest are carried as `Unknown { command, payload }` by this crate.

/// `CCoinJoinAccept`: the client asks a masternode to start or join a
/// session for one denomination, with a collateral transaction.
pub const DSACCEPT: &str = "dsa";
/// `CCoinJoinQueue`: a masternode announces a queue (operator BLS signed).
pub const DSQUEUE: &str = "dsq";
/// `CCoinJoinEntry`: the client's inputs, collateral and outputs.
pub const DSVIN: &str = "dsi";
/// Session id and the final transaction to sign.
pub const DSFINALTX: &str = "dsf";
/// The client's signed inputs.
pub const DSSIGNFINALTX: &str = "dss";
/// Session completion (session id, message id).
pub const DSCOMPLETE: &str = "dsc";
/// Session status update (session id, state, status, message id).
pub const DSSTATUSUPDATE: &str = "dssu";
/// A broadcast mixing transaction (tx, masternode outpoint, BLS sig, time).
pub const DSTX: &str = "dstx";
/// Ask the peer to (not) relay `dsq` messages.
pub const SENDDSQUEUE: &str = "senddsq";
