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
- Reads are bounded too (D45): the subscription is `Session::subscribe_bounded` (new in dw-p2p), which holds 64
  frames for the peer's forwarder and stops reading the socket while they wait, and the run's event channel holds
  1,024 events. Each frame's bytes, headers included, count against the peer's 16 MiB inbound budget from the read
  until the run has handled its event; a frame past the budget closes the peer.

### 2.2 Requests and what a peer may send

Core accepts an object or a vote only from a peer that announced it or that it asked
(`PeerConsumeObjectRequest`, `net_governance.cpp:188`, `:221`), and completes the request only for that peer. The
wallet keeps the same rule in `Requests`:

- `announced`: per peer, the hashes it announced and the wallet has not fetched; at most
  `MAX_PEER_ANNOUNCEMENTS` = 100,000 (Core `MAX_PEER_OBJECT_ANNOUNCEMENTS`, `net_processing.cpp:88`). Further
  announcements from that peer are dropped.
- `pending`: item (hash and kind, as Core keys requests by `CInv`, D54) → (peer, queued at, sent at). A `getdata`
  goes to one announcer at a time, at most 1,000 entries per message (Core `MAX_GETDATA_SZ`), and at most 5,000
  requests are in flight per peer; the rest wait in that peer's queue and go out as each answer, `notfound` or
  timeout frees a slot, in `getdata`s of up to 1,000 (D55). A request queued for `REQUEST_TIMEOUT` moves to another
  announcer if there is one; its peer, never asked, stays askable after the others (D57).
- An item is accepted only when it answers a request **to that peer**, now or earlier (a late delivery, below).
  Anything else is unsolicited: it is dropped after decoding its hash, with no penalty, as Core does (D59). Undecodable
  and invalid votes (a malformed vote, a vote whose signature does not verify) count as misbehaviour, and so do
  undecodable objects; a peer past
  1,000 is closed (D10). A vote signed by a key the list does not hold is *suspect*, not misbehaviour (the list may
  lag), with its own bound of 10,000 per peer. A vote whose signature the wallet recovered and then did not keep
  (obsolete, of the same time, or past its key's limit) costs no penalty, as honest peers relay such votes; its
  verdict is remembered by hash and signature, in the one form of the signatures that recover the same key (up to
  100,000, until the list's entries change or the resolved masternodes are set), so the copy is not recovered again while remembered.
  Past 25,000 such recoveries in a run (`max_unkept`), a peer's vote copies are refused before recovery, at no
  penalty, so the next announcer is asked. A vote past the sender's orphan bound is refused before its signature
  is recovered, and its object asked for (D62, D63). A vote
  Core relays but the wallet does not tally (§4.1) is *ignored*:
  no penalty, and its hash is remembered so it is not fetched again (D20). An object that decodes but that the wallet refuses is remembered (up to 10,000, oldest
  forgotten first) and costs no penalty (D35).
- A request ends when its item arrives, when the peer answers `notfound` for it, when the item does not validate (an
  item that does not decode names no hash: its request runs to its timeout), when the peer closes, or when it times out. The timeout is `REQUEST_TIMEOUT` = 30 s after the
  peer's **last delivery** (not after the request was sent), so a peer working through a long queue is not timed
  out, and at most `MAX_REQUEST_AGE` = 120 s after it was sent, so a peer delivering other items cannot hold one
  (D55). Then the next peer that announced the hash is asked (MG-01 D17 "clear a pending hash"); a hash no other peer
  announced is asked of the same peer once more, then given up (D12). A vote's hash leaves out its signature, so a
  refused signature is retried with the next announcer too.
- An item that arrives after its request timed out is still accepted from the peer that was asked (Core answers a
  long queue in order). Each `getdata` buys one answer: a copy or a `notfound` from its peer spends it, whether the
  copy is kept or refused, so a peer cannot replay a copy for it (D60); a re-announced copy is a new request, and a copy recovered and not kept is not recovered again while remembered (D62, D63). Items given up during a sync phase are counted (also after a later peer's `notfound` when an earlier one timed
  out, and until a copy is accepted, D57), and a phase with a given-up item that no
  peer delivered fails (D13); an item the wallet refused never fails it (D35).
- A peer that does not answer a `govsync` with its `ssc` within `ANSWER_TIMEOUT` = 60 s is closed. No peer is
  connected in its place during the run: its object sync is dropped (the other fresh peers' syncs still count),
  and its vote syncs are asked again of another connected peer unless it announced and delivered every vote.
- Every peer that announced an object counts towards its confirmation (§3), whether or not it was the one asked.
- An orphan vote makes the wallet ask every connected peer for the object (`govsync <hash>` with the empty filter,
  D52), once per object and peer, as Core's scheduler does, at most 100 per peer and run.
- A vote inv counts towards a peer's vote sync only when the vote is on that sync's object (D46): the store or the
  run knows the object of a vote it holds or decoded, and a vote not known yet is counted once a copy from any peer
  decodes. Each `ssc(11)` count, answered in the order of the asks, must be met by the votes on its own object; a
  vote the run gives up on without decoding it may make up for one. Votes on objects not asked of the peer are fetched
  but counted only towards a later vote sync of their object on the connection: Core leaves a vote it already
  announced there out of that sync's invs but still counts it in `ssc` (D36).
- A hash announced as an object and as a vote is two items, each asked of its own announcers (D54). A `notfound`
  ends a request sent to that peer, of its own kind (one for a request still queued there is ignored); one naming a request of the other kind ends that request as misbehaviour, and
  the request moves on with its own kind (D50).

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
- **SyncingVotes**: `govsync(hash, filter)` per current proposal (end epoch in the future), each asked of up to 3
  connected peers (Core `REALNET_PEERS_PER_HASH`, D56), spread over them, with the non-empty match-nothing filter (Core reads an empty filter as an object fetch,
  `net_governance.cpp:116`; MG-01 §7). Votes on triggers are not asked for (D7 of MG-01: not tallied).
- An object confirmed after the objects phase (a second announcer relayed it) that is a current proposal gets its
  vote `govsync` then, from up to 3 peers not asked for that object within the window.
- Objects only one peer announced at the end of the objects phase are asked of the other connected peers with
  `govsync(hash, empty filter)` (at most 200 per run, each first sender's objects in turn, D53): a peer that has the
  object announces it and confirms it; a
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
  `unbounded_depth` and a depth check of our own (D41); a test bounds the parser's stack for the deepest accepted object)
  and is an object, or Core's legacy form `[["name", {…}]]`, whose inner object Core reads (`GetJSONObject`); its
  `type` is an integer equal to the wire type, which picks the checks below; any other wire type is refused (D5). Integers are read from their text
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
  more than an hour ahead and not older than two superblock cycles (`MasternodeRateCheck`, `governance.cpp:747`);
  like Core, which skips that check until its governance is synced, the store applies it only once a sync has
  reached Synced in the session, so the first sync accepts the same triggers a fresh Core does.
  It expires as `CSuperblock::IsExpired` says for a valid trigger: 576 blocks (or one cycle if shorter) after its
  event height, or one cycle once it has been paid (its event height is at or below the tip); off mainnet, a trigger
  more than two cycles ahead is expired too. The event height is any 32-bit integer, negative included (Core's
  `getInt<int>`); a negative one simply never funds.
- **Erasure** (`UpdateCachesAndClean`, D23): an expired proposal is erased 10 minutes after it expired
  (`GOVERNANCE_DELETION_DELAY`) and its hash is kept for the session; a trigger is erased once it expires (a cycle
  after its height, D17) and its hash is kept until two cycles and 10 minutes after its creation
  (`mapErasedGovernanceObjects`). A kept hash is never fetched again. Votes on erased or
  expired objects are ignored (`governance.cpp:843`).
- **Bounds** (D22, D53): unconfirmed objects are capped at 500 and 8 MiB of data per first sender, 2,000 and 32 MiB
  in all, and dropped an hour after they arrived if no second peer confirmed them. Past a sender's share a further
  object is refused (`store_full`) without penalty; past the bounds in all, earlier runs' unvoted unconfirmed objects
  are evicted first, oldest first, and only then is it refused. An object arrives with its announcers counted: the
  peers that announced it while it was asked for, and those of the late-copy record while that has room, so one two
  peers announced is stored confirmed, outside these bounds. All objects, confirmed or not, are capped at 5,000 and
  64 MiB of data (D47): past either, objects no valid masternode's key voted on (triggers included) are evicted, those
  not announced since the last object phase began first, then the oldest, and fetched again once announced; a voted
  object is never evicted for an unvoted one, and an object that only voted ones would make room for is refused
  (`store_full`). A run that evicts an object announced in it, or refuses one past a bound in all (not a sender's
  share), fails (D51).
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
`GetApprovedProposals` (MG-01 `tally::fundable`), computed on the estimates, with each amount read as Core's trigger
builder reads it (`ParsePaymentAmount` on the number's text, `signing.cpp:72`): a valid proposal whose amount has an
exponent is skipped there, so it is never fundable here either. "Too close to call" (MG-00 D45) is
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
  loses every peer reconnects after 5 s; a failed one backs off. Disabling stops the task and keeps the store; enabling again before it has ended starts the new task once the old one ends (D39). The session holds this state (`governance/sync.rs`), so the only shared engine edits are the
  session's field, its open and close, and the event variant.
- **Voting masternodes.** `voting_masternodes` derives voting keys 0–19 through the keychain
  (`masternode_keys`), hashes them to key ids and matches the tip list's `voting_key_id`. A match carries its own
  vote only when one can be told to be its own (D49): a vote whose outpoint is resolved to it, or any vote under a key
  no other list entry holds. A shared key's latest vote is reported as the key's (`key_last_vote`), with the number
  of masternodes holding it.
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
  ranges could move it in or out of the fundable set (D48): the set computed on every proposal's minimum or maximum
  differs from the estimate's for it, or, funded, the set leaves it out with it at its minimum and every other
  proposal at its maximum, or, unfunded, takes it with it at its maximum and the others at their minimum.
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
| `gobject getcurrentvotes <hash> [txid vout]` | Core's vote hash → `"outpoint:time:outcome:signal:weight"` (MG-01 D10); in Core's order (by outpoint as `COutPoint` compares); the filter needs both txid and vout, the vout read as `LocaleIndependentAtoi<uint32_t>` reads it, as in Core |
| `voteraw <txid> <n> <hash> funding <outcome> <time> <sig>` | `relay_vote`, with Core's argument checks and order; Core's "Voted successfully" or its error (D30, D31) |

`gobject vote-many`, `vote-alias`, `prepare`, `list-prepared`, `submit`, `check`, `deserialize` and `diff` stay
"Not available in SPV mode." until MG-05/MG-06 (the vote and proposal flows); `vote-conf` is not a v24 command and
gets Core's "Must be a valid command". With governance sync off every governance command but `getsuperblockbudget`
answers -1 (D29). Until a run first reaches Synced, the `gobject` commands that read proposals or votes (`count`,
`list`, `get`, `getcurrentvotes`) and `voteraw` (whose obsolete and hourly checks need every current vote) answer -10
(`RPC_CLIENT_IN_INITIAL_DOWNLOAD`) with the phase (D40); `getgovernanceinfo` needs only a tip (-10 until there is one),
so it does not wait. The signals
`delete` and `endorsed` match nothing (the wallet tallies funding votes only), as an SPV node would honestly report.

## 7. Tests and evidence

| What | How |
|---|---|
| Transport, request rules, bounds, liveness, phases | `dw-governance` unit tests against `dw_p2p::testing::MockPeer` (unrequested items dropped, a peer closed past its misbehaviour bound, `notfound` and timeouts move to the next announcer, late deliveries accepted, given-up items fail the phase, mute peers closed by the answer timeout, the phase deadline, ignored votes cost no penalty, own votes wait for Synced, one-peer objects confirmed by the others, idle → Failed, no Synced after failure, dead peer closed by the idle timeout under paused time; the fake node mirrors Core's known-inv filter). |
| Object rules | unit tests per Core rule, plus every object of the regtest suite and of `governance_live.json` validates. |
| Votes, orphans, tally ranges | unit tests (shared keys, mixed types, more outpoints than masternodes, banned masternodes counted, list changes, orphan expiry and eviction, per-peer cap). |
| Vote signatures | MG-01's regtest vector, plus a **testnet** vector of raw votes captured by one per-object `govsync` (MG-01 D12): each vote round-trips byte for byte and hashes to a vote in dashd's `getcurrentvotes`; the recorded key ids are the wallet's own recovery at capture, so they guard against regressions rather than check against Core. |
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
| D5 | The object type is the wire type, which picks the checks; the data's `type` must equal it. | Core's `Unserialize` never runs `LoadData`, so a received object keeps its wire type and `IsValidLocally` switches on it. `ValidateProposal` then wants data type 1 (`ValidateType`) and the `CSuperblock` constructor wants a wire trigger with data type 2, so on every object Core accepts the two agree; a mismatch is refused either way. |
| D6 | `vote` is a contract stub until MG-05; `relay_vote` and `voting_masternodes` are implemented. | Signing needs the `Governance` grant purpose and the `MasternodeVoting` scope, MG-05's row; no existing purpose fits (`RevealSecret` would hand the key out). Relaying a vote signed outside needs no grant, and it exercises the whole relay path on regtest. |
| D7 | Add `EngineEvent::Governance { network }`; dw-ffi drops it (the M3 FFI is out, MG-01); dw-app maps it to `AppEvent::Governance` and the generated TS gains it. | MG-01 D3 left the event to this contract. dw-app's `From<EngineEvent>` is total, so the variant needs an AppEvent; binding the facade's calls is E0-13's. |
| D8 | `GovernanceError` is the facade's own enum and wraps `EngineError`; `EngineError` gains no variant. | As m4's domain errors; it keeps the m1–m3 §4 tables and the Swift codes unchanged. |
| D9 | 157.2 s per block for every governance ETA (superblock, cutoff); the proposal epochs of the Create Proposal form and `getgovernanceinfo.targetSpacing` keep 150 s. | Core's trigger builder estimates superblock times with `now + blocks × 2.62 × 60` (`signing.cpp:59`), and the ETAs should agree with it. dash-qt's form sets epochs at 150 s a block (`proposalcreate.cpp`) and counts payments with 150 s (`Proposal::paymentsRequested`); epochs at 157.2 s would make a proposal of `n` payments read as `n + 1` there. |
| D10 | Unsolicited objects and votes are dropped with no penalty (D59). Undecodable, malformed or invalid items count as misbehaviour, and a peer past 1,000 is closed. A vote whose key the list does not hold is suspect, with its own bound of 10,000 per peer; ignored votes (D20) count for nothing. Signature recovery runs outside the store lock. | Core ignores unsolicited items and penalizes invalid ones (a peer is banned after a few). The wallet's bound is looser, because a vote signed by a key its list does not know yet is not misbehaviour: the list may lag. The suspect bound still stops a peer from making the wallet recover signatures without end, and running recovery unlocked keeps the reads responsive while it does. |
| D11 | At most `2 × n_K + 2` unresolved outpoints per (object, voting key); resolved outpoints are exact and not limited. | The store's memory per key holder is bounded; honest keys never come close (`c ≤ n_K`). |
| D12 | A request times out 30 s after the peer's last delivery (and 120 s after it was sent, D55) and moves to the next announcer; with no other announcer, the same peer is asked once more, then the item is given up. At most 5,000 requests are in flight per peer. A peer that leaves a `govsync` unanswered for 60 s is closed. A sync phase fails past 30 minutes. The governance session idles out after 5 minutes. | MG-01 D17: stuck pending hashes and undetected dead peers. Core answers a `getdata` in order, so a peer with a long queue is slow but alive: timing from the request would time out healthy peers on a full sync, timing from the last delivery does not. Core's per-type request interval is of that order; dw-p2p pings every 60 s here, so a live peer is never idle. |
| D13 | A run that does not complete both phases is Failed, keeps what it stored, and retries with a 30 s → 5 min backoff. An item given up during a phase fails the phase unless it still arrives; an item arriving after its request timed out is accepted from the peer that was asked. | MG-01 D17 "do not report Synced after a failed phase": a phase missing an item it was promised is incomplete. Late deliveries are honest (D12) and would otherwise be thrown away and penalized. |
| D14 | G7's reference is DashCentral's budget API. | The program's mainnet oracle is in IBD at block 2,094,670 (2026-10-16) and serves no governance data; DashCentral reads a mainnet Core node, as in r2's G7. |
| D15 | `GOVERNANCE_UPDATE_MIN` is not mirrored. | MG-01 D13: Core times it with its own receive clock. |
| D16 | Votes of PoSe-banned masternodes count; the threshold uses valid weight only. | Core: `CountMatchingVotes` uses `GetMNByCollateral` (all entries), `UpdateSentinelVariables` uses `m_valid_weighted`. |
| D17 | A trigger whose height is at or below the tip counts as paid: its proposals are funded until one cycle after its height. | Core keeps an executed trigger for a cycle and an unpaid one for 576 blocks (`CSuperblock::IsExpired`); which one paid is known only from the block, which the wallet does not check. The best-voted trigger is the one paid, and trigger votes are not synced (MG-01 D7). |
| D18 | Fields outside the object hash (the collateral txid, the wire type) are as the first sender gave them. | Core's object hash leaves them out. The wire type must equal the data's (D5), which the hash covers; the collateral txid is shown for information only. |
| D19 | An outpoint claimed under more than one voting key counts once as a voter, each claim weighing between 0 and its key's highest weight; the estimate goes to the one claim whose key is not oversubscribed, or to none. | Keyed by (outpoint, key) (D1), the real masternode's vote and a foreign key holder's claim both stay; which one Core counts is not knowable without the outpoint's key. Changes D1, which kept one vote per outpoint and let a key holder replace another masternode's vote. Test `a_key_holder_cannot_replace_another_masternodes_vote`, `a_contested_outpoint_ranges_and_estimates_the_fitting_claim`. |
| D20 | Votes Core relays but the wallet does not tally (signals other than funding, outcome `NONE`, votes on triggers, on expired or erased proposals) are ignored: no penalty, and their hashes are remembered (up to 100,000, oldest forgotten first) so they are not fetched again. | Core accepts and relays them, so penalizing them would close honest peers, and an unremembered hash would be fetched again from every announcer. The vote hash covers the fields that made the vote ignored, so remembering it cannot hide a tallied vote. |
| D21 | Only funding votes are tallied; `delete`, `valid` and `endorsed` votes are not, so the wallet does not see an object Core deleted by vote. | MG-01 D7/D13. A deleted proposal stays listed until it expires; its funding tally is still Core's. Deletion needs a masternode supermajority to vote it; the gap is shown, not hidden (follow-up if the UI needs it). The console's `delete`/`endorsed` filters match nothing (§6). |
| D22 | Unconfirmed objects are bounded at 500 per first sender (and 8 MiB, D53), 2,000 and 32 MiB in all, and dropped after an hour; further ones are refused without penalty. | The two-peer rule (D3) keeps objects one peer sent; without a bound one peer could fill memory with invented objects. Real counts are a few hundred objects of at most a few KiB, so honest peers never reach it. |
| D23 | Objects are erased as Core's `UpdateCachesAndClean` erases them (a proposal 10 minutes after it expired, a trigger once it expired), and their hashes kept (a proposal's for the session, a trigger's until two cycles and 10 minutes after creation); votes on them are ignored. | Matches Core, so an expired object is neither listed nor fetched again from every peer that still announces it. |
| D24 | The wallet's own votes are announced only after the run reached Synced. | Core does not announce an item back to the peer that announced it, but still counts it in that peer's `ssc`; announced during the vote phase, the wallet's vote would be counted and never sent, and the phase would never complete. |
| D25 | Objects only one peer announced when the objects phase ends are asked of the other connected peers by hash (`govsync(hash, empty filter)`), at most 200 a run. | A peer full-synced inside the fulfilled-request window answers no second full sync, so it never announces an object again; asking by hash lets it confirm the object (D3). |
| D26 | A vote with a BLS signature is malformed on a proposal and unverifiable on an unknown parent; the wallet never verifies or signs with operator keys. | Core signs trigger votes with the operator key; funding votes on proposals are always ECDSA. Without the parent the wallet cannot tell which applies, so it neither penalizes nor keeps the vote. The brief's guard: no operator-key path. |
| D27 | Superseded by D46: vote invs counted towards a peer's vote syncs while any was open, whatever object they were on. | Sol r1 Major 4 showed that a relay of another proposal's vote could then settle a sync before its own votes came, and a peer closing then left its sync unasked again. |
| D28 | A non-funding ECDSA vote on an unknown parent is ignored without penalty; Core gives it 20 points. | The wallet cannot tell an unknown trigger from an unknown proposal, and a trigger's vote of another signal is valid in Core; looser than Core, never stricter, so honest peers are never penalized. |
| D29 | With governance sync off the console answers -1 "Governance is disabled: …"; Core has no such error. | Core always has a governance manager (`-disablegovernance` leaves it empty); the wallet's sync is a setting the user turns on, and an empty answer would read as "no proposals". |
| D30 | `voteraw` takes only the `funding` signal; `valid`, `delete` and `endorsed` answer "Not available in SPV mode.". | Core relays all four; the wallet tallies funding votes only (D20, D21), and the facade never handles operator-key votes (D26). |
| D31 | `voteraw`'s "Failure to find masternode in list : <outpoint>" is given only for a recovered key that only banned masternodes hold; for a key the list does not hold it is folded into "Failure to verify vote.", and a bad signature reports "Failure to verify vote." even if the masternode is banned. A vote rejected for the per-key limit, or on a trigger (refused after the base64 check, where Core would verify it), gets a wallet-worded "Error voting : …". | The SML carries no collateral outpoints, so the wallet learns the masternode only from the recovered voting key (D1): Core finds the masternode before verifying, the wallet only after. The per-key limit (D11) and the refusal of votes on triggers (D20) have no Core counterpart. Every other check and text follows Core's `voteraw`, including the `<type>:` prefix of its governance exceptions and the default-built exception of a known or expired vote ("Error voting : GOVERNANCE_EXCEPTION_NONE:"). |
| D32 | A vote stored with no peer connected answers `no_peers` (console -9), not success. | The vote is kept and announced to the next peer, but nothing has left the wallet yet; the caller should know. |
| D33 | The console shows proposals only: `gobject count` reports `triggers`, `other` and `erased` as 0 and `votes` as the store's vote count; `gobject list … triggers` lists nothing; `gobject get` and `getcurrentvotes` of a trigger answer as for an unknown object. | The facade lists proposals only; triggers are kept for the paid check (D17) but not exposed, since no MG screen shows them and a trigger record would widen the contract for the console alone; erased objects are not counted (D23). The keys stay so that scripts parsing Core's shape keep working. |
| D34 | A masternode whose list entry has a null operator key (revoked) has an unknown type: its key's votes range between 1 and 4, estimated at 1. | Core's `ResetOperatorFields` (ProUpRevTx) sets the state to the legacy version, whose SML entry serializes no `nType`, so every SPV client decodes a revoked evonode as regular while Core counts its vote at 4 (`dmn->nType`). Found by the testnet vector: 7 of 11 votes on 01934d6d… come from revoked evonodes (dashd's `getcurrentvotes` weight 4). Revoked entries are banned, so the threshold is unaffected. Mainnet has 354 revoked entries and G7 matches Core with them at weight 1, hence the estimate. The wallet's own revoked evonode shows as regular in `voting_masternodes` until MG-03 resolves its outpoint. |
| D35 | An item the wallet refuses (an object that fails validation, a vote that is malformed or does not verify) is asked of its next announcer but never fails the phase; a refused object costs no penalty and its hash is remembered (up to 10,000, oldest forgotten first) so it is not fetched again. | Core relays only objects it accepted, so an object an honest peer sends and the wallet refuses means the wallet's rules differ from Core's for that object; failing the phase on it would keep the wallet Failed for as long as the object lives, and penalizing it would close honest peers. Core gives an invalid object 20 points; the wallet is looser, never stricter. Votes keep their D10 penalty: a malformed or unverifiable vote is the same under any list. A vote whose key the list does not hold may only mean the list lags (D10); it is kept for a later list (D43). An object refused for its wire type alone is neither penalty-free nor remembered (D42). |
| D36 | A vote a peer announced outside a vote sync is credited to that peer's later sync of the vote's object, once the run has decoded the vote (from any peer) and so knows its object. At most 100,000 such votes are remembered per peer. | Core answers `govsync <hash>` with `ssc(11)` = every vote of the object, but `PeerRelayInv` skips votes in the connection's known-inventory filter, so a vote relayed before the sync is counted and never announced again; without the credit that peer's sync never completes. A relay of a vote the wallet held from an earlier run and did not decode this run is not credited (its object is unknown to the run); Core relays a vote only when it first accepts it, so this needs the vote to have reached the wallet's earlier run first, and the phase deadline bounds it. |
| D37 | The wallet's own vote (`voteraw`) replacing a current vote timed less than an hour before now is refused as Core's "Masternode voting too often"; the message's `last update` is that vote's time. | Core refuses a masternode's vote update within `GOVERNANCE_UPDATE_MIN` (1 h) of when the node accepted its current vote, a time the wallet cannot see; the vote's own time is when honest nodes accepted it, give or take propagation. Without the check the wallet would store and show a vote the network refuses. Peers' votes are not checked: Core relays only votes that passed. The regtest suite compares the three refusals with Core's own `voteraw`. |
| D38 | `voteraw` checks that its `time` is an integral number in range before looking the object up; Core looks the object up first. | The facade takes a typed vote, so the console must read the time before calling it. Only a non-integral or out-of-range number on an unknown object answers differently ("JSON integer out of range" for "Governance object not found"); the JSON conversion and type check come first in both, as in dash-qt. |
| D39 | Turning sync off stops the task without waiting; a start right after it spawns the new task behind the stopping one (it awaits the old handle), so two tasks never share the store and the old task's final `Disabled` never lands after the new phase. | Review finding #11. The window needs a task that is mid-run and closing its peers, which the contract test cannot reach without peers; that test covers the slot logic (a stopped task still in the slot is replaced, not taken as running), and the fix is otherwise by construction. A run stopped while it connects returns before any full sync, so turning sync off and on does not spend the peers' repeat windows. |
| D40 | Until the first Synced, the `gobject` reads and `voteraw` answer -10 "Must wait for governance data to sync with the masternode network (phase: …). Try again in a minute or so."; Core answers from whatever it has. | MG-00 D21: no partial tally. The code and wording follow Core's own "Must wait for client to sync with masternode network" (`gobject submit`, -10), with the phase added; `voteraw` waits too, since before Synced its obsolete and hourly checks would run against a partial vote set and could accept a vote Core refuses; `getgovernanceinfo` answers as Core once it has a tip. |
| D41 | UniValue's nesting limit (`MAX_JSON_DEPTH` 512, counted on the container stack) is checked by a byte scan before parsing, and serde_json then parses with its own limit of 128 off (`unbounded_depth`). | `UniValue::read` fails as soon as its stack passes 512. The scan counts the same opening brackets outside strings, so for valid JSON both accept exactly the documents no deeper than 512. Core, though, ignores `read`'s result (`object.cpp:288`, `:670`) and keeps what it parsed before the failure, so a deeper document whose fields come first passes in Core; the wallet refuses it, without penalty and remembered (D35), as it does trailing bytes. In the other direction serde_json takes lone-surrogate escapes in strings, which UniValue refuses: looser, never stricter. serde_json recurses per level, and `the_deepest_object_parses_on_a_small_stack` bounds that stack use. Enabling `unbounded_depth` (and `raw_value`) changes no other crate: the feature only adds `disable_recursion_limit`, and every other parser keeps the limit of 128. |
| D42 | An object whose data names the other type than its wire type (proposal or trigger) is refused as mistyped: the sender misbehaved, its copy is asked of the next announcer, and the hash is not remembered. | The wire type is outside the object hash (`GetHash`), so any peer can relabel a genuine object; were the refusal remembered (D35), the first peer to answer could hide a proposal and its votes for the session. Honest peers send objects their Core stored, which pass `IsValidLocally` only with agreeing types, so a mismatch is never honest; Core gives an invalid object 20 points and fetches it again on the next inv, as the wallet now does. |
| D43 | A vote on a current proposal refused only because the list does not hold its recovered key is kept, per (vote, key), up to 10,000 (1,000 per peer) and for an hour, and placed when a later list holds the key; it does not fail the phase. | Peers announce a vote once per connection, so after Synced nothing would fetch it again, and a voting key changed in the last block or two would be missing from the tally for the session. Keyed by the recovered key, a forged copy (the signature is outside the vote hash) cannot stand in for the genuine one; the bound and TTL keep invented keys from growing the store, and the per-peer share and the current-proposal rule stop one peer's junk from evicting other peers' lagging votes. A kept vote is placed with its arrival time, so a proposal that ended meanwhile still takes it; harmless, as the proposal shows as ended. |
| D44 | The gate at 5ddd95a3 lost `dw-engine` `platform::trust_tests::after_masternode_sync_an_unknown_quorum_fails_closed` to its 100 ms "failed closed at once" bound at load 95–110; it passed alone 3/3 (0.31–1.31 s with setup). The rerun at 23957e1c (load 145–196) lost two E0-04 lease session tests instead, `mode_b_a_funding_permit_holds_the_coin_fence_until_its_lock_is_built` and `r1_f3_a_hanging_hand_off_ends_at_the_deadline_as_maybe_sent`, each to its own `tokio::time::timeout` (`Elapsed(())`), and each passed alone 3/3. MG-02's gate reruns such failures alone, up to three times, through the shared BL-100 rerun list, which now holds both messages and BL-181's three tests. | The tests touch no governance code (the trust test came with main's BL-162, 2288d7fa); a wall-clock bound under this load is BL-100's case, and a test's own timeout expiring is one by definition. The gate used a local copy of the list until the manager added both messages to the shared one. |
| D45 | A governance peer's inbound frames are bounded by count and bytes: its session holds 64 frames for the forwarder and stops reading the socket while they wait (`Session::subscribe_bounded`), the run's event channel holds 1,024 events, and each frame's bytes (headers included) count against a 16 MiB budget per peer from the read until the run handled its event; a frame past the budget closes the peer. | Sol r1 Major 1: unbounded subscription and event channels let a peer queue gigabytes of `govobj` frames or 50,000-entry `inv`s while the loop parsed, since the announcement and misbehaviour limits act only after dequeue. Backpressure keeps honest peers inside the budget: everything they send is answers to the wallet's own requests (at most 5,000 in flight) and the sync invs, a few MiB for a full mainnet vote sync. The event channel is shared, so a flooding peer slows the others' events until its budget closes it; at most 16 MiB of any one peer's frames wait, plus one frame being read. |
| D46 | A vote inv counts towards a peer's vote sync only when the vote is on that sync's object, learned from the store (stored, ignored and orphan votes keep their object), from a vote decoded this run, or once any peer's copy decodes; each `ssc(11)` count must be met by its own object's votes, and a vote the run gave up on without decoding it (or did not fetch past the announcement bound) may make up for one. | Sol r1 Major 4: one counter per peer let another proposal's relay settle a sync before its own votes came, and a peer closing then had its sync counted done and not asked again. Core answers the asks in order, so each count belongs to its ask; a vote that never decodes has no known object, and without the allowance an honest peer whose vote went missing would hold the phase until its idle timeout. The allowance is per peer, and so is its risk: a peer may spend it only on votes it announced but never delivered in decodable form, so it can at most settle its own syncs early by that many votes; another peer's syncs, and the phase's request accounting (every fetched vote is still fetched), are untouched. |
| D47 | All objects, confirmed or not, are capped at 5,000 and 64 MiB of data. Past either, objects no key of a valid masternode of the current list voted on are evicted, triggers included (the wallet tallies no votes on triggers, so they always count as unvoted): first those not announced or sent since the last object phase began (a run without one keeps the last one's objects current), then the oldest. A voted object is never evicted for an unvoted one; an object that only voted ones would make room for is refused (`store_full`, no penalty). An object arrives with every peer that announced it this run counted towards its confirmation, so one two peers announced is stored confirmed and the unconfirmed bounds (D22) never refuse it; when a bound does refuse an object, its announcers stay in the late-copy record while that has room (D53), so a later announcement confirms it on arrival. An evicted hash is not remembered, so it is fetched again when announced: a listed key's vote on it arrives as an orphan, which asks every peer for it (D52). An object past the cap is therefore not shown until a listed key votes on it. MG-02 submits no objects of the wallet's own; the task that adds proposal submission must keep them out of eviction. Evicting or refusing an object announced in the current run fails the run (D51). | Sol r1 Major 2: the 500/2,000/32 MiB bounds counted unconfirmed objects only, so two peers could confirm batch after batch of fee-less proposals without end. A proposal no listed key voted on carries no weight in the tally or the budget, so evicting those first keeps every proposal that matters. Revised after the r1 validator: evicting by age alone let fakes that arrived after honest proposals, which carry no votes until the vote phase, push them out within one run; and the wallet does not check triggers' operator signatures, so never evicting triggers let forged ones fill the store for the session. Ordering by run first makes leftovers of earlier runs (a past attacker's batches) go silently. Revised after the r2 validator: an object two honest peers announced was still checked against the unconfirmed bounds as if only its sender had, so one peer's forged triggers (up to the 3 MiB frame each, not otherwise bounded) filling the 32 MiB unconfirmed bound hid every honest object silently; a run with no object phase made every object stale, so its follow phase evicted honest ones silently; and a banned masternode's key, which Core's tally leaves out, kept fakes from eviction. Mainnet holds a few hundred objects, so an honest network never reaches the cap. |
| D48 | A close call (MG-00 D45) also tests, per budget candidate, the set with it at its minimum and every other candidate at its maximum (if funded on the estimates) or the reverse (if not). | Sol r1 Major 6: the whole-set minimum and maximum keep the order of proposals whose ranges move together, so two proposals with overlapping ranges could swap their place in the budget with neither flagged. Core's budget is greedy with skips, so these 2n sets are not an exhaustive search of every ordering the ranges allow; they cover each proposal's worst and best rank, which is what the neutral indication needs. |
| D49 | `voting_masternodes` reports a vote as a masternode's own only when the vote's collateral outpoint is resolved to it (MG-03's resolution, now carrying the proTxHash), or when no other entry of the list holds its voting key; a shared key's latest vote is reported as the key's (`key_last_vote`, `key_last_vote_time`) with `key_holders`, never copied to each holder. The UI words it neutrally ("Last vote seen with this shared key: Yes (2 masternodes use this key)"). | Sol r1 Major 5: copying the key's latest vote to every holder showed a masternode as voted when another one sharing its key voted, and with the other one's outcome. Votes are signed by the key, not the masternode, and the SML carries no collateral, so only a resolved outpoint tells them apart. Under a key one entry holds, Core relays only that masternode's votes. Contract-Version 3. |
| D50 | A `notfound` ends the request of its own item (hash and kind, D54); one naming the hash as the other kind of a request to that peer counts as misbehaviour, ends it all the same, and the request moves on with its own kind. A `notfound` for an item not asked of the peer is ignored, as Core ignores one. | Sol r1 Major 3: the retry took the peer's kind, so a peer could turn an object request into a vote request to the next announcer, whose own `notfound` then settled it without counting it lost: Synced with a proposal missing. |
| D51 | A sync run whose object cap (D47) evicted or refused an object announced in that run, or whose unconfirmed bounds in all (D22; not a sender's share, D53) refused one even after evicting earlier runs' unconfirmed objects, fails (`sync incomplete: the governance store is full …`) instead of reaching or staying `Synced`; the store keeps every voted object and is shown as it is. | Validator on D47: once honest objects can be pushed out before their votes arrive, a `Synced` run could show a list missing them with nothing to tell the user. Failing makes the shortfall visible; two flooding peers can keep failing runs, which is a denial of the sync rather than a false view, and needs the cap to be passed, which an honest network does not do (D47). Accepted risk: one valid masternode's voting key voting on 5,000 fakes keeps them from eviction, so every later run fails while they last; that costs the owner nothing but is only a denial, and it shows the owner's key in every fake's votes. |
| D52 | An orphan vote's object is asked of every connected peer (`govsync <hash>` with an empty filter), once per object and peer; the vote's sender pays, at most 100 objects per sender and run; the vote's sender alone is no longer asked. | Validator r2 m2: asked of the sender only, the object came back with one announcer and stayed unconfirmed and unshown though a listed key had voted on it. Core's scheduler asks every peer for its orphan votes' objects (`net_governance.cpp:47-60`), and Core does not rate-limit an object fetch (`object_fetch` is not a fulfilled request, `:116-120`), so asking honest peers costs no penalty. Revised after the r3 validator: the budget was charged to each asked peer, so one peer's orphan votes on 100 invented objects spent every peer's budget for the rest of the run and later orphans went unasked; charged to the sender, a peer spends only its own. |
| D53 | Unconfirmed objects one peer sent first are also capped at 8 MiB of data, so the 32 MiB in all takes at least four senders; an object's announcers this run come from the run's request records as well as the late-copy record, which once full takes no new hashes instead of being cleared, so a peer cannot clear them; and the end-of-phase confirmation asks (D25) take only objects announced this run, each first sender's in turn. When the bounds in all are full, earlier runs' unvoted unconfirmed objects are evicted, oldest first, before an object is refused. | Validator r3 MJ1 and m1: one peer filled the 32 MiB alone with forged triggers (up to the 3 MiB frame each), then cleared the late-copy record of an object's other announcers by announcing and `notfound`ing 150,000 junk hashes, so its copy arrived unconfirmed and was refused unseen; and one peer's 500 unconfirmed objects could take all 200 asks of a run whose other peers were synced inside their window. The request records count against each peer's announcement limit, so no peer can grow them past its share; with the share and every announcer counted, a refusal past the bounds in all needs several senders and fails the run (D51). Real objects are a few hundred of at most a few KiB, far below the share. Revised after the r4 validator: unconfirmed objects left from earlier runs (an hour-long expiry) kept the bounds full, so this run's honest objects arriving after another peer's share were refused and the run failed; those objects are shown nowhere, so evicting them costs nothing. One peer can still fill the late-copy record (100,000 hashes, `max_late_announcers`) with junk; it then takes no new hashes, but every object still being asked for keeps its announcers in the request records, so this only loses announcements of objects already arrived or refused. Revised after the r5 validator: an announcer that closes before the object arrives was dropped from the request record, so with the late-copy record full the object arrived with one announcer and stayed unshown; a closing peer's announcements of objects not fetched yet now move to the late-copy record past its bound, which holds no more than the request records held for that peer. |
| D54 | Requests are keyed by item, hash and kind, as Core keys them by `CInv`: a hash announced as an object and as a vote is two requests, each asked of its own announcers, and one never waits on the other. | Validator r4 MJ1: keyed by hash, the first announcement fixed the kind, so a peer announcing an object's hash as a vote and then answering `notfound` left it never fetched from honest object announcers. The first fix (asking the other kind once every announcer of the first failed) still let the liar's request hold it (validator r5 MJ1): it was not a phase item, so the run reported `Synced` without the proposal, and a peer that kept delivering other items never timed out. A liar now costs only its own announcement limit. |
| D55 | A request is also timed out 120 s (`max_request_age`) after it was sent, however much else its peer delivers; a freed in-flight slot sends the peer's next queued request; a request queued for 30 s moves to another announcer if one can be asked; a peer's queue drops entries of ended requests once it holds more than twice its announcements. | Validator r5: the timeout ran from the peer's last delivery, so a peer delivering refused objects every 200 ms held any request for good (MJ1); only a new inv sent a queued request, and a queued one never timed out, so a peer holding its 5,000 slots kept a phase item unsent and failed every run, and two honest peers with more than 5,000 items for one peer stalled (MJ2); entries of requests ended while queued stayed, so inv + `notfound` pairs grew the queue without bound (MJ3). 120 s is four timeouts: an honest peer answers 5,000 items far sooner. |
| D56 | Each current proposal's votes are asked of up to 3 connected peers not asked for them within the window (Core `REALNET_PEERS_PER_HASH`), and a closed peer's vote syncs only top that up; each peer's `ssc(11)` is met by its own invs (D46). | Validator r5 m3: asked of one peer, a peer answering `ssc(11, 0)` withheld every vote on the proposals it was given, and the run reported `Synced` with them missing. Core's scheduler asks three peers per object on mainnet and testnet (`sync_manager.cpp:55`); on regtest it asks all, which with the wallet's 3 peers is the same. Votes on a proposal are fetched once whoever announces them first. Accepted risk (validator r6 m2): a run inside the window after a failed one asks only peers not asked for a proposal's votes in that window, as Core penalizes a repeat (`net_governance.cpp:124`); if the only fresh peers for it withhold, the run is `Synced` without those votes until a run past the window. Failing instead would hold every honest retry for up to an hour after any failed vote phase; reaching it needs the earlier run to fail after its vote syncs and the fresh peers to be the attacker's, from the random masternode picker. |
| D57 | A lost item stays lost until the wallet accepts a copy: a later request's `notfound` or a refused copy no longer replaces its record, and the record keeps every peer asked. Given up on after a later peer's `notfound`, an item whose request timed out at an earlier peer is lost too (that peer may still deliver it). A request moved from a queue never sent it, so its peer stays askable after the others. A `notfound` for a request not sent to the peer is ignored. Once `Synced`, no item belongs to a phase; lost records are bounded (records not lost make room; a loss with no room is still counted). Freed slots and timed-out re-asks go out in `getdata`s of up to 1,000 per peer per event or tick. | Validator r6: a request moved from A's queue marked A asked although A was never sent it, so after B and then the liar, the liar's `notfound` ended the vote uncounted and the run reported `Synced` without votes two honest peers held (MJ1); a liar announcing a lost vote and answering `notfound` replaced its lost record, ending the loss, and the honest late copy was then refused (MJ2); the same follows if the item's later request is refused, as a vote's hash leaves out its signature. Lost records had no bound and every insert past 100,000 rescanned them (m1), and single-item `getdata`s after a mass timeout could pass the peer's 4,096-message send queue (nit). D50 said a `notfound` for an item not asked was ignored, but one for a queued request ended it (nit). |
| D58 | A refused copy never replaces another peer's request for the item that is still out. A copy the wallet takes ends any request still out for it, whose peer's copy is then late, not unsolicited. A vote copy the wallet drops for a reason its sender chooses (a 96-byte signature on a vote whose object it lacks, `Unverifiable`; a valid vote past the sender's orphan bound, not kept) is refused, not taken, so the next announcer is asked and a loss stays. | Validator r7 MJ1: a vote's hash leaves out its signature, so a peer in an item's late record could send a re-signed copy; refused, its retry reassigned the item to the honest peer whose request was out, overwriting it: a leaked slot, a second `getdata`, and the honest peer charged for answering it twice, closed after 1,001 repeats (1,001 of the attacker's 10,000 suspects), which then left a proposal's votes asked only of the attacker. m1: both drops counted as a copy taken, which ended the request and a loss. nit1: a copy taken late left another peer's phase request, whose timeout could count a loss for a stored vote. |
| D59 | An unsolicited item is dropped with no penalty, as Core drops one a peer neither announced nor was asked for (`net_governance.cpp`, "Received unrequested object/vote"). A copy from a peer asked earlier leaves the request still out for the item in place until the wallet takes a copy, which then ends it (D58). A vote copy past its key's limit (`KeyLimit`, D11) is refused, not taken. The session's record of vote syncs is pruned of entries past the repeat window each time it doubles (from 1,024), and the record of full syncs with it. An object's collateral txid is the one its first delivered copy carries. | Validator r8. MJ1: a request timed out at peer A moves to B; B's copy taken first, A's copy (Core answers its queue in order) was unsolicited, so one slow link's mass timeout closed A past 1,000. MJ2: junk `inv` + `notfound`, free, filled the late record, whose bound drops every record not lost, so honest second copies (asked twice, D12; ends of D58) became offences; chained with m2, 1,001 forged copies closed an honest peer. Penalizing what was not asked bought nothing Core has: the cost of an unsolicited item is a decode and a hash, bounded per peer by D45, and an honest late copy whose record is gone is a duplicate. With no penalty, a record evicted at the bound only drops a copy of an item not lost; lost records are never evicted (D57). m2: a refused copy from a peer in the live request's `asked` (it answered `notfound`) released that request before the refusal, so the retry asked its peer again, which then owed two copies. MJ3: a vote's hash leaves out its signature, so a listed key's holder could re-sign any honest vote with its own key; past that key's limit the copy was dropped but taken, ending the request, and the run reached Synced without the honest vote. `Obsolete` and `SameTimestamp` stay taken (see D61 and D62 on what honest peers relay), and a re-signed copy can only meet them for outpoints the attacker's key already holds, at most `2·n_K + 2` placed votes per object (D11; an orphan slot is not limited until its object arrives, when `KeyLimit` applies again and the vote sync that follows fetches the honest copy). Accepted risk: below the limit, a re-signed copy that arrives first is kept under the attacker's key, and the honest copy is a duplicate (same hash); bounded by D11, and the outcome, outpoint and time it counts are the honest voter's. m3: the record of vote syncs gained an entry per (peer, object) for the whole session. nit: Core leaves the fee transaction out of the object hash on purpose (`governance/common.cpp`) and checks it against the chain; the wallet has no chain to check it against, so `ProposalDetail.collateral_txid` is as reported by the peer that delivered the object first. |
| D60 | A `getdata` buys one answer, as in Core (`PeerConsumeObjectRequest`: one answer per `getdata`). A request records the peers asked before (not asked again) apart from those still owed an answer: a peer whose request timed out may still send one copy, from the request now out for the item or from its late record. A copy, kept or refused, or a `notfound` spends its sender's one; a peer asked twice (D12) keeps one for its second copy once its first arrives; given up on, a peer is owed one copy however often it was asked. A late record with no copy owed and no loss is not kept. The store counts its unconfirmed objects as they change instead of scanning them on every sync event. | Validator r9 MJ1: a peer that answered `notfound` (free) stayed in the item's late record, so it could replay the same copy without end: an old, superseded vote (public once relayed) passes the precheck, costs a signature recovery and two store-lock passes, and is refused as `Obsolete` without a penalty or a spent record, ~110× the cost of an unsolicited copy in a debug build, under D45's byte bound since that bounds memory, not work. A peer asked before could do the same while the request was out (D59's arm). Counting each copy against `suspect` would have charged honest late copies of older votes; spending the answer is Core's rule and also ends the late records only a `notfound` made (no copy owed). nit4: each sync event refreshes progress, which counted unconfirmed objects by scanning all of them (up to 5,000) under the store lock, also for a free empty `inv`. m1: a late copy wrongly dropped no longer shows as misbehaviour (D59), so the D57 rule that a timed-out peer's late copy ends a loss is now pinned by its own test. |
| D61 | A copy from a request's peer is taken only once its `getdata` went out. A copy taken from a late record, or from a peer asked before while the request is still out, carries none of the record's other grants into a retry: they stay where they were. A `notfound` for a copy still owed spends it. A vote whose signature the wallet recovered and then did not keep (`Obsolete`, `SameTimestamp`, `KeyLimit`, or an orphan past the sender's bound) costs its sender a suspect (bound 10,000, D10; withdrawn in D62). The run counts its pending phase requests as they change instead of scanning them after every event. | Validator r10. MJ1: a refused late copy copied the record's remaining grants into its `Taken`; the retry gave up and merged them back into the record still there, so each refused copy (free: a 96-byte signature on a vote with no object) doubled them, without bound in memory, and they never ran out. MJ2: an obsolete copy ends its request, so its peer could announce it again and be asked again, a signature recovery per `inv` and copy with no penalty. Core rejects an obsolete vote before checking its signature (`governance/object.cpp`, `ProcessVote`), which the wallet cannot do, not knowing every outpoint's key; and Core's vote file drops a vote once a newer one from the same outpoint arrives (`votedb.cpp`, `RemoveOldVotes`), so an honest peer sends an obsolete copy only in a race. D59's "honest peers relay older votes" was wrong. Charging the recovery is the D10 suspect rule (a recovery the wallet cannot use), which bounds a peer at 10,000 recoveries per run. (D62: wrong too; `RemoveOldVotes` keeps votes of the same time, and the wallet's store can hold a newer vote than Core's.) m1: a copy for a request still queued (no `getdata` sent) was taken. m2: once a phase's peers were done, every event, a free empty `inv` too, scanned all pending requests (up to 100,000 per peer) for a phase item. nit: a `notfound` after a peer's first copy left its second grant. |
| D62 | A vote whose signature the wallet recovered and then did not keep (`Obsolete`, `SameTimestamp`, `KeyLimit`) costs its sender nothing. The store remembers the verdict by (hash, signature), up to `MAX_DISMISSED_VOTES` = 100,000 (oldest forgotten first), cleared when the masternode list changes; a copy with those bytes gets the verdict again with no recovery, and a copy of the same hash under another signature is checked afresh, so a forged copy cannot hide the honest one (D59 MJ3). A remembered `KeyLimit` copy is refused, as in D59. A vote on an object the store lacks, from a peer at its orphan bound (D58), is refused before its signature is recovered, at no cost: Core does not check an orphan's signature either. D61's suspect charge is withdrawn. | Validator r11. MJ1: Core's vote file drops only strictly older votes (`votedb.cpp`, `RemoveOldVotes`), so a masternode that voted YES, NO and ABSTAIN with one timestamp leaves all three, and Core's vote sync announces each; the wallet keeps one and recovers the others as `SameTimestamp`, so every honest peer paid 2 suspects per (masternode, object) and per announcer, and 100 masternodes of one key on 51 proposals closed an honest peer past 10,000. MJ2: a key's holder hands the wallet a vote newer than the one Core holds, or fills its key's limit on an object with outpoints Core never relays; Core's current vote is then `Obsolete` or `KeyLimit` for the wallet, and each honest relayer paid. A suspect cannot tell a peer that serves Core's state from one that replays it; what D61 needed was only that a peer cannot make the wallet recover the same copy without end (r10 MJ2), which remembering the verdict gives, and the bound caps the memory (about 8 MB). (D63: remembering alone did not bound it: one signing gave 64 signatures, and a cycle past the bound is recovered every time.) |
| D63 | The remembered verdict (D62) is keyed by the signature's canonical form: header `27 + ((header − 27) & 7)`, s made low with the recovery parity flipped, which every signature that recovers the same key maps to; the compressed flag names another key and stays. The memory is also cleared whenever the resolved masternodes are set, as a resolved outpoint is not limited (D11). Each peer has a budget per run of `max_unkept` = 25,000 recoveries not kept (any outcome but kept, suspect or invalid); past it, its vote copies are refused before recovery, with no penalty, so the next announcer is asked; a refusal neither counts nor ends a loss (D57). A vote refused at the sender's orphan bound still asks for its object (D52). | Validator r12. MJ1: Core's `RecoverCompact`, like the wallet's, reads only the low 3 bits of the header, and (r, n − s) with the other parity recovers the same key, so one public vote the wallet does not keep gave 64 copies, each with a new digest and recovered, by a peer with no key; and a cycle of more copies than the memory holds (1,563 public votes × 64, or a key's holder signing about 100,000 obsolete votes once) is recovered on every pass. Canonical digests make one signing one entry; the budget bounds what a cycle can cost to 25,000 recoveries per peer per run (as D10 bounds unknown keys at 10,000), which memory alone cannot (any eviction order loses to twice its size). The budget is not a penalty: r11 showed that charging what honest peers relay closes them. With the memory, an honest ladder or planted vote (D62) is recovered once while remembered, whoever relays it (the memory outlives runs and is cleared when the list's entries change or the resolved masternodes are set), so honest peers spend the budget only on copies no other peer delivered first. Accepted risk: a peer past its budget has its keepable votes refused too; another announcer's copy is asked for; with none the vote is missing for that run, and if an earlier announcer's request for it timed out it stays lost and its phase fails (D57, D13, validator r13); reaching it takes 25,000 unkept votes from that one peer in one run (r11's 100-masternode, 51-proposal ladder construction is 10,200). m1: the pre-recovery orphan refusal skipped `fetch_parent`. m2: a remembered `KeyLimit` verdict outlived `set_resolved`. m3: the `Obsolete` and `SameTimestamp` docs said "not remembered". Validator r13: the accepted risk said such a vote is "not lost", and the memory "once per run"; both corrected as above, the loss pinned by `a_budget_refusal_after_a_timeout_keeps_the_loss`, and the canonical form checked against recovery over every header and both s forms. Load flake: `a_peer_that_keeps_delivering_cannot_hold_a_request` (wall-clock request age, D55) failed in 2 of 9 mutant runs at load average about 275 and passed 6 of 6 alone, 3 of them under one of those mutants; `confirmation_asks_are_shared_between_first_announcers` (sync idle timeout) failed once in a full run under load and passed alone and in the next full run. Gate 17 (9fefbf3c): Playwright `routes.spec.ts` "route settings light" failed in `page.screenshot` ("Protocol error (Page.captureScreenshot): Unable to capture screenshot") at load average about 148, 1,307 of 1,308 passed; the spec alone passed 82 of 82 three times; this commit range touches no UI code. |
