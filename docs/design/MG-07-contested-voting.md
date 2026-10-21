# MG-07: voting on contested DPNS names

ROADMAP MG-07 (release 1.1, optional, size M, engine, tier T2, stack-independent): the wallet's masternodes vote on
contested DPNS usernames with their voting keys (IOS-079, AND-079…083). Acceptance: a vote cast from the wallet on
dashmate changes the contest tally.

Inputs:
- MG-05 ([`MG-05-authorization.md`](MG-05-authorization.md)): the `MasternodeVoting` scope, the `Governance` grant, the
  `voting` lease, the `VoteSigner` and the row-less `ArtifactKind::Vote` in the fence. MG-05 §6 reserved this row "a
  second `VoteSigner` method with its own digest, not a digest scope".
- MG-03 ([`MG-03-masternodes.md`](MG-03-masternodes.md)): the SPV list and the attached `voting` keys of tracked
  masternodes (D24, D29: an attached voting key holds only when it matches the list).
- MG-02 / MG-05 `Governance::vote`: the key choice (keychain voting keys `m/9'/coin'/3'/1'/i` at indexes 0–19, then the
  attached key) and the per-masternode result shape (MG-05 D6, D13).
- DP1-03 / DP1-04: the DPNS poll (`dpns_poll`), the vote-state page and the end-date index (`ContestNet`).
- Platform at the engine's pin `82fb33aff8`, the protocol reference:
  - rs-sdk `platform/transition/vote.rs` (`PutVote`: the voter identity, the nonce, `try_from_vote_with_signer`, the
    broadcast);
  - rs-sdk `platform/transition/masternode_vote_keys.rs` (`voter_identity_id`, `voter_identity_voting_key`,
    `is_voting_key_failure`, `diagnose_voting_key_failure`);
  - rs-dpp `masternode_vote_transition/v0/{mod,v0_methods}.rs` (the transition and its signing);
  - rs-sdk-ffi `contested_resource/transitions/cast_vote.rs`, iOS's counterpart (byte order, key id 0);
  - rs-drive-abci `state_transitions/masternode_vote/{transform_into_action,state/v1,advanced_structure}/v0` (the rules
    of §4).

D-numbers in this file are MG-07's (D1 and up).

| Part | Where |
|---|---|
| The vault's name-vote signature | `rust/crates/dw-vault/src/vote.rs` (`VoteSigner::sign_name_vote`, `NameVote`) |
| The facade calls | `rust/crates/dw-engine/src/governance/contested.rs` (new), records in `governance/records.rs`, codes in `governance/errors.rs` |
| The Platform seam | `governance/contested.rs` (`NameVoteNet`, the SDK and a test script) |
| The command line | `rust/bin/dwcli/src/masternodes.rs` (`mn names`, `mn vote-name`) |
| The fixtures for MG-11 | `rust/crates/dw-engine/tests/fixtures/mg07/*.json` |

## 1. The facade

Two calls on `Governance` (mg-engine §2.6). They need the SPV masternode list and Platform; they do **not** need
governance sync (D2).

`contested_names(wallet) -> ContestedNames`:
- `contests`: the open DPNS contests, soonest-ending first: the first page (100) of Platform's end-date index from now
  on, each with one vote-state page (100 contenders, Drive's default; D9): `label` (normalized), `ends_at` (Unix s),
  `contenders` (`identity` base58, `votes`), `lock_votes`, `abstain_votes`, `contenders_complete` (false for a full
  page).
- `voters`: the wallet's masternodes eligible to vote, list order: every SPV list entry whose voting key the wallet
  holds, as `Governance::vote` chooses keys (keychain 0–19, then an attached voting key whose recorded id is the list's,
  D3). Each with `pro_tx_hash`, `kind`, `weight` (Platform's strength: 1 regular, 4 evonode), `valid`, `key`
  (`keychain{index}` or `attached`), and `current`: its present choice in each listed contest, read with one proved
  votes-by-identity query per voter (D4).
- `votes_allowed` (Platform's `votes_allowed_per_masternode`, 5 at the pin) and, per voter and contest,
  `votes_used`: the votes this app saw Platform accept for that masternode and label, a lower bound (D6).

`vote_name(wallet, label, choice, masternodes, grant) -> Vec<NameVoteCast>`: one `MasternodeVoteTransition` per
masternode (proTxHash, display order) with `choice` (`towards_identity{identity}`, `abstain`, `lock`) on the DPNS
poll of `label`, all under one `voting` lease begun from `grant` and ended when the call returns (MG-05 §4). Each cast is
signed in the vault, admitted through the fence, broadcast, and waited for (Platform's proof of the executed
transition). `NameVoteCast{pro_tx_hash, transition, rejected}`: `transition` is the transition's hash (hex) once it
was broadcast; `rejected` says why the vote does not count (§3).

## 2. Authorization: MG-05's scope and grant, signing only in the vault

- The grant is a `Governance` grant of `wallet` (any unlock that issues one: the passphrase, or quick unlock at
  value 0, MG-05 D2), redeemed as the `voting` flow. No new purpose, flow or budget: a name vote moves no funds, its
  fee comes out of the contest's fund (Drive `PaidFixedCost` from `contested_document_single_vote_cost`), and the
  voter identity holds no credits (D1).
- **The vault builds the transition.** `VoteSigner::sign_name_vote(key, vote)` takes `NameVote{pro_tx_hash, label,
  choice, nonce}` and returns the signed transition's serialized bytes and hash. It fills in the rest itself: the DPNS
  contract id, `domain`, `parentNameAndLabel`, the index values `["dash", label]`, the voter identity
  `create_voter_identifier(pro_tx_hash, hash160(voting key))` and `signature_public_key_id` 0. It signs dpp's
  `signable_bytes()` of that transition (the compact recoverable ECDSA over their double SHA-256, which is what
  `sign_external` asks of an `ECDSA_HASH160` signer). The label must be a contested normalized DPNS label: 3–19
  characters of `a-hj-km-np-z01-` (DPNS's `normalizedLabel` pattern within its contested index's
  `^[a-zA-Z01-]{3,19}$`), no `-` at either end; anything else is refused (`PathNotAllowed`) before a key is read. So the voting key
  signs only DPNS vote transitions whose fields the vault laid out; there is still no digest scope, no `Signer`
  and no generic Platform signer for a voting key (MG-05 D1; D5).
- **Fence.** Each transition is a row-less `ArtifactKind::Vote` keyed by its hash, admitted under the voting lease
  (its authority, MG-05 D5) and handed off by the broadcast. A lock revokes the lease and drains the permit, so no
  vote is broadcast after the lock's drain. `Sent` once DAPI took the bytes; `NotSent` when DAPI refused them with a
  consensus error (CheckTx); otherwise (a transport failure after the request left) `MaybeSent`: the vote may count
  (D7).
- **Unlock.** A locked vault ends the batch: the remaining masternodes are `cancelled` (MG-05 D13).
- The masternode's vote key is chosen as in MG-05 §5 (keychain first, attached second, D20/D23 included), so an
  unreadable keychain falls through to an attached key.

## 3. Errors as typed codes

**Whole-call** (`GovernanceError`), checked in this order, before the grant is redeemed:

| Code | When |
|---|---|
| `invalid_argument` | a malformed label, contender id or proTxHash; no masternodes, more than the list holds, or one given twice |
| `governance.list_unavailable` | SPV has no masternode list (existing code) |
| `platform.unavailable` (`GovernanceError::Platform`) | the session runs without Platform, or the poll could not be read |
| `governance.contest_not_found{label}` | no DPNS poll for `label` (no contender and no stored result) — **new** |
| `governance.contest_closed{label}` | the poll resolved (a winner, Locked or no winner is stored), or its end time is past (D8) — **new** |
| `governance.not_a_contender{identity}` | `towards_identity` names an identity the poll does not list (D9) — **new** |
| `platform.grant_invalid`, … | the lease's refusals (MG-05 D12) |

**Per masternode** (`NameVoteCast.rejected`): the engine's reasons `unknown_masternode` (not in the SPV list),
`no_voting_key`, `cancelled`, `signer_unavailable`, `in_flight` (MG-05's meanings), `vote_limit` (this app saw the
masternode use its 5 votes, D6), `platform_unavailable` (the nonce read or the broadcast failed before the bytes
left), `unconfirmed` (the bytes may have left and no proof came back, D7); and Platform's refusals, mapped from the
typed consensus error (D10):

| Reason | Platform's error (code) |
|---|---|
| `already_voted` | `MasternodeVoteAlreadyPresentError` (40304): the same choice again |
| `vote_limit` | `MasternodeVotedTooManyTimesError` (40303) |
| `contest_closed` | `VotePollNotAvailableForVotingError` (40302): not started, awarded or locked |
| `contest_not_found` | `VotePollNotFoundError` (40301) |
| `masternode_not_found` | `MasternodeNotFoundError` (40300): not in Platform's masternode list |
| `wrong_voting_key` | `MasternodeIncorrectVotingAddressError` (40305), `MasternodeIncorrectVoterIdentityIdError` (40306) |
| `choice_not_allowed` | `VoteChoiceNotAllowedForVotePollError` (a lock where the index allows none, or a non-contender) |
| `no_voter_identity`, `voting_key_disabled` | a signature error about key 0 (`is_voting_key_failure`), told apart by `diagnose_voting_key_failure` (one identity fetch, only after the failure) |
| `refused` | any other consensus error (logged at debug) |

No §4 code reaches the m1–m4 tables, and the Swift host binds no governance call, so its `engineCodes` are unchanged
(MG-02 D8, mg-engine §4).

## 4. The vote-change rules Platform enforces (pin `82fb33aff8`)

- **Re-votes.** A masternode may change its choice while the poll runs. Drive keeps one choice per (masternode,
  poll) and moves its weight from the old choice to the new one (`transform_into_action/v0`).
- **The same choice again** is refused (`MasternodeVoteAlreadyPresentError`) and does not count as a change.
- **The vote limit.** Drive stores a count with each choice: 1 for the first vote, plus one per change. A vote whose
  stored count is already `votes_allowed_per_masternode` (5) is refused (`MasternodeVotedTooManyTimesError`). So a
  masternode votes at most five times per poll: one vote and four changes. The proved votes-by-identity answer
  carries the choice but not the count, so the wallet's `votes_used` is what this app saw Platform accept, a lower
  bound, and the wallet refuses locally only at 5 (D6).
- **The poll's end.** Votes are taken while the poll's stored status is `Started`. The poll's end is fixed when the
  first contender's block starts it (`add_contested_document_for_contract_operations/v0`): two weeks later on mainnet;
  on any other network two weeks under voting versions v1 and 90 minutes under v2 (`dpp_voting_versions`). The poll
  is resolved in the first block after its end; from then on Drive refuses with
  `VotePollNotAvailableForVotingError`. The wallet refuses from `ends_at` on (D8).
- **The voter.** The vote names the masternode's proTxHash in Platform's order (display order, the reverse of the
  wire bytes) and the voter identity derived from it and the voting key's hash160. Drive checks the masternode is in
  its list, that the signing key's hash160 is the list's voting address (40305) and that the voter identity is the
  derived one (40306). A key rotated by a ProUpRegTx leaves the old voter identity with a disabled key.
- **Weight.** Regular masternodes count 1, evonodes 4.
- **Lock** is allowed on DPNS's contested index (resolution `MasternodeVote`). A vote towards an identity must name a
  contender only from protocol version 14 on; the wallet checks it at every version (D9).

## 5. Tests and fixtures

- **dw-vault:**
  - `sign_name_vote`'s bytes equal Platform's own construction at the pin, byte for byte, for each choice and for a
    keychain and an attached key: `MasternodeVoteTransition::try_from_vote_with_signer` with a single-key
    `ECDSA_HASH160` signer (what rs-sdk-ffi's `cast_vote` builds), the same nonce and the voter key from
    `voter_identity_voting_key`. The signature recovers to the voting key's hash160.
  - the label rule refuses before any key is read; a lock ends the signer.
- **dw-engine** (a scripted `NameVoteNet` seam, as DP1-04's `ContestNet`):
  - `contested_names`: open contests with tallies, the voters (keychain and attached, an unmatched attached key
    left out), their current choices and `votes_used`;
  - `vote_name` casts one transition per masternode and records `votes_used` only for accepted votes;
  - refusals: no voting key; the poll ended (whole call `contest_closed` before any grant, and Platform's 40302 per
    masternode); over the vote limit (locally at 5 used, and Platform's 40303); `already_voted`; a non-contender; an
    unknown masternode; a lock mid-batch (`cancelled`); `unconfirmed` and `platform_unavailable`;
  - the mapping of every Platform error of §3 from real dpp consensus errors.
- **dwcli:** `mn vote-name`'s argument errors print one JSON line; `mn names` serializes the facade's record.
- **Fixtures for MG-11:** `tests/fixtures/mg07/contested_names.json` (two contests, three voters with mixed keys and
  choices) and `name_vote_casts.json` (every `rejected` reason once). A test reads each back as the facade's record and
  writes it out identically, so they cannot drift from the records.
- **T2** on a dashmate `local` network: the closest proof of D17 (the SPV list is the blocker).

## 6. Contract delta

- `docs/contracts/mg-engine.md`: §0 file row `governance/contested.rs` (MG-07); new §2.6 with the two calls; the new
  records in §3 (regenerated); §4 gains `governance.contest_not_found`, `governance.contest_closed`,
  `governance.not_a_contender` and the `NameVoteCast.rejected` vocabulary. **Contract-Version 7 → 8.**
- m4 is unchanged: no new grant purpose, flow, budget or Platform code (D1).
- dw-app binds nothing yet: the two calls get `EXCLUDED` rows "deferred (MG-11)" and the E0-13 coverage doc is
  regenerated (D11).
- `docs/parity.md` IOS-079: `partial`, engine and `dwcli` only, the UI is MG-11.

## D-rows

| D | Decision | Why |
|---|---|---|
| D1 | A name vote uses MG-05's `Governance` grant and `voting` flow and lease as they are; no new purpose, flow, budget or code. | A vote moves no funds and costs the voter nothing: Drive pays it from the contest's fund. MG-05 §6 reserved exactly this. One passphrase per batch, as for governance votes (MG-05 D11). |
| D2 | The calls live on `Governance` (mg-engine) and need the SPV list and Platform, not governance sync. | They share MG-05's grant, lease and key choice; govsync (Core's governance objects) has nothing to do with Platform polls. |
| D3 | Voters are the list entries whose voting key the wallet holds: a keychain key at 0–19, or an attached `voting` key whose recorded id equals the list's (MG-03 D29). Signing re-checks the key in the vault (MG-05 `choose_key`). | The same set `Governance::vote` signs for, so a masternode votes on proposals and names with the same key. IOS-079's voter set is the same: wallet keys plus keys attached to tracked masternodes. |
| D4 | `current` comes from one proved votes-by-identity query per voter (Drive's `ContestedResourceVotesGivenByIdentityQuery`, `limit` 100), filtered to the listed contests. Drive keys it by the masternode's proTxHash (Platform order), not the voter identity (`register_contested_resource_identity_vote_v0`), so a rotated voting key keeps its votes. | One request per voter instead of one per (voter, contest). |
| D5 | The vault builds and signs the whole DPNS vote transition from typed fields; the engine broadcasts its bytes. The label is checked against the contested normalized-label shape before any key is read. | MG-05 D1: a voting key must not sign what a caller chooses. The vault picks every byte but the label (charset-bound), the contender id, the nonce and the proTxHash, behind dpp's fixed `MasternodeVote` prefix, so no transaction sighash and no other transition is reachable. rs-sdk's `PutVote` takes a generic `Signer`, which would be such a scope; its construction is reproduced instead, and a test holds the bytes equal to it. |
| D6 | `votes_used` counts the votes this app saw Platform accept per (masternode, label), in app.sqlite (`governance.name_votes:<proTxHash>:<label>`, global scope); the wallet refuses locally at `votes_allowed` and otherwise leaves the limit to Platform (`vote_limit`). | Drive's count is not in the proved answer. Android keeps the same local count ("One vote left"). A count from this app is a lower bound, so it never refuses a vote Platform would take. |
| D7 | A transition DAPI refused with a consensus error is `NotSent`; one whose request may have left without an answer is `MaybeSent`, reported `unconfirmed`, not counted in `votes_used`. | A vote moves nothing and its nonce stops a second execution, so an unknown outcome is safe to report and the host re-reads the tally. |
| D8 | The wallet refuses the whole call from the poll's `ends_at` on, and for a resolved poll, as `governance.contest_closed`. An open poll whose end is not on the end-date page (more than 100 polls end sooner) is voted on, and Platform decides. | Drive resolves the poll in the first block after its end; a vote sent at the end would race it. |
| D9 | A vote towards an identity must name a contender on the poll's first vote-state page (100); else `governance.not_a_contender`. | Drive checks this only from protocol version 14; before that a vote for a non-contender is taken and wasted. DPNS doubles the contest fund from 250 contenders, so a 100+ contest is rare; voting for its later contenders is a follow-up. |
| D10 | Platform's refusals are mapped from the typed consensus error, never from text. Signature errors about key 0 are told apart by rs-sdk's `diagnose_voting_key_failure`, only after a failure. | rs-sdk's own `is_voting_key_failure` rule and iOS's (rs-sdk-ffi `cast_vote`): one identity fetch on the failure path, none on success. |
| D11 | dw-app does not bind the calls yet (`EXCLUDED` "deferred (MG-11)"); MG-11 binds them with the UI. | ROADMAP: MG-07 is the engine row, MG-11 the UI. The fixtures give MG-11 the records to start from. |
| D12 | The transition is broadcast by `BroadcastStateTransition::broadcast` and its outcome read by `wait_for_response` (the proof of the executed transition, waited for up to 60 s), as `PutVote::put_to_platform_and_wait_for_response` does, minus its "already exists" re-fetch: `Error::AlreadyExists` (the same bytes in the mempool) goes on to the wait, as rs-sdk's `Waitable` does. `ensure_valid_state_transition_structure` is `pub(crate)` in rs-sdk and is not run: the vault's construction is held byte-equal to `try_from_vote_with_signer`, and Drive runs the same check. | The same requests rs-sdk sends. A repeated choice is Drive's typed `MasternodeVoteAlreadyPresentError`, mapped to `already_voted`. |
| D13 | dw-app gains the three new `GovernanceCode`s (and the regenerated TS types), with no `data` yet: the label or identity is in the message. MG-11 adds `data` when it binds the calls. | `AppError::from(GovernanceError)` is an exhaustive match, and the codes are the contract's; no call reaches them in the UI before MG-11. |
| D14 | The SPV list and Platform the two calls read have test-only slots on the session's `GovernanceRuntime` (`name_list`, `name_net`), as DP1-04's `contests.net`; production reads `read_list` and the SDK. | MG-03's list has no test hook, and the governance flow tests already run on a regtest session with a fixture wallet and vault. |
| D15 | A label is normalized as DPNS normalizes it (`convert_to_homograph_safe_chars`: lower case, `o`→`0`, `i`/`l`→`1`) before the contested-label check, so `vote-name Alice` votes on `a11ce`. The vault checks the normalized label again. | The user types the name they see; Platform's poll is keyed by the normalized label. |
| D16 | A transition refused at the broadcast (CheckTx) is `NotSent` in the fence; one refused in the block that executed it is `Sent`; a local error before the request (a transition that does not decode) is `NotSent` (`platform_unavailable`); any other failure is `MaybeSent` (`unconfirmed`). Only an executed vote (`Counted`) adds to `votes_used`; Platform's `MasternodeVotedTooManyTimesError` sets it to Platform's count. | The fence's states must not claim a send that did not happen. Platform's own count is the better figure when it gives one. |
| D17 | **T2 acceptance not met as written; closest proof run instead.** On dashmate `local` (3 masternodes, the standard count; images 5.0.0-beta.2) dash-spv never finishes its masternode list: the network has no `llmq_test_dip0024` quorum (Core: size 4, minSize 4), so the seed never answers QRInfo, and `dwcli mn --spv names` returns `masternodes.list_unavailable`. A 4-masternode network did not get through dashmate's DKG setup in 5 tries (`waitForQuorumConnections`: the masternodes share externalIp 172.17.0.1 and Core makes no automatic quorum connections between them). The proof run: a temporary engine unit test (not committed; patch and log in `/work/scratch/dw-mg-07/t2/`) on a real engine session against the dashmate gateways. It registers two identities, each contesting `c0ntest` through rs-sdk's `register_dpns_name`. It attaches the three masternodes' voting keys (dashmate's owner keys) in the vault, puts Core's `protx list valid true` in the D14 list slot (the one substitution for SPV), and calls `Governance::contested_names` / `vote_name` on the production `SdkNames`. Platform's contenders query showed A 0→8 (2 evonodes, weight 4) and lock 0→4. A repeat came back `already_voted` (Drive's 40304), and a changed vote moved 4 to B, with `votes_used` 2. Also needed: dashmate's self-signed gateway certificates are SHA-1 with no SAN, which rustls refuses, so they were reissued (SHA-256, SAN IP 172.17.0.1, the same keys). | The vote path is the shipped code; only the list's source differs, and the list read is MG-03's, tested there. The full dwcli path needs a network with rotating quorums (4+ masternodes) or a dash-spv change; that is a follow-up, not an MG-07 change. |
