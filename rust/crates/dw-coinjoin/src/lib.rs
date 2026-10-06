//! CoinJoin client (DESIGN-opus §1.4 `dw-coinjoin`), ported from Dash Core
//! `src/coinjoin/{client,common,options,util}.cpp` and
//! `src/wallet/coinjoin.cpp` with dashj (Dash Android) as the SPV
//! reference. Mixing outputs use the DIP9 CoinJoin account
//! `m/9'/coin'/4'/…` (DESIGN.md R2).
//!
//! Owner: R1. The engine integration (per-wallet mixing state, the
//! `CoinJoin` event, the FFI calls of docs/contracts/m3-engine.md §2.1) lives
//! in `dw-engine/src/coinjoin.rs`; it implements [`client::MixingWallet`].
//!
//! Modules:
//! - [`denoms`]: denominations, collateral bounds, minimum balance.
//! - [`settings`]: Options → CoinJoin values, defaults and ranges.
//! - [`status`]: session status codes and pool messages.
//! - [`messages`]: `dsa`/`dsq`/`dsi`/`dsf`/`dss`/`dsc`/`dssu`/`dstx` codecs.
//! - [`bls`]: operator signature checks of `dsq`/`dstx`.
//! - [`rounds`]: rounds per outpoint by walking the input chain and the
//!   "fully mixed" rule with the per-wallet salt (QT-043).
//! - [`progress`]: dash-qt's progress formula and tooltip parts (QT-042).
//! - [`planner`]: create-denominations / make-collaterals amounts and the
//!   collateral transaction's change.
//! - [`queue`]: known `dsq` queues and per-masternode queue counters.
//! - [`client`]: one session's protocol run against a masternode.
//! - [`recovery`]: IOS-057 "move mixed coins" chunking.
//!
//! Not built: post-V24 promotion/demotion entries (the client only joins
//! standard 1:1 sessions; it signs final transactions that contain other
//! participants' rebalance entries, as Core does).

pub mod bls;
pub mod client;
pub mod denoms;
pub mod messages;
pub mod planner;
pub mod progress;
pub mod queue;
pub mod recovery;
pub mod rounds;
pub mod settings;
pub mod status;
