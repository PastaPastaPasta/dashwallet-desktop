# MG governance engine contract (`dw-engine` facade)

Contract-Version: 8

Status: **draft**, 2026-10-16 (ROADMAP MG-02). Milestone 1.1, masternodes and governance (UX-SPEC §4.25, MG-00).
Code: `rust/crates/dw-engine/src/governance/` (§0). Design: [`MG-02-governance.md`](../design/MG-02-governance.md);
its D-rows are cited as "D1" and so on. Everything in [`m1-engine.md`](m1-engine.md) §1 (conventions) applies, with
the changes in §1 below.

The facade is plain Rust in dw-engine. `dwcli` and the console drive it directly; dw-app binds it for the Tauri UI
(`api/governance.rs`, BL-232), with binding derives in the binding crate only. MG-03 (masternodes), MG-05 (voting
grants), MG-06 (proposals) and MG-07 (contested names) extend this file; **a change to a signature, a record or a code updates this file and
raises `Contract-Version` in the same change.** `rust/crates/dw-engine/tests/mg_governance_contract.rs` enforces it
the way `m4_dashpay_contract.rs` enforces m4: the generated §3 listing with its SHA-256 (`DW_BLESS=1` to approve a
raised version), one §2 row per call with matching Kind and Errors, the §4 codes per variant and the `vote_rejected`
reasons, and the stub calls' `governance.not_implemented{call}`.

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
| `governance/contested.rs` | MG-07 | §2.6 (its Platform seam is not facade) |
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
| `proposal(hash: String)` | sync | `ProposalDetail`: the row, the object's fields, the raw data JSON, the funded height, the `valid`, `delete` and `endorsed` tallies (D82) and when the wallet marked the proposal deleted by vote (D84). `governance.not_synced{phase}` until a run reached `synced`, then `invalid_argument` for a hash that is not 64-character hex, then `governance.proposal_not_found{hash}` for an unknown, unconfirmed or non-proposal hash. **Implemented.** | `GovernanceError` |
| `current_votes(hash: String, outpoint: Option<String>)` | sync | Core's `gobject getcurrentvotes`: each current vote of every signal (D86) with its outpoint, signal, time, outcome and weight range, ordered by outpoint as Core's `COutPoint` compares (the txid's internal bytes, then the index), then by signal. Refuses like `proposal`: `governance.not_synced{phase}`, `invalid_argument` for a malformed hash, `governance.proposal_not_found{hash}`. **Implemented.** | `GovernanceError` |

### 2.4 Voting

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `voting_masternodes(wallet: WalletId, proposal: String)` | async | The masternodes of the tip list whose voting key the wallet derives (keychain indexes 0–19), each with type, weight, validity and its own current vote on `proposal` as seen. A vote counts as a masternode's own only when its collateral outpoint is resolved to that masternode, or when no other masternode of the list holds the key (D49); `key_holders` gives how many hold it, and for a shared key `key_last_vote`/`key_last_vote_time` give the key's latest vote seen, by whichever of them. The UI shows that state neutrally, as "Last vote seen with this shared key: Yes (2 masternodes use this key)", never as each masternode's vote. Watch-only wallets answer from their xpub-derived keys. `invalid_argument` for a malformed `proposal` (checked first); `governance.list_unavailable` while SPV has no masternode list. **Implemented.** | `GovernanceError` |
| `relay_vote(vote: RawVote)` | async | A vote signed elsewhere (Core `voteraw`): checked as a peer's vote is (§4.1 of the design), stored, announced to the connected peers and served on `getdata`. Returns `VoteRelayed { hash, peers }`. `governance.sync_disabled` while sync is off (checked first); `governance.not_synced{phase}` until a run reached `synced`, as its checks need every current vote; `invalid_argument` for a malformed proposal hash, collateral outpoint or signature; `governance.vote_rejected{reason}` if a check fails; `governance.no_peers` if no peer is connected (the vote is kept and announced on the next connection). **Implemented.** | `GovernanceError` |
| `vote(wallet: WalletId, proposal: String, outcome: VoteOutcome, masternodes: Vec<String>, grant: String)` | async | Signs and relays a funding vote on `proposal` per masternode (proTxHash), under one `voting` lease begun from `grant` (a `Governance` grant of `wallet`; MG-05 §4) and ended when the call returns. Returns `Vec<VoteCast>`, one per masternode in the given order (MG-05 D6). The key is the wallet's keychain voting key at indexes 0–19, else the masternode's attached `voting` key, whichever hashes to the list's voting key id; each vote is stamped with the time it is signed. **Whole-call errors**, before the grant is redeemed: `governance.sync_disabled`, `governance.not_synced`, `governance.list_unavailable`, `governance.proposal_not_found`, and `invalid_argument` for no masternodes, more than the list holds, a malformed or repeated proTxHash. The lease's errors are `GovernanceError::Platform` (`platform.grant_invalid` for a grant of another purpose or wallet, or one used; MG-05 D12). **Per masternode**, `rejected` is `unknown_masternode` (not in the tip list), `collateral_unresolved` (MG-02 D80), `no_voting_key`, `cancelled` (a lock), `signer_unavailable` (a vault error, an unreadable keychain included; MG-05 D23), `in_flight` (the same vote in flight), or a store refusal with `vote_rejected`'s reasons (§4); else `hash` is set and `peers` counts the peers it went to, 0 with none connected: kept and announced on the next connection, no error (MG-05 D7, D13). | `GovernanceError` |

### 2.5 The wallet's own proposals (MG-06)

Design: [`MG-06-proposals.md`](../design/MG-06-proposals.md), cited as "MG-06 D1" and so on. A `ProposalDraft` is the
wizard's input: name, URL, payment address, the amount per payment in duffs, the payment count, the first
superblock's height (one of `upcoming_superblocks(12)`) and optional coin control (`coins`, outpoints as
`<txid>-<n>`, as `getcurrentvotes` prints them). `MyProposalState` is derived on every read (MG-06 §2.3):
`fee_unsent`, `fee_paid{confirmations}`,
`ready_to_submit{reason}` (`no_peer_accepted`, `sync_disabled`, `not_synced`), `confirming{confirmations}`,
`public{status}`, `fee_failed`, `expired`. `SuperblockDate` (from `upcoming_superblocks`) carries the voting cutoff's
height, its estimate and `cutoff_soon` (at most two days away, MG-06 D13).

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `preview_proposal(wallet: WalletId, draft: ProposalDraft)` | async | The review step: `ProposalPreview` with the proposal's data JSON and hex, epochs, its hash as if created now (the created object is stamped with its own creation time, so its hash differs; MG-06 D3), the 1 DASH fee and the fee transaction's estimated network fee. Reserves, signs and saves nothing. `governance.not_synced{phase}` before a tip is known; `governance.invalid_proposal{fields}` with every failing field (MG-06 D11); `invalid_argument` for a malformed coin. **Implemented.** | `GovernanceError` |
| `create_proposal(wallet: WalletId, draft: ProposalDraft, grant: String)` | async | Validates as `preview_proposal`, then pays the fee under a `proposal` lease begun from `grant` (a `Spend` grant of `wallet` whose cap covers 1 DASH plus the network fee; MG-06 D10) and ended when the call returns. The fee is one `OP_RETURN <hash>` output of 1 DASH and P2PKH change (D1). The proposal and its signed fee are saved before the fee is broadcast (D4, MG-00 D18). Returns `MyProposal`. A save that fails broadcasts nothing; a hand-off that definitely did not send deletes the proposal and returns the send error; one that may have sent keeps it (`fee_unsent`). `governance.submit_refused{in_flight}` while the same proposal (the same draft in the same second) is being created. The wallet submits it on its own once the fee has a confirmation (D6). The lease's errors are `GovernanceError::Platform`; the draft's are `GovernanceError::Engine` (`send` with `grant_exceeded`, `insufficient_funds` and the rest of m1's send failures). **Implemented.** | `GovernanceError` |
| `my_proposals(wallet: WalletId)` | sync | The wallet's own proposals, newest first, each a `MyProposal` with its derived state and `last_error`, the failure of its fee's last hand-off if it failed, reported while the wallet has not seen the fee (MG-06 D10). Works with sync off. **Implemented.** | `GovernanceError` |
| `submit_proposal(wallet: WalletId, hash: String)` | async | The [Submit] action: a `fee_unsent` or `fee_paid{0}` proposal's saved fee is broadcast again (no grant, MG-06 D10), its coins claimed first and the hand-off recorded before it; a `no_peer_accepted` one is looked for by a new sync run at once (D7). Returns the `MyProposal` after. `invalid_argument` for a malformed hash; `governance.proposal_not_found{hash}` for a hash that is not the wallet's; `governance.submit_refused{reason}` for a `public`, `expired` or `fee_failed` proposal, and `in_flight` while a create or another [Submit] hands its fee off; `send.outpoint_unavailable` for a fee coin another payment or an asset lock holds, that was locked after the fee was built (coin control, dust, a masternode's, MG-04 D47), or that the wallet reads spent by another transaction; a resend's broadcast failure as `send.broadcast_unknown` (kept as `last_error`); `governance.sync_disabled` while sync is off (except for a resend). **Implemented.** | `GovernanceError` |

### 2.6 Contested DPNS names (MG-07)

Design: [`MG-07-contested-voting.md`](../design/MG-07-contested-voting.md), cited as "MG-07 D1" and so on. Both calls
need the SPV masternode list and Platform, not governance sync (MG-07 D2). The voters are the SPV list's masternodes
whose voting key the wallet holds: a keychain key at indexes 0–19, else an attached `voting` key whose recorded id is
the list's (MG-07 D3). A label is normalized as DPNS does (lower case, `o`→`0`, `i` and `l`→`1`) and must then be a
contested label: 3–19 of `a-hj-km-np-z01-`, no `-` at either end. Platform takes at most `votes_allowed` (5) votes per
masternode and poll, one vote and its changes; `votes_used` counts the ones this app saw Platform take (MG-07 D6).

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `contested_names(wallet: WalletId)` | async | `ContestedNames`: the open DPNS contests, soonest ending first (the first page of 100 of Platform's end-date index from now on, each with its first page of 100 contenders, `contenders_complete` false for a full page; MG-07 D9), the voters in list order, each with its key (`keychain{index}` or `attached`), Platform's weight (1, 4 for an evonode) and `current`: its proved choice (one votes-by-identity query per voter, MG-07 D4) and `votes_used` in each listed contest where it has either; and `votes_allowed`. | `GovernanceError` |
| `vote_name(wallet: WalletId, label: String, choice: NameChoice, masternodes: Vec<String>, grant: String)` | async | Casts `choice` (`towards_identity{identity}` base58, `abstain`, `lock`) on DPNS's poll of `label` for each masternode (proTxHash), under one `voting` lease begun from `grant` (a `Governance` grant of `wallet`) and ended when the call returns. Each vote is a `MasternodeVoteTransition` the vault lays out and signs (MG-07 D5), admitted through the fence as a row-less vote keyed by its hash, broadcast and waited for (MG-07 D12). Returns `Vec<NameVoteCast>`, one per masternode in the given order. **Whole-call errors**, before the grant is redeemed, in this order: `invalid_argument` (a label that cannot be contested, a malformed identity, no masternodes, a malformed or repeated proTxHash), `governance.list_unavailable`, `invalid_argument` for more masternodes than the list holds, `platform.unavailable` (no Platform, or a poll read failed), `governance.contest_not_found`, `governance.contest_closed`, `governance.not_a_contender`, then `wallet_not_found`; then the lease's `platform.*` errors (MG-05 D12). **Per masternode**, `rejected` is `unknown_masternode`, `no_voting_key`, `cancelled`, `signer_unavailable`, `in_flight` (MG-05's meanings), `vote_limit` (this app saw it use `votes_allowed`, or Platform refused it as over the limit; at the local limit a choice that is already the masternode's present one is `already_voted`, as Drive answers, MG-07 D18), `platform_unavailable` (the nonce read failed, or the transition never left; no `transition` then), `unconfirmed` (it may have left: no answer or no proof came back, or the dispatch permit's deadline came during the broadcast; MG-07 D7, D16), or Platform's refusal: `already_voted`, `contest_closed`, `contest_not_found`, `masternode_not_found`, `wrong_voting_key`, `choice_not_allowed`, `no_voter_identity`, `voting_key_disabled`, `refused` (MG-07 D10), `already_voted` also when the broadcast is answered "already exists" and the present choice is the one cast (MG-07 D18); else `transition` is the transition's hash (hex) and the vote counts. `transition` is also set on every refusal after the broadcast. | `GovernanceError` |

## 3. Surface (generated)

Generated by `mg_governance_contract.rs` from the facade's source files; `DW_BLESS=1` rewrites it after
`Contract-Version` is raised.

<!-- BEGIN GENERATED: governance-surface -->
<!-- surface-sha256: 7ff58ba738cb410bea0a752897d06f6d88710cebc24b73d25386fda3589799bc version: 8 -->

```rust
// src/governance/contested.rs
impl ContestNet for SdkNames;
impl NameVoteNet for SdkNames;
impl Governance {
    pub async fn contested_names(&self, wallet: WalletId) -> Result<ContestedNames, GovernanceError>;
    pub async fn vote_name(&self, wallet: WalletId, label: String, choice: NameChoice, masternodes: Vec<String>, grant: String) -> Result<Vec<NameVoteCast>, GovernanceError>;
}
// src/governance/errors.rs
#[derive(Debug, thiserror::Error)]
pub enum GovernanceError {
    SyncDisabled,
    NotSynced { phase: GovernanceSyncPhase },
    ProposalNotFound { hash: String },
    VoteRejected { reason: String },
    NoPeers,
    ListUnavailable,
    InvalidProposal { fields: Vec<ProposalField> },
    SubmitRefused { reason: SubmitRefusal },
    ContestNotFound { label: String },
    ContestClosed { label: String },
    NotAContender { identity: String },
    NotImplemented { call: String },
    Engine(EngineError),
    Platform(PlatformError),
}
impl GovernanceError {
    pub fn code(&self) -> &'static str;
    pub fn engine(&self) -> Option<&EngineError>;
}
// src/governance/mod.rs
pub use errors::GovernanceError;
pub use records::{ ContestedName, ContestedNames, NameChoice, NameContender, NameVoteCast, NameVoter, NameVoterState, NameVotingKey, };
pub use records::{ CurrentVoteRow, GovernanceClock, GovernanceInfo, GovernanceSyncPhase, GovernanceSyncState, MasternodeType, MyProposal, MyProposalState, ProposalCounts, ProposalDetail, ProposalDraft, ProposalField, ProposalPreview, ProposalQuery, ProposalRow, ProposalScope, ProposalStatus, RawVote, SignalTallies, SubmitRefusal, SubmitWait, SuperblockDate, VoteCast, VoteOutcome, VoteRelayed, VoteSignal, VoteTally, VotingMasternode, WeightRange, };
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
// src/governance/proposals.rs
impl Governance {
    pub async fn preview_proposal(&self, wallet: WalletId, draft: ProposalDraft) -> Result<ProposalPreview, GovernanceError>;
    pub async fn create_proposal(&self, wallet: WalletId, draft: ProposalDraft, grant: String) -> Result<MyProposal, GovernanceError>;
    pub fn my_proposals(&self, wallet: WalletId) -> Result<Vec<MyProposal>, GovernanceError>;
    pub async fn submit_proposal(&self, wallet: WalletId, hash: String) -> Result<MyProposal, GovernanceError>;
}
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
impl From<dw_governance::tally::RangedTally> for VoteTally;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SignalTallies {
    pub valid: VoteTally,
    pub delete: VoteTally,
    pub endorsed: VoteTally,
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
    pub signals: SignalTallies,
    pub deletion_marked_at: Option<i64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoteSignal {
    Funding,
    Valid,
    Delete,
    Endorsed,
}
impl From<dw_governance::vote::SentinelSignal> for VoteSignal;
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
    pub signal: VoteSignal,
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
    pub key_holders: u32,
    pub key_last_vote: Option<VoteOutcome>,
    pub key_last_vote_time: Option<i64>,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoteCast {
    pub pro_tx_hash: String,
    pub hash: Option<String>,
    pub peers: u32,
    pub rejected: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum NameChoice {
    TowardsIdentity { identity: String },
    Abstain,
    Lock,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameContender {
    pub identity: String,
    pub votes: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContestedName {
    pub label: String,
    pub ends_at: u64,
    pub contenders: Vec<NameContender>,
    pub lock_votes: Option<u32>,
    pub abstain_votes: Option<u32>,
    pub contenders_complete: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum NameVotingKey {
    Keychain { index: u32 },
    Attached,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameVoterState {
    pub label: String,
    pub choice: Option<NameChoice>,
    pub votes_used: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameVoter {
    pub pro_tx_hash: String,
    pub kind: MasternodeType,
    pub weight: u32,
    pub valid: bool,
    pub key: NameVotingKey,
    pub current: Vec<NameVoterState>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContestedNames {
    pub contests: Vec<ContestedName>,
    pub voters: Vec<NameVoter>,
    pub votes_allowed: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameVoteCast {
    pub pro_tx_hash: String,
    pub transition: Option<String>,
    pub rejected: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuperblockDate {
    pub height: u32,
    pub estimated_time: u64,
    pub budget: u64,
    pub voting_cutoff_height: u32,
    pub voting_cutoff_estimate: u64,
    pub cutoff_soon: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalDraft {
    pub name: String,
    pub url: String,
    pub payment_address: String,
    pub payment_amount: u64,
    pub payment_count: u32,
    pub first_superblock_height: u32,
    pub coins: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalField {
    Name,
    Url,
    PaymentAddress,
    PaymentAmount,
    PaymentCount,
    FirstPayment,
    Payload,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalPreview {
    pub hash: String,
    pub data_json: String,
    pub data_hex: String,
    pub start_epoch: i64,
    pub end_epoch: i64,
    pub fee: u64,
    pub network_fee: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyProposal {
    pub hash: String,
    pub name: String,
    pub url: String,
    pub payment_address: String,
    pub payment_amount: u64,
    pub payment_count: u32,
    pub start_epoch: i64,
    pub end_epoch: i64,
    pub fee_txid: String,
    pub created_at: u64,
    pub submitted_at: Option<u64>,
    pub state: MyProposalState,
    pub last_error: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MyProposalState {
    FeeUnsent,
    FeePaid { confirmations: u32 },
    ReadyToSubmit { reason: SubmitWait },
    Confirming { confirmations: u32 },
    Public { status: Option<ProposalStatus> },
    FeeFailed,
    Expired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmitWait {
    NoPeerAccepted,
    SyncDisabled,
    NotSynced,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmitRefusal {
    Public,
    Expired,
    FeeFailed,
    InFlight,
}
impl SubmitRefusal {
    pub fn as_str(self) -> &'static str;
}
// src/governance/voting.rs
pub const VOTING_KEY_INDEXES: u32;
impl Governance {
    pub async fn voting_masternodes(&self, wallet: WalletId, proposal: String) -> Result<Vec<VotingMasternode>, GovernanceError>;
    pub async fn relay_vote(&self, vote: RawVote) -> Result<VoteRelayed, GovernanceError>;
    pub async fn vote(&self, wallet: WalletId, proposal: String, outcome: VoteOutcome, masternodes: Vec<String>, grant: String) -> Result<Vec<VoteCast>, GovernanceError>;
}
```

<!-- END GENERATED: governance-surface -->

`VoteOutcome` is the facade's own enum (`yes`, `no`, `abstain`) with conversions to and from
`dw_governance::VoteOutcome`. A `RawVote`'s signal is always funding, so the record has no signal field.

## 4. Error codes

| Error enum | Codes |
|---|---|
| `GovernanceError` | `governance.sync_disabled`, `governance.not_synced`, `governance.proposal_not_found`, `governance.vote_rejected`, `governance.no_peers`, `governance.list_unavailable`, `governance.invalid_proposal`, `governance.submit_refused`, `governance.contest_not_found`, `governance.contest_closed`, `governance.not_a_contender`, `governance.not_implemented` |

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
- `governance.invalid_proposal{fields}`: a proposal draft failed validation; `fields` lists every failing one in
  wizard order: `name`, `url`, `payment_address`, `payment_amount`, `payment_count`, `first_payment`, `payload`
  (MG-06 D11).
- `governance.submit_refused{reason}`: `submit_proposal` on a proposal past submitting; `reason` is `public`,
  `expired` or `fee_failed`, or `in_flight` while its fee is being handed off (MG-06 D10).
- `governance.contest_not_found{label}`: no DPNS poll for the label: no contender and no stored result (MG-07).
- `governance.contest_closed{label}`: the poll resolved (a winner, locked, or no winner), or its voting ended (MG-07
  D8).
- `governance.not_a_contender{identity}`: a vote towards an identity the poll's first page does not list (MG-07 D9).
- `governance.not_implemented{call}`: a stub (none now; MG-05 D17).
- `Platform(PlatformError)`: a lease error of `vote`, `vote_name` or `create_proposal`, or `platform.unavailable` from
  `contested_names` and `vote_name`, with its `platform.*` code (m4 §4; MG-05 D12). The Swift host
  binds no governance call, so its `engineCodes` are unchanged.
- `Engine(EngineError)`: `network_not_open`, `wallet_not_found`, `invalid_argument`, `storage`, `internal` and the
  rest of m1 §4, unchanged. These are not new §4 codes for the m1–m3 tables, so the Swift `engineCodes` are
  unchanged (D8).

## 5. Events and the sync state machine

`EngineEvent::Governance { network }` (D7), at most once a second with the trailing edge kept, after any change of
the store (objects, votes, confirmations), of `sync_state` or of the masternode list's voting keys. The host re-reads
what it shows. dw-app forwards it as `AppEvent::Governance`, and binds the facade's calls as its `governance` domain
(BL-232, design D92–D97); dw-ffi does not forward it, as the Swift host has no governance calls (D95).

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
