//! Dash governance without a full node (DESIGN-opus §1.4 `dw-governance`,
//! §1.14): governance objects and votes come from full-node peers over P2P
//! `govsync`, votes are checked against the voting keys of the SPV
//! masternode list, and tallies, statuses and the superblock clock are
//! computed locally. Ported from Dash Core `src/governance/*.cpp` and
//! dash-qt `src/qt/proposal*.cpp`.
//!
//! Owner: R2. The engine integration (sync lifecycle, the `Governance`
//! event, the FFI calls of docs/contracts/m3-engine.md §2.2) lives in
//! `dw-engine/src/governance.rs`.
//!
//! Module layout:
//! - [`params`]: superblock cycle, maturity window, quorum floor, fees per
//!   network (works).
//! - `object`: `CGovernanceObject` serialization and hash, proposal JSON
//!   with dash-qt's key order, the ≤ 512-byte payload rule.
//! - `vote`: `CGovernanceVote` serialization, hash, ECDSA signing with a
//!   voting key and verification against the list's voting key id.
//! - `sync`: `govsync` full sync and per-object vote sync over `dw-p2p`,
//!   dedupe, validity, the G7 measurement counters.
//! - `tally`: weighted Y/N/A (EvoNode weight 4), the passing threshold
//!   `max(min_quorum, weighted_valid / 10)`, the eight dash-qt statuses in
//!   dash-qt's evaluation order, the fundable set.
//! - `clock`: last/next superblock, voting cutoff, ETA, budget committed
//!   (QT-026, QT-134).
//! - `proposal`: create (1 DASH `OP_RETURN <hash>` collateral transaction),
//!   pending proposals, submit at ≥ 1 confirmation (QT-132, QT-133).

pub mod params;
