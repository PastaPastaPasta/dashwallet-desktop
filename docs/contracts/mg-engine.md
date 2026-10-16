# MG governance engine contract (`dw-engine` facade)

Contract-Version: 1

Status: **draft**, 2026-10-16 (ROADMAP MG-02). Milestone 1.1, masternodes and governance (UX-SPEC §4.25, MG-00).
Code: `rust/crates/dw-engine/src/governance/` (§0). Design: [`MG-02-governance.md`](../design/MG-02-governance.md);
its D-rows are cited as "D1" and so on. Everything in [`m1-engine.md`](m1-engine.md) §1 (conventions) applies, with
the changes in §1 below.

The facade is plain Rust in dw-engine. `dwcli` and the console drive it directly; E0-13 binds it for the UI stack,
with binding derives in the binding crate only. MG-03 (masternodes), MG-05 (voting grants) and MG-06 (proposals)
extend this file; **a change to a signature, a record or a code updates this file and raises `Contract-Version` in
the same change.** Slice 2 of MG-02 adds `rust/crates/dw-engine/tests/mg_governance_contract.rs`, which enforces it
the way `m4_dashpay_contract.rs` enforces m4: the generated §3 listing with its SHA-256 (`DW_BLESS=1` to approve a
raised version), one §2 row per call with matching Kind and Errors, the §4 codes per variant, and the stub calls'
`governance.not_implemented{call}`.

## 0. Object model and files

```
Engine ──open_network──▶ NetworkSession ──governance()──▶ Governance   (one shared state per session)
superblock_budget(network, height)                                      (free function, no session)
```

`NetworkSession::governance()` is sync and infallible and returns a cheap handle (`Clone`) on the session's
governance state: the `dw_governance::GovernanceStore`, the sync task and the list snapshot it feeds. Any number of
handles may exist. After `close_network` every call returns `network_not_open`.

| File | Owner | Contents |
|---|---|---|
| `governance/mod.rs` | MG-02 | `Governance`, `NetworkSession::governance`, the re-exports |
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
| Times | Unix seconds, `u64`. Every governance ETA uses 157.2 s per block from the tip's time (D9). |
| Sync and async | A sync call reads memory only. An async call touches the wallet keychain, app.sqlite or the network, on the engine runtime. |
| Records | `serde` `Serialize` and `Deserialize`, plus `Debug`, `Clone` and `PartialEq`. Unit enums are snake-case strings; enums with data are tagged with `kind`. |
| Errors | `GovernanceError` (§4); `code()` is a stable string, `engine()` returns a wrapped `EngineError`. |
| Peer-seen numbers | Vote counts are what the peers sent, never claimed complete (MG-00 D45). A weight that the list cannot pin down is a `WeightRange` with `estimate` (D2). |
| No partial tally | Proposal rows, tallies and `info`'s vote numbers answer `governance.not_synced{phase}` until a run reached `synced` (MG-00 D21). The clock, the budget and `sync_state` need only the tip. |
| Operator keys | No call signs with, takes or returns an operator key (MG-01's guard). |

## 2. Calls

Kind is `sync`, `async` or `free, pure`. Each row says whether the call is implemented.

### 2.1 Sync

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `NetworkSession.governance()` | sync | The handle (§0). Infallible: returns `Governance`, not a `Result`. **Implemented.** | — |
| `sync_enabled()` | sync | Whether governance sync is on (`governance.sync_enabled` in app.sqlite, read at session open; default off). **Implemented.** | `GovernanceError` |
| `set_sync_enabled(enabled: bool)` | async | Persists the setting, then starts or stops the sync task. Stopping keeps the store. Idempotent. **Implemented.** | `GovernanceError` |
| `sync_state()` | sync | `GovernanceSyncState`: phase (§5), counts, peers, bytes, unconfirmed objects, last error, timings. **Implemented.** | `GovernanceError` |
| `resync()` | sync | Starts a new run now (after `synced` or during a backoff); `governance.sync_disabled` when off. **Implemented.** | `GovernanceError` |

### 2.2 Clock, info and budget

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `clock()` | sync | `GovernanceClock` for the tip: cycle progress, last and next superblock, blocks and ETA to it, the estimated voting cutoff (`voting_cutoff_estimate`, MG-00 D19) and whether voting is open. `governance.not_synced{phase}` only before a tip is known. **Implemented.** | `GovernanceError` |
| `info()` | sync | `GovernanceInfo`: the clock, the budget of the next superblock, Core's `getgovernanceinfo` numbers, the passing threshold, valid masternodes and evonodes, and (once synced) proposal counts by status, the estimated allocation and shortfall. **Implemented.** | `GovernanceError` |
| `upcoming_superblocks(count: u32)` | sync | The next `count` (at most 24) superblocks with their ETAs, for the proposal form (MG-06). **Implemented.** | `GovernanceError` |
| `superblock_budget(network: Network, height: u32)` | free, pure | Core's `CSuperblock::GetPaymentsLimit` in duffs; 0 for a height that is not a superblock. Returns `u64`, no `Result`. **Implemented.** | — |

### 2.3 Proposals and votes

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `proposals(query: ProposalQuery)` | sync | Confirmed proposals (D3) as `ProposalRow`, sorted by MG-00's status groups then net yes. `query.scope`: `current` (end epoch not past), `all`. `query.text`: a case-folded substring of the name or URL. **Implemented.** | `GovernanceError` |
| `proposal(hash: String)` | sync | `ProposalDetail`: the row, the object's fields, the raw data JSON, the funded height. `governance.proposal_not_found{hash}` for an unknown or unconfirmed hash. **Implemented.** | `GovernanceError` |
| `current_votes(hash: String, outpoint: Option<String>)` | sync | Core's `gobject getcurrentvotes`: each current funding vote with its outpoint, time, outcome and weight range. **Implemented.** | `GovernanceError` |

### 2.4 Voting

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `voting_masternodes(wallet: WalletId, proposal: String)` | async | The masternodes of the tip list whose voting key the wallet derives (keychain indexes 0–19), each with type, weight, validity and its current vote on `proposal` as seen. Watch-only wallets answer from their xpub-derived keys. **Implemented.** | `GovernanceError` |
| `relay_vote(vote: RawVote)` | async | A vote signed elsewhere (Core `voteraw`): checked as a peer's vote is (§4.1 of the design), stored, announced to the connected peers and served on `getdata`. Returns `VoteRelayed { hash, peers }`; `governance.vote_rejected{reason}` if a check fails, `governance.no_peers` if no peer is connected (the vote is kept and announced on the next connection). **Implemented.** | `GovernanceError` |
| `vote(wallet: WalletId, proposal: String, outcome: VoteOutcome, masternodes: Vec<String>, grant: String)` | async | Signs and relays a funding vote per masternode (proTxHash) with its voting key. **Stub** until MG-05 (the `Governance` grant purpose): `governance.not_implemented{call: "Governance.vote"}` (D6). | `GovernanceError` |

## 3. Surface (generated)

Generated by `mg_governance_contract.rs` in MG-02 slice 2; until then the records are as follows.

```rust
pub enum GovernanceSyncPhase { Disabled, Waiting, SyncingObjects, SyncingVotes, Synced, Failed }

pub struct GovernanceSyncState {
    pub phase: GovernanceSyncPhase,
    pub objects: u32,              // stored, confirmed
    pub unconfirmed_objects: u32,  // announced by one peer only (D3)
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

pub struct GovernanceClock {
    pub tip_height: u32,
    pub cycle_progress: f64,
    pub last_superblock: u32,
    pub next_superblock: u32,
    pub blocks_to_superblock: u32,
    pub superblock_eta: u64,
    pub voting_cutoff_estimate: u64,   // time; MG-00 D19
    pub voting_cutoff_height: u32,
    pub voting_open: bool,
}

pub struct GovernanceInfo {
    pub clock: GovernanceClock,
    pub superblock_cycle: u32,
    pub maturity_window: u32,
    pub target_spacing: u32,           // Core's 150
    pub proposal_fee: u64,
    pub budget: u64,                   // next superblock
    pub passing_threshold: u32,
    pub valid_masternodes: u32,
    pub valid_evonodes: u32,
    pub valid_weight: u32,
    pub proposals: Option<ProposalCounts>,   // None until synced
    pub allocated_estimate: Option<u64>,
    pub unallocated_estimate: Option<u64>,
}

pub struct ProposalCounts { pub passing: u32, pub failing: u32, pub unfunded: u32, pub total: u32, pub close_call: u32 }

pub enum ProposalScope { Current, All }
pub struct ProposalQuery { pub scope: ProposalScope, pub text: Option<String> }

pub struct WeightRange { pub min: u32, pub max: u32, pub estimate: u32 }

pub struct VoteTally {
    pub yes: WeightRange, pub no: WeightRange, pub abstain: WeightRange,
    pub net_yes_estimate: i64,
    pub voters: u32,                   // outpoints seen
    pub ranged_votes: u32,             // votes whose weight is not exact (D2)
}

pub enum ProposalStatus { Passing, Funded, NotFunded, Failing, Voting, Pending, Expired, Closed }  // MG-01 tally::Status

pub struct ProposalRow {
    pub hash: String,
    pub name: String,
    pub url: String,
    pub payment_address: String,
    pub payment_amount: u64,
    pub start_epoch: u64,
    pub end_epoch: u64,
    pub payments_remaining: u32,
    pub status: ProposalStatus,
    pub close_call: bool,              // MG-00 D45 "too close to call"
    pub tally: VoteTally,
    pub funded_height: Option<u32>,
}

pub struct ProposalDetail {
    pub row: ProposalRow,
    pub collateral_txid: String,
    pub created_at: u64,
    pub data_json: String,
}

pub struct CurrentVoteRow {
    pub outpoint: String,
    pub time: u64,
    pub outcome: VoteOutcome,
    pub weight: WeightRange,
    pub exact: bool,
}

pub enum MasternodeType { Regular, Evonode }

pub struct VotingMasternode {
    pub pro_tx_hash: String,
    pub kind: MasternodeType,
    pub weight: u32,
    pub valid: bool,                   // PoSe-banned masternodes still vote (D16)
    pub key_index: u32,
    pub current_vote: Option<VoteOutcome>,
    pub vote_time: Option<u64>,
}

pub struct RawVote {                   // Core voteraw's arguments
    pub collateral: String,            // "<txid>-<n>"
    pub proposal: String,
    pub outcome: VoteOutcome,
    pub time: u64,
    pub signature: String,             // base64, 65 bytes
}

pub struct VoteRelayed { pub hash: String, pub peers: u32 }

pub struct SuperblockDate { pub height: u32, pub estimated_time: u64, pub budget: u64 }
```

`VoteOutcome` is `dw_governance::VoteOutcome` (`yes`, `no`, `abstain`).

## 4. Error codes

| Error enum | Codes |
|---|---|
| `GovernanceError` | `governance.sync_disabled`, `governance.not_synced`, `governance.proposal_not_found`, `governance.vote_rejected`, `governance.no_peers`, `governance.list_unavailable`, `governance.not_implemented`, and the `EngineError` codes through `Engine` |

- `governance.sync_disabled`: a call that needs the sync task while it is off (`resync`, `relay_vote`).
- `governance.not_synced{phase}`: no run reached `synced` yet (or no tip, for the clock); `phase` is the current one.
- `governance.proposal_not_found{hash}`: unknown, unconfirmed (D3) or not a proposal.
- `governance.vote_rejected{reason}`: `reason` is one of `unknown_object`, `not_a_proposal`, `bad_signal`,
  `bad_outcome`, `bad_signature`, `unknown_voting_key`, `wrong_voting_key`, `obsolete`, `too_far_ahead`,
  `key_limit` (`dw_governance::VoteRejection`, snake-case).
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
| `syncing_*` | `failed` | 20 s without progress while counts are short, or every peer lost |
| `failed` | `waiting` | after the backoff (30 s doubling to 5 min), or `resync()` |
| `synced` | `waiting` | every peer lost, or `resync()` |
| any | `disabled` | `set_sync_enabled(false)` |

`synced` keeps following relays from the connected peers. The store survives every transition but `close_network`.
