# MG-02: the `Governance` facade

ROADMAP MG-02 (release 1.1, size L, engine, tiers T1 and mainnet read-only, stack-independent): the `Governance`
facade, with govsync over `dw-p2p`, proposals, votes, the clock, the info numbers and the superblock budget; the
console commands come back; the contract is [`docs/contracts/mg-engine.md`](../contracts/mg-engine.md).

Acceptance: the governance regtest suite (4 masternodes) is green, and the G7 measurement on mainnet matches Core's
Y/N counts on the bumped pin.

Inputs: MG-01's crates and its §7 hand-over ([`MG-01-salvage.md`](MG-01-salvage.md)), MG-00's UX rows
([`MG-00-masternodes-ux.md`](MG-00-masternodes-ux.md) D9, D11–D21, D45), and Dash Core **v24.0.0-rc.3** (the
oracle binary `/work/chains/bin/current`; sources read at that tag of `~/workspace/dash`). Core line references
below are to that tag.

D-numbers in this file are MG-02's (D1 and up); MG-00 and MG-01 rows are cited as "MG-00 D45", "MG-01 D13".

| Part | Where |
|---|---|
| govsync on `dw_p2p::Session`, per-peer request tracking, bounds, confirmations | `rust/crates/dw-governance/src/{peer,sync}.rs` (`net.rs` deleted) |
| Object validation (Core's proposal and trigger rules) | `rust/crates/dw-governance/src/validate.rs` |
| Votes checked against the whole tip list, orphans | `rust/crates/dw-governance/src/store.rs` |
| Tally by voting key, with the MG-00 D45 range | `rust/crates/dw-governance/src/tally.rs` |
| The facade, its records, errors and the coalesced event | `rust/crates/dw-engine/src/governance/` |
| Console commands | `rust/crates/dw-console/src/governance.rs` |
| Regtest suite (4 masternodes) | `regtest/functional/dwd_governance.py` |
| G7 measurement | `docs/research/g7-govsync-mg02.md`, `scripts/g7-compare-tallies.py` |

## 1. What the SPV list can and cannot prove

Everything below follows from one fact. The simplified masternode list (SML) that dash-spv keeps carries, per
masternode, the proTxHash, service address, operator BLS key, **voting key id**, validity and type, and **no
collateral outpoint** (platform-wallet `masternode::list` says so; DIP-4 `CSimplifiedMNListEntry`). Core keys every
governance vote and every trigger by the masternode's collateral outpoint (`GetMNByCollateral`, `object.cpp:392`,
`:741`), and it checks proposal collateral with `-txindex` (`IsCollateralValid`, `object.cpp:790`). The wallet has
neither.

| Core check | What the wallet does instead | Row |
|---|---|---|
| A vote's outpoint is a masternode of the tip list, and its signature recovers that masternode's voting key | The signature recovers a voting key that **some** masternode of the tip list holds (PoSe-banned included, as `GetMNByCollateral` finds them). The vote is then attributed to that key's masternodes; the weight is exact when the key's masternodes have one type and no more outpoints vote with the key than it has masternodes, otherwise a range (MG-00 D45). An outpoint the wallet resolved itself (MG-03, MG-00 D43) is checked exactly. | D1, D2 |
| Proposal collateral: an `OP_RETURN <hash>` output of ≥ 1 DASH, 6 confirmations | Not checkable: no transaction index, and BIP158 filters leave `OP_RETURN` outputs out. An object counts only once **two distinct peers** announced it (each of them checked the collateral before relaying it). | D3 |
| A trigger is signed by the operator key of the masternode at its outpoint | Not checkable without the outpoint. Triggers get Core's structural rules, the two-peer rule and Core's expiry; only such triggers give a proposal its funded height. | D4 |
| Proposal data rules (`ValidateProposal`) | The same rules, ported (`validate.rs`). | — |
| Object type | Read from the data's `type`, as `LoadData` does (`object.cpp:646`), not from the wire field. | D5 |

## 2. govsync over `dw_p2p::Session`

`dw-governance/src/net.rs` (r2's own transport, MG-01 D4) is deleted. `peer.rs` wraps one `dw_p2p::Session` per
full node and turns its messages into governance events; `sync.rs` drives them.

### 2.1 The peer

- `Session::connect` with dw-p2p's defaults (protocol 70233, `fRelay` off, no `senddsq`), a 60 s ping and a
  **5 minute idle timeout**: a peer that answers neither pings nor anything else for 5 minutes is closed, which
  ends its stream and drops it from the run (MG-01 D17 "follow-mode liveness"). The session answers pings itself.
- `Session::bytes_received()` is new in dw-p2p (an atomic the read loop adds frame sizes to), for the G7 counters.
  It is the only dw-p2p change.
- The peer subscribes to `inv`, `getdata`, `notfound`, `govobj`, `govobjvote` and `ssc`. `inv`/`getdata`/`notfound`
  are decoded with Core's `MAX_INV_SZ` (50,000) and keep only types 17 (object) and 18 (vote).

### 2.2 Requests and what a peer may send

Core accepts an object or a vote only from a peer that announced it or that it asked
(`PeerConsumeObjectRequest`, `net_governance.cpp:188`, `:221`), and completes the request only for that peer. The
wallet keeps the same rule in `Requests`:

- `announced`: per peer, the hashes it announced and the wallet has not fetched; at most
  `MAX_PEER_ANNOUNCEMENTS` = 100,000 (Core `MAX_PEER_OBJECT_ANNOUNCEMENTS`, `net_processing.cpp:88`). Further
  announcements from that peer are dropped.
- `pending`: hash → (peer, sent at). A `getdata` goes to one announcer at a time, at most 1,000 entries per message
  (Core `MAX_GETDATA_SZ`).
- An item is accepted only when it answers a pending request **to that peer**. Anything else is unsolicited: it is
  dropped without decoding beyond the hash, and counted; a peer with more than `MAX_UNSOLICITED` = 1,000 is closed
  (D10).
- A request ends when its item arrives, when the peer answers `notfound` for it, when the item does not decode or
  does not validate, when the peer closes, or after `REQUEST_TIMEOUT` = 30 s. Then the next peer that announced the
  hash is asked, if any (MG-01 D17 "clear a pending hash").

### 2.3 Phases

```
            enable                     tip + list known, peers connected
 Disabled ─────────▶ Waiting ─────────────────────────────────▶ SyncingObjects
    ▲                  ▲  ▲                                          │ every fresh peer answered ssc(10)
    │ disable          │  │ retry after backoff (30 s → 5 min)       │ and announced that many; nothing pending
    │ (from any)       │  └──────────── Failed{reason} ◀──── idle 20 s / peers lost
    │                  │                    ▲                        ▼
    │                  │                    └──────────────────  SyncingVotes
    │                  │ every peer lost                             │ every current proposal's ssc(11) answered,
    │                  └──────────────────────── Synced ◀────────────┘ that many announced; nothing pending
    │                                         (follows relays)
```

- **Waiting** until the tip height and a non-empty masternode list are known (the store needs the list's voting
  keys first, MG-01 D13), then connects to up to 3 full nodes (`PeerPicker::full_nodes`: valid masternodes in random
  order, then the configured SPV peers).
- **SyncingObjects**: `govsync(0, empty filter)` to **every** fresh peer (one not full-synced in this session within
  the network's fulfilled-request window: 1 h on mainnet, 5 min elsewhere, Core `nFulfilledRequestExpireTime`), not
  only one. Each answers `ssc(10, n)` and n object invs. Asking all of them costs a few hundred inv entries and
  gives the two-peer confirmation (D3) at once. The phase completes when every fresh peer answered and announced
  its count and nothing is pending.
- **SyncingVotes**: `govsync(hash, filter)` per current proposal (end epoch in the future), spread over the fresh
  peers, with the non-empty match-nothing filter (Core reads an empty filter as an object fetch,
  `net_governance.cpp:116`; MG-01 §7). Votes on triggers are not asked for (D7 of MG-01: not tallied).
- **Synced**: only after both phases completed. A phase that idles out (nothing for 20 s while counts are short) or
  loses every peer ends the run as **Failed** with the reason, and the store keeps what arrived (MG-01 D17 "no
  `Synced` after a failed phase"). The next run starts after a backoff (30 s, doubling to 5 min).
- **Follow**: after Synced, new invs from any peer are fetched under the same request rules. When every peer is
  gone the run ends and the next one reconnects; peers synced inside the window only follow.
- **Re-sync**: a run that reaches Synced is not repeated. A new run happens after a failure, after every peer was
  lost, or when the engine asks (`resync`).

### 2.4 Orphan votes

A vote whose object has not arrived is kept only after the checks that need no object (MG-01 D23). Core keeps them
for `GOVERNANCE_ORPHAN_EXPIRATION_TIME` (10 minutes, `governance.cpp:34`) in a bounded cache. The store does the
same, plus a per-peer bound (MG-01 §7):

- each orphan carries its arrival time and the peer that sent it; it expires after 10 minutes;
- at most `MAX_ORPHAN_VOTES` = 50,000 in all; when full, the oldest is evicted;
- at most `MAX_ORPHANS_PER_PEER` = 2,000 from one peer; past that, that peer's orphans are refused;
- at most one orphan per (parent, outpoint): a newer vote replaces an older one.

During a normal sync orphans are rare: votes are requested only for objects already stored.

## 3. Objects

`validate.rs` ports Core's rules for objects arriving from peers (`IsValidLocally(…, fCheckCollateral=true)` minus
what §1 says cannot be checked):

- **Any object**: the data parses as a JSON object, and its `type` is an integer (`LoadData`); that `type` is the
  object's type (D5). Parent hash zero; revision 1 (Core checks neither, but relays only these; refused objects are
  logged).
- **Proposal** (`ValidateProposal(hex, fCheckExpiration=false, fAllowScript=true)`, `object.cpp:1000`): data
  at most 512 bytes; `type` 1; `name` a string of 1–40 characters of `[-_a-z0-9]` after lower-casing; `start_epoch`
  and `end_epoch` JSON integers with end > start; `payment_amount` a JSON number above zero; `payment_address` a
  string without whitespace that decodes to a P2PKH or P2SH destination of this network; `url` a string without
  whitespace, at least 4 characters, passing `CheckURL`. Expired proposals are kept, as Core keeps them until its
  own clean-up.
- **Trigger** (`CSuperblock` constructor, `superblock.cpp:75`, `ParsePaymentSchedule`): `type` 2; integer
  `event_block_height`; `payment_addresses`, `payment_amounts` and `proposal_hashes` strings split on `|` into equal,
  non-empty lists of valid destinations, `ParsePaymentAmount` amounts and 64-hex hashes. Its creation time is not
  more than an hour ahead and not older than two superblock cycles (`MasternodeRateCheck`, `governance.cpp:747`).
  It expires as `CSuperblock::IsExpired` says for a valid trigger: 576 blocks (or one cycle if shorter) after its
  event height, or one cycle once it has been paid (its event height is at or below the tip).
- **Two-peer rule** (D3): an object is *confirmed* once two distinct peers announced it (by address). Unconfirmed
  objects are stored but not listed, tallied or used for funded heights; a later announcement by a second peer
  confirms them. `sync_state` reports how many wait.
- **Funded height** (dash-qt `getProposalFundedHeight`, `node/interfaces.cpp:446`): the event height of a
  confirmed, valid, unexpired trigger at or below the tip that lists the proposal. `funded_height` reads only such
  triggers, which closes MG-01's precondition.

## 4. Votes and the tally

### 4.1 Which votes are kept (`store.rs`)

The store gets the **whole tip list**, PoSe-banned masternodes included, on every list change
(`set_masternode_list`): per voting key id, the masternodes that hold it (proTxHash, type, valid). MG-01's
`set_voting_keys(outpoint → key)` goes: the list has no outpoints. A vote is kept when:

- it is a funding vote with outcome yes, no or abstain on a proposal (MG-01 D13);
- its time is at most an hour ahead (`MAX_TIME_FUTURE_DEVIATION`);
- it is newer than the current vote of its (object, outpoint), or as new with a higher outcome code
  (`ProcessVote`, `object.cpp:421`);
- its compact signature recovers a voting key id that the list holds (Core checks the masternode's own key; D1);
- if its outpoint is one the wallet resolved (`set_resolved_outpoints`, MG-03), it recovers that masternode's key;
- the (object, key) pair has fewer than `2 × n_K + 2` outpoints already (`n_K` masternodes hold the key), so one
  key's holder cannot grow the store without bound (D11).

On a list change, votes whose key no longer belongs to any masternode are dropped (Core `RemoveInvalidVotes` and
`ClearMasternodeVotes`), and their hashes become unseen so a newer vote can be fetched.

### 4.2 Weights (`tally.rs`)

Core counts each current vote with the voting weight of the masternode at its outpoint: 1 regular, 4 evonode
(`CountMatchingVotes`, `object.cpp:880`). Per (object, key K) the wallet sees `c` outpoints voting with K, and the
list gives K's `n_K` masternodes with their weights:

| Case | Each vote counts | Exact |
|---|---|---|
| The outpoint was resolved (MG-03) | that masternode's weight | yes |
| `c ≤ n_K` and K's masternodes all have weight `w` | `w` | yes, if the votes are genuine (D1) |
| `c ≤ n_K`, mixed types | between the lowest and the highest weight | no |
| `c > n_K` | between 0 and the highest weight | no |

The tally reports, per outcome, a `WeightRange { min, max }` and an **estimate**: exact votes at their weight;
ranged votes at the most common weight of K's masternodes (ties to the lower), counting at most `n_K` of them
(newest first). Every number is "from the votes peers sent" (MG-00 D45): completeness of the vote set is never
claimed. Unique voters (regular, evonode) follow the same attribution.

The passing threshold is Core's `max(nGovernanceMinQuorum, m_valid_weighted / 10)` from the list's valid entries
(`UpdateSentinelVariables`, `object.cpp:968`), exact. The fundable set is node `getFundableProposalHashes` over
`GetApprovedProposals` (MG-01 `tally::fundable`), computed on the estimates, and "too close to call" (MG-00 D45) is
whether the minimum or maximum net yes moves a proposal across the threshold or changes the fundable set.

### 4.3 Casting votes

Signing a vote needs the voting key: the wallet's keychain key (`derive_provider_key_at_index`) or an attached key
(`Vault::masternode_key`, a `RevealSecret` grant today, MG-01 D11). The grant purpose `Governance` and the scope
`MasternodeVoting` are MG-05's row, so MG-02 ships `vote` as a contract stub that returns
`governance.not_implemented{call: "Governance.vote"}` (D6). What MG-02 does ship:

- `voting_masternodes(wallet, proposal)`: the masternodes whose voting key the wallet derives (keychain indexes
  0–19, the provider-key gap the keychain view uses), with each one's last vote seen (MG-00 D45);
- `relay_vote(vote)`: a signed vote from outside (Core's `voteraw`), checked as the store checks a peer's vote, then
  announced to up to 3 peers and served to their `getdata`;
- the vote builder and digest (`GovernanceVote::funding`, `signing_digest`), which MG-05 calls with the key.

## 5. The facade

The facade is plain Rust in `dw-engine/src/governance/`, one handle per network session
(`NetworkSession::governance()`), with the m4 conventions: records derive serde, unit enums are snake-case strings,
every call returns `Result<_, GovernanceError>`, sync calls read memory only, async calls run on the engine
runtime. The contract lists the calls, records and codes.

- **Sync lifecycle.** `set_sync_enabled(true)` persists `governance.sync_enabled` in app.sqlite and starts the sync
  task; the session starts it at open when the setting is on. The task waits for the tip and the list, feeds the
  store the list on every list change (the pump's masternode-count refresh calls it), and runs §2.3. Disabling stops
  the task and keeps the store.
- **Reads.** `clock`, `info`, `proposals`, `proposal`, `current_votes` and `sync_state` read the store, the list
  snapshot and the sync tracker's tip. Proposal rows and tallies are answered only once a run reached Synced
  (`governance.not_synced{phase}` before; MG-00 D21: no partial tally). The clock and the budget need only the tip.
- **Estimates of time.** Every ETA and the proposal epochs use Core's trigger estimate of 157.2 s per block
  (`2.62 * 60`, `signing.cpp:59`; MG-01 D18), so a proposal's end epoch and the trigger builder agree (D9). The
  cutoff is named the **estimated cutoff** (`voting_cutoff_estimate`, MG-00 D19).
- **Superblock budget.** `superblock_budget(network, height)` is MG-01's `GetPaymentsLimit` port, a free function;
  the vectors of `governance_budget.json` and `governance_live.json` check it.
- **Event.** `EngineEvent::Governance { network }`, coalesced to at most one a second with the trailing edge kept,
  after any change of the store, the sync state or the list (D7).

## 6. Console commands

The console answers these with Core's result shapes (`rpc/governance.cpp` at the tag) from the facade:

| Command | Answer |
|---|---|
| `getgovernanceinfo` | Core's object; `fundingthreshold`, `governancebudget`, `lastsuperblock`, `nextsuperblock`, `superblockcycle`, `superblockmaturitywindow`, `targetSpacing`, `proposalfee`, `relayRequiredConfs`, `requiredConfs` |
| `getsuperblockbudget <height>` | the budget in DASH |
| `gobject count [json\|all]` | counts of the synced, confirmed objects |
| `gobject list [valid\|funding\|delete\|endorsed\|all] [proposals\|triggers\|all]` | Core's object per hash; vote counts are the estimates; `fBlockchainValidity` is the local validity (D3) |
| `gobject get <hash>` | Core's object, with `FundingResult` etc. from the tally |
| `gobject getcurrentvotes <hash> [txid vout]` | `hash: "outpoint:time:outcome:signal:weight"` (MG-01 D10) |
| `voteraw <txid> <n> <hash> funding <outcome> <time> <sig>` | `relay_vote`; Core's "Voted successfully" or its error |

`gobject vote-many`, `vote-alias`, `vote-conf`, `prepare`, `submit`, `check`, `deserialize` stay "Not available in
SPV mode." until MG-05/MG-06 (the vote and proposal flows). Without governance sync on, every governance command
answers Core's error for a disabled governance manager. The signals `delete` and `endorsed` match nothing (the
wallet tallies funding votes only), as an SPV node would honestly report.

## 7. Tests and evidence

| What | How |
|---|---|
| Transport, request rules, bounds, liveness, phases | `dw-governance` unit tests against `dw_p2p::testing::MockPeer` (unrequested items dropped, a peer closed after `MAX_UNSOLICITED`, `notfound` and timeouts move to the next announcer, idle → Failed, no Synced after failure, dead peer closed by the idle timeout under paused time). |
| Object rules | unit tests per Core rule, plus every object of the regtest suite and of `governance_live.json` validates. |
| Votes, orphans, tally ranges | unit tests (shared keys, mixed types, more outpoints than masternodes, banned masternodes counted, list changes, orphan expiry and eviction, per-peer cap). |
| Vote signatures | MG-01's regtest vector, plus a **testnet** vector of raw votes captured by one per-object `govsync` (MG-01 D12), signature recovered byte for byte. |
| Facade contract | `dw-engine/tests/mg_governance_contract.rs`: the generated surface, call rows, codes, stub calls (as m4's). |
| Console | `dw-console/tests/console_governance.rs` against a fixture store. |
| T1 | `regtest/functional/dwd_governance.py`: 4 masternodes, proposals by node 0, votes by the masternodes (including an evonode-weight check when the framework offers one), `voteraw` relayed by the wallet, the wallet's tallies equal `gobject list`'s, the superblock's trigger makes the proposal Funded, `getsuperblockbudget` and `getgovernanceinfo` equal. |
| G7 | §8. |

## 8. G7 on mainnet

The reference node of the program (`/work/chains/mainnet`) is still in initial block download (block 2,094,670 of
about 2,558,000 on 2026-10-16) and has no governance data, so the reference is DashCentral's budget API, which reads a
mainnet Core node, as r2's G7 did (D14). Read-only, concurrency 1: one `dwcli` run from an empty data dir (dash-spv
headers and list, then govsync from 3 masternodes), and one reference fetch within a minute of it. Pass: every
proposal's yes and no estimate equals the reference, and the reference lies inside the range; the report lists the
ranged votes, the bytes, the times and the peers.

## 9. Slices

| # | Commits | Contents |
|---|---|---|
| 1 | `docs(design)`, `feat(dw-p2p)`, `refactor(dw-governance)`, `feat(dw-governance)` | This file and the contract draft; `Session::bytes_received`; govsync on `Session` with §2's rules (net.rs deleted); object validation, the list-keyed store, the ranged tally, 157.2 s. **Checkpoint.** |
| 2 | `feat(dw-engine)`, `feat(dw-console)`, `test` | The facade, the event, the contract test and §3 surface; the console commands. |
| 3 | `test(regtest)`, `test(dw-governance)`, `docs(research)` | The 4-masternode suite; the testnet vote vector; G7. |

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | A vote counts when its signature recovers a voting key of the tip list; it is not tied to its outpoint unless the wallet resolved that outpoint. | The SML has no collateral outpoints (§1). Deriving every masternode's outpoint means downloading every ProRegTx block (MG-00 D43's method, thousands of blocks). A key holder can sign votes for outpoints that are not its masternodes; Core drops them, the wallet counts them against the key's own masternodes only (D11, §4.2), so the error is bounded by that holder's own weight. |
| D2 | Report per-outcome weight ranges and an estimate (MG-00 D45); the estimate takes the most common weight of the key's masternodes. | A key shared by an evonode and a regular masternode, or used for more outpoints than it has masternodes, leaves the weight open. Mainnet r2's G7 matched Core exactly with key mapping, so ranges are expected to be rare; the report counts them. |
| D3 | An object counts once two distinct peers announced it; objects are full-synced from every fresh peer. | Collateral needs a transaction index, and BIP158 filters omit `OP_RETURN` outputs, so the wallet cannot see the burn. Core relays an object only after its collateral checked out, so two random masternodes announcing it stand in for that check. A single peer can invent objects (votes cannot be forged, so an invented proposal would show no votes, but it would still be listed). The extra full syncs cost a few hundred inv entries. |
| D4 | Triggers are not checked against an operator signature. | The signer is named by its collateral outpoint, which the SML lacks; trying every operator key costs thousands of BLS verifications per trigger, which a peer could make the wallet spend. Structure, the two-peer rule and Core's expiry stand in; a funded height comes only from such triggers. |
| D5 | The object type is the data's `type`. | Core overwrites the wire field with it (`LoadData`); a peer could otherwise send a trigger labelled as a proposal. |
| D6 | `vote` is a contract stub until MG-05; `relay_vote` and `voting_masternodes` are implemented. | Signing needs the `Governance` grant purpose and the `MasternodeVoting` scope, MG-05's row; no existing purpose fits (`RevealSecret` would hand the key out). Relaying a vote signed outside needs no grant, and it exercises the whole relay path on regtest. |
| D7 | Add `EngineEvent::Governance { network }`; dw-ffi drops it (the M3 FFI is out, MG-01); dw-app maps it to `AppEvent::Governance` and the generated TS gains it. | MG-01 D3 left the event to this contract. dw-app's `From<EngineEvent>` is total, so the variant needs an AppEvent; binding the facade's calls is E0-13's. |
| D8 | `GovernanceError` is the facade's own enum and wraps `EngineError`; `EngineError` gains no variant. | As m4's domain errors; it keeps the m1–m3 §4 tables and the Swift codes unchanged. |
| D9 | 157.2 s per block for every governance ETA and the proposal epochs; `getgovernanceinfo.targetSpacing` stays 150. | Core's trigger builder decides payability with `now + blocks × 2.62 × 60` (`signing.cpp:59`); a different estimate puts end epochs on the wrong side of it. |
| D10 | Unsolicited objects and votes are dropped; more than 1,000 from one peer close it. | Core ignores them (no penalty). Ignoring still costs a decode for the hash, so a bound stops a peer from making the wallet decode forever. |
| D11 | At most `2 × n_K + 2` outpoints per (object, voting key). | The store's memory per key holder is bounded; honest keys never come close (`c ≤ n_K`). |
| D12 | Requests time out after 30 s and move to the next announcer; the governance session idles out after 5 minutes. | MG-01 D17: stuck pending hashes and undetected dead peers. Core's per-type request interval is of that order; dw-p2p pings every 60 s here, so a live peer is never idle. |
| D13 | A run that does not complete both phases is Failed, keeps what it stored, and retries with a 30 s → 5 min backoff. | MG-01 D17 "do not report Synced after a failed phase". |
| D14 | G7's reference is DashCentral's budget API. | The program's mainnet oracle is in IBD at block 2,094,670 (2026-10-16) and serves no governance data; DashCentral reads a mainnet Core node, as in r2's G7. |
| D15 | `GOVERNANCE_UPDATE_MIN` is not mirrored. | MG-01 D13: Core times it with its own receive clock. |
| D16 | Votes of PoSe-banned masternodes count; the threshold uses valid weight only. | Core: `CountMatchingVotes` uses `GetMNByCollateral` (all entries), `UpdateSentinelVariables` uses `m_valid_weighted`. |
