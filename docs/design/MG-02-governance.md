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
- Sends never wait: `Session::try_send` queues the message (a bounded queue, 4,096 for governance peers) and
  closes the session when the queue is full, and the writer closes a session whose socket accepts nothing for the
  idle timeout. A peer that stops reading cannot stall the sync loop (review item). These are the dw-p2p changes.
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
  (Core `MAX_GETDATA_SZ`), and at most 5,000 requests are in flight per peer; the rest wait in that peer's queue and
  go out as answers arrive.
- An item is accepted only when it answers a request **to that peer**, now or earlier (a late delivery, below).
  Anything else is unsolicited: it is dropped after decoding its hash. Unsolicited, undecodable and invalid votes (a
  malformed vote, a vote whose signature does not verify) count as misbehaviour, and so do unsolicited and
  undecodable objects; a peer past
  1,000 is closed (D10). A vote signed by a key the list does not hold is *suspect*, not misbehaviour (the list may
  lag), with its own bound of 10,000 per peer. A vote Core relays but the wallet does not tally (§4.1) is *ignored*:
  no penalty, and its hash is remembered so it is not fetched again (D20). An object that decodes but that the wallet refuses is remembered (up to 10,000, oldest
  forgotten first) and costs no penalty (D35).
- A request ends when its item arrives, when the peer answers `notfound` for it, when the item does not decode or
  does not validate, when the peer closes, or when it times out. The timeout is `REQUEST_TIMEOUT` = 30 s after the
  peer's **last delivery** (not after the request was sent), so a peer working through a long queue is not timed
  out. Then the next peer that announced the hash is asked (MG-01 D17 "clear a pending hash"); a hash no other peer
  announced is asked of the same peer once more, then given up (D12). A vote's hash leaves out its signature, so a
  refused signature is retried with the next announcer too.
- An item that arrives after its request timed out is still accepted from the peer that was asked (Core answers a
  long queue in order). Items given up during a sync phase are counted, and a phase with a given-up item that no
  peer delivered fails (D13); an item the wallet refused never fails it (D35).
- A peer that does not answer a `govsync` with its `ssc` within `ANSWER_TIMEOUT` = 60 s is closed. No peer is
  connected in its place during the run: its object sync is dropped (the other fresh peers' syncs still count),
  and its vote syncs are asked again of another connected peer unless it announced and delivered every vote.
- Every peer that announced an object counts towards its confirmation (§3), whether or not it was the one asked.
- An orphan vote makes the wallet ask its sender for the object (`govsync <hash>` with the empty filter), once per
  object and peer, as Core does, at most 100 per peer and run.
- Vote invs count towards a peer's announced votes only while that peer is answering a vote `govsync`; relayed
  votes arriving at other times are fetched but not counted, so they cannot complete a phase early (D27). Core
  leaves a vote it already announced on the connection out of a later vote sync but still counts it in `ssc`, so
  such a relay is credited to the sync of its object once the wallet knows that object (D36).

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
- An object confirmed after the objects phase (a second announcer relayed it) that is a current proposal gets its
  vote `govsync` then, from a peer not asked for that object within the window.
- Objects only one peer announced at the end of the objects phase are asked of the other connected peers with
  `govsync(hash, empty filter)` (at most 200 per run): a peer that has the object announces it and confirms it; a
  peer synced earlier in the window would otherwise never announce it again (D25).
- A run with no fresh peer reaches Synced only if an earlier run of the session did (the store is kept); otherwise it
  fails and retries, so a retry within the window never reports a store that was never filled.
- **Synced**: only after both phases completed. A phase that idles out (no progress for 20 s while counts are short;
  chatter that stores nothing new does not reset it), passes its 30-minute deadline, gave up an item, or loses
  every peer ends the run as **Failed** with the reason, and the store keeps what arrived (MG-01 D17 "no `Synced`
  after a failed phase"). The next run starts after a backoff (30 s, doubling to 5 min).
- The wallet's own votes (`relay_vote`) are announced only once the run is Synced. Core skips announcing an item
  back to a peer that announced it, and a vote announced before that peer's vote sync would be left out of its
  answer while still counted in its `ssc`, so the phase could never complete (D24).
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

- **Any object**: the data parses as JSON (UniValue's nesting limit of 512, matched with serde_json's
  `unbounded_depth` and a depth check of our own; a test bounds the parser's stack for the deepest accepted object)
  and is an object, or Core's legacy form `[["name", {…}]]`, whose inner object Core reads (`GetJSONObject`); its
  `type` is an integer (`LoadData`), and that `type` is the object's type (D5). Integers are read from their text
  as UniValue's `getInt` (`from_chars`, whole token): `-0` and negative values parse, `4224.0` and out-of-range
  values do not. The parent hash and the revision are **not checked**, by Core or by the wallet: both are part of
  the object hash, and Core relays any value. Refused objects are logged.
- **Proposal** (`ValidateProposal(hex, fCheckExpiration=false, fAllowScript=true)`, `object.cpp:1000`): the data a
  top-level JSON object (the legacy array form is refused for proposals, as `ValidateProposal` does); data
  at most 512 bytes; `type` 1; `name` a string of 1–40 characters of `[-_a-z0-9]` after lower-casing; `start_epoch`
  and `end_epoch` JSON integers with end > start; `payment_amount` a JSON number above zero; `payment_address` a
  string without whitespace that decodes to a P2PKH or P2SH destination of this network; `url` a string without
  whitespace, at least 4 characters, passing `CheckURL`. Expired proposals are kept, as Core keeps them until its
  own clean-up.
- **Trigger** (`CSuperblock` constructor, `superblock.cpp:75`, `ParsePaymentSchedule`): `type` 2; integer
  `event_block_height`; `payment_addresses`, `payment_amounts` and `proposal_hashes` strings split on `|` into equal,
  non-empty lists of valid destinations (surrounding spaces allowed, as Core's base58 decoder skips them, unlike a
  proposal's address), `ParsePaymentAmount` amounts and 64-hex hashes. Its creation time is not
  more than an hour ahead and not older than two superblock cycles (`MasternodeRateCheck`, `governance.cpp:747`).
  It expires as `CSuperblock::IsExpired` says for a valid trigger: 576 blocks (or one cycle if shorter) after its
  event height, or one cycle once it has been paid (its event height is at or below the tip); off mainnet, a trigger
  more than two cycles ahead is expired too. The event height is any 32-bit integer, negative included (Core's
  `getInt<int>`); a negative one simply never funds.
- **Erasure** (`UpdateCachesAndClean`, D23): an expired proposal is erased 10 minutes after it expired
  (`GOVERNANCE_DELETION_DELAY`) and its hash is kept for the session; a trigger is erased once it expires (a cycle
  after its height, D17) and its hash is kept until two cycles and 10 minutes after its creation
  (`mapErasedGovernanceObjects`). A kept hash is never fetched again. Votes on erased or
  expired objects are ignored (`governance.cpp:843`).
- **Bounds** (D22): unconfirmed objects are capped at 500 per announcing peer, 2,000 and 32 MiB in all, and dropped
  an hour after they arrived if no second peer confirmed them; a further object is refused (`store_full`) without
  penalty.
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

- its signal is in 0–4 and its outcome in 0–3 (Core `IsValid`; others are malformed);
- its time is at most an hour ahead (`MAX_TIME_FUTURE_DEVIATION`);
- it is a funding vote with outcome yes, no or abstain on a live proposal (MG-01 D13). Votes Core relays that the
  wallet does not tally (other signals, `NONE`, votes on triggers, on expired or erased proposals) are **ignored**:
  dropped without penalty, their hash remembered (D20, D21);
- it carries an ECDSA compact signature; a BLS signature (an operator's) on a proposal is malformed, and on an
  unknown parent unverifiable (D26), since the wallet never handles operator keys;
- its signature recovers a voting key id that the list holds (Core checks the masternode's own key; D1);
- if its outpoint is one the wallet resolved (`set_resolved_outpoints`, MG-03), it recovers that masternode's key;
- it is newer than the current vote of its (object, outpoint, key), or as new with a higher outcome code
  (`ProcessVote`, `object.cpp:421`); older is obsolete;
- for a new unresolved (object, outpoint, key), the (object, key) pair has fewer than `2 × n_K + 2` unresolved
  outpoints already (`n_K` masternodes hold the key), so one key's holder cannot grow the store without bound
  (D11).

Votes are kept per **(outpoint, voting key)**, not per outpoint (D1, D19): a key holder signing for another
masternode's outpoint adds a competing claim next to that masternode's own vote instead of replacing it. The check
runs in two steps so signature recovery happens without the store lock: `precheck` (duplicate, time, shape,
ignored), `recover`, then `add_checked` (which repeats the cheap checks).

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
| `c ≤ n_K`, mixed types, or a revoked masternode among them (type not in the list, D34) | between the lowest and the highest weight | no |
| `c > n_K` | between 0 and the highest weight | no |
| The outpoint has claims under more than one key (D19) | each claim between 0 and its key's highest weight; the outpoint is one voter | no |

Here `c` counts K's claims on unresolved outpoints, contested ones included, and `n_K` excludes K's masternodes
whose outpoints the wallet resolved. Of a contested outpoint's claims, the estimate counts the one whose key is not oversubscribed
(`c ≤ n_K`) if exactly one is, and none otherwise.

The tally reports, per outcome, a `WeightRange { min, max }` and an **estimate**: exact votes at their weight;
ranged votes at the most common weight of K's masternodes (ties to the lower, revoked ones counted as regular),
counting at most `n_K` of them
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
- `relay_vote(vote)`: a signed vote from outside (Core's `voteraw`), checked in Core's `voteraw` order (the object,
  the base64, the signature and key, the banned masternode, the time, then the object's checks: duplicate, expired,
  obsolete, same timestamp, the hourly update limit; D31, D37), then announced to up to 3 peers and served to their
  `getdata`;
- the vote builder and digest (`GovernanceVote::funding`, `signing_digest`), which MG-05 calls with the key.

## 5. The facade

The facade is plain Rust in `dw-engine/src/governance/`, one handle per network session
(`NetworkSession::governance()`), with the m4 conventions: records derive serde, unit enums are snake-case strings,
every call returns `Result<_, GovernanceError>`, sync calls read memory only, async calls run on the engine
runtime. The contract lists the calls, records and codes.

- **Sync lifecycle.** `set_sync_enabled(true)` persists `governance.sync_enabled` in app.sqlite and starts the sync
  task; the session starts it at open when the setting is on. A follower, running from session open to close
  whether sync is on or off, polls the tip and the list every 2 s (the list is read again only when the tip moved,
  and fed to the store only when it changed), so `info`'s counts and threshold and `voting_masternodes` do not
  depend on the task. The task waits until the followed list is not empty, then runs §2.3. A synced run that
  loses every peer reconnects after 5 s; a failed one backs off. Disabling stops the task and keeps the store. The session holds this state (`governance/sync.rs`), so the only shared engine edits are the
  session's field, its open and close, and the event variant.
- **Voting masternodes.** `voting_masternodes` derives voting keys 0–19 through the keychain
  (`masternode_keys`), hashes them to key ids and matches the tip list's `voting_key_id`; each match carries the
  key's latest vote seen on the proposal.
- **Relaying.** `relay_vote` refuses a vote whose proposal the store does not hold confirmed (`unknown_object`) or
  that is a trigger (`not_a_proposal`), then runs the store's own checks; the vote is queued for announcement after
  Synced (D24).
- **Reads.** `clock`, `info`, `proposals`, `proposal`, `current_votes` and `sync_state` read the store, the list
  snapshot and the sync tracker's tip. Proposal rows and tallies are answered only once a run reached Synced
  (`governance.not_synced{phase}` before; MG-00 D21: no partial tally). The clock and the budget need only the tip.
- **Estimates of time.** Every ETA uses Core's trigger estimate of 157.2 s per block (`2.62 * 60`,
  `signing.cpp:59`; MG-01 D18, D9). The cutoff is named the **estimated cutoff** (`voting_cutoff_estimate`, MG-00
  D19).
- **Superblock budget.** `superblock_budget(network, height)` is MG-01's `GetPaymentsLimit` port, a free function;
  the vectors of `governance_budget.json` and `governance_live.json` check it.
- **Event.** `EngineEvent::Governance { network }`, coalesced to at most one a second with the trailing edge kept,
  after any change of the store, the sync state or the list (D7). It is engine-side: dw-ffi drops it, dw-app maps it
  to `AppEvent::Governance` and the renderer invalidates nothing yet.
- **Info counts.** `passing` counts Passing and Funded, `failing` Failing and Voting, `unfunded` Unfunded; `total`
  is the current proposals. A proposal is a close call when the range of its net yes crosses the threshold, or the
  fundable set computed on every proposal's minimum or maximum differs from the one on the estimates for it.
- **`dwcli governance`** turns sync on, waits for Synced (and optional proposal and voter counts), prints the
  records as JSON lines, and can relay a vote and run console lines in the same session: the regtest suite and G7
  use it.

## 6. Console commands

The console answers these with Core's result shapes (`rpc/governance.cpp` at the tag) from the facade:

| Command | Answer |
|---|---|
| `getgovernanceinfo` | Core v24's eight keys in its order: `governanceminquorum`, `proposalfee`, `superblockcycle`, `superblockmaturitywindow`, `lastsuperblock`, `nextsuperblock`, `fundingthreshold` (`valid weight / 10`, no quorum floor, as Core prints it), `governancebudget` |
| `getsuperblockbudget <height>` | the budget in DASH; answers with sync off, since Core reads only the chain for it |
| `gobject count [json\|all]` | Core's `ToJson` keys or `ToString` text; proposals only, `triggers`, `other` and `erased` are 0 (D33) |
| `gobject list [valid\|funding\|delete\|endorsed\|all] [proposals\|triggers\|all]` | Core's object per hash, in Core's map order; vote counts are the estimates; `fBlockchainValidity` is the local validity (D3) |
| `gobject get <hash>` | Core's object, with `FundingResult` from the tally; `fCachedFunding` is `net yes >= passing threshold` (Core's rule) |
| `gobject getcurrentvotes <hash> [txid vout]` | Core's vote hash → `"outpoint:time:outcome:signal:weight"` (MG-01 D10); the filter needs both txid and vout, as in Core |
| `voteraw <txid> <n> <hash> funding <outcome> <time> <sig>` | `relay_vote`, with Core's argument checks and order; Core's "Voted successfully" or its error (D30, D31) |

`gobject vote-many`, `vote-alias`, `prepare`, `list-prepared`, `submit`, `check`, `deserialize` and `diff` stay
"Not available in SPV mode." until MG-05/MG-06 (the vote and proposal flows); `vote-conf` is not a v24 command and
gets Core's "Must be a valid command". With governance sync off every governance command but `getsuperblockbudget`
answers -1 (D29); before the first Synced, -10 (`RPC_CLIENT_IN_INITIAL_DOWNLOAD`) with the phase. The signals
`delete` and `endorsed` match nothing (the wallet tallies funding votes only), as an SPV node would honestly report.

## 7. Tests and evidence

| What | How |
|---|---|
| Transport, request rules, bounds, liveness, phases | `dw-governance` unit tests against `dw_p2p::testing::MockPeer` (unrequested items dropped, a peer closed past its misbehaviour bound, `notfound` and timeouts move to the next announcer, late deliveries accepted, given-up items fail the phase, mute peers closed by the answer timeout, the phase deadline, ignored votes cost no penalty, own votes wait for Synced, one-peer objects confirmed by the others, idle → Failed, no Synced after failure, dead peer closed by the idle timeout under paused time; the fake node mirrors Core's known-inv filter). |
| Object rules | unit tests per Core rule, plus every object of the regtest suite and of `governance_live.json` validates. |
| Votes, orphans, tally ranges | unit tests (shared keys, mixed types, more outpoints than masternodes, banned masternodes counted, list changes, orphan expiry and eviction, per-peer cap). |
| Vote signatures | MG-01's regtest vector, plus a **testnet** vector of raw votes captured by one per-object `govsync` (MG-01 D12), signature recovered byte for byte. |
| Facade contract | `dw-engine/tests/mg_governance_contract.rs`: the generated surface, call rows, codes, stub calls (as m4's). |
| Console | `dw-console/tests/console_governance.rs` against a fixture store. |
| T1 | `regtest/functional/dwd_governance.py`: 4 masternodes, three regular and one evonode (added with `dynamically_add_masternode(evo=True)`; the framework's static masternodes are all regular), proposals by node 0, votes by the masternodes, `voteraw` signed with the voting key (`signmessage`: off testnet the vote digest is the signed-message hash of `GetSignatureString`) and relayed by the wallet, the wallet's tallies equal Core's `FundingResult` (the evonode's vote at weight 4, every weight exact), the superblock's trigger makes the proposal Funded at 1500, `getsuperblockbudget` and `getgovernanceinfo` equal. Real time and Core's default regtest budget parameters: the wallet checks times against its clock and cannot see `-budgetparams`; with real time, sporks 2 and 19 are switched off (no quorums lock anything, and enforced ChainLocks hold unlocked transactions out of blocks for 10 minutes). |
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
| D1 | A vote counts when its signature recovers a voting key of the tip list; it is not tied to its outpoint unless the wallet resolved that outpoint. Votes are kept per (outpoint, voting key) (D19). | The SML has no collateral outpoints (§1). Deriving every masternode's outpoint means downloading every ProRegTx block (MG-00 D43's method, thousands of blocks). A key holder can sign votes for outpoints that are not its masternodes; Core drops them. The wallet keeps them as claims beside the real masternode's own vote, never in its place, and the tally ranges over them (§4.2), so the error is bounded by the holder's own `(2n + 2) × max weight` per object (D11) and no holder can hide or replace another masternode's vote. |
| D2 | Report per-outcome weight ranges and an estimate (MG-00 D45); the estimate takes the most common weight of the key's masternodes. | A key shared by an evonode and a regular masternode, or used for more outpoints than it has masternodes, leaves the weight open. Mainnet r2's G7 matched Core exactly with key mapping, so ranges are expected to be rare; the report counts them. |
| D3 | An object counts once two distinct peers announced it; objects are full-synced from every fresh peer. | Collateral needs a transaction index, and BIP158 filters omit `OP_RETURN` outputs, so the wallet cannot see the burn. Core relays an object only after its collateral checked out, so two random masternodes announcing it stand in for that check. A single peer can invent objects (votes cannot be forged, so an invented proposal would show no votes, but it would still be listed). The extra full syncs cost a few hundred inv entries. |
| D4 | Triggers are not checked against an operator signature. | The signer is named by its collateral outpoint, which the SML lacks; trying every operator key costs thousands of BLS verifications per trigger, which a peer could make the wallet spend. Structure, the two-peer rule and Core's expiry stand in; a funded height comes only from such triggers. |
| D5 | The object type is the data's `type`. | Core overwrites the wire field with it (`LoadData`); a peer could otherwise send a trigger labelled as a proposal. |
| D6 | `vote` is a contract stub until MG-05; `relay_vote` and `voting_masternodes` are implemented. | Signing needs the `Governance` grant purpose and the `MasternodeVoting` scope, MG-05's row; no existing purpose fits (`RevealSecret` would hand the key out). Relaying a vote signed outside needs no grant, and it exercises the whole relay path on regtest. |
| D7 | Add `EngineEvent::Governance { network }`; dw-ffi drops it (the M3 FFI is out, MG-01); dw-app maps it to `AppEvent::Governance` and the generated TS gains it. | MG-01 D3 left the event to this contract. dw-app's `From<EngineEvent>` is total, so the variant needs an AppEvent; binding the facade's calls is E0-13's. |
| D8 | `GovernanceError` is the facade's own enum and wraps `EngineError`; `EngineError` gains no variant. | As m4's domain errors; it keeps the m1–m3 §4 tables and the Swift codes unchanged. |
| D9 | 157.2 s per block for every governance ETA (superblock, cutoff); the proposal epochs of the Create Proposal form and `getgovernanceinfo.targetSpacing` keep 150 s. | Core's trigger builder estimates superblock times with `now + blocks × 2.62 × 60` (`signing.cpp:59`), and the ETAs should agree with it. dash-qt's form sets epochs at 150 s a block (`proposalcreate.cpp`) and counts payments with 150 s (`Proposal::paymentsRequested`); epochs at 157.2 s would make a proposal of `n` payments read as `n + 1` there. |
| D10 | Unsolicited objects and votes are dropped. Unsolicited, undecodable, malformed or invalid items count as misbehaviour, and a peer past 1,000 is closed. A vote whose key the list does not hold is suspect, with its own bound of 10,000 per peer; ignored votes (D20) count for nothing. Signature recovery runs outside the store lock. | Core ignores unsolicited items and penalizes invalid ones (a peer is banned after a few). The wallet's bound is looser, because a vote signed by a key its list does not know yet is not misbehaviour: the list may lag. The suspect bound still stops a peer from making the wallet recover signatures without end, and running recovery unlocked keeps the reads responsive while it does. |
| D11 | At most `2 × n_K + 2` unresolved outpoints per (object, voting key); resolved outpoints are exact and not limited. | The store's memory per key holder is bounded; honest keys never come close (`c ≤ n_K`). |
| D12 | A request times out 30 s after the peer's last delivery and moves to the next announcer; with no other announcer, the same peer is asked once more, then the item is given up. At most 5,000 requests are in flight per peer. A peer that leaves a `govsync` unanswered for 60 s is closed. A sync phase fails past 30 minutes. The governance session idles out after 5 minutes. | MG-01 D17: stuck pending hashes and undetected dead peers. Core answers a `getdata` in order, so a peer with a long queue is slow but alive: timing from the request would time out healthy peers on a full sync, timing from the last delivery does not. Core's per-type request interval is of that order; dw-p2p pings every 60 s here, so a live peer is never idle. |
| D13 | A run that does not complete both phases is Failed, keeps what it stored, and retries with a 30 s → 5 min backoff. An item given up during a phase fails the phase unless it still arrives; an item arriving after its request timed out is accepted from the peer that was asked. | MG-01 D17 "do not report Synced after a failed phase": a phase missing an item it was promised is incomplete. Late deliveries are honest (D12) and would otherwise be thrown away and penalized. |
| D14 | G7's reference is DashCentral's budget API. | The program's mainnet oracle is in IBD at block 2,094,670 (2026-10-16) and serves no governance data; DashCentral reads a mainnet Core node, as in r2's G7. |
| D15 | `GOVERNANCE_UPDATE_MIN` is not mirrored. | MG-01 D13: Core times it with its own receive clock. |
| D16 | Votes of PoSe-banned masternodes count; the threshold uses valid weight only. | Core: `CountMatchingVotes` uses `GetMNByCollateral` (all entries), `UpdateSentinelVariables` uses `m_valid_weighted`. |
| D17 | A trigger whose height is at or below the tip counts as paid: its proposals are funded until one cycle after its height. | Core keeps an executed trigger for a cycle and an unpaid one for 576 blocks (`CSuperblock::IsExpired`); which one paid is known only from the block, which the wallet does not check. The best-voted trigger is the one paid, and trigger votes are not synced (MG-01 D7). |
| D18 | Fields outside the object hash (the collateral txid, the wire type) are as the first sender gave them. | Core's object hash leaves them out. The type is read from the data (D5); the collateral txid is shown for information only. |
| D19 | An outpoint claimed under more than one voting key counts once as a voter, each claim weighing between 0 and its key's highest weight; the estimate goes to the one claim whose key is not oversubscribed, or to none. | Keyed by (outpoint, key) (D1), the real masternode's vote and a foreign key holder's claim both stay; which one Core counts is not knowable without the outpoint's key. Changes D1, which kept one vote per outpoint and let a key holder replace another masternode's vote. Test `a_key_holder_cannot_replace_another_masternodes_vote`, `a_contested_outpoint_ranges_and_estimates_the_fitting_claim`. |
| D20 | Votes Core relays but the wallet does not tally (signals other than funding, outcome `NONE`, votes on triggers, on expired or erased proposals) are ignored: no penalty, and their hashes are remembered (up to 100,000, oldest forgotten first) so they are not fetched again. | Core accepts and relays them, so penalizing them would close honest peers, and an unremembered hash would be fetched again from every announcer. The vote hash covers the fields that made the vote ignored, so remembering it cannot hide a tallied vote. |
| D21 | Only funding votes are tallied; `delete`, `valid` and `endorsed` votes are not, so the wallet does not see an object Core deleted by vote. | MG-01 D7/D13. A deleted proposal stays listed until it expires; its funding tally is still Core's. Deletion needs a masternode supermajority to vote it; the gap is shown, not hidden (follow-up if the UI needs it). The console's `delete`/`endorsed` filters match nothing (§6). |
| D22 | Unconfirmed objects are bounded at 500 per first sender, 2,000 and 32 MiB in all, and dropped after an hour; further ones are refused without penalty. | The two-peer rule (D3) keeps objects one peer sent; without a bound one peer could fill memory with invented objects. Real counts are a few hundred objects of at most a few KiB, so honest peers never reach it. |
| D23 | Objects are erased as Core's `UpdateCachesAndClean` erases them (a proposal 10 minutes after it expired, a trigger once it expired), and their hashes kept (a proposal's for the session, a trigger's until two cycles and 10 minutes after creation); votes on them are ignored. | Matches Core, so an expired object is neither listed nor fetched again from every peer that still announces it. |
| D24 | The wallet's own votes are announced only after the run reached Synced. | Core does not announce an item back to the peer that announced it, but still counts it in that peer's `ssc`; announced during the vote phase, the wallet's vote would be counted and never sent, and the phase would never complete. |
| D25 | Objects only one peer announced when the objects phase ends are asked of the other connected peers by hash (`govsync(hash, empty filter)`), at most 200 a run. | A peer full-synced inside the fulfilled-request window answers no second full sync, so it never announces an object again; asking by hash lets it confirm the object (D3). |
| D26 | A vote with a BLS signature is malformed on a proposal and unverifiable on an unknown parent; the wallet never verifies or signs with operator keys. | Core signs trigger votes with the operator key; funding votes on proposals are always ECDSA. Without the parent the wallet cannot tell which applies, so it neither penalizes nor keeps the vote. The brief's guard: no operator-key path. |
| D27 | Vote invs count towards a peer's announced votes only while that peer is answering a vote `govsync`. | Relayed votes arrive at any time; counted outside a vote sync they would complete the phase before the synced votes arrived. |
| D28 | A non-funding ECDSA vote on an unknown parent is ignored without penalty; Core gives it 20 points. | The wallet cannot tell an unknown trigger from an unknown proposal, and a trigger's vote of another signal is valid in Core; looser than Core, never stricter, so honest peers are never penalized. |
| D29 | With governance sync off the console answers -1 "Governance is disabled: …"; Core has no such error. | Core always has a governance manager (`-disablegovernance` leaves it empty); the wallet's sync is a setting the user turns on, and an empty answer would read as "no proposals". |
| D30 | `voteraw` takes only the `funding` signal; `valid`, `delete` and `endorsed` answer "Not available in SPV mode.". | Core relays all four; the wallet tallies funding votes only (D20, D21), and the facade never handles operator-key votes (D26). |
| D31 | `voteraw`'s "Failure to find masternode in list : <outpoint>" is given only for a recovered key that only banned masternodes hold; for a key the list does not hold it is folded into "Failure to verify vote.", and a bad signature reports "Failure to verify vote." even if the masternode is banned. A vote rejected for the per-key limit, or on a trigger (refused after the base64 check, where Core would verify it), gets a wallet-worded "Error voting : …". | The SML carries no collateral outpoints, so the wallet learns the masternode only from the recovered voting key (D1): Core finds the masternode before verifying, the wallet only after. The per-key limit (D11) and the refusal of votes on triggers (D20) have no Core counterpart. Every other check and text follows Core's `voteraw`, including the `<type>:` prefix of its governance exceptions and the empty exception of a known or expired vote. |
| D32 | A vote stored with no peer connected answers `no_peers` (console -9), not success. | The vote is kept and announced to the next peer, but nothing has left the wallet yet; the caller should know. |
| D33 | `gobject count` reports `triggers`, `other` and `erased` as 0, and `votes` as the store's vote count. | The facade lists proposals only; triggers are kept for the paid check (D17) but not exposed, and erased objects are not counted (D23). The keys stay so that scripts parsing Core's shape keep working. |
| D34 | A masternode whose list entry has a null operator key (revoked) has an unknown type: its key's votes range between 1 and 4, estimated at 1. | Core's `ResetOperatorFields` (ProUpRevTx) sets the state to the legacy version, whose SML entry serializes no `nType`, so every SPV client decodes a revoked evonode as regular while Core counts its vote at 4 (`dmn->nType`). Found by the testnet vector: 7 of 11 votes on 01934d6d… come from revoked evonodes (dashd's `getcurrentvotes` weight 4). Revoked entries are banned, so the threshold is unaffected. Mainnet has 354 revoked entries and G7 matches Core with them at weight 1, hence the estimate. The wallet's own revoked evonode shows as regular in `voting_masternodes` until MG-03 resolves its outpoint. |
| D35 | An item the wallet refuses (an object that fails validation, a vote that is malformed or does not verify) is asked of its next announcer but never fails the phase; a refused object costs no penalty and its hash is remembered (up to 10,000, oldest forgotten first) so it is not fetched again. | Core relays only objects it accepted, so an object an honest peer sends and the wallet refuses means the wallet's rules differ from Core's for that object; failing the phase on it would keep the wallet Failed for as long as the object lives, and penalizing it would close honest peers. Core gives an invalid object 20 points; the wallet is looser, never stricter. Votes keep their D10 penalty: a vote's validity does not depend on rules the wallet could get wrong in the same way. |
| D36 | A vote a peer announced outside a vote sync is credited to that peer's later sync of the vote's object, once the run has decoded the vote (from any peer) and so knows its object. At most 100,000 such votes are remembered per peer. | Core answers `govsync <hash>` with `ssc(11)` = every vote of the object, but `PeerRelayInv` skips votes in the connection's known-inventory filter, so a vote relayed before the sync is counted and never announced again; without the credit that peer's sync never completes. A relay of a vote the wallet held from an earlier run and did not decode this run is not credited (its object is unknown to the run); Core relays a vote only when it first accepts it, so this needs the vote to have reached the wallet's earlier run first, and the phase deadline bounds it. |
| D37 | The wallet's own vote (`voteraw`) replacing a current vote timed less than an hour before now is refused as Core's "Masternode voting too often"; the message's `last update` is that vote's time. | Core refuses a masternode's vote update within `GOVERNANCE_UPDATE_MIN` (1 h) of when the node accepted its current vote, a time the wallet cannot see; the vote's own time is when honest nodes accepted it, give or take propagation. Without the check the wallet would store and show a vote the network refuses. Peers' votes are not checked: Core relays only votes that passed. The regtest suite compares the three refusals with Core's own `voteraw`. |
| D38 | `voteraw` checks that its `time` is an integral number in range before looking the object up; Core looks the object up first. | The facade takes a typed vote, so the console must read the time before calling it. Only a non-integral or out-of-range number on an unknown object answers differently ("JSON integer out of range" for "Governance object not found"); the JSON conversion and type check come first in both, as in dash-qt. |
