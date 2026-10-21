//! Masternode provider transactions (DESIGN-opus §1.4 `dw-protx`): the
//! owner-side builders and payload signing dash-qt's Masternodes tab uses
//! (Dash Core `src/evo/providertx*.cpp`, `src/qt/masternode*.cpp`,
//! `src/qt/sharedmn*.cpp`) and the v24 shared-masternode codecs.
//!
//! Salvaged from `m3/r3-protx` (MG-01, docs/design/MG-01-salvage.md). The
//! wallet never holds an operator secret: there is no BLS secret, no
//! ProUpServTx or ProUpRevTx builder, and `tests/no_operator_signing.rs`
//! fails if one appears anywhere in the workspace. The engine integration
//! (list model, owned detection, collateral resolution per D43) is MG-03's;
//! platform-wallet's masternode code is preferred where the two overlap
//! (docs/design/MG-01-salvage.md §5).
//!
//! Module layout:
//! - [`params`]: collateral amounts, default ports, share limits.
//! - [`service`]: Core P2P service and Platform port rules of version-2
//!   payloads (`MnNetInfo`, `CheckProviderNetworkFields`).
//! - [`bls`]: operator public-key parsing and both serializations.
//! - [`payloads`]: placeholders and finalizers of ProRegTx (fund new /
//!   existing collateral with the `payout|reward|owner|voting|payloadHash`
//!   message) and ProUpRegTx (owner-signed), and collateral resolution.
//! - [`rules`]: Core's checks of those payloads, with its reject reasons
//!   (`IsTriviallyValid`, `CheckProRegTx`, `CheckProUpRegTx`), which the
//!   engine runs before it signs.
//! - [`shared`]: v24 shared masternodes — codecs of ProDisTx (type 10),
//!   ProUpShareTx (11), ProUpSharedRegTx (12) and the share table; absent
//!   from rust-dashcore at the pin. The shared session protocol (envelope,
//!   coordinator/participant state machines, the version-3 ProRegTx with
//!   extended network info) is not written yet.

pub mod bls;
pub mod params;
pub mod payloads;
pub mod rules;
pub mod service;
pub mod shared;
