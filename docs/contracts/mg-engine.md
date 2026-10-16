# MG governance engine contract (`dw-engine` facade)

Contract-Version: 2

Status: **draft**, 2026-10-16 (ROADMAP MG-02). Milestone 1.1, masternodes and governance (UX-SPEC §4.25, MG-00).
Code: `rust/crates/dw-engine/src/governance/` (§0). Design: [`MG-02-governance.md`](../design/MG-02-governance.md);
its D-rows are cited as "D1" and so on. Everything in [`m1-engine.md`](m1-engine.md) §1 (conventions) applies, with
the changes in §1 below.

The facade is plain Rust in dw-engine. `dwcli` and the console drive it directly; E0-13 binds it for the UI stack,
with binding derives in the binding crate only. MG-03 (masternodes), MG-05 (voting grants) and MG-06 (proposals)
extend this file; **a change to a signature, a record or a code updates this file and raises `Contract-Version` in
the same change.** `rust/crates/dw-engine/tests/mg_governance_contract.rs` enforces it the way
`m4_dashpay_contract.rs` enforces m4: the generated §3 listing with its SHA-256 (`DW_BLESS=1` to approve a raised
version), one §2 row per call with matching Kind and Errors, the §4 codes per variant and the `vote_rejected` reasons,
and the stub calls' `governance.not_implemented{call}`.

## 0. Object model and files

```
Engine ──open_network──▶ NetworkSession ──governance()──▶ Governance   (one shared state per session)
superblock_budget(network, height)                                      (free function, no session)
```

`NetworkSession::governance()` is sync and infallible and returns a cheap handle (`Clone`) on the session's
governance state: the `dw_governance::GovernanceStore`, the sync task, and the tip list snapshot that a follower
feeds for the session's life, with sync on or off. Any number of
handles may exist. After `close_network` every call returns `network_not_open`.

| File | Owner | Contents |
|---|---|---|
| `governance/mod.rs` | MG-02 | `Governance`, `NetworkSession::governance`, the sync-task calls of §2.1, `superblock_budget`, the re-exports |
| `governance/sync.rs` | MG-02 | the sync task and its tracker (not facade) |
| `governance/records.rs` | MG-02 | the records of §3 |
| `governance/reads.rs` | MG-02 | the read calls of §2.2–§2.3 |
| `governance/voting.rs` | MG-02, MG-05 | §2.4 |
| `governance/errors.rs` | MG-02 | `GovernanceError` |

## 1. Conventions (changes to m1-engine.md §1)

| Topic | Rule |
|---|---|
| Hashes | Object hashes, proTxHashes and txids are 64-character lower-case hex in Core's display order (byte-reversed), as `gobject list` prints them. An outpoint is `"<txid>-<n>"` (Core's `ToStringShort`). |
| Amounts | Duffs, `u64`. Weights and vote counts `u32`. |
| Times | Unix seconds. Clock times and ETAs are `u64`. Times that come from a governance object or vote (epochs, `created_at`, a vote's `time`) are `i64`, as Core stores them. Every governance ETA uses 157.2 s per block from the tip's time (D9). |
| Transport | govsync runs over `dw_p2p::Session` (`dw-governance::peer`); dw-p2p gains `Session::bytes_received()` for the byte counters, outside the m3 §6 surface CoinJoin uses. |
| Sync and async | A sync call reads memory only. An async call touches the wallet keychain, app.sqlite or the network, on the engine runtime. |
| Records | `serde` `Serialize` and `Deserialize`, plus `Debug`, `Clone` and `PartialEq`. Unit enums are snake-case strings; enums with data are tagged with `kind`. |
| Errors | `GovernanceError` (§4); `code()` is a stable string, `engine()` returns a wrapped `EngineError`. |
| Peer-seen numbers | Vote counts are what the peers sent, never claimed complete (MG-00 D45). A weight that the list cannot pin down is a `WeightRange` with `estimate` (D2). |
| No partial tally | Proposal rows, tallies and `info`'s vote numbers answer `governance.not_synced{phase}` until a run reached `synced` (MG-00 D21). The clock, the budget and `sync_state` need only the tip. |
| Operator keys | No call signs with, takes or returns an operator key (MG-01's guard). |

## 2. Calls

Kind is `sync`, `async` or `free, pure`. Each row says whether the call is implemented.

### 2.1 The sync task

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `NetworkSession.governance()` | sync | The handle (§0). Infallible: returns `Governance`, not a `Result`. **Implemented.** | — |
| `sync_enabled()` | sync | Whether governance sync is on (`governance.sync_enabled` in app.sqlite, read at session open; default off). Returns `bool`. **Implemented.** | `GovernanceError` |
| `set_sync_enabled(enabled: bool)` | async | Persists the setting, then starts or stops the sync task. Stopping keeps the store. Idempotent. **Implemented.** | `GovernanceError` |
| `sync_state()` | sync | `GovernanceSyncState`: phase (§5), counts, peers, bytes, unconfirmed objects, last error, timings. Needs only the open session. **Implemented.** | `GovernanceError` |
| `resync()` | sync | Starts a new run now (after `synced` or during a backoff); `governance.sync_disabled` when off. Returns `()`. **Implemented.** | `GovernanceError` |

### 2.2 Clock, info and budget

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `clock()` | sync | `GovernanceClock` for the tip: cycle progress, last and next superblock, blocks and ETA to it, the estimated voting cutoff (`voting_cutoff_estimate`, MG-00 D19) and whether voting is open. `governance.not_synced{phase}` only before a tip is known. **Implemented.** | `GovernanceError` |
| `info()` | sync | `GovernanceInfo`: the clock, the budget of the next superblock, Core's `getgovernanceinfo` numbers, the passing threshold, valid masternodes and evonodes, and (once synced) proposal counts, the estimated allocation and shortfall. `proposals` counts the current proposals (end epoch in the future): `total` is all of them, `passing` is those Passing or Funded, `failing` those Failing or Voting, `unfunded` those Unfunded, `close_call` those whose status the weight ranges could change. `governance.not_synced{phase}` only before a tip is known. **Implemented.** | `GovernanceError` |
| `upcoming_superblocks(count: u32)` | sync | The next `count` superblocks with their ETAs and budgets, for the proposal form (MG-06). A `count` above 24 is refused with `invalid_argument`, before the tip is read. **Implemented.** | `GovernanceError` |
| `superblock_budget(network: &DashNetwork, height: u32)` | free, pure | Core's `CSuperblock::GetPaymentsLimit` in duffs; 0 for a height that is not a superblock. Returns `u64`, no `Result`. **Implemented.** | — |

### 2.3 Proposals and votes

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `proposals(query: ProposalQuery)` | sync | Confirmed proposals (D3) as `ProposalRow`, sorted by MG-00's status groups then net yes. `query.scope`: `current` (end epoch not past), `all`. `query.text`: a case-folded substring of the name or URL. `governance.not_synced{phase}` until a run reached `synced`. **Implemented.** | `GovernanceError` |
| `proposal(hash: String)` | sync | `ProposalDetail`: the row, the object's fields, the raw data JSON, the funded height. `governance.not_synced{phase}` until a run reached `synced`, then `invalid_argument` for a hash that is not 64-character hex, then `governance.proposal_not_found{hash}` for an unknown, unconfirmed or non-proposal hash. **Implemented.** | `GovernanceError` |
| `current_votes(hash: String, outpoint: Option<String>)` | sync | Core's `gobject getcurrentvotes`: each current funding vote with its outpoint, time, outcome and weight range, ordered by outpoint as Core's `COutPoint` compares (the txid's internal bytes, then the index). Refuses like `proposal`: `governance.not_synced{phase}`, `invalid_argument` for a malformed hash, `governance.proposal_not_found{hash}`. **Implemented.** | `GovernanceError` |

### 2.4 Voting

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `voting_masternodes(wallet: WalletId, proposal: String)` | async | The masternodes of the tip list whose voting key the wallet derives (keychain indexes 0–19), each with type, weight, validity and its current vote on `proposal` as seen. Watch-only wallets answer from their xpub-derived keys. `invalid_argument` for a malformed `proposal` (checked first); `governance.list_unavailable` while SPV has no masternode list. **Implemented.** | `GovernanceError` |
| `relay_vote(vote: RawVote)` | async | A vote signed elsewhere (Core `voteraw`): checked as a peer's vote is (§4.1 of the design), stored, announced to the connected peers and served on `getdata`. Returns `VoteRelayed { hash, peers }`. `governance.sync_disabled` while sync is off (checked first); `governance.not_synced{phase}` until a run reached `synced`, as its checks need every current vote; `invalid_argument` for a malformed proposal hash, collateral outpoint or signature; `governance.vote_rejected{reason}` if a check fails; `governance.no_peers` if no peer is connected (the vote is kept and announced on the next connection). **Implemented.** | `GovernanceError` |
| `vote(wallet: WalletId, proposal: String, outcome: VoteOutcome, masternodes: Vec<String>, grant: String)` | async | Signs and relays a funding vote per masternode (proTxHash) with its voting key. Returns `Vec<VoteRelayed>`, one per masternode. **Stub** until MG-05 (the `Governance` grant purpose): `governance.not_implemented{call: "Governance.vote"}` (D6). | `GovernanceError` |

## 3. Surface (generated)

Generated by `mg_governance_contract.rs` from the facade's source files; `DW_BLESS=1` rewrites it after
`Contract-Version` is raised.

<!-- BEGIN GENERATED: governance-surface -->
<!-- surface-sha256: 548b0892ceecbe2bc247b0c59abebb4ee16d3c7c451f2bc6f292acda1619e4f5 version: 2 -->

```rust
// src/governance/errors.rs
#[derive(Debug, thiserror::Error)]
pub enum GovernanceError {
    SyncDisabled,
    NotSynced { phase: GovernanceSyncPhase },
    ProposalNotFound { hash: String },
    VoteRejected { reason: String },
    NoPeers,
    ListUnavailable,
    NotImplemented { call: String },
    Engine(EngineError),
}
impl GovernanceError {
    pub fn code(&self) -> &'static str;
    pub fn engine(&self) -> Option<&EngineError>;
}
// src/governance/mod.rs
pub use errors::GovernanceError;
pub use records::{ CurrentVoteRow, GovernanceClock, GovernanceInfo, GovernanceSyncPhase, GovernanceSyncState, MasternodeType, ProposalCounts, ProposalDetail, ProposalQuery, ProposalRow, ProposalScope, ProposalStatus, RawVote, SuperblockDate, VoteOutcome, VoteRelayed, VoteTally, VotingMasternode, WeightRange, };
#[derive(Clone)]
pub struct Governance {
    ..
}
impl NetworkSession {
    pub fn governance(self: &Arc<Self>) -> Governance;
}
impl Governance {
    pub fn sync_enabled(&self) -> Result<bool, GovernanceError>;
    pub async fn set_sync_enabled(&self, enabled: bool) -> Result<(), GovernanceError>;
    pub fn sync_state(&self) -> Result<GovernanceSyncState, GovernanceError>;
    pub fn resync(&self) -> Result<(), GovernanceError>;
}
pub fn superblock_budget(network: &DashNetwork, height: u32) -> u64;
// src/governance/reads.rs
pub const MAX_UPCOMING_SUPERBLOCKS: u32;
impl Governance {
    pub fn clock(&self) -> Result<GovernanceClock, GovernanceError>;
    pub fn info(&self) -> Result<GovernanceInfo, GovernanceError>;
    pub fn upcoming_superblocks(&self, count: u32) -> Result<Vec<SuperblockDate>, GovernanceError>;
    pub fn proposals(&self, query: ProposalQuery) -> Result<Vec<ProposalRow>, GovernanceError>;
    pub fn proposal(&self, hash: String) -> Result<ProposalDetail, GovernanceError>;
    pub fn current_votes(&self, hash: String, outpoint: Option<String>) -> Result<Vec<CurrentVoteRow>, GovernanceError>;
}
// src/governance/records.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceSyncPhase {
    Disabled,
    Waiting,
    SyncingObjects,
    SyncingVotes,
    Synced,
    Failed,
}
impl From<dw_governance::sync::Phase> for GovernanceSyncPhase;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GovernanceSyncState {
    pub phase: GovernanceSyncPhase,
    pub objects: u32,
    pub unconfirmed_objects: u32,
    pub votes: u64,
    pub orphan_votes: u32,
    pub peers: u32,
    pub bytes_received: u64,
    pub last_synced_at: Option<u64>,
    pub last_error: Option<String>,
    pub next_retry_at: Option<u64>,
    pub objects_secs: Option<f64>,
    pub votes_secs: Option<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GovernanceClock {
    pub tip_height: u32,
    pub cycle_progress: f64,
    pub last_superblock: u32,
    pub next_superblock: u32,
    pub blocks_to_superblock: u32,
    pub superblock_eta: u64,
    pub voting_cutoff_estimate: u64,
    pub voting_cutoff_height: u32,
    pub voting_open: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GovernanceInfo {
    pub clock: GovernanceClock,
    pub superblock_cycle: u32,
    pub maturity_window: u32,
    pub target_spacing: u32,
    pub proposal_fee: u64,
    pub budget: u64,
    pub passing_threshold: u32,
    pub valid_masternodes: u32,
    pub valid_evonodes: u32,
    pub valid_weight: u32,
    pub proposals: Option<ProposalCounts>,
    pub allocated_estimate: Option<u64>,
    pub unallocated_estimate: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalCounts {
    pub passing: u32,
    pub failing: u32,
    pub unfunded: u32,
    pub total: u32,
    pub close_call: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalScope {
    Current,
    All,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalQuery {
    pub scope: ProposalScope,
    pub text: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WeightRange {
    pub min: u32,
    pub max: u32,
    pub estimate: u32,
}
impl From<dw_governance::tally::WeightRange> for WeightRange;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VoteTally {
    pub yes: WeightRange,
    pub no: WeightRange,
    pub abstain: WeightRange,
    pub net_yes_estimate: i64,
    pub voters: u32,
    pub ranged_votes: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Funded,
    Lapsed,
    Confirming,
    Pending,
    Passing,
    Failing,
    Voting,
    Unfunded,
}
impl From<dw_governance::tally::Status> for ProposalStatus;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalRow {
    pub hash: String,
    pub name: String,
    pub url: String,
    pub payment_address: String,
    pub payment_amount: u64,
    pub start_epoch: i64,
    pub end_epoch: i64,
    pub payments_requested: u32,
    pub status: ProposalStatus,
    pub close_call: bool,
    pub tally: VoteTally,
    pub funded_height: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalDetail {
    pub row: ProposalRow,
    pub collateral_txid: String,
    pub created_at: i64,
    pub data_json: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoteOutcome {
    Yes,
    No,
    Abstain,
}
impl From<dw_governance::vote::VoteOutcome> for VoteOutcome;
impl From<VoteOutcome> for dw_governance::vote::VoteOutcome;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentVoteRow {
    pub outpoint: String,
    pub time: i64,
    pub outcome: VoteOutcome,
    pub weight: WeightRange,
    pub exact: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MasternodeType {
    Regular,
    Evonode,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VotingMasternode {
    pub pro_tx_hash: String,
    pub kind: MasternodeType,
    pub weight: u32,
    pub valid: bool,
    pub key_index: u32,
    pub current_vote: Option<VoteOutcome>,
    pub vote_time: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawVote {
    pub collateral: String,
    pub proposal: String,
    pub outcome: VoteOutcome,
    pub time: i64,
    pub signature: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoteRelayed {
    pub hash: String,
    pub peers: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuperblockDate {
    pub height: u32,
    pub estimated_time: u64,
    pub budget: u64,
}
// src/governance/voting.rs
pub const VOTING_KEY_INDEXES: u32;
impl Governance {
    pub async fn voting_masternodes(&self, wallet: WalletId, proposal: String) -> Result<Vec<VotingMasternode>, GovernanceError>;
    pub async fn relay_vote(&self, vote: RawVote) -> Result<VoteRelayed, GovernanceError>;
    pub async fn vote(&self, wallet: WalletId, proposal: String, outcome: VoteOutcome, masternodes: Vec<String>, grant: String) -> Result<Vec<VoteRelayed>, GovernanceError>;
}
```

<!-- END GENERATED: governance-surface -->

`VoteOutcome` is the facade's own enum (`yes`, `no`, `abstain`) with conversions to and from
`dw_governance::VoteOutcome`. A `RawVote`'s signal is always funding, so the record has no signal field.

## 4. Error codes

| Error enum | Codes |
|---|---|
| `GovernanceError` | `governance.sync_disabled`, `governance.not_synced`, `governance.proposal_not_found`, `governance.vote_rejected`, `governance.no_peers`, `governance.list_unavailable`, `governance.not_implemented` |

- `governance.sync_disabled`: a call that needs the sync task while it is off (`resync`, `relay_vote`).
- `governance.not_synced{phase}`: no run reached `synced` yet (or no tip, for the clock); `phase` is the current one.
- `governance.proposal_not_found{hash}`: unknown, unconfirmed (D3) or not a proposal.
- `governance.vote_rejected{reason}`: `reason` is one of `unknown_object`, `not_a_proposal`, `expired`, `duplicate`,
  `malformed`, `bad_signature`, `unknown_voting_key`, `wrong_voting_key`, `masternode_not_found`, `obsolete`,
  `same_timestamp`, `too_often`, `too_far_ahead`, `key_limit`. `unknown_object` is an unknown parent or a vote the
  store could not verify or place; `expired` is a vote on a proposal past its end epoch; `masternode_not_found` a
  key held only by banned masternodes; `same_timestamp` a vote of the current one's time with a lower outcome;
  `too_often` a vote replacing one less than an hour old (Core `GOVERNANCE_UPDATE_MIN`). A `RawVote` carries an outcome enum and always votes on funding, so there is no
  bad-signal or bad-outcome reason.
- `governance.no_peers`: no full node to talk to.
- `governance.list_unavailable`: the call needs the masternode list and SPV has none yet.
- `governance.not_implemented{call}`: a stub (`"Governance.vote"`).
- `Engine(EngineError)`: `network_not_open`, `wallet_not_found`, `invalid_argument`, `storage`, `internal` and the
  rest of m1 §4, unchanged. These are not new §4 codes for the m1–m3 tables, so the Swift `engineCodes` are
  unchanged (D8).

## 5. Events and the sync state machine

`EngineEvent::Governance { network }` (D7), at most once a second with the trailing edge kept, after any change of
the store (objects, votes, confirmations), of `sync_state` or of the masternode list's voting keys. The host re-reads
what it shows. dw-app forwards it as `AppEvent::Governance`; dw-ffi does not forward it until E0-13 binds the facade.

Phases (`GovernanceSyncPhase`; design §2.3):

| From | To | When |
|---|---|---|
| `disabled` | `waiting` | `set_sync_enabled(true)`, or session open with the setting on |
| `waiting` | `syncing_objects` | a tip and a non-empty list are known and at least one full node connected |
| `syncing_objects` | `syncing_votes` | every fresh peer answered `ssc(10)` and announced its count; nothing pending |
| `syncing_votes` | `synced` | every current proposal's `ssc(11)` answered and announced; nothing pending |
| `waiting` | `failed` | no candidate full node could be reached |
| `syncing_*` | `failed` | 20 s without progress while counts are short, the phase's 30-minute deadline, an item given up that never arrived, no peer completing its sync, or every peer lost (design D12, D13) |
| `failed` | `waiting` | after the backoff (30 s doubling to 5 min), or `resync()` |
| `synced` | `failed` | every peer lost; the next run starts 5 s later, without backoff |
| `synced` | `waiting` | `resync()` |
| any | `disabled` | `set_sync_enabled(false)` |

`synced` keeps following relays from the connected peers, and only then announces the wallet's own votes (design
D24). The store survives every transition but `close_network`. `sync_state().next_retry_at` is set while a backoff
runs.
