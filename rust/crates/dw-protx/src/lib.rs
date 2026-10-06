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
//! - [`params`]: collateral amounts, default ports, share limits (works).
//! - `register`: ProRegTx (fund new / existing UTXO / external collateral
//!   with the `payout|operatorReward|ownerAddr|votingAddr|payloadHash`
//!   message), v24 network-info lists and Platform fields.
//! - `update_service`, `update_registrar`, `revoke`: ProUpServTx,
//!   ProUpRegTx (only changed fields), ProUpRevTx (reason 0–3).
//! - `bls`: operator key generation and checks (basic scheme).
//! - `keychain`: provider key derivation per role and index, WIF, legacy
//!   BLS form, Tenderdash node id.
//! - `shared`: v24 shared masternodes — `payloads` (ProDissolveTx type 10,
//!   ProUpShareTx type 11, ProUpSharedRegTx type 12 and the share fields of
//!   ProRegTx; absent from rust-dashcore at the pin), `envelope`
//!   (`dash-shared-mn-session` v1 JSON, fingerprint, session code, 2 MiB
//!   cap, network check), `session` (coordinator and participant state
//!   machines), `dissolve` (now / together / standby).

pub mod params;
