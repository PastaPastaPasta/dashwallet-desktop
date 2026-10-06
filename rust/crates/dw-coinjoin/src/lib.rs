//! CoinJoin client (DESIGN-opus §1.4 `dw-coinjoin`), ported from Dash Core
//! `src/coinjoin/{client,common,options,util}.cpp` with dashj (Dash Android)
//! as the SPV reference. Mixing outputs use the DIP9 CoinJoin account
//! `m/9'/coin'/4'/…` (DESIGN.md R2).
//!
//! Owner: R1. The engine integration (per-wallet mixing state, the
//! `CoinJoin` event, the FFI calls of docs/contracts/m3-engine.md §2.1) lives
//! in `dw-engine/src/coinjoin.rs`.
//!
//! Module layout:
//! - [`denoms`]: denominations, collateral bounds, minimum balance (works).
//! - [`settings`]: Options → CoinJoin values, defaults and ranges (works).
//! - [`status`]: the session status codes and pool messages (works).
//! - `rounds`: rounds per outpoint by walking the input chain (Core
//!   `GetRealOutpointCoinJoinRounds`) and the "fully mixed" rule with the
//!   per-wallet salt (`SHA256(outpoint ‖ salt)` odd, QT-043).
//! - `progress`: dash-qt's progress formula and tooltip parts (QT-042).
//! - `planner`: create-denominations / make-collaterals transactions, the
//!   denominations goal and hard cap, post-V24 promotion/demotion.
//! - `messages`: `dsa`/`dsq`/`dsi`/`dsf`/`dss`/`dsc`/`dssu`/`dstx` codecs.
//! - `queue`: `dsq` handling with operator BLS signature checks against the
//!   masternode list.
//! - `session`: one mixing session's state machine (Core `PoolState`), 30 s
//!   queue and 15 s signing timeouts.
//! - `client`: the per-wallet manager (multi-session, masternode selection,
//!   the status code of [`status`]).
//! - `recovery`: IOS-057 recovery scan and "move mixed coins" chunking.

pub mod denoms;
pub mod settings;
pub mod status;
