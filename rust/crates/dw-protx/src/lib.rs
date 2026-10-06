//! Masternode provider transactions (DESIGN-opus §1.4 `dw-protx`): the
//! builders and payload signing dash-qt's Masternodes tab uses (Dash Core
//! `src/evo/providertx*.cpp`, `src/qt/masternode*.cpp`,
//! `src/qt/sharedmn*.cpp`), the v24 shared-masternode session protocol, and
//! the masternode keychain (owner / voting / operator BLS / platform
//! ed25519 keys on their DIP3/DIP9 paths).
//!
//! Owner: R3. The engine integration (list model from the SPV masternode
//! list, owned detection, tracked masternodes, the `Masternodes` event and
//! the FFI calls of docs/contracts/m3-engine.md §2.3–2.5) lives in
//! `dw-engine/src/masternodes.rs`. platform-wallet already has the list
//! summaries, wallet aggregation, tracked masternodes, revive (ProUpServTx)
//! and evonode withdrawal; this crate adds what it lacks.
//!
//! Module layout:
//! - [`params`]: collateral amounts, default ports, share limits.
//! - [`service`]: Core P2P service and Platform port rules of version-2
//!   payloads (`MnNetInfo`, `CheckProviderNetworkFields`).
//! - [`bls`]: operator key generation, parsing, both serializations, and
//!   basic-scheme payload signatures.
//! - [`payloads`]: placeholders and finalizers of ProRegTx (fund new /
//!   existing collateral with the `payout|reward|owner|voting|payloadHash`
//!   message), ProUpServTx, ProUpRegTx and ProUpRevTx.
//! - [`keychain`]: DIP3 provider key paths, Tenderdash key form.
//! - [`shared`]: v24 shared masternodes — codecs of ProDisTx (type 10),
//!   ProUpShareTx (11), ProUpSharedRegTx (12) and the share table; absent
//!   from rust-dashcore at the pin. The shared session protocol (envelope,
//!   coordinator/participant state machines, the version-3 ProRegTx with
//!   extended network info) is not written yet.

pub mod bls;
pub mod keychain;
pub mod params;
pub mod payloads;
pub mod service;
pub mod shared;
