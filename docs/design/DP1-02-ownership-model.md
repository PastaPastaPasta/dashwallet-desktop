# DP1-02: the registration funding ownership and evidence model

Status: design rev2, approved with D15 (Sol design r2, APPROVE-WITH-CHANGES). Base:
`dw/dp1-02-registration` at 6bcaa2f, P2b fix r4
at 52215e3 plus its schema-6 work in progress. Inputs: Sol r1 to r8, Opus high, rev1 with the
§13 rulings, Sol's design review (OD-01 to OD-06), and the rulings on OD-01, OD-02 and OD-06.
One ledger and the functions of §5 replace the per-path rules of r1 to r8.

**Changelog.** rev2 (Sol's design review):
- **OD-05:** a calling row takes only its own `Step` lock (§5.1); §6's Finish rule now matches.
- **OD-03:** consumed value is reconciled apart from unspent value. It attaches to an open
  `row_of(S)` with its discovered identity, or settles in place (§5.2a). Restored and legacy
  outpoints get an induction base (§5.2c).
- **OD-04:** a voided outpoint counts as no outpoint. One closure write (§5.2b) clears it and
  closes rows whose step Repair cancelled or whose value another row holds (new `Cancelled`).
- **New from P2b's WIP:** a flagged resolved step reads `None` whatever P2b re-reads (C4), and a
  stale flagged marker closes its row (C5).
- **The rest:** the OD-01/02/06 rulings are folded into §3.1, §8.1 and §9; Q1 now covers
  unspent value only; §10 maps r1–r8, OD-01–06 and the rev1 model-check findings C1–C3; §11
  adds Sol's OD regressions. (rev1, fb6483f, added the §13 rulings to rev0.)
- **D15 (Sol design r2):** a row closed with a cancel code is no step's `row_of` (§3.2, §5.2b);
  the model check covers it (§11.1).

## 1. Why the holes kept coming

rev1 fixed two-valued evidence (R8-01), evidence keyed by row (R8-02) and per-path filters
(R7-01, R7-02, R8-03). rev2 adds terminal evidence the rows never absorb (a holderless consumed
lock, OD-03; a voided outpoint, OD-04), the pre-marker window (OD-05), starting rows that never
passed `may_take` (OD-03), and P2b's readings of steps that are never called again (C4, C5).

## 2. Facts (DP1-02 6bcaa2f, P2b 52215e3; pin bc41f1b, identical to ebe37f8 in these files)

- **F1.** The funding marker, with the step's `FundingKey`, is durable before the library starts.
- **F2.** `note_signed` records a signed lock before the persister. At 52215e3 a failed write
  only logs; OD-01 replaces that (§9).
- **F3.** A `NotSent` supersede drops the step's key, self-spends and signed records
  (`fence.rs:1052`). PA-3 keeps the txids as tombstones.
- **F4.** Repair refuses `NoKnownLock` for a step with no signed lock (`mode_b.rs:855`).
- **F5.** A lock's bytes don't name its step. A rebuilt lock reports index 0, the credit address
  is the account's next, and the amounts are fixed. Only the signed txid is exact.
- **F6.** Restore numbers rows anew, and wallet removal keeps every step whose claim isn't `None`
  (§3.1). Orphan steps are a supported state.
- **F7.** P2b vetoes a second call of a standing step, and a step stays bound to its key.
- **F8.** `funding_steps_of` lists each step with a standing marker or a `Sent` row.
- **F9.** The pin builds every lock's credit at vout 0 (`build.rs:969`). So a signed txid t names
  `lock(t) = (t, 0)` and `identity_for(lock(t))`, whether or not the lock is tracked.
- **F10.** A registration step's key is `(IdentityRegistration, identity_index, 0)`
  (`DraftKey::registration`). For a CoreBalance row, Q5's "exact key" means "same index".
- **F11.** Backup restore (`import_wallet_rows`, `backup.rs:665`) bypasses every ownership rule:
  fresh `AUTOINCREMENT` ids (never reused), recorded outpoints kept, only for a wallet that isn't
  loaded, so its rows meet the steps F6 keeps.
- **F12.** `create_identity` adopts an existing identity and never issues IdentityCreate for a
  tracked `Consumed` lock (flow `:2358-2375`). `identities()` gives each identity's key index.

## 3. State model

### 3.1 Step evidence

PA-1 reads, in one J step, every `registration/<draft>/funding` step of the wallet: standing,
`Sent`, or resolved with tombstones (OD-06). A journal that can't be read returns `Unavailable`,
never an empty set.

```rust
StepView { draft: i64, key: FundingKey, status: Option<DispatchState>,
           signed: Vec<Txid>,     // the active attempt's txids
           dead: Vec<Txid>,       // resolved attempts' txids (PA-3), never resurrected
           durable_record: bool } // OD-02: marker written under PA-2
```

| claim | when |
|---|---|
| `None` | status ∈ {None, NotSent}; P2b derives `NotSent` for C4's step (Q13) |
| `Exact(T)` | status ∈ {WillBeSent, MaybeSent, Sent}, `signed` = T ≠ ∅ |
| `Unknown` | status ∈ {WillBeSent, MaybeSent, Sent}, `signed` = ∅ |

P2b applies OD-01 and OD-02 inside `status`. A flagged marker with no `funding_release` row
(schema 7, E0-04 §2a.7), no durable record and no running call reads `NotSent`, but stands, so
F7 still vetoes its step (C5). One whose call released a funding signature and recorded no lock
reads `MaybeSent` for good (P2b r6 R6-01: a lock may have reached a broadcaster no store of it
passed), and an unflagged one reads `Unknown` (§8.1). So `Unknown` means one of: a call before
its record, a `NotSent` write in flight, a released call with no recorded lock, or an unflagged
marker.

**C4.** P2b re-reads a resolved step `MaybeSent` while any unattributed registration row stands
(`markerless_unattributed_row`, L-2), and `Sent` once a re-keyed row matches its key. For a
flagged step with tombstones neither is its money: PA-2 recorded every lock its one signing call
built, all are in `dead` and conflict with the ChainLocked self-spend, and I12 forbids another
call. Read as `Unknown`, it would freeze the row as `Contested` (§3.4), and P2b attributes the
row only once it funds an identity: a cycle. Under Q13 P2b derives this reading itself: a
flagged step with tombstones whose marker is not `Sent` reads `NotSent`, and DP1-02 reads P2b's
status with no override.

**Wallet removal keeps the steps** (P2b 5a1e3ab, `dw-appdb` probe): `remove_wallet` deletes the
rows and `erase_wallet` only DEC-160's compactable artifacts, so every step whose claim isn't
`None` keeps its marker, key and signed txids, and re-adding the mnemonic (same wallet id) finds
them with no rows. Today a resolved step's tombstones go; PA-3 must keep them (§9).

### 3.2 Derived predicates (over one ledger: rows, tracked locks, identities, steps, callers)

- **`voided(o)`**: o's txid is in some step's `dead`: a ChainLocked self-spend took its inputs.
- **`live(r)`**: r's recorded outpoint unless voided. Every predicate below reads `live`.
- **`consumed(o)`**: o is tracked `Consumed`, or `identity_for(o)` is discovered, or a row holds
  o with an identity.
- **`cancelled(S)`**: S is in the view and its claim is `None`. Either Repair resolved it (P2b's
  policy: a new draft, never the old step, L-2), or after a restart its flagged marker proves
  nothing was signed (no `funding_release` row) but still stands (C5). S is never called again.
- **`calling(r)`**: r's in-call flag. `enter_funding` (`:2084`) sets it in an actor job before
  P2b marks the call, and the guard clears it after the post-call writes (`:2202`).
- **`open(r)`**: r is readable CoreBalance, has an index, no `live` outpoint and no identity.
  Its failure code doesn't matter.
- **`signer(S)`**: the row whose id is `S.draft`. If there is none, the heir: the oldest open
  row at `S.key`'s index (F10) whose id drafts no step. Otherwise none.
- **`own(S)`**: `signed(S)`. If that is empty, the txid of `live(signer(S))`. Otherwise ∅.
- **`holder(S)`**: the row whose `live` outpoint is `lock(t)` for some t in `own(S)`. The
  outpoint is unique, and there is one lock per call.
- **`row_of(S)`** (D15): choose `holder(S)`, otherwise `signer(S)`. If the chosen row is closed
  with `registration.funded_elsewhere` or `registration.funding_cancelled`, return none;
  otherwise return that row. Exact possession precedes draft id and heir. The exclusion does not
  remove any step or lock evidence.
- **`settled(S)`**: `own(S)` ≠ ∅, and every `lock(t)` is consumed and is either held, or
  `row_of(S)` is none or not open. Consumed value settles its step unless an open row of the
  step still has to receive the identity (§5.2a).
- **`outstanding(S)`**: claim ≠ `None` and ¬`settled(S)`. **`steps(r)`**: the outstanding S
  with `row_of(S)` = r.

### 3.3 Row standing (first matching line wins)

| # | condition | standing | committed |
|---|---|---|---|
| 1 | `interpret` says unsupported | `Unreadable` | iff DEC-171 (3)'s may-own rule |
| 2 | not CoreBalance | `Unfunded` (no user funds; never funds) | no |
| 3 | `live(r)` = o, ¬`consumed(o)` | `Funded(o)` | yes |
| 4 | `steps(r)` ≠ ∅, `open(r)` | `Awaiting` | yes |
| 5 | `steps(r)` ≠ ∅, ¬`open(r)` | `Closed` (its own lock can't come back to it) | yes |
| 6 | identity, or `live(r)` consumed | `Settled` | no |
| 7 | ¬`calling(r)`; r = `signer(S)` with `holder(S)` ∉ {none, r}, or r already failed with this code | `Cancelled(funded_elsewhere)` | no |
| 8 | ¬`calling(r)`; r records a voided outpoint, or r = `signer(S)` with `cancelled(S)`, or r already failed with this code | `Cancelled(funding_cancelled)` | no |
| 9 | anything else | `Unfunded` | no (yes while calling) |

Rows 7 and 8 match only open rows with no outstanding step. They hold from the moment the
evidence exists, so the gates are right before §5.2b writes the failure. A row whose voided
outpoint sits beside an outstanding step of its own is `Awaiting` (row 4): its own lock may still
come, and it takes it.

### 3.4 Lock owner

This covers a registration lock of account 0 that no row holds `live` and that isn't consumed.
Consumed signed value is §5.2a's.

| # | condition | owner |
|---|---|---|
| 1 | txid ∈ `dead(S)`, or ∈ `signed(S)` with S not outstanding | `Dead` |
| 2 | txid ∈ `signed(S)` | `Step(S)` |
| 3 | an unreadable row may own it (DEC-173 (2)) | `Parked` |
| 4 | an `Unknown` outstanding S with `own(S)` = ∅, `known_index(L)` ∈ {None, S's index}, and L's amount = S's `lock_duffs` (Q6) | `Contested` |
| 5 | anything else | `Free` |

## 4. Invariants

- **I1. Three-valued evidence.** A missing txid is never read as missing funding.
- **I2. Ownership outlives rows.** `owner` and `outstanding` read steps, not rows. Discard
  refuses every committed row.
- **I3. Unspent signed value goes only to an open `row_of(S)`, else to `Recovery`.** Every
  outpoint write passes `may_take` or §5.2, so a recorded outpoint is trusted by induction.
  Starting rows that bypassed it (F11) are judged by §5.2c.
- **I4. A reserved row stays put.** A row with an outstanding step never funds and never moves
  index, takes only its own `Step` locks, and is committed.
- **I5. Uncertainty quarantines.** No row takes a `Contested` lock, and Finish takes one only
  for `Recovery` when every step contesting it is wedged with a known release (§8.1, DEC-198).
- **I6. One funding per intent.** A row funds only from `Unfunded` at a free index. `start`
  refuses while a row is committed and not Done, or while an outstanding step has no row.
- **I7. Only evidence releases.** Only a `None` claim, consumption or a voided txid ends a hold.
  Failure codes, phases and lock statuses never do.
- **I8. One function.** Every path builds one `Ledger` in its actor job and asks only §5.
  **I9. Every hold has an exit (§8).** No DP1-02 hold waits on another DP1-02 hold.
- **I10. A running attempt owns its window (OD-05).** From `enter_funding` to the post-call
  writes, the calling row takes only its own `Step` lock.
- **I11. Terminal evidence reconciles, and never creates value.** §5.2 never creates an
  identity, funds or moves value between rows.
- **I12. A cancelled step is never called again.** Its row closes. No row funds at another
  draft's step index (§6 `index_owner`, `next_index`), so L-2's same-key retraction can't reopen
  a resolved step, and C4 reads past the unattributed one.

## 5. The decision functions

```rust
pub(crate) enum Standing { Unreadable { committed: bool }, Unfunded, Funded(OutPoint),
    Awaiting, Closed, Settled, Cancelled(&'static str) }
pub(crate) enum Owner { Step(i64), Parked, Contested, Dead, Free }
pub(crate) enum Taker { Row(i64), Recovery }
pub(crate) enum Reconcile {
    Attach { row: i64, lock: OutPoint, identity: String, index: u32 },  // §5.2a
    Close { row: i64, clear: bool, code: Option<&'static str> },        // §5.2b
}
impl Ledger {  // rows, locks, identities, steps (PA-1), calling
    pub(crate) fn read(..) -> Result<Self, PlatformError>; // one actor job; Unavailable → Err
    pub(crate) fn standing(&self, row: &RegistrationRow) -> Standing;      // §3.3
    pub(crate) fn owner(&self, lock: &LockInfo) -> Option<Owner>;          // §3.4
    pub(crate) fn may_take(&self, taker: Taker, lock: &LockInfo) -> bool;  // §5.1
    pub(crate) fn reconcile(&self) -> Vec<Reconcile>;                      // §5.2
    pub(crate) fn committed(&self, row: &RegistrationRow) -> bool;         // §3.3
    pub(crate) fn pending(&self) -> u32;                                   // §5.3
}
```

### 5.1 `may_take`: allocating unspent value

| owner | `Row(r)` | `Recovery` |
|---|---|---|
| `Step(S)` | r = `row_of(S)` and `open(r)`, calling or not | `row_of(S)` is none, or readable and not `open` |
| `Free` | `standing(r)` = `Unfunded`, `open(r)`, ¬`calling(r)` | yes |
| `Contested` | no | every step contesting it is wedged, with a known release (§8.1, DEC-198) |
| `Parked`, `Dead` | no | no |

**OD-05 is the `¬calling(r)` term.** `enter_funding` sets the flag in an actor job, and every
ledger is read and written in one. A Finish or tap job that runs first may give r a `Free`
lock, and then `enter_funding` sees the outpoint and returns `Again` without calling. A job that
runs after it sees r calling and gives r only its own `Step` lock, whether or not P2b has marked
yet. That closes the actor/J gap with no new lock order.

**It also removes rev1's contradiction.** A `Step` lock whose `row_of(S)` is open goes to that
row, calling or not, and never to `Recovery`. A write during the call meets `record_lock`'s CAS
as `Moved`.

**Eligibility only picks a candidate:** the tap tries the in-call row, else a row at the known
index; the run its own lock, else the index restore; Finish the row at the known index, else the
oldest waiting `Unfunded` row whose amount the lock covers, else `Recovery`.

### 5.2 `reconcile`: terminal evidence

Each `Reconcile` is one CAS under the row's owner, in an actor job. The run applies its own
row's at the top of each `KeysPrepared`, `FundingSent` or `ProofWaiting` step, and Finish
applies all of them before it allocates. Status reads only compute standing.

**(a) Consumed signed value (OD-03).** For t ∈ `signed(S)` where `lock(t)` is consumed and no
row holds it:
- **`row_of(S)` = r is open and not calling, and discovery reports `(i, id)` with
  `id = identity_for(lock(t))`:** `Attach` writes `{outpoint lock(t), identity id,
  identity_index i, phase IdentityRegistered}` and clears the failure. The identity's own index
  replaces the row's (its keys are there). The same write settles S, so I4 holds. The run goes
  on to names and the profile, with no IdentityCreate and no funding.
- **r is open, the identity undiscovered:** no write; r stays `Awaiting` (`Network`) and
  `pending` counts S.
- **`row_of(S)` is none or not open:** no write; S settles in place (§3.2), since the identity
  is already the wallet's.

This is the only consumed-value path: a consumed lock never reaches `Recovery`, a new row or
IdentityCreate, so Q1's recovery rows are for unspent value only, and a Done row the user
discarded never comes back (C1).

**(b) Clear and close (OD-04).** For a row r that isn't calling, `Close` makes one write:
- **`clear`** (r records a voided outpoint): it clears the outpoint and sets phase
  `KeysPrepared`. The match is exact, by txid in `dead`, so a live substitute T2 is never
  cleared because some other T1 was resolved.
- **`code`** (r is `Cancelled(code)` and its failure isn't `(code, false)` yet): it sets that
  non-retryable failure, re-arms `backed_up` and schedules the automatic backup (MN-2), so a
  later restore carries the closed row.

`Close` is idempotent; when both codes apply, `funded_elsewhere` wins. Once `Close` writes a
cancel code, D15 excludes that row from `row_of(S)`, so later evidence cannot make its step
reopen the row. The row takes no Free or Step lock. Consumed signed value settles in place;
returning unspent signed value goes to Recovery under §5.1. An Awaiting row whose voided outpoint
is cleared without a cancel code remains eligible for its own Step lock. A `Cancelled` row can
be discarded, not resumed; to register again the user
starts a new draft (P2b's Repair policy, I12).

**(c) The induction base for restored and legacy rows (OD-03).** A recorded outpoint is trusted
unless exact step evidence contradicts it; starting rows get no other rule. The txid decides:
- **In a `dead` set:** the outpoint is voided. (b) clears it, and §3.3 decides the rest.
- **In `signed(S)`:** the holder is `row_of(S)` and keeps the value (`Funded`, `Settled`, or
  (a)). An open `signer(S)` that isn't the holder matches row 7 (`funded_elsewhere`) and never
  funds that intent again. A signer that isn't open holds its own value and is unaffected. A
  restored copy holding its original's lock is the holder, so the heir rule binds no second row.
- **Signed by no step:** the record is the only claim, and it stands.

The holder keeps the value. Moving it to the signer needs a two-row write, and is impossible
once the holder has an identity. One lock still funds one identity, and nothing is lost (Q8).

### 5.3 `pending`

Finish's `still_pending` and the held path share one formula, on a fresh ledger after writes.

```text
pending = |{ S outstanding : no row holds a txid of own(S) live }|
        + |{ L : owner(L) ∈ {Parked, Contested, Free} }|
```

A `Step` lock counts once, through its step, and so does consumed value awaiting (a). `Dead`
locks and `Cancelled` rows aren't money. An `Unknown` step and a `Contested` lock that may be the
same money count twice; that is conservative, and the report says so.

### 5.4 Gates by standing

| standing | discard | in progress | resume (clear failure) | the run |
|---|---|---|---|---|
| `Unfunded` | allowed (refused while calling) | if live or retryable | if retryable or `broadcast_unknown` | `index_owner`, then fund |
| `Funded(o)` | refused | unless Done | yes | record o, then the identity |
| `Awaiting` | refused | yes | refused (it waits) | (b) clear, (a), own lock, else `maybe_sent` |
| `Closed` | refused | unless Done | as its failure | the identity step of what it holds |
| `Settled` | allowed | no | as its failure | identity, names, profile |
| `Cancelled(c)` | allowed | no | refused | (b), then stop |
| `Unreadable` | refused | if committed | refused | `Stop` |

"In progress" = (no failure and phase ≠ Done) or retryable or (committed and phase ≠ Done), plus
I6's wallet-level term. `funds_committed` keeps §16.5's display meaning (Q4): `Cancelled` reads
false, `Settled` and Done true. An `Unknown` hold reads `RegistrationWait::FundingUnknown`.

## 6. Every path mapped onto it

| path at 6bcaa2f | becomes |
|---|---|
| `Evidence::new(..).with_signed` (flow `:1014`; reg `:454`, `:526`, `:567`, `:723`, `:854`) | `Ledger::read`. `Unavailable` fails the decision closed: storage error, no write |
| `Driver::step`, for `KeysPrepared`/`FundingSent`/`ProofWaiting` | §5.2 for its own row first; a write → `Again` |
| `Driver::fund` before the call (`:2121-2159`) | `Cancelled` → (b). `Awaiting` → own lock → `record_lock`, else `maybe_sent`. `Closed` → `maybe_sent`. `Unfunded` → `index_owner`, then the call. P2b's permit is the backstop |
| `enter_funding` (`:2084`) | unchanged; it sets `calling` (OD-05) |
| `Driver::fund` after the call (`:2233-2287`) | still calling, on a fresh ledger: r's `live` outpoint is `lock(t)`, t ∈ `signed(S_r)` (the tap or Finish wrote it) → phase by `IsTimeout`, `Again`; else `own_lock` → `record_lock`; else claim `Exact`/`Unknown` → `maybe_sent`; else (claim `None`) by error: `Refused` → `failed`, `CoinsLocked`, `NotSent` → `unfund` (H12: P2b's `NotSent` row superseded the marker and nothing was signed, so the same step may retry), `Ok` → `maybe_sent`. An `Ok(o)` whose txid ∉ `signed(S_r)` is never recorded |
| `lock_of` (`:1090`) | the `live` outpoint's lock; else `own_lock`; else, for an `Unfunded` row only, the `Free` lock at its known index |
| `index_owner` (`:1214`) | a `Free` lock at the index → `Lock`. An identity at the index: its lock by `may_take(Row r)` → `Lock`, another's → `Taken`, `Contested`/`Parked` → `Unknown`. **Another draft's step at the index → `Unknown` while outstanding, else `Taken`.** Another row at the index that holds something, is committed or is older → `Taken`. An unindexed `Free`/`Contested`/`Parked` lock → `Unknown` |
| tap `note_lock_tracked` (`:1441`) | candidates at the index with no `live` outpoint, filtered by `may_take(Row r, L)`; the in-call row first, else the oldest |
| Finish `import_stranded` (`:1553`) | `reconcile()` writes first. Then each unheld lock by owner: `Step` → `row_of(S)` if `may_take(Row)`, else `Recovery` if `may_take(Recovery)`, else it stays; `Free` → §5.1; `Contested` → `Recovery` if `may_take(Recovery)` (§8.1), else it stays; others → none. The held path writes nothing. Returns `pending()` |
| `adopt_registration_lock` (appdb) | only after `may_take(Recovery, L)`, so unspent value only (a CAS writer, not an authority) |
| backup import (`import_wallet_rows`) | unchanged; §5.2c judges its rows |
| `funding_status`, `funds_committed`, `resumable` (`:1288-1359`); `in_progress`, `settled`, `discard_refused` (reg `:430-447`, `:811`) | `committed`, `standing`, §5.4 |
| `statuses`, `idle_wait` (reg `:669`, `:685`) | standing: `Awaiting` → `ChainLock`, (a) → `Network`, `Unknown` → `FundingUnknown` |
| `next_index` (`:1363`) | also skips the key index of every step in the view: standing, settled or cancelled |
| `assign_index`, `create_identity`, the proof wait | unchanged: only `Unfunded` rows move, and only rows with a `live` outpoint create identities |

## 7. What gets deleted

`Provenance` (now `Ledger`); `Evidence::orphaned`, `destined_lock` and `awaiting_a_lock`'s
`broadcast_unknown` arm; Finish's `calling` and `deferred` and `Flows::calling` (now the per-row
`calling` in `may_take`); the `reserved` and `orphaned` counters and the held path's own return
(now `pending()`); `settled(row)` and its exemptions; `funding_status`'s ad-hoc branches and the
`(funded, tracked)` match. DP1-02 adds no schema; the new failure codes
`registration.funding_cancelled` and `registration.funded_elsewhere` bump m4's contract version.

## 8. Liveness: every hold has an exit

| hold | exit |
|---|---|
| `Funded(o)`, tracked | the run: proof, identity, `Settled` |
| `Funded(o)`, untracked, `Exact` | reconstruction, or Repair → `NotSent` → o voided → (b) |
| `Awaiting`, `Exact`, lock present / consumed | the run, the tap or Finish attaches it / (a), once discovery reports the identity |
| `Closed` or no row, `Exact`, lock present / consumed | Finish → `Recovery` → identity / settles in place |
| `Awaiting`/`Closed`, `Exact`, lock away | it returns, or Repair → `NotSent` (P2b residual: `LockInputsSpent`) |
| `Awaiting`, `Unknown`, `durable_record` | in a call: it ends `Exact` or `NotSent`. After it, with no release row: `None` and the row closes (C5); with one: none, the step is wedged, and the user may start past it (§8.1, DEC-197; no adoption, DEC-198) |
| `Awaiting`, `Unknown`, unflagged marker | none (§8.1) |
| `Contested` | freed when no `Unknown` step stands (C4 removes the persistent one); contested only by wedged steps, Finish → `Recovery` (§8.1, DEC-198) |
| `Parked`; a `Step(S)` lock whose `row_of(S)` is unreadable | a build that reads the row (DEC-173) |
| orphan step | its holder or heir, as above, else `Recovery` or settling in place |
| `Cancelled` | none needed: not committed, and discardable; a new draft registers again |

**No cycle.** DP1-02's holds wait only on P2b's derivation, the chain, discovery and Repair.
P2b's own waits (`UnattributedLock`, the markerless gate) wait on attribution. A `Free` lock
reaches attribution through Finish → `Recovery` → identity. The only link through DP1-02 is
`Contested` → `Unknown`, which ends in P2b under OD-01/02; C4 cuts the one link back through
attribution. So, apart from §8.1, every hold ends through Finish, Repair, a proof or discovery,
with no further user decision. Assumptions remain: Parked's compatible build, working storage
and network, P2b's `LockInputsSpent`, and discovery within its gap (Q12).

### 8.1 OD-02's liveness consequence

**Flagged markers: acceptable.** After a crash mid-call, a flagged marker with nothing signed
reads `NotSent` but stands, and F7 vetoes another call of its step. Its row closes as
`Cancelled(funding_cancelled)`: nothing was paid, discard and `start` are allowed, and a new
draft registers. It is never retried on the same step (Q14).

**Unflagged markers: acceptable only under the initialization contract.** One with no signed
record reads `Unknown` for good: its row is `Awaiting` (discard and `start` refused, `Contested`
locks frozen), with no user-visible exit. Only a developer's journal can hold one (desktop main
is at schema 2; schemas 3–5 exist only on the unmerged P2b branch), and every marker migrated to
schema 6 is unflagged. It fails closed (no double funding); `FundingUnknown` names the step, the
log flags it, and a fresh profile is the developer's way out. The release must ship P2b and
DP1-02 together at schema ≥ 6 (Q11); a release at 3–5 makes this a product wedge needing a user
Release (rev1's rejected fallback). No cheaper sound exit exists: a missing record proves nothing
for an unflagged writer, and F4 leaves Repair nothing to conflict with.

**Released markers with no recorded lock (P2b r6).** A flagged marker whose call released a
funding signature and recorded no lock reads `MaybeSent` for good (E0-04 §2a.7): the vault
locked mid-build, the library's build failing after signing, or the wallet leaving the
library's manager between the signing and the store. Repair refuses it (`NoKnownLock`). Its row
is `Awaiting` like an unflagged one's, so the same wedge follows (discard and `start` refused, a
lock at its index `Contested`), and here a shipped build can reach it. DEC-197 (Q15) gives it
one exit, and DEC-198 makes it the only one: P2b doesn't adopt the step's lock from chain
evidence. At the pin a registration's credit key is the next pool address, chosen after signing
and persisted best effort, so an exact key match can't tell two steps apart (BL-108 tracks the
pin change).

**A start past the step (DP1-02).** `start_registration` still refuses.
`start_registration_past_unknown_funding(req, grant, past)` starts a new draft once the user has
confirmed each blocker in `past`, named as `registration.in_progress` names it. A named row
passes only if it is `Awaiting` and every step it awaits is wedged: `Unknown`, flagged, with
nothing voided (a tombstone means Repair resolved it), and no call and no record write under
it. A named orphan step passes only if it is wedged. Two more
conditions must also hold. SPV must be synced; otherwise the start returns
`registration.not_synced`. Each passed step's release row must be at least an hour old;
otherwise it returns `registration.funding_unknown_recent` with the seconds left, or with none
while the release time is unknown. Anything else still blocks with `registration.in_progress`.
The new draft takes a fresh index (§6 skips every step's index) and gets its own exactly
attributed lock. The old row stays committed, `Awaiting`, and can't be discarded.

The release time comes from P2b's `funding_release.at`. Until P2b r7 exposes it, the seam
(`registration_ledger::released_at`) returns none, so the override fails closed.

**Contested locks of a wedged step (DP1-02 decision, for ruling).** A wedged step contests every
unattributed lock at its index, or of unknown index, of its amount, and with no adoption nothing
ever attributes one to it. So a lock that only wedged steps contest, each with a known release,
goes to `Recovery`, never to a row: `may_take(Recovery, L)` holds, `may_take(Row, L)` doesn't,
and Finish imports it into a recovery row. While a contester's release is unknown (until P2b r7,
always) the lock stays `Contested` and pending (fail closed). That is how the earlier attempt's
lock, if it confirms, comes back: a lock the wallet recovers through Finish, never attached to
the old row or to the new one. A step that isn't wedged (unflagged, in a call, under a record
write, or with a tombstone) still contests in full. This carves `Recovery` out of I3 for a
wedged step's own unrecorded lock, and out of S4 for a lock only wedged steps contest: DEC-198
sends that value to `Recovery`, and the model judges each such take against ground truth (the
lock's true signer must be wedged). Without this the model check's liveness failed: a rebuilt
lock of unknown index stayed `Contested` for good, and the new row's index read `Unknown` (it
would wait for a ChainLock that never settles the contest). Residual: until the user runs
Finish, a new row that is open and unfunded when such a lock shows up reads its index `Unknown`
and waits with `RegistrationWait::ChainLock`, which only Finish resolves; after Finish it funds at
its own index. A wait reason naming Finish would be an m4 change (proposed, not made).

**Ask of P2b r7** (DEC-197's seam). `released_at` must be some only for a released step, and it
must be the release of the step the view shows (its `step_id`, so not an earlier generation's
after a `NotSent` re-mark), read in the same J step as `registration_funding_steps` or never
going from none to some for a step the view still shows as unreleased. The view shows no release,
and an unreleased flagged step can read `MaybeSent` while the wallet's rows are unknown.

Passing an unflagged step is refused by decision, not by a safety rule. Its marker predates
PA-2, so it has no release row, and §8.1 leaves it to the developer's fresh
profile.

**The wedge's copy.** The UI must show this text, and the user must confirm it, before it calls
the override:

> An earlier registration's payment could not be confirmed, and it may still appear on the
> network. You can start a new registration now. The earlier payment may still confirm. If it
> does, its funds show up as a transfer this wallet can recover later with Tools ▸ Repair ▸
> Finish transfers, as a separate identity, so you may have paid for two registrations. The
> earlier registration can't be discarded.

While the hour runs (`funding_unknown_recent`): "The earlier registration's payment was sent
less than an hour ago. Try again in {minutes} minutes." With no `wait_secs` (the release time is
unknown): "This wallet can't yet tell when the earlier registration's payment was sent. Try again
later." While SPV syncs (`not_synced`): "Wait until the wallet has finished syncing, then try
again."

## 9. P2b asks

- **PA-1 (approved; extended per OD-06).** `registration_funding_steps(wallet) ->
  Result<Vec<StepView>, Unavailable>` is one J step with §3.1's shape. It lists
  `funding_steps_of` plus every resolved step that kept tombstones, returns `dead` apart from
  `signed`, and `durable_record` (a resolved step's from its stashed marker, for C4). A closed or
  unreadable journal returns `Unavailable`.
- **PA-2 (the OD-01 ruling).** A durable-record gate guards every broadcast path P2b controls:
  the builder's send, direct and deferred resume, and the startup and reconnect resumers. P2b
  proves that no library path sends a `Built` funding lock outside them. DP1-02's `PwLib::fund`
  uses only `create_funded_asset_lock_proof`, never the build-only APIs.
- **PA-3 + OD-06.** Tombstones survive reopen and compaction, and are never resurrected. **New:**
  they also survive `erase_wallet` (§5.2c); otherwise a restored row recording a voided lock reads
  `Funded` for ever.
- **OD-02.** Each marker carries `durable_record` from creation (P2b's WIP schema-6 column). P2b
  documents §8.1's contract.
- **New: `StepCancelled`.** `funding_call_permit` refuses a step with tombstones. DP1-02 never
  requests such a call (I12), but C4's reading rests on it, so it is required, not optional.
  E0-04's H12 ("back to retryable only on `NotSent`") gains: not a step with tombstones or a
  standing marker (C5).
- **Optional.** A Repair `NotSent` signals the step's flow, so (b) runs promptly; otherwise the
  next run, resume or Finish does, and the gates are right meanwhile (§3.3 row 8).

## 10. Closure table

| finding | closed by |
|---|---|
| R1-02, R1-08, R2-01, R1-06, R1-07, R2-04, R3-01, R4-01, R6-02, R6-04, MN-3, MN-4, R5 N1 | kept: mechanisms that aren't evidence (discard guards, `interpret`, P2b's fence, DEC-179, error classes); §11.3 adds MN sites, §11.2 item 14 keeps N1 |
| R1-01 synthetic index lets a restore fund twice | `known_index` in `owner`/`index_owner`; I6; I12 |
| R1-03 KeysPrepared row with its own outpoint | `Funded(o)` → the run (DEC-171 (4)) |
| R1-04 unknown encoding erases evidence | `Unreadable{committed}`; I2 |
| R1-05 index identity adopted before the check | `index_owner`'s identity branch asks `may_take`; I3 |
| R2-02 Finish takes an unreadable row's lock | `Parked`; I3 |
| R2-03 stale evidence write | kept (CAS); every ledger is read in its writer's actor job (I8) |
| Opus MJ-1 failure after the identity wedges | `Settled`: not committed, not in progress |
| MN-1 untapped index-0 lock and a non-Ok call | `Step` by txid; post-call `own_lock`; after a restart, OD-01/02 |
| MN-2 backup re-arm | kept for `unfund`; (b) re-arms too |
| R6-01 own proof races attribution | ownership by txid, whatever the status |
| R6-03 call-window heuristic | deleted: `Contested` after marking, I10 before it |
| R7-01 Finish substitutes T2, orphans T1 | `Awaiting` takes no `Free` lock (I4); `Closed`'s own lock → `Recovery` |
| R7-02 identity fallback steals A's consumed T1 | `index_owner` refuses B (`may_take`); (a) gives A T1 and its identity |
| R8-01 lost record: discard, replace, substitute | `Unknown` (I1) → `Awaiting`; I5, I6; exits OD-01/02 and §8.1 |
| R8-02 settled-row discard frees T1 | read by step (I2); A `Closed`, T1 → `Recovery`; a consumed T1 settles in place |
| R8-03 held path drops the count | one `pending()` (I8) |
| OD-01 a parallel resume sends before the record | PA-2 = the shared durable-record gate (§9, P2b) |
| OD-02 markers written before PA-2 | per-marker `durable_record`; §8.1's contract |
| OD-03 consumed signed lock with no holder | §5.2a; induction base §5.2c; row 7 |
| OD-04 a voided outpoint keeps its row funded | `live`, row 8, §5.2b; I12 |
| OD-05 `Free` substitution before the marker | `¬calling` in `may_take` (I10); §6's Finish follows §5.1 |
| OD-06 PA-1 can't see tombstones | PA-1 lists resolved steps' `dead`; PA-3 retention (§9) |
| C1 (rev1 check) a discarded Done row's step is outstanding for ever | `settled` in place once `row_of(S)` is none |
| C2 (rev1 check) a row holds a lock that can't land | `live`, row 8, §5.2b |
| C3 (rev1 check) a non-heir copy funds on an orphan's slot | `index_owner`'s step arm; `next_index`. By F10 a real copy at the index is the heir |
| C4 (rev2, P2b WIP) an unattributed row retracts a resolved step; `Contested` freezes the row for ever | §3.1's tombstone claim; `StepCancelled` required |
| C5 (rev2, P2b WIP) a flagged recordless marker reads `NotSent` but stays vetoed | `cancelled` covers it: the row closes (§8.1); Q14. P2b r6: only with no release row; a released one reads `MaybeSent` (Q15) |
| Closure rebound (rev2 check, Sol design r2) a coded row reads `Awaiting`/`Funded` again after a restart or a holder's discard | D15: `row_of` excludes cancel-coded rows; returning value goes to `Recovery` or settles in place |

## 11. Test plan

### 11.1 Model check

`docs/design/checks/dp1_02_ownership_model.py` (`python3 -I -B`, exit 0 = pass; a scenario
past its 600,000-state guard prints `BOUND HIT` and exits 2) checks rev2 with D15. It explores
rows A, B and C, restored copies, recovery rows, steps, locks T1–T3, calls, restarts and
restores exhaustively, with flagged and unflagged markers. It covers `calling` in `may_take` and
Finish per §5.1; `live`, `settled`'s in-place arm, `reconcile` (attach, clear, close) and
`index_owner`'s step arm; C4's claim and C5's stale marker; D15's `row_of` exclusion; and events
for the pre-marker window, a consumed signed lock no row holds, an unattributed row after a
resolution and a wrong-holder backup. DEC-197/198 add three things:
- a signing path where the lock leaves unstored and the process stops;
- the override start, past wedged steps only;
- no adoption (DEC-198): a lock only wedged steps contest goes to a Finish recovery row.

rev2 holds S1–S6, I3, I4, I6, I10–I12 and D15 with no expected violation. Every stuck flagged
state reaches, by system events alone, DEC-197's rest: nothing outstanding but wedged steps
(any lock of theirs that showed up recovered), with the override open, the steps' only exit.
Every other stuck state is §8.1's unflagged one. Witnesses: the override is taken; A's lock,
sent unstored, shows up and Finish recovers it while A stays `Awaiting`; after the override C
funds with its own lock while a recovery row holds A's; and after the override A stays
`Awaiting`, not discardable. Each mutation must fail: a calling row takes `Free`; `Recovery` takes an open
caller's `Step` lock; no attach or no closure; the closure clears an outpoint that isn't `dead`;
the signer takes contrary value; `next_index` ignores resolved steps; C4 reads `Unknown`; a
cancel-coded row is again `row_of` (D15); rev1's rejected rules; and DEC-198's: a row takes a lock
only wedged steps contest, and `Recovery` takes any `Contested` lock. Run it under
`systemd-run -p MemoryMax=4G -p RuntimeMaxSec=1500 -p Nice=10`.

### 11.2 Rust regressions

These run in `registration_tests` with the real `LeaseTable`, dispatch and appdb SQLite, and the
Fake library, which counts `fund` and `register_identity` calls.

1. **R8-01a/b** (rev1 items 1–2). Add schema-7 flagged-marker variants. With no release row,
   after a reopen it reads `NotSent`, nothing is funded, and A closes as
   `Cancelled(funding_cancelled)` (C5, Q14). With one, it reads `MaybeSent`: A is `Awaiting`
   (`FundingUnknown`), discard and `start` are refused, nothing is funded (P2b r6, Q15). DEC-197:
   the override is refused within an hour of the release, and while SPV is behind. After both,
   it starts C at a fresh index and C completes. A stays committed, can't be discarded, and
   contests a lock at its index. An unflagged step and a row holding its lock still block, and a
   malformed name is refused. DEC-198: A's lock showing up after the override, rebuilt with its
   index unknown, while C is open and unfunded, is `Contested`; C may not take it and its index
   reads `Unknown`; Finish recovers it into a recovery row, never to A or C; then C funds at its
   own index and completes, and A stays waiting and committed. The override passes a wedged
   orphan step named `registration/<n>/funding`.
2. **OD-02.** Open the r8 probe's real pre-6 dispatch DB with the new build. A is `Awaiting`
   (`FundingUnknown`), discard and `start` are refused, nothing is funded, and T2 stays unheld.
3. **OD-01** (P2b's test, a dependency): Sol's blocked-record resume race, zero broadcasts.
4. **OD-03, Sol's r7 fixture.** A is open, S_A's durable T1 is `Consumed`, and
   `identity_for(T1)` is discovered at index j ≠ A's index. A gets `{T1, identity, j, IdentityRegistered}`
   and reaches Done; `register_identity` and `fund` run 0 times; B is refused T1; `pending` goes
   1 → 0 and `start` is allowed after. Variant: undiscovered, A stays `Awaiting` (`Network`,
   `pending` = 1) until discovery reports it.
5. **OD-03, restoring a wrong holder.** A backup has B′ holding T1 ∈ S_A, and A′ open at S_A's
   index. A′ reads `Cancelled(funded_elsewhere)`, never funds, and can be discarded. B′ keeps T1.
6. **OD-04.** Run a successful tap, then lose tracking. P2b's real recorded-input self-spend is
   ChainLocked and the step goes `NotSent`. Then resume, Finish and discard. A is
   `Cancelled(funding_cancelled)` with its outpoint cleared and `backed_up` re-armed; it isn't
   committed; discard and `start` are allowed; S_A is never called again. Controls: a live
   legacy substitute T2 stays `Funded(T2)`; an `Awaiting` A with a voided T2 is cleared and still
   takes its own T1; a pre-closure backup row holding T1, restored after a wallet removal,
   closes the same way.
7. **OD-05.** Pause between `enter_funding` and P2b's marking, and between Finish's ledger read
   and its write; add a `Free` T2; run Finish and the tap. A never takes T2, which goes to
   `Recovery`, and A's own T1 goes to A. Controls: marked-`Unknown` and marked-`Exact` running
   calls; Finish never gives an open caller's T1 to `Recovery`.
8. **OD-06.** Supersede, restart, Finish. The tracked `Built` T1 is `Dead`, and no recovery row
   appears. A call of S_A is refused (`StepCancelled`); P2b's test shows a later attempt never
   resurrects a tombstone.
9. **C1.** Discard a Done row whose T1 is consumed. `start` is allowed, and `pending` = 0.
10. **I12.** A row at a cancelled step's index moves to a fresh index before it funds.
11. **C4.** After (6), reconstruct an unspent registration lock U with a foreign txid at index 0
    (unattributed). P2b reads S_A `MaybeSent`, DP1-02 `None`. U goes to `Recovery`, its identity
    attributes it, and P2b's block lifts; A stays `Cancelled`. Control: unflagged, S_A reads
    `Unknown` and U is `Contested`.
12. **C5.** Kill the process after A's marker, before signing, and restart. A is
    `Cancelled(funding_cancelled)`, not committed; `start` is allowed; S_A is never called again,
    not even by the same step (Q14).
13. **R8-02, R8-03, restore orphan, `Ok` mismatch** (rev1 items 4–7). An unconsumed T1 goes to
    `Recovery`, and Sol r8's Done-row control follows Q1.
14. **Every earlier regression**, under the new names, plus Sol r8's five controls and R5 N1.

### 11.3 Mutations and traps

A named test must kill each mutation site: `outstanding`'s `Unknown` arm, the heir rule,
`row_of`'s holder-first rule and `live`; `may_take`'s `Step`, `Recovery` and `¬calling` arms;
`owner`'s `Contested` and `dead` rows; `settled`'s in-place arm; (a)'s discovery requirement and
(b)'s exact `dead` match; `pending`'s step term and `committed` for `Closed`; the post-call `Ok`
routing; the step arms of `next_index` and `index_owner`; discard's `committed` check. Sol r8's
KNOWN_INDEX, RUN_RESERVATION and ORPHAN_IMPORT stay; ORPHAN_IMPORT now expects `Recovery`.
DEC-197 adds the override's gates (wedged needs a flagged marker, the row must be named, every
step it awaits wedged, SPV synced, the release an hour old) and `may_take`'s wedge-only `Recovery` arm (DEC-198).

Traps: sign through the Fake's `fund` (a record needs a running call); the Fake's txid is
`[index+1; 32]`; P2b's 5 s lease tests flake at load ~200; run the check with `-I -B`.

## 12. Open questions for the manager

- **Q8. The holder keeps contrary value (§5.2c)** rather than moving it to the signer? A move
  needs a two-row write and fails once the holder has an identity. The closed intent
  re-registers through a new draft and payment; the earlier value stays in the holder's identity.
- **Q9. Route PA-3 retention across `erase_wallet` to P2b?** Without it, restoring a backup
  taken before a closure gives a `Funded` row with no end.
- **Q10. Route `StepCancelled` to P2b as required (C4)?**
- **Q11. Make §8.1's release order a gate?** P2b and DP1-02 ship together at schema ≥ 6
  (≥ 7 since P2b r6, DEC-191).
  Otherwise, rule on a Release for schema 3–5 markers.
- **Q12. Attach waits for discovery (§5.2a).** `find_identity` gives the id, not the key index.
  The residual is an identity outside discovery's gap, which DP1-05 can't see either. Accept?
- **Q13. C4's reading.** Should P2b derive it too (a flagged resolved step reads `NotSent` over
  unattributed and re-keyed rows), so its gate, events and DP1-02 agree? DP1-02 is sound either
  way, given `StepCancelled`.
- **Q14. C5: close, or retry the same step?** Today the row closes and the user starts a new
  draft (nothing was paid). Alternatively P2b's permit supersedes a flagged marker with nothing
  signed and no running call before marking anew; then `cancelled` narrows to `dead` ≠ ∅.
- **Q15 (ruled: DEC-197, DEC-198, §8.1, §13.1). A released step with no recorded lock (P2b r6).** It reads `MaybeSent` for good and
  wedges the wallet's registrations (§8.1). Either keep the wedge (fail closed: no second lock
  while one may be out), or let `start` pass such a step: its row stays `Awaiting`, committed and
  not discardable, a lock at its index stays `Contested`, and the new draft takes a fresh index
  with its own exactly attributed lock, at the cost of possibly paying twice.

## 13. Manager rulings

**On rev0 (2026-10-11):** **PA-1, PA-2, PA-3** approved; P2b writes all three. PA-2 lets
"marker, no signed record, no running call" read `NotSent` only because no lock reaches the wire
without a durable record; P2b's review checks that directly. No user-Release fallback. **Q1**
approved: a signed lock its signer can never take goes to a recovery row through Finish (rev2:
unspent value only, §5.2a). **Q4:** `funds_committed` keeps §16.5's display meaning, the gate
has its own predicate, a `Settled` or Done row may be discarded, and
`RegistrationWait::FundingUnknown` is added. **Q5:** a heir only on an exact key match (by F10,
the same index). **Q6:** narrow `Contested` by amount, yes; trusted-index assignment, no.
**Q7:** `registrations()` reports an orphan step read-only, and `InProgress` names it. **F6:**
verified (§3.1).

**On Sol's design review (2026-10-11):** **OD-01:** PA-2 becomes a durable-record gate before
every broadcast path P2b controls (§9). **OD-02:** schema 6 flags each marker
`durable_record`; only flagged markers read `NotSent` from a missing record; no shipped build
wrote markers. **OD-06:** PA-1 enumerates resolved tombstones, which survive reopen; a later
attempt doesn't resurrect them.

### 13.1 Manager rulings on rev2 (2026-10-11)

- **Q8: approved.** The row holding a contended lock keeps the value. The signer that doesn't hold it closes as `Cancelled(funded_elsewhere)` (§5.2c).
- **Q9, Q10: routed to P2b as required.** Tombstones survive `erase_wallet`. The funding permit refuses any step with tombstones (`StepCancelled`), and E0-04's H12 gets the matching exception.
- **Q11: a release gate.** P2b and DP1-02 ship in the same release at dispatch schema ≥ 6 (≥ 7 since P2b r6, DEC-191). No release ships one without the other. This is recorded in the program's decision log.
- **Q12: accepted as a residual.** An identity outside discovery's gap leaves the row waiting with `RegistrationWait::Network`. The UI explains it, and widening discovery is a BACKLOG item.
- **Q13: P2b derives C4's reading.** One source of truth. P2b's status for a flagged step with tombstones and no signed lock is resolved, and DP1-02 reads it rather than overriding P2b. P2b's gate and events agree with it by construction.
- **Q14: close the row** (`Cancelled(funding_cancelled)`), as rev2 has it. P2b doesn't replace stale markers.
- **Before implementation:** update the model check (`docs/design/checks/dp1_02_ownership_model.py`) to rev2 and run it with the §11.1 mutations, bounded so it finishes. Then a narrow Sol design r2.
- **D15: approved (Sol design r2, APPROVE-WITH-CHANGES).** `row_of(S)` picks `holder(S)`, else `signer(S)`, and returns none if that row is closed with `registration.funded_elsewhere` or `registration.funding_cancelled`; it never falls through to another candidate, and no step or lock evidence is dropped. A closed row takes no lock: consumed signed value settles in place, returning unspent signed value goes to Recovery (§5.1).
- **Q15 → DEC-197: neither pure option; the wedge must be resolvable.** (Its adoption half is superseded by DEC-198.) P2b adopts a released, recordless step's lock from chain evidence for the life of the step. DP1-02 adds an explicit override, `start_registration_past_unknown_funding` (m4 Contract-Version 14). It requires SPV synced, the release at least an hour old and the user's confirmed copy (§8.1). The old row stays committed. The model gains the unstored-send path and the override. Every flagged scenario ends with nothing outstanding, or with a wedged step whose only exit is the override.
- **DEC-198: P2b's adoption by funding key is deferred.** At the pin a registration's credit key is the next pool address, chosen after signing and persisted best effort, so an exact key match can't tell two steps apart; BL-108 tracks the pin change. The DEC-197 override is the wedge's only exit. The copy promises no adoption: the earlier attempt may still confirm, and its funds then show up as a lock the wallet recovers later. DP1-02's consequence, for ruling (§8.1): a lock only wedged steps contest goes to `Recovery` through Finish, never to a row.
