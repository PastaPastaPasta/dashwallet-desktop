//! Dash governance without a full node (DESIGN-opus §1.4 `dw-governance`,
//! §1.14): governance objects and votes come from full-node peers over P2P
//! `govsync`, votes are checked against the voting keys of the SPV
//! masternode list, and tallies, statuses and the superblock clock are
//! computed locally. Ported from Dash Core `src/governance/*.cpp` and
//! dash-qt `src/qt/proposal*.cpp`.
//!
//! Salvaged from the parked branch `m3/r2-governance` by MG-01
//! (docs/design/MG-01-salvage.md). The engine integration (sync lifecycle,
//! the `Governance` event, the facade calls) is MG-02's.
//!
//! Module layout:
//! - [`params`]: superblock cycle, maturity window, quorum floor, fees per
//!   network.
//! - [`object`]: `CGovernanceObject` serialization and hash.
//! - [`vote`]: `CGovernanceVote` serialization, hash, what a voting key
//!   signs, and recovering the signer's key id.
//! - [`proposal`]: the Create Proposal validation, the data JSON with
//!   dash-qt's key order and the honoured payment date, parsing synced
//!   proposal and trigger data.
//! - [`clock`]: last/next superblock, voting cutoff, ETA, the superblock
//!   budget (QT-026, QT-134).
//! - [`tally`]: the passing threshold `max(min_quorum, weighted_valid / 10)`,
//!   the fundable set, the eight dash-qt statuses in dash-qt's evaluation
//!   order, and the tally of votes attributed by voting key with weight
//!   ranges (MG-02 §4.2).
//! - [`validate`]: Core's object rules for what peers send.
//! - [`store`]: synced objects and current votes in memory.
//! - [`net`]: the P2P session governance uses until MG-02 moves govsync
//!   onto `dw_p2p::Session`.
//! - [`sync`]: `govsync` objects then current-proposal votes, following
//!   relays, the G7 counters; relaying the wallet's own objects and votes.
//!
//! The engine builds the collateral transaction, keeps pending proposals and
//! signs votes with the vault (MG-02, MG-06).

pub mod clock;
pub mod net;
pub mod object;
pub mod params;
pub mod proposal;
pub mod store;
pub mod sync;
pub mod tally;
pub mod validate;
pub mod vote;
