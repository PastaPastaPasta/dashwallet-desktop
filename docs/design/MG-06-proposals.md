# MG-06: proposal creation and resume

ROADMAP MG-06 (release 1.1, size M, engine, tier T1, stack-independent): the collateral (fee) transaction, its
confirmations, the submission, validation and the superblock dates.

Acceptance: a proposal created on regtest appears in govsync and can be voted on.

Inputs:
- MG-00 D16 (one action, auto-submit at 1 confirmation with retry, the two states), D18 (save before the fee is
  broadcast, drafts in backups), D29 (the verbatim fee warning) and D35 (lease "Proposal"), plus the MG-06 hand-over
  row (P2PKH-only fee outputs, the next 12 superblocks with an "estimated cutoff within two days" flag)
  ([`MG-00-masternodes-ux.md`](MG-00-masternodes-ux.md));
- UX-SPEC §4.25.3 (Create proposal, Mine);
- MG-02 D3 (an object counts once two distinct peers announced it), D24 (the wallet's own items are announced only
  once a run is synced) and the `dw-governance` pieces already merged: `proposal.rs` (`Draft`, `epochs`,
  `proposal_json`, `validate`, `proposal_object`), `object.rs` (`GovernanceObject::hash`), `validate.rs`
  (`check_object`), `store.rs`, `sync.rs` (outbox, `on_get_data`, `send_object`);
- MG-05 §6 and D8 (the fee is a plain spend under its own "Proposal" flow) and D17 (`NotImplemented` stays).

D-numbers in this file are MG-06's (D1 and up). This is the **phase-1 note**: Core facts, the shape, the contract
changes and the test plan. The implementation follows once the manager has read it.

## 1. Core v24 facts (tag v24.0.0-rc.3, 0364ed626e)

Paths are relative to Core's `src/`.

| Fact | Where |
|---|---|
| The object hash is sha256d of `hashParent ‖ revision (i32) ‖ time (i64) ‖ HexStr(vchData)` as a string (compact size + lower-case hex) `‖ masternodeOutpoint ‖ 0x00 ‖ 0xffffffff ‖ vchSig`. The collateral txid and the object type are left out ("fee_tx is left out on purpose"). A proposal has a null outpoint (`n = 0xffffffff`) and an empty signature. `dw-governance` `object.rs` already computes this and `tests/golden.rs` pins it against Core. | `governance/common.cpp:23-39`, `primitives/transaction.h:65-67` |
| `gobject prepare` builds the object with a null collateral hash, runs `ValidateProposal` with the expiration check and `IsValidLocally(…, fCheckCollateral=false)`. It then calls `GenBudgetSystemCollateralTx`, **writes the object to the wallet (`WriteGovernanceObject`) and only then commits the transaction**, and returns the fee txid. | `rpc/governance.cpp:144-257`, save at `:245`, commit at `:250` |
| `GenBudgetSystemCollateralTx`: one recipient `OP_RETURN <object hash>` of `GetMinCollateralFee()`, not fee-subtracted; change to the wallet's normal change output at a random position; optional single coin-control outpoint; no InstantSend (`use-IS` is ignored). | `wallet/spend.cpp:1296-1314` |
| The fee is `GOVERNANCE_PROPOSAL_FEE_TX = 1 * COIN` on every network (a compile-time constant). `GOVERNANCE_FEE_CONFIRMATIONS = 6`, `GOVERNANCE_MIN_RELAY_FEE_CONFIRMATIONS = 1`. | `governance/object.h:87-89`, `object.cpp:774-788` |
| `IsCollateralValid` needs `-txindex`. **Every output must be P2PKH or unspendable** (`"Invalid Script %s"`). Some output must equal `OP_RETURN <hash>` with value **≥** the fee. Confirmations come only from the block on the active chain; InstantSend locks give none. At 0 confirmations it refuses with "Collateral tx %s is not mined yet" and does not set `fMissingConfirmations`. At 1–5 it sets `fMissingConfirmations` (", pre-accepted -- waiting for required confirmations"). | `governance/object.cpp:790-878` |
| A peer's `govobj` is taken only if the peer announced it or was asked for it (`PeerConsumeObjectRequest`, else "Received unrequested object"). If `ProcessObject` returns false, the sender gets **`Misbehaving(20)`**. | `governance/net_governance.cpp:171-199` |
| `ProcessObject`: an invalid object with `fMissingConfirmations` goes into `mapPostponedObjects` and is not relayed. Any other invalid object returns false (so a 0-confirmation fee costs the sender 20 points). | `governance/governance.cpp:228-275` |
| `CheckPostponedObjects` runs on every tip once synced. At 6 confirmations the object is added (`AddGovernanceObjectInternal`) and relayed from then on. A collateral back to 0 confirmations (reorg) erases the postponed object. There is no timeout. | `governance/governance.cpp:856-887`, `UpdatedBlockTip` `:1095-1119` |
| `gobject submit` (local node): sync check; `ValidateProposal`; `IsValidLocally` with the collateral; `fMissingConfirmations` → `AddPostponedObject` + `RelayObject`; else `AddGovernanceObject`. Returns the object hash. | `rpc/governance.cpp:312-423` |
| Proposals are unsigned. `IsValidLocally` runs `ValidateProposal` (+ collateral) only; the signature and masternode checks are the triggers'. `MasternodeRateCheck` returns true for every non-trigger. The time windows (`MAX_TIME_FUTURE_DEVIATION`, two cycles back) are the triggers' too. A proposal's only clock check is `end_epoch` > adjusted time (prepare, submit, the cleanup pass). | `object.cpp:731-765`, `governance.cpp:726-788`, `:474-479` |
| `ValidateProposal` order and texts: JSON ≤ 512 bytes and an object; `type` = 1; `name` 1–40 of `[-_a-z0-9]` after `ToLower`; `start_epoch` < `end_epoch` (numbers), `end_epoch` not past; `payment_amount` a number > 0; `payment_address` without whitespace, valid for the network, P2PKH or P2SH (`fAllowScript` defaults to true); `url` without whitespace, ≥ 4 characters, `CheckURL`. | `object.cpp:31-300`, `:1000-1033`, `object.h:367` |
| Superblocks: valid height `h ≥ nSuperblockStartBlock && h % nSuperblockCycle == 0`. Regtest: start 1500, cycle 20, maturity window 10 (`-budgetparams` can move the start). Testnet/devnet: cycle 24, maturity 8. Mainnet: cycle 16616, maturity 1662. | `governance/superblock.cpp:119-163`, `chainparams.cpp:168-176, 801-809` |

## 2. Shape

### 2.1 Create: one call, one passphrase

`Governance::create_proposal(wallet, draft, grant)` (async) does this, in order:

1. **Validate.** Run `proposal::validate` (every failing field, wizard order) and refuse with
   `governance.invalid_proposal{fields}`. Build the object at the wall clock (`proposal_object`, parent zero,
   revision 1, as dash-qt's `prepareProposal`), then run `validate::check_object` on it. That is Core's
   `ValidateProposal` with the expiration check, the same check a peer applies, so a draft Core would refuse never
   costs a fee (D11).
2. **Lease.** Begin a lease of the new flow `FlowKind::Proposal` ("Proposal", MG-00 D35) from `grant`, a `Spend`
   grant of `wallet` (D10).
3. **Prepare the fee transaction.** Use a `TxDraft` that the engine restricts:
   - one `OP_RETURN <object hash>` output of exactly `proposal_fee` (1 DASH);
   - change to a fresh BIP44 internal address;
   - source `Any`, or the draft's coin-control outpoints (Core's `outputHash`/`outputIndex`, any number of them).

   No recipient address and no custom change address are accepted. Before signing, every non-`OP_RETURN` output is
   checked to be P2PKH (D1, D2). It is signed through the lease (`prepare_leased`, purpose `Spend`), and the charge is
   the 1 DASH plus the network fee, because the `OP_RETURN` output is outflow.
4. **Save** the proposal row in app.sqlite: the object fields, the signed fee transaction and its txid. This happens
   **before** the broadcast (MG-00 D18, Core `rpc/governance.cpp:245-250`). If the save fails, nothing is broadcast:
   the reservation is released and the lease charge refunded, the same path as a failed prepare (D4).
5. **Broadcast** the fee through the dispatch fence, as a contact payment is broadcast. A lock before the hand-off
   cancels it, and a cancelled fee deletes the row, because nothing was paid. A `MaybeSent` or sent fee keeps the row.
6. End the lease and return the `MyProposal`.

### 2.2 Submit: automatic at the first confirmation

A per-session task, woken by tip and wallet-history changes, walks each wallet's proposal rows. A row that is not
yet public is handled as follows:

- **Fee confirmations ≥ 1** (from the wallet's history, block-only like Core's count), seen for 15 s by a wallet
  whose chain sync is caught up, so that its peers have the block too (D21): queue the object in the
  governance outbox (`INV_GOVERNANCE_OBJECT`) and announce it to the connected peers of a synced run (MG-02 D24),
  once per queuing round and connection (D19). It is not put in the store: the store takes the wallet's own
  objects as confirmed, and "public" must come from peers (D8). It is served on
  `getdata` through the existing `on_get_data`/`send_object`. The first `getdata` for it marks the row
  `submitted_at`: the protocol has no acceptance message, and a Core peer asks for an object only to process it
  (D6).
- **Never at 0 confirmations.** A Core peer would refuse it and score the wallet 20 points
  (`net_governance.cpp:197`) (D6).
- **Postponed at 1–5 confirmations.** A Core peer that fetched it holds it postponed and neither lists nor relays
  it until the fee's sixth confirmation (D19). A fetched proposal below 6 confirmations is waiting normally: no
  retry, no error.
- **Retry.** A fetch is expected while none was recorded, or after a reorg took the fee back to the mempool
  (Core erased the postponed copy; the object is withdrawn and queued again as a new round, announced to every
  peer). A fee missing from the wallet's history only withdraws the object: that may be a stale read, not a
  reorg. The waits below belong to one synced run and start again with each new run, whose peers are new
  connections; a run that is not synced counts none. With no fetch 60 s into a synced run, the row reports `ready_to_submit{no_peer_accepted}` and a new sync
  run (new connections) looks again, at 60 s doubling to 30 min, while the app runs (D7). `submit_proposal`
  retries at once (UX [Submit]).
- **Public.** Once the store holds the object confirmed (two distinct peers announced it, MG-02 D3), the row is
  public and its status is the public row's. A peer never announces back what the wallet announced to it, so the
  echo comes from a fresh run's sync: 120 s after the sixth confirmation with no echo, a new run looks for it, on
  the same backoff (D20).

### 2.3 States (MG-00 D16 f1)

`MyProposalState` is derived on every read from facts, not stored: the fee transaction in the wallet's history, the
row's `fee_sent_at`, `submitted_at` and `last_error`, the store, and the clock. A restored row therefore reads
correctly (D5).

| State | When | UX (§4.25.3 Mine) |
|---|---|---|
| `fee_unsent` | The row exists, but the wallet has never seen the fee: the hand-off was `MaybeSent`, or the backup was restored before the broadcast. | `Ready to submit`; [Submit] broadcasts the saved bytes again |
| `fee_paid{confirmations}` | 0 confirmations (mempool); or ≥ 1 and announced, no peer fetched it yet and no wait reason | `Fee paid 0 of 1`; at ≥ 1 `Submitting` |
| `ready_to_submit{reason}` | ≥ 1 confirmation, not submitted: `no_peer_accepted`, `sync_disabled`, `not_synced` | `Ready to submit`, [Submit], the error |
| `confirming{confirmations}` | Submitted, not yet public; `confirmations` is 1–6+. Below 6 this is normal waiting (D19). | `Confirming 4 of 6` |
| `public{status}` | The store confirmed it | the public chip (MG-00 D17) |
| `fee_failed` | An input of the fee was spent by another confirmed transaction | terminal, error |
| `expired` | `end_epoch` passed before it went public | terminal (Core refuses it, D11) |

Resending the saved fee (`fee_unsent`) needs no grant and no signature: the bytes were signed and charged once. The
saved bytes go to the library's broadcast directly, as dash-qt's Resend does (QT-091), not through the fence (D10).

### 2.4 Persistence and backups (MG-00 D18)

A new app.sqlite table `mg_proposals` (migration `V2026102001__mg_proposals.sql`, STRICT):

| Column | Meaning |
|---|---|
| `wallet_id`, `hash` | Key: the wallet and the object hash (wire order) |
| `revision`, `time`, `data` | The object fields (parent is always zero; `data` is the JSON bytes) |
| `fee_txid`, `fee_tx` | The signed fee transaction |
| `created_at`, `fee_sent_at`, `submitted_at`, `last_error` | Facts that §2.3 derives from |

- It is listed in `WALLET_TABLES` with the insert trigger of DEC-210, so a removed wallet's rows go with it.
- `.dwbackup` carries it with no format change: `rows.rs` exports every table with a `wallet_id` column (D4).
- A restore rebuilds the fee's confirmations by the scan from the birth height. The fee spends the wallet's own coins,
  so the scan finds it.

### 2.5 Superblock dates

`upcoming_superblocks(count)` already gives height, ETA and budget (`count` ≤ 24). `SuperblockDate` gains:
- `voting_cutoff_height`: the superblock height minus the maturity window, the clock's cutoff (MG-02);
- `voting_cutoff_estimate`;
- `cutoff_soon`: the estimated cutoff is within two days (172,800 s) of now (D13).

The wizard asks for 12 (`PAYMENT_DATE_CHOICES`), and `validate` takes a first superblock only from those 12.

### 2.6 What is not in MG-06

- **The UI** (create wizard, Mine) is MG-09.
- **`dwcli gov …` and the nightly suite** are MG-08. MG-06 adds only `dwcli governance --propose …`, which the
  regtest check needs (D17).
- **Console `gobject prepare`/`submit`/`list-prepared`** stay unavailable. The console has no grant prompt (D14).
- **BL-247** stays with MG-09.

## 3. Contract changes

**mg-engine 5 → 6.** §2 gets a new subsection, §2.5 Proposals I create:

| Call | Kind | Notes | Errors |
|---|---|---|---|
| `preview_proposal(wallet, draft: ProposalDraft)` | async | Validates. Returns `ProposalPreview { hash, data_json, data_hex, start_epoch, end_epoch, fee, network_fee }` (the review step's "1.00000000 + network fee", "Proposal data (JSON)"). The network fee is a `TxDraft` estimate; nothing is reserved or signed. | `GovernanceError` |
| `create_proposal(wallet, draft, grant)` | async | §2.1. Returns `MyProposal`. | `GovernanceError` |
| `my_proposals(wallet)` | sync | The wallet's rows, newest first, each with its derived `MyProposalState`. | `GovernanceError` |
| `submit_proposal(wallet, hash)` | async | The [Submit]: resends an unsent fee or retries the announcement now. Returns `MyProposal`. | `GovernanceError` |

- **Records:**
  - `ProposalDraft { name, url, payment_address, payment_amount, payment_count, first_superblock_height, coins: Vec<String> }`;
  - `ProposalPreview`;
  - `MyProposal { hash, name, url, payment_address, payment_amount, payment_count, start_epoch, end_epoch, fee_txid, created_at, submitted_at, state }`;
  - `MyProposalState` (§2.3);
  - `ProposalField` (`name`, `url`, `payment_address`, `payment_amount`, `payment_count`, `first_payment`, `payload`);
  - `SuperblockDate` gets three new fields (§2.5).
- **New §4 codes:**
  - `governance.invalid_proposal{fields}`;
  - `governance.submit_refused{reason}`, where `reason` is `public`, `expired` or `fee_failed`.

  `governance.proposal_not_found{hash}` also covers a hash the wallet has no row for. Fee errors are the
  `Engine(Send)` `send.*` codes and lease errors are `Platform(…)`, both unchanged. dw-app's `ErrorData` gains
  `fields` (MG-02 D94 already has `proposalHash`).
- **m4 32 → 33:** `FlowKind::Proposal` (the flows list of item 59, the dw-app DTO mirror).
- **dw-app:** the four calls are bound in the `governance` domain (BL-232), and the facade coverage doc is
  regenerated. The Swift host binds no governance call, so its `engineCodes` are unchanged.

## 4. Tests

**dw-governance**
- The created object's hash against Core's for the same fields (a regtest `gobject prepare` vector next to
  `tests/golden.rs`).
- The fee script is exactly `OP_RETURN <32-byte hash>`.
- `check_object` accepts the built object and refuses it past `end_epoch`.
- The 512-byte payload limit.
- Sync with a fake peer:
  - the object is announced only once the run is synced;
  - it is served on `getdata`, and the fetching peer is reported;
  - a reconnect announces it again.

**dw-engine** (session tests, fake broadcaster)
1. The row exists when the broadcaster is entered (D18).
2. A failing save broadcasts nothing; the reservation is released and the charge refunded.
3. The fee transaction's shape:
   - exactly one `OP_RETURN` of 1 DASH with the hash;
   - every other output P2PKH;
   - coin control honoured.
4. The cap and the authority:
   - a `Spend` grant below 1 DASH plus fee is `grant_exceeded`;
   - a `Governance` or `MasternodeOp` grant is refused for the flow;
   - a lock before the hand-off is cancelled and the row deleted.
5. Auto-submit:
   - nothing is announced at 0 confirmations;
   - at 1 confirmation the object is announced after 15 s, and only while the chain sync is caught up (D21);
   - `getdata` makes the state `confirming`;
   - two announcing peers make it `public`;
   - with no fetch, the state becomes `ready_to_submit{no_peer_accepted}`, then a new run follows, on the backoff;
   - a fetched proposal at 1–5 confirmations stays `confirming` with no retry; at 6, no echo within 120 s starts
     one new run (D19, D20);
   - `submit_proposal` retries at once, and resends a fee seen unmined;
   - the waits start again with each synced run, and count nothing while not synced.
6. A reorg to 0 confirmations gives `fee_paid` and withdraws the object; the return to 1 queues it again and a
   fetch is expected again. A fee missing from history withdraws it but expects no new fetch.
7. Terminal states:
   - `expired` past `end_epoch`;
   - `fee_failed` on a conflicting confirmed spend;
   - `submit_refused` for each of them and for `public`.
8. Backups: a `.dwbackup` round trip carries the row, and the restored wallet reports the same state.
   Wallet removal deletes the row.
9. `invalid_proposal` lists every failing field.
10. `cutoff_soon` at the two-day boundary.

**Contracts:** mg-engine 6 (§3 SHA via `DW_BLESS=1`, the §2 rows, the §4 codes), m4 33, and dw-app coverage.

**Regtest T1** (the acceptance), check 7 of `regtest/functional/dwd_governance.py`:
1. dwcli, with a funded regtest wallet, runs `--propose` and pays the fee.
2. One block is mined. dwcli submits on its own, with no submit command: a Core node fetched it.
3. At 1–5 confirmations no Core node's `gobject list` holds the proposal (postponed, D19).
4. Mine to 6 confirmations. Every Core node's `gobject list` holds the proposal with dwcli's hash, and its
   `collateralHash` is dwcli's fee txid.
5. The masternodes `vote-many yes`.
6. dwcli, synced again, lists the proposal public with Core's counts.

Asserts along the way:
- Core's `gobject check` accepts dwcli's data hex;
- the fee transaction has no non-P2PKH output;
- the `debug.log` of each Core node has no "Governance object is invalid", "Can't find collateral tx" or
  "not mined yet" line (D6, D21).

**Mutants** (by hand, as in MG-05):
- the broadcast before the save;
- the announcement at 0 confirmations;
- the P2PKH check removed, and the automatic-change refusal removed;
- the fee output's value other than `proposal_fee` (no code compares it: the engine sets it);
- the withdrawal at 0 confirmations after a reorg removed;
- a retry below 6 confirmations once fetched;
- the `cutoff_soon` boundary;
- the propagation wait (D21) removed, and the caught-up condition ignored;
- the waits not keyed to the synced run (D7);
- `lost` set for a fee missing from history (D7);
- no resend of a fee seen unmined (D10).

**Also in this branch:** the MG-05 r2 nits.
- **N1:** `coinjoin.rs` `path_of` goes through `own_address_info`.
- **N2:** the no-derivation PSBT claim is asserted through `create_unsigned`, with the previous transaction put in
  the history store.

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | The fee transaction is Core's `GenBudgetSystemCollateralTx`: one `OP_RETURN <hash>` output of exactly `proposal_fee`, and change to a fresh BIP44 internal (P2PKH) address at a random position, as Core's `RANDOM_CHANGE_POSITION` (`wallet/spend.cpp:1307`). It takes no recipient, no custom change address and no CoinJoin source. After signing, before anything is reserved or saved, every non-`OP_RETURN` output is checked to be P2PKH; a failure is `internal` (the charge is refunded), because the API cannot produce one. | Core refuses the fee if any output is not P2PKH or unspendable (`object.cpp:833-844`), and a P2SH change output would burn the 1 DASH (MG-00 hand-over row). Core accepts `≥` the fee; the wallet pays exactly the fee, as Core's own wallet does. |
| D2 | The engine assembles the fee transaction itself: planner inputs and change, the `OP_RETURN` output, then P2PKH signing input by input, as the CoinJoin collateral is signed (`coinjoin.rs` `collateral_tx`). It does not use key-wallet's `TransactionBuilder`. | The pinned builder has only `add_op_return(data)` at value 0 (`transaction_builder.rs:264`) and no valued data output. A pin change to add one is out of scope. The plan, the cap check and the reservation stay the draft's. |
| D3 | The object has parent zero, revision 1, and the wall clock at creation as its time. Its epochs come from the chosen first superblock (`proposal::epochs`). | dash-qt's `prepareProposal` hardcodes a null parent (`wallet/interfaces.cpp:864-881`). Core checks no proposal time except `end_epoch` (§1). |
| D4 | The proposal row is saved before the broadcast, in app.sqlite `mg_proposals`, keyed by `wallet_id`. It is carried by `.dwbackup` unchanged and deleted with the wallet. A failed save broadcasts nothing; a cancelled hand-off deletes the row. | MG-00 D18, Core's order (`rpc/governance.cpp:245-250`). `rows.rs` already exports every `wallet_id` table. A row for a fee never sent would show a payment that did not happen. |
| D5 | The state is derived on each read: the fee's history entry, the row's marks, the store and the clock. Only facts are stored. | A restore or crash at any point reads correctly, with no state to repair. |
| D6 | The wallet submits at ≥ 1 confirmation by announcing the object (`inv`) to the peers of a synced run and serving it on `getdata`. A peer's `getdata` counts as "submitted". It never announces at 0 confirmations. | MG-00 D16. Core takes an object only if announced or requested (`net_governance.cpp:182-191`), and penalises an invalid one with 20 points (`:197`). A 0-confirmation fee is invalid without `fMissingConfirmations` (`object.cpp:798-808`). At 1–5 it is postponed (`governance.cpp:262-266`). The protocol has no acknowledgement. |
| D7 | The wallet retries until a peer fetched the proposal: every new run announces it; a fee back at 0 confirmations withdraws it, and its return queues it again as a new round to every peer; with no fetch 60 s into a synced run it reports `ready_to_submit{no_peer_accepted}` and starts a new sync run, at 60 s doubling to 30 min, while the app runs. `submit_proposal` starts one at once, its wait starting when that run is synced. Each wait (this one and D20's) is keyed to the synced run (`last_synced_at`, in seconds: two runs synced in the same second share a key, which only shortens one wait) and starts again with a new one. Only a fee seen back in the mempool after a fetch expects a new fetch; a fee missing from history withdraws the object and expects nothing new. | MG-00 D16 ("retrying while the app runs"). `CheckPostponedObjects` erases a postponed object whose fee went back to 0 confirmations (`governance.cpp:872-886`). Re-announcing on the same connection does nothing: Core skips known inventory (D20), so a retry needs new connections. |
| D8 | The proposal is public once the store has the object confirmed (two distinct peers announced it, MG-02 D3). The wallet's own copy is not stored at submission (the store takes the wallet's own objects as confirmed); Core's checks run on it at creation (D11). | The wallet cannot check the fee itself (MG-02 D3: no txindex, BIP158 leaves out `OP_RETURN`). Peers relay the proposal only from 6 confirmations, so the public state means the network took it. |
| D9 | Submitting needs governance sync on and a synced run. Without them the row reports `ready_to_submit{sync_disabled}` or `{not_synced}`. | The wallet's own items are announced only once a run is synced (MG-02 D24), and the governance peers are the sync's. Core's node submits on its own: a deviation of means, not of rules. |
| D10 | The fee is signed under a "Proposal" lease (`FlowKind::Proposal`) from a `Spend` grant. The cap counts 1 DASH plus the network fee. The fee goes through the dispatch fence like a contact payment. A resend of the saved fee (`fee_unsent`, or `fee_paid{0}`: seen but unmined) needs no grant and goes to the library's broadcast directly, unfenced. The object itself is not fenced. | MG-05 §6 and D8, MG-00 D35. The `OP_RETURN` value leaves the wallet. A resend is the same signed txid, as dash-qt's Resend (QT-091): it pays nothing new. Like QT-091 it checks no reservation: the fee's coins are already spent by those same bytes. A proposal carries no key and moves no money. |
| D11 | A draft passes `proposal::validate` (every failing field, `invalid_proposal{fields}`) and then `check_object` on the built object, which is Core's `ValidateProposal` with the expiration check. The wizard limits stay: 1–12 payments, and a first payment among the next 12 superblocks. A proposal past `end_epoch` before going public is `expired`. | A draft Core would refuse must not cost a fee. Core's cleanup pass drops expired proposals (`governance.cpp:474-479`). The two wizard limits are UX-SPEC's (§4.25.3) beyond Core. |
| D12 | The payment address may be P2PKH or P2SH. Only the fee's outputs must be P2PKH. | Core `ValidatePaymentAddress` with `fAllowScript = true` (`object.cpp:177-204`), as `proposal::payment_address_valid` already does. |
| D13 | `SuperblockDate` gets `voting_cutoff_height`, `voting_cutoff_estimate`, and `cutoff_soon`, which is set when the estimated cutoff is within 172,800 s of now. The cutoff is the superblock height minus the maturity window. | The MG-00 hand-over row and UX-SPEC §4.25.3, which preselects the next superblock when the cutoff is within two days. It is the same cutoff the clock card shows (MG-02). |
| D14 | Console `gobject prepare`, `submit` and `list-prepared` stay unavailable. | They spend or need a grant, and the console has no passphrase prompt. MG-08/MG-09 own those surfaces. |
| D15 | The wallet's proposals have no delete. Ended ones stay; the UI hides them by default (MG-00 D17 "Ended"). | Core keeps prepared objects in the wallet, and dash-qt has no delete. A deleted row could no longer be resumed. |
| D16 | The fee is `proposal_fee` (1 DASH) on every network. | `GOVERNANCE_PROPOSAL_FEE_TX` is a compile-time constant with no per-network override (`object.h:87`). |
| D17 | `dwcli governance --propose name:url:address:amount:count:first-superblock` (with `--passphrase-file`) drives the regtest check. The full `gov` command set stays MG-08's. | The T1 acceptance needs a creator. Keeping it to one option leaves MG-08's command design open. |
| D18 | MG-05 r2's N1 and N2 are folded in. | The manager's request: both are cheap and test-only or a one-line route. |
| D19 | A peer holds a proposal postponed at 1–5 fee confirmations: "submitted" (fetched) can happen at 1, "public" not before 6. A fetched proposal below 6 waits with no retry and no error. Announcements go once per queuing round and connection, so a peer holding it postponed is not sent it again; a new connection's single `inv` is ignored by that peer. | `fMissingConfirmations` puts it in `mapPostponedObjects` (`governance.cpp:262-266`); `CheckPostponedObjects` adds and relays it at 6 or erases it (`:856-887`); `ConfirmInventoryRequest` does not request an object held postponed (`:622`). Fee confirmations: 6 for the fee, 1 to relay (`object.h:87-89`, `object.cpp:863-866`). The manager's MG-06 approval. |
| D20 | "Public" comes from a fresh connection: 120 s after the fee's sixth confirmation with no echo, a new sync run looks for the proposal, on D7's backoff. | Core marks every inventory a peer announced as known (`AddKnownInv`, `net_processing.cpp:4561`) and skips known inventory when it relays (`:1247-1249`), so a peer the wallet announced to never announces it back on that connection. A fresh run's inventory sync asks every peer anew (MG-02). |
| D21 | The wallet announces a proposal only once its chain sync is caught up and it has seen the fee's first confirmation for 15 s. While not caught up, the wait does not start. | A peer that lacks the fee's block finds no collateral: "Can't find collateral tx" (or "not mined yet") returns invalid with no `fMissingConfirmations` (`object.cpp:801-813`), which scores the wallet 20 points (`net_governance.cpp:197`), not a postponement. A wallet that sees a block first, or whose view leads, could announce before its peers have it. 15 s covers a block's relay to the peers of a caught-up node; the cost is a 15 s later "submitted". The regtest check fails on either log line from any node. |
| D22 | Once the fee is broadcast, a failure to record that (sent, deleted or the error) is logged and the call still answers by the fee's outcome. A save that fails, or whose task fails, abandons the signed fee and deletes the row as far as it can (a commit may have landed before the error). A row left behind reads `fee_unsent`, and its [Submit] broadcasts the saved fee. Each tick also withdraws any announced proposal whose row is gone (its wallet removed). | The fee's fate decides the answer: a row left unrecorded reads `fee_unsent`, and a fee that went out shows in history. Failing the call would invite a second proposal for a fee that was paid. |
