# DP1-02: the registration funding ownership and evidence model

Status: design rev0, for the manager (simplify-early rule). Base: `dw/dp1-02-registration` at
6bcaa2f, P2b fix r4 at 52215e3. Inputs: Sol r1 to r8, Opus high, the implementer's handoff.
Nothing here is implemented. It replaces the per-path rules of r1 to r8 with one ledger and the
functions of §5.

## 1. Why the holes kept coming

1. **Evidence had two values.** `Provenance::read` turns "no signed txid" into `Unclaimed`
   without reading the step's status (`registration_flow.rs:933`), so a standing marker whose
   signed record was lost on restart reads as nothing (R8-01).
2. **Evidence was keyed by row.** It is read only for rows that exist (`:930`), so deleting a
   row deleted its ownership (R8-02).
3. **Every path had its own filter on top**: `reserved`, `orphaned`, `destined_lock`,
   `settled`, Finish's `calling`, `awaiting_a_lock`'s `broadcast_unknown` arm, the post-call
   `(funded, tracked)` match, three pending counters. Each fix covered one path and the next
   round found another (R7-01, R7-02, R8-03).

The model removes all three: evidence has three values, is keyed by funding step, is read once
per decision, and every path asks the same functions.

## 2. Facts the model rests on (code, P2b 52215e3, pin bc41f1b)

- **F1.** The funding marker is durable before the library starts and carries the step's
  `FundingKey` (`fence.rs` `CallStepInfo.funding`).
- **F2.** The signed record is written in `note_signed` (`journal.rs:362` →
  `record_signed_locks`, `fence.rs:3220`), before the persister, so before the broadcast. A
  failed write only logs: the record stays in memory (`durable: false`) and is lost on restart.
  The marker is not, so after a restart P2b reads `MaybeSent` with no txid.
- **F3.** A `NotSent` supersede drops the step's key, self-spends and signed records, on disk as
  in memory (`fence.rs:1052-1059`).
- **F4.** Repair refuses `NoKnownLock` whenever the step has no signed lock (`mode_b.rs:855`):
  a marked step without a signed record has no path to `NotSent` today.
- **F5.** A lock's bytes don't identify the step that built it. A lock rebuilt from the chain
  reports index 0 (`reconstruction.rs:68`). The credit output is the registration account's
  next unused address, not keyed by identity index (`build.rs` `peek_next_funding_address`:
  "ignored by others"). Amounts are fixed (3,000,000 or 25,000,000). Only the signed txid is
  exact; the library's live metadata (a `Built` row, a non-zero index) comes close.
- **F6.** Restore imports rows with new ids (`import_wallet_rows` skips rowid aliases). On a
  machine whose journal kept the wallet's steps, a restored copy of row A is a different draft,
  and S_A is a step with no row. Orphan steps are a supported state, not only a legacy one.
  Wallet removal keeps the steps (verified, §3.1).
- **F7.** P2b vetoes only a second call of the same step (`funding_gate`); a step stays bound
  to its key (`StepKeyMismatch`).
- **F8.** `FenceState::funding_steps_of(wallet)` (`fence.rs:953`, `pub(super)`) lists every
  step whose marker or `Sent` row stands.

## 3. State model

### 3.1 Step evidence (requirement 1)

The ledger reads every `registration/<draft>/funding` step of the wallet in one J step, whether
or not its row exists (PA-1, §9): `StepView { draft: i64, key: FundingKey, status:
Option<DispatchState>, signed: Vec<Txid> }`.

| claim | when |
|---|---|
| `None` | status ∈ {None, NotSent}: nothing of the step can be out |
| `Exact(T)` | status ∈ {WillBeSent, MaybeSent, Sent} and `signed` = T ≠ ∅ |
| `Unknown` | status ∈ {WillBeSent, MaybeSent, Sent} and `signed` = ∅ |

`Unknown` means a call was marked and may have signed, but the txid is unknown. It comes from a
running call before its signature, a record lost on restart, or a `NotSent` row not yet landed.

**Wallet removal keeps the steps (F6, verified on P2b 5a1e3ab).** `remove_wallet` deletes the
wallet's `dp_registration` rows (`AppDb::delete_wallet`). It then erases the wallet's journal
rows (`erase_wallet_rows` → `DispatchJournal::erase_wallet`). That erase takes only the
artifacts DEC-160's `COMPACTABLE` selects: no standing marker and no `Sent` or `MaybeSent` row.
Their `funding_marker`, `self_spend` and `signed_lock` rows go with them (`compact`). So every
step whose claim isn't `None` keeps its marker, key and signed txids, on disk and in memory
(`forget_erased` drops only the erased artifacts). This holds whether or not the `dispatch` rows
go. A step resolved `NotSent` is erased, but its claim was `None` already. The wallet id derives
from the seed (`wallet_id_for_seed`). So re-adding the same mnemonic, with or without a restore,
finds those steps and no rows: each is an orphan, and `row_of` is the heir or none. A crash
between the row delete and the erase keeps a superset.

The probe was a throwaway `dw-appdb` test, not committed. It used four steps: a bare marker, a
marker with a signed record, a `Sent` step, and a `NotSent` step with a signed record. Erased
with `dispatch` false and with it true, the first three steps and one signed row stayed each
time; only the fourth went.

### 3.2 Derived predicates (over one ledger: rows, tracked locks, identities, steps, in-call drafts)

- **`consumed(o)`**: o's lock is tracked `Consumed`, or `identity_for(o)` is an identity of the
  wallet or of a row.
- **`own(S)`**: the txids in `signed(S)`; if empty, the txid of the outpoint `row_of(S)` holds
  (written through `may_take`, I3); else ∅.
- **`settled(S)`**: `own(S)` ≠ ∅ and every txid in it is held by a row whose lock is
  `consumed`. This is requirement 1's "consumed by this row". An identity consumed while no row
  holds its lock does not settle the step (R7-02).
- **`outstanding(S)`**: claim ≠ `None` and ¬`settled(S)`. An `Unknown` step holding no outpoint
  never settles; only P2b's `NotSent` ends it.
- **`row_of(S)`**: the row whose id is `S.draft`. If that row is gone, the heir (F6): the oldest
  readable CoreBalance row at `S.key.identity_index` that is `KeysPrepared`, holds no outpoint
  and no identity, and has no outstanding step of its own. Otherwise none.
- **`steps(r)`**: the outstanding S with `row_of(S)` = r.
- **`open(r)`**: r is a readable CoreBalance row at `KeysPrepared` with no outpoint and no
  identity. Its failure code doesn't matter; attaching a lock clears it.

### 3.3 Row standing (first matching line wins)

| # | condition | standing | committed |
|---|---|---|---|
| 1 | `interpret` says unsupported | `Unreadable` | iff DEC-171 (3)'s may-own rule (unchanged) |
| 2 | not CoreBalance | `Unfunded` (no user funds; never funds) | no |
| 3 | holds outpoint o, ¬`consumed(o)` | `Funded(o)` | yes |
| 4 | `steps(r)` ≠ ∅, `open(r)` | `Awaiting` | yes |
| 5 | `steps(r)` ≠ ∅, ¬`open(r)` | `Closed` (its own lock can't come back to it) | yes |
| 6 | identity present | `Settled` | no |
| 7 | anything else | `Unfunded` | no (yes while its funding call runs) |

### 3.4 Lock owner (a registration lock of account 0 that no row holds and that isn't consumed)

| # | condition | owner |
|---|---|---|
| 1 | txid ∈ `signed(S)`, S not outstanding | `Dead` (resolved `NotSent`; can't land) |
| 2 | txid ∈ `signed(S)` | `Step(S)` |
| 3 | an unreadable row may own it (`may_be_unreadables`, DEC-173 (2)) | `Parked` |
| 4 | some `Unknown` outstanding S holds no own lock, and `known_index(L)` ∈ {None, `S.key.identity_index`} | `Contested` |
| 5 | anything else | `Free` |

Requirement 3 (hold the lock, or give it to the uncertain row on evidence?): **hold it.** By F5
nothing attributes a `Contested` lock to an `Unknown` step: its inputs are in the lost record,
its amount is fixed, and its funding key isn't in the bytes. The library's live metadata is the
only near-exact source, and under PA-2 (§9) it can't coexist with a lost record. Without PA-2,
see Q6.

## 4. Invariants

- **I1. Evidence has three values.** Every outstanding step is `Exact` or `Unknown`. A missing
  txid is never read as missing funding.
- **I2. Ownership outlives rows.** `owner` and `outstanding` read steps, never rows, so deleting
  or losing a row changes neither. Discard refuses every committed row. An orphan step acts
  through its heir or through Recovery.
- **I3. A signed lock goes only to its signer.** A lock signed by S is written only to
  `row_of(S)` while that row is `open`. With no such row, or one not open, it goes only to a
  recovery row (Finish's new unnamed row, which has no step and never funds), never to another
  row. Every outpoint write passes `may_take`, the successful call's return included, so a
  recorded outpoint is trusted by induction.
- **I4. A reserved row stays put.** A row with an outstanding step, own or inherited, never
  funds, never moves index, takes only its own `Step(S)` locks, and is committed.
- **I5. Uncertainty quarantines locks.** While an `Unknown` step is outstanding, no row takes a
  `Contested` lock and no Finish imports one.
- **I6. One funding per intent.** A row funds only from `Unfunded` with a free index. `start`
  refuses while any row is committed and not Done, or any outstanding step has no row. So no
  replacement funding is admitted while an earlier one may be out.
- **I7. Only evidence releases.** Only P2b's `NotSent` or `settled` ends an outstanding step.
  Failure codes, phases, `retryable` and lock statuses are never `NotSent` evidence.
- **I8. One function.** Every path builds one `Ledger` in its actor job and asks only the §5
  functions. No path keeps its own ownership filter.
- **I9. Every hold has an exit (§8),** and no DP1-02 hold waits on another DP1-02 hold.

## 5. The decision function

```rust
pub(crate) struct Ledger { locks: Vec<LockInfo>, identities: Vec<(u32, String)>,
    rows: Vec<RegistrationRow>, steps: Vec<StepView>, calling: Vec<i64> }
pub(crate) enum Standing { Unreadable { committed: bool }, Unfunded, Funded(OutPoint),
    Awaiting, Closed, Settled }
pub(crate) enum Owner { Step(i64 /* signer's draft */), Parked, Contested, Dead, Free }
pub(crate) enum Taker { Row(i64), Recovery }

impl Ledger {
    pub(crate) fn read(/* rows, lib locks and identities, leases, flows */) -> Self; // one actor job + one J read (PA-1)
    pub(crate) fn standing(&self, row: &RegistrationRow) -> Standing;     // §3.3
    pub(crate) fn owner(&self, lock: &LockInfo) -> Option<Owner>;         // §3.4; None: held or consumed
    pub(crate) fn may_take(&self, taker: Taker, lock: &LockInfo) -> bool; // §5.1
    pub(crate) fn committed(&self, row: &RegistrationRow) -> bool;        // §3.3, or its call runs
    pub(crate) fn pending(&self) -> u32;                                  // §5.2
    fn own_lock(&self, row: &RegistrationRow) -> Option<&LockInfo>;       // a Step(S) lock, S ∈ steps(r), if may_take
}
```

### 5.1 `may_take`

| owner | `Row(r)` | `Recovery` |
|---|---|---|
| `Step(S)` | r = `row_of(S)` and `open(r)` | `row_of(S)` is none, or readable and not `open` |
| `Free` | `standing(r)` = `Unfunded` and `open(r)` | yes |
| `Parked`, `Contested`, `Dead` | no | no |

A path then adds only its eligibility, which picks a candidate and never decides ownership. The
tap picks the row in call, else a row at the lock's known index. The run picks its own lock,
else the index restore. Finish picks the row at the lock's known index, else the oldest
`Unfunded` row waiting for a lock whose amount the lock covers, else `Recovery`.

### 5.2 `pending`

`pending` is Finish's `still_pending` before the per-row runs; the held path uses the same
formula. It is read on a fresh ledger after Finish's writes, so a lock just given a row is held
and drops out.

```text
pending = |{ S outstanding : no row holds a txid of own(S) }|
        + |{ L : owner(L) ∈ {Parked, Contested, Free} }|
```

A `Step(S)` lock counts once, through its step; `Dead` locks aren't money. An `Unknown` step and
a `Contested` lock that may be the same money count as two: conservative, and the report says so.

### 5.3 Gates by standing

| standing | discard | in progress | `funds_committed` | resume (clear failure) | the run's funding step |
|---|---|---|---|---|---|
| `Unfunded` | allowed (refused while in call) | if live or retryable | while in call | if retryable or `broadcast_unknown` | `index_owner`, then fund |
| `Funded(o)` | refused | unless Done | yes | yes | record o, then the identity |
| `Awaiting` | refused | yes | yes | refused (it waits) | own lock present → record it, else `maybe_sent` |
| `Closed` | refused | unless Done | yes | as its failure | the identity step of what it holds |
| `Settled` | allowed | no | no | as its failure | none |
| `Unreadable` | refused | if committed | if committed | refused | `Stop` |

"In progress" is (no failure and phase ≠ Done) or retryable or (committed and phase ≠ Done),
plus I6's wallet-level term: an outstanding step with no row.

## 6. Every path mapped onto it

| path today | becomes |
|---|---|
| `Evidence::new(..).with_signed` (flow `:1014`; reg `:454`, `:526`, `:567`, `:723`, `:854`) | `Ledger::read` |
| `Driver::fund` before the call (`:2121-2159`): `signed_lock`, `reserved → maybe_sent`, `index_owner`, the `funding_status` gate | `own_lock` → `record_lock`; `Awaiting`/`Closed` → `maybe_sent`; `Unfunded` → `index_owner` → call. P2b's permit stays as the backstop |
| `Driver::fund` after the call (`:2233-2287`, the `(funded, tracked)` match) | a fresh ledger: `own_lock` → `record_lock` (phase by `IsTimeout`). An `Ok(o)` that isn't the own lock breaches I3: log, `maybe_sent`. `Awaiting` → `maybe_sent`. `Unfunded` → by error: `Refused` → `failed`, `CoinsLocked`, `NotSent` → `unfund`, `Ok` → `maybe_sent` (fail closed) |
| `lock_of` (`:1090`) | outpoint → that lock; else `own_lock`; else, for an `Unfunded` row only, the `Free` lock at its known index |
| `index_owner` (`:1214`), reached by `Unfunded` rows only | a `Free` lock at its known index → `Lock`. An identity at the index: its lock by `may_take(Row r)` → `Lock`, another row's `Step` → `Taken`, `Contested`/`Parked` → `Unknown`. Another row at the index that holds an outpoint or identity, is committed, or is older → `Taken`. An unheld `Free`, `Contested` or `Parked` lock with no known index → `Unknown` |
| tap `note_lock_tracked` (`:1441`) | candidates at the index with no outpoint, filtered by `may_take(Row r, L)`; the in-call row first, else the oldest. Its `unreadable` local and `broadcast_unknown` filter go |
| Finish `import_stranded` (`:1553`) | each unheld lock by owner: `Step` → `row_of(S)` if open and not in call, else `Recovery`; `Free` → per §5.1; others → none. The held path gives nothing. Returns `pending()` |
| Finish's run loop (reg `:908`) | unchanged: it runs the funded rows |
| `adopt_registration_lock` (appdb, new rows) | called only after `may_take(Recovery, L)`. Exempt: the appdb is a CAS writer with a unique outpoint, not an authority |
| `record_lock` after `Ok` (`:2240`) | routed through `own_lock` (above) |
| `lock_of` trusting a recorded outpoint | exempt by I3's induction. A legacy row holding a substitute is `Closed`, and its own lock goes to `Recovery` |
| `funding_status`, `funds_committed`, `resumable` (`:1288-1359`) | `committed`, `standing` (§5.3) |
| `in_progress`, `settled`, `discard_refused` (reg `:430-447`, `:811`) | §5.3 |
| `statuses.funds_committed`, `idle_wait` (reg `:669`, `:685`) | `committed`; `Awaiting` → `ChainLock` (see Q4) |
| `next_index` (`:1363`) | also skips every outstanding step's key index, so no fresh row lands on an orphan's slot |
| `assign_index`, `create_identity`, the proof wait | unchanged: only `Unfunded` rows move, and only rows holding an outpoint create identities |

## 7. What gets deleted

- `Provenance` (by-row `read`, `ownership`, `reserved`, `may_take`), replaced by `Ledger`.
- `Evidence::orphaned` (rule 4's pending for ever), replaced by `Step` → `Recovery`.
- `destined_lock` and its `MaybeSent` arm. An unmarked restored row has no funding of this
  device's in flight; its possible lock is `Free`, for Finish.
- `awaiting_a_lock`'s `broadcast_unknown` arm (those rows are now `Awaiting`).
- `signed_lock`, renamed `own_lock`.
- Finish's `calling` and `deferred`, and `Flows::calling`: a running call's step is `Unknown`
  or `Exact`, so its lock is `Contested` or `Step`.
- The `reserved` and `orphaned` counters and the held path's own return, replaced by `pending()`.
- `settled(row)` and its exemptions in `in_progress` and `discard_refused`.
- `funding_status`'s ad-hoc branches (phase, outpoint, `broadcast_unknown`, reserved, destined)
  and the `(funded, tracked)` match.
- No schema is added under PA-2. The fallback in §9 needs one.

## 8. Liveness: every hold has an exit (requirement 3)

| hold | exit |
|---|---|
| `Funded(o)`, lock tracked | the run: proof, then identity, then `Settled` |
| `Funded(o)`, lock untracked | reconstruction tracks it again; or, for an `Exact` step, Repair → `NotSent` |
| `Awaiting`, `Exact`, lock present | the run, the tap or Finish attaches it |
| `Closed`, `Exact`, lock present | Finish → `Recovery` → its identity → `settled` |
| `Awaiting`/`Closed`, `Exact`, lock away | it returns (above), or a Repair self-spend over its recorded inputs → `NotSent` (P2b residual: `LockInputsSpent`) |
| `Awaiting`, `Unknown` | with PA-2, transient: the call ends `Exact` or `NotSent`, and after a restart P2b derives `NotSent`. Without PA-2, none (F4 `NoKnownLock`, F5 no content evidence): it needs §9's Release |
| `Contested` lock | freed when no `Unknown` step stands |
| `Parked` | unchanged (DEC-173): until a build that reads the row runs it |
| orphan step, `Exact` | its heir takes the lock, or `Recovery`; if the lock is away, Repair |

**No cycle.** DP1-02's holds wait only on P2b's derivation, the chain and Repair. P2b's own
waits (Repair's `UnattributedLock`, the gate's markerless rule) wait only on attribution, which
a `Free` unattributed lock reaches through Finish → `Recovery` → identity → P2b's
`attributed_row`. The only link through DP1-02 is `Contested` → `Unknown` → PA-2, which ends in
P2b. So with PA-2 every reachable hold ends through Finish, Repair or a proof, with no further
user decision.

Without PA-2, an `Unknown` step after a restart is a permanent wedge. It takes an F2 write
failure plus a restart, or a `NotSent` row that didn't land plus a restart. It blocks the row,
new registrations and every `Contested` lock.

## 9. P2b asks

- **PA-1 (required).** On `LeaseTable`, `pub(crate) fn registration_funding_steps(&self,
  wallet) -> Vec<FundingStepView>`, one J step. For each `registration/<draft>/funding` step in
  `funding_steps_of`, it returns the parsed draft, the marker's `FundingKey`, `funding_status`
  and the signed txids. Read-only, no schema. It replaces DP1-02's per-row calls to
  `signed_lock_txids` and `funding_status`.
- **PA-2 (recommended): no broadcast before a durable signed record.** This closes the only
  wedge by construction.
  - `note_signed` must not return to the library until the step's `signed_lock` row is
    durable; if the write keeps failing, it blocks or stops the process. The library
    broadcasts only after its store returns, so no lock reaches the wire without a record.
  - P2b may then read `NotSent` for a marked step with no signed record and no running call.
    That covers the state after a restart, and a call that released signatures but never
    stored a lock. M-2 still applies to stored-then-rejected locks, which have records.
  - `Unknown` then lasts only for a call's pre-signature window or a `NotSent` write in
    flight, and R8-01's state can't arise. P2b reviews the derivation's soundness itself.
- **PA-3 (minor).** Keep a superseded step's signed txids as dead in the view (F3 drops them).
  Without it, a still-tracked `Built` lock of a `NotSent` step reads `Free`, and Finish gives
  it a recovery row that waits for a proof for ever. No funds are lost, but an unnamed row is
  stuck.
- **Fallback if PA-2 is refused: a user Release.**
  - Repair ▸ "Stop waiting for this funding", for an `Unknown` step (`Awaiting` or orphan) with
    no call running.
  - It needs a durable record: a P2b resolution `Abandoned`, or a DP1-02 table
    `dp_funding_release(wallet_id TEXT, step TEXT, released_at INTEGER, PRIMARY KEY (wallet_id, step))`.
  - The step stops being outstanding. Its row reads `Unfunded` with a non-retryable
    `registration.funding_abandoned`: discardable, never funds again. `Contested` locks turn
    `Free`, and Finish gives each a recovery row.
  - The user accepts that a lost lock landing later becomes an unnamed identity, and that a
    new registration may have paid a second lock. No funds are lost.

## 10. Closure table, r1 to r8

"Kept" means a mechanism that isn't evidence (claims, the actor, CAS writes, the fence); this
model leaves it unchanged.

| finding | closed by |
|---|---|
| R1-01: synthetic index lets a restore fund twice | `known_index` (kept) in `owner` and `index_owner`; I6 |
| R1-02: overlapping discard guards | kept (exclusive take) |
| R1-03: KeysPrepared row with its own outpoint is stranded | `Funded(o)` → the run (DEC-171 (4)) |
| R1-04: unknown encoding erases funding evidence | `Unreadable{committed}`; I2 |
| R1-05: index identity adopted before the ownership check | `index_owner`'s identity branch asks `may_take`; I3 |
| R1-06: unsupported profile refused only after funding | kept (`interpret`) |
| R1-07, R2-04, R3-01, R4-01: coins and reservations | kept (P2b fence, DEC-179) |
| R1-08, R2-01: retired claims | kept (run ids, owner generation) |
| R2-02: Finish consumes a lock an unreadable row may own | owner `Parked`; I3 |
| R2-03: stale evidence write | kept (CAS), with the tap's ledger read in its own job; I8 |
| R5 N1: whole-path tap regression | tests kept |
| Opus MJ-1: a failure after the identity exists wedges registration | `Settled`: neither committed nor in progress; I7 |
| MN-1: untapped index-0 lock plus a non-Ok call | owner `Step` by txid, at any index or status; after a restart, I1 and I5, plus PA-2 |
| MN-2, MN-3, MN-4 | kept |
| R6-01: own lock's proof races the attribution | ownership by txid, independent of status; I3 |
| R6-02, R6-04: settlement on error classes and failed reads | kept (allow-list, `after_read`) |
| R6-03: call-window heuristic | deleted. During a call a foreign lock is `Contested` (I5); after it, `Free` with no known index, so `index_owner` says `Unknown`: Finish only |
| R7-01: Finish substitutes a lock, then orphans the signed one | `Awaiting` takes no `Free` lock (I4); a `Closed` row's own lock goes to `Recovery` (I3, I9) |
| R7-02: identity fallback bypasses another row's signed ownership | `may_take` in `index_owner`; `settled` needs a holding row, so A keeps T1 (I4), and B is `Unfunded` and moves |
| R8-01: lost record allows discard, replacement and substitution | `Unknown` (I1) → `Awaiting`: discard and `start` refused (I6); T2 and T1 `Contested` (I5); exit PA-2 |
| R8-02: settled-row discard frees T1 | read by step (I2); `settled()` deleted; A is `Closed`, so committed: discard refused until T1 settles through `Recovery` |
| R8-03: held path drops the pending count | one `pending()` for every path (I8) |

## 11. Test plan

### 11.1 Model check

`docs/design/checks/dp1_02_ownership_model.py`, in the style of P2b's `e0_04_design_model.py`:
exhaustive, run as `python3 -I -B`, exit 0 means pass.

- **State:** rows A and B (phase, index, outpoint, identity); steps S_A and S_B (marker, status,
  signed, durable); locks T1 (A's), T2 (foreign, same seed) and T3 (B's), each tracked or not,
  trusted index or not, proved, consumed; in-call flags.
- **Events:** mark, sign, record (lands or fails), tap write (lands or fails), reject and
  untrack; `Ok`/`Err` returns; a restart (drops undurable records); reconstruct at index 0; a
  foreign lock appears; an identity is created; Finish, held or not; discard A or B; start C,
  and C funds; Repair (admitted per P2b r4); `NotSent`; restore as new ids (an orphan plus a
  copy); PA-2 on or off.
- **Safety:**
  - **S1:** no funding call starts while a discarded or lost row's step is outstanding.
  - **S2:** no row holds a lock signed by another step.
  - **S3:** no committed row is deleted.
  - **S4:** no `Contested` lock is held.
  - **S5:** `pending` ≥ the outstanding steps whose own lock no row holds.
  - **S6:** an outstanding step's row never changes index.
- **Liveness:** with PA-2, every reachable state reaches "nothing outstanding" through Finish,
  Repair and proofs. Without PA-2, the stuck set is exactly the `Unknown`-after-restart states.
- **Rejected rules, each of which must fail the check:** provenance read by row; `Unknown` read
  as Unclaimed; the `settled()` exemption; a held path that omits the step term; rule 4's
  pending for ever (liveness); no heir (S1 through a restore); a new row taking a `Contested`
  lock.

### 11.2 Rust regressions

In `registration_tests`, with the real `LeaseTable`, dispatch and appdb SQLite, and the Fake
library:

1. **R8-01a, discard and replace.** A trigger rejects only `signed_lock` INSERTs; the
   three-lock-write-failure fixture; a real close and reopen; a discovered identity at index 1
   so the replacement's outpoint differs (Sol's trick). Assert: A is `Awaiting` with
   `funds_committed`, discard returns `Err`, `start` returns `Err(InProgress)`, one funding
   call.
2. **R8-01b, substitution.** The same fixture with T2 reconstructed at index 0. Assert: Finish
   leaves T2 unheld with `still_pending` = 2; when T1 returns it is unheld too, `still_pending`
   = 3, and no row's outpoint changes. Under PA-2, run the same check with the Fake holding a
   call before its signature.
3. **PA-2, if adopted.** A failed `signed_lock` write stops the call before the broadcast.
   After reopening, the step is `NotSent` and A is retryable.
4. **R8-02.** Sol's settled-orphan fixture. Assert: discard is refused; Finish gives T1 to a
   recovery row (label "", no step), never a user row; after that, A can be discarded.
5. **R8-03.** On the held path, the reserved row whose lock is away counts (`still_pending` =
   1), and a step whose lock is present counts once.
6. **Restore orphan.** Remove and restore the wallet, keeping the journal. Assert: copy A′ is
   the heir, doesn't fund, and takes T1 when it returns; `next_index` gives a new row a slot
   other than S_A's.
7. **`Ok` mismatch.** The Fake returns `Ok` with an outpoint that isn't the call's lock. Assert:
   it isn't recorded, and the row reads `broadcast_unknown`.
8. **Every earlier regression.** All r1 to r8 regressions stay green under the new names, and
   Sol r8's five passing controls are re-run.

### 11.3 Mutations and traps

Each mutation site must be killed by a named test: `outstanding`'s `Unknown` arm; the heir
rule; `may_take`'s `Step`/`Recovery` arms; `owner`'s `Contested` row; `pending`'s step term;
`committed` for `Closed`; the post-call `Ok` routing; `next_index`'s step term; discard's
`committed` check.

Traps:
- A signed record needs a running funding call, so sign through the Fake's `fund`.
- The Fake's txid is `[index+1; 32]`, so two fundings at one index collide.
- P2b's 5 s lease tests flake at load ~200; rerun them alone.
- `-I -B` keeps `__pycache__` out of `checks/`.

## 12. Open questions for the manager

- **Q1. Change rule 4 to recovery?** A signed lock whose signer can never take it would go to a
  recovery row through Finish (I3) rather than stay pending for ever, which strands the funds
  and breaks liveness. Sol r8's ordinary Done-row control, which expects T1 to stay pending,
  changes accordingly. Approve?
- **Q2. PA-2, or the Release fallback?** PA-2 makes `Unknown` transient and needs no schema.
  Without it, an `Unknown` step after a restart ends only by the user's Release (§9), which
  needs a durable record. Route PA-2 to P2b (whose Sol r5 is running), or approve the fallback?
- **Q3. Who writes PA-1?** P2b, or DP1-02 as a commit that P2b reviews?
- **Q4. `funds_committed` and discard of finished rows.** `funds_committed` becomes `committed`
  or in call, so a Done or `Settled` row reads false (today a consumed lock reads `Sent`, so
  true), and a `Settled` row, Done included, may now be discarded. Or keep the display field
  on §16.5's meaning? Add `RegistrationWait::FundingUnknown` so the UI can point at Repair
  (Opus N-4)?
- **Q5. Approve the heir rule?** It binds a restored copy to an orphan step at the same HD
  index (F6). The alternative is that a restored copy always waits for Finish.
- **Q6. Narrow `Contested`?** By amount: exclude locks whose amount differs from the step's
  readable `lock_duffs` (safe either way; narrowing only shortens holds). Without PA-2, also
  give a trusted-index lock to the single `Unknown` step whose marker key matches?
- **Q7. How do users see an orphan step?** With no heir it has no row, yet it blocks `start`
  (I6). Should `registrations()` report it, or should `InProgress` name "Finish transfers"?

## 13. Manager rulings (2026-10-11)

- **PA-1, PA-2, PA-3: approved, and P2b writes all three** (Q2, Q3). P2b owns the lease table and
  the signed-record durability. PA-2 makes "marker, no signed record, no running call" read
  `NotSent` only because no lock can reach the wire without a durable record. P2b's review
  must check that soundness argument directly. The user-Release fallback is not built.
- **Q1: approved.** A signed lock whose signer can never take it goes to a recovery row through
  Finish. Stuck funds are worse than an unnamed identity. Sol r8's Done-row control is updated to
  expect this.
- **Q4:** `funds_committed` keeps its §16.5 display meaning: true once a lock is committed or
  consumed, so a Done row still reads true. The gate uses its own `committed or in call`
  predicate internally and doesn't reuse the display field. Discarding a `Settled` or Done row is
  allowed; it removes only the local record. Add `RegistrationWait::FundingUnknown`.
- **Q5: the heir rule is approved only on an exact key match.** A restored copy inherits an
  orphan step only if its derived funding key equals the marker's `FundingKey`. Index equality
  alone is not enough. Otherwise it waits for Finish.
- **Q6:** narrow `Contested` by amount: yes. The trusted-index assignment for `Unknown`: no; PA-2
  makes it unnecessary.
- **Q7:** `registrations()` reports an orphan step as a read-only entry, so the UI can say why
  `start` is refused, and `InProgress` names it.
- **F6** (whether wallet removal keeps journal steps) is verified during implementation and
  written into §3.
