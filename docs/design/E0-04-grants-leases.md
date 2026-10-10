# E0-04: grants and leases — design

**Status:** design (D), **rev2** with amendment 1 (E0-08 alignment) and amendment 2 (closure round 3), for the
manager's final closure check (DEC-57).
- rev0 (6b4cdf9) was reviewed in r1: GPT REWORK, Opus APPROVE-WITH-CHANGES.
- rev1 (d2a21c6) was reviewed in r2: GPT REWORK, Opus APPROVE-WITH-CHANGES.
- rev2 (70cbeee) with amendment 1 (692fd20) was reviewed in r3: Opus APPROVE-WITH-NITS, GPT REWORK on one E0-04
  major (F2).
- The manager's rulings and every finding's disposition are in "Amendment 2", "Amendment 1", "Changes in rev2"
  and "Changes in rev1" below.
- DEC-65 accepted rev0's Q2–Q20. DEC-67 accepted Q21, rejected Q22, and accepted Mode B with its promise in user
  terms.
- Q1 (the platform PR) is still pasta's call (B5), so the design specifies both outcomes (§2a).

Once approved, it replaces the **DRAFT** E0-04 text in [`DASHPAY.md`](DASHPAY.md) §2.6 ("Commit points", "At lock
time" and the open-issues list) and the draft clauses of the E0-04 row in [`ROADMAP.md`](ROADMAP.md).

**Inputs:**
- DASHPAY §2.6 and §3.3–3.4;
- the design-input checks `docs/design/checks/e0_04_{dispatch_model,split_model,mutations}.py`;
- E0-03's merged vault code (`rust/crates/dw-vault`, `rust/crates/dw-engine/src/platform/`);
- reviews DW-E0-03 r3 (Opus) and r4 (GPT), and the fix4 findings listed in DASHPAY §2.6;
- reviews DW-E0-04-design r1 and r2 (GPT and Opus, `~/workspace/node-program/reviews/DW-E0-04-design-r{1,2}-*.md`),
  r3 (`DW-E0-04-design-r3-gpt.md`, `DW-E0-04-E0-08-r3-opus.md`), with the reviewers' scratch probes;
- the m4 DashPay contract on `dw/e0-08-dashpay-facade` at cf9ba8e, c3c9bbd and 369f8f0 (Contract-Version 3), and
  its closure check DW-E0-08 r2 (Opus). **Names and shapes follow the contract; the safety semantics are this
  design's** (the manager's correction to rev2 ruling 4). §16 states both, and E0-08 matches its semantics (§15);
- DEC-18, DEC-57, DEC-65, DEC-67 and DECISIONS-PENDING B5.

**Pins read:**
- platform `bc41f1bc23`. `PW` = `packages/rs-platform-wallet/src`, `PWS` = `packages/rs-platform-wallet-storage/src`,
  `SDK` = `packages/rs-sdk/src`.
- rust-dashcore `40268cc0` (`dash-spv`, `key-wallet`).

**Spec check:** `python3 -I docs/design/checks/e0_04_design_model.py` (exit 0 = pass; about 80 s; 86 checks).

## Amendment 2: closure round 3

**Reviews of 692fd20.**
- Opus r3 (`DW-E0-04-E0-08-r3-opus.md`): APPROVE-WITH-NITS. All its r2 majors are closed, and it accepts all three
  of amendment 1's extensions.
- GPT r3 (`DW-E0-04-design-r3-gpt.md`): REWORK. Six of its seven r2 items are closed, and one E0-04 major is left
  (F2).
- GPT F1, and Opus C-1, C-2, C-4, C-5, C-7 and G-1, are E0-08's. E0-08 adopts §16.5 and §16.6 word for word, so the
  fence does not move toward m4.

**Manager rulings for amendment 2** (2026-10-08), each applied:
1. GPT F2: the auto-lock copy uses DEC-67's promise while a call runs, then the observed outcome, consistent with
   C2 and C3. The model checks C5 on GPT's trace.
2. Opus F-1: H16's nonce evidence is exact.
3. Opus F-2: Mode B matches tracked rows by the draft's own funding key as well as by the marker's.
4. Opus F-3: `FlowKind::Discovery` is dropped. The identity scan takes a grant, not a lease, as in E0-08.
5. Nit C-6: cite Contract-Version 3, which becomes 4 after E0-08's next round.
6. §16.5 and §16.6 stay as they are unless F-2 forces a change, which is then stated exactly (below).

| Item | Disposition | Where | Model |
|---|---|---|---|
| GPT F2: C5 promises "unlock to finish" whenever something is committed | **Fixed.** C4 and C5 share one ordered rule, re-applied on every change of the flow's calls and outcomes; C5 is "Locked automatically" followed by C4's line. While a library call of the flow runs (Mode B), or a committed First of it is in flight (Mode A), the line is DEC-67's promise. Afterwards it follows the observed outcome: "cancelled" with nothing committed; "may have been sent" or "will be sent" while an artifact is pending, with "unlock to finish" added when the flow is parked; a plain "unlock to finish" only when the flow is parked with nothing pending; otherwise "sent". | §4.4, §4.6, §4.8, §16.10 | `step2_findings` with C5, rev2's classifier kept: rev2's C5 promises a stop on `sign S1 → Lock → submit S1 → Platform executes S1`; a rule that puts "unlock to finish" first fails when S1 executes late after a back-off; amendment 2 passes |
| Opus F-1: H16's nonce rule ignores Platform's 24-slot window | **Fixed.** `NotSent` needs both proofs: that h's own nonce slot is used by a different transition (one this engine signed in the same nonce space with the same nonce, whose proved execution result it holds), and that h did not execute, which follows because a slot admits one transition. A fetched nonce past n is no evidence: n still executes while its slot is marked missing, up to 24 below the current value (`rs-dpp/src/identity/identity_nonce.rs:17`, `:99-176` at `bc41f1bc23`; the identity-contract nonce merge in rs-drive uses the same window). A closed slot alone is no evidence either, since h may have closed it. Otherwise h stays `MaybeSent`. Both nonce spaces are named. | H16, §2a.5, §4.6, §16.6 | `nonce_notsent`: "the nonce advanced" answers NotSent while h can still execute in its missing slot, and "the slot is closed" answers NotSent after h itself executed |
| Opus F-2: a Mode B row with no marker is not matched to its flow | **Fixed.** Rows match a draft by the draft's own funding key (funding type, identity index and account, kept in `app.sqlite`) as well as by the marker's. A matching row with no marker reads by its status, and a row matching several drafts counts for each (fail closed). | §2a.5, §16.5, §16.6 | `mode_b_absent`: matching only by the marker's key funds twice after a deleted or rolled-back journal |
| Opus F-3: `FlowKind::Discovery` | **Dropped** (manager ruling), back to rev2's text. `discover_identities` takes only an `IdentityScan` grant id; a lease id there is `platform.grant_invalid`. | §4.1, §16.1 | — |
| Opus n-1, C-6: the contract version cited | **Fixed.** §16 cites 369f8f0, Contract-Version 3, which becomes 4 when E0-08 adopts §16.5 and §16.6. | Status, §16 | — |
| Opus n-2: §4.6's `dispatch_status` lacks the `Result` | **Fixed.** | §4.6 | — |
| Opus n-3: `restart_retry` hard-codes `None` | **Noted** in §11: it checks how the host reads the answer after a restart, not how the answer is derived. | §11 | — |

**What changed in §16.5 and §16.6.** E0-08 copies both word for word. F-1 and F-2 force three changes, and the
`None` semantics are unchanged: the host reads every `None` as unknown, and only the engine's funding gates read an
asset lock's `None` as never registered.
- **§16.5, Mode B** (F-2): "a funding marker of the flow exists" becomes "a funding marker of the flow exists, or a
  tracked row matches the draft's own funding key". Without it, a journal lost under a `Broadcast` row would read
  `false` and show "Lock to cancel" while the lock is out. Mode A already counts a tracked row with no entry.
- **§16.6, the funding gate** (F-2): "no entry, no marker and no tracked row" becomes "no entry, no marker and no
  tracked row matching the step (in Mode A by its txid, in Mode B by the draft's own funding key or the marker's)".
- **§16.6, the table's transition `NotSent` row** (F-1): "its nonce consumed by another transition (H16)" becomes
  "a different transition this engine signed is proved executed in its nonce slot (H16)".

## Amendment 1: E0-08 alignment

**Manager correction to rev2 ruling 4** (2026-10-08, after DW-E0-08 r2 Opus):
- **Names and shapes follow E0-08's implemented contract (c3c9bbd)**, not rev2's §16: `will_be_sent`, `GrantAction`
  with an `identity` argument, `DispatchState`'s four values (`WillBeSent`, `MaybeSent`, `Sent`, `NotSent`), rows
  moving to `Unlock` or `Authorize` with no `needs_grant` field, and `DispatchResolved.resolution`.
- **The safety semantics stay this design's:**
  - retry and discard never proceed on a missing journal entry, except for asset locks. A state transition or a
    `TxDraft` send with no entry is unknown, not "not sent";
  - a transition known to be unsent keeps a tombstone, so `dispatch_status` can still answer `NotSent` after the
    fact.
- Also: define `Committed{will_resend: false}` or remove it; map `lease_revoked` and `lease_expired`; add
  `WalletClosed` to the causes; align `funds_committed` with N-3, stated precisely in §16.5.

| Item | Disposition | Where | Model |
|---|---|---|---|
| Names and shapes | **Applied.** §16 restates c3c9bbd's surface: `dispatch_status(artifact) -> Result<Option<DispatchState>, PlatformError>` with the four values, and rev2's `Unknown` is gone. `FlowKind` is c3c9bbd's closed list, `Discovery` included: a lease that holds only `IdentityScan` and admits nothing (§16.1). *Amendment 2 drops `Discovery` again (Opus r3 F-3).* The other names already matched. | §4.1, §4.6, §16 | — |
| `None` and the asset-lock exception (N-1) | **Applied.** §16.6 says what each kind of artifact answers and how the host reads `None`. For an asset lock, `None` (no entry **and** no tracked row of any status) means never registered, and it allows a second funding: `register` precedes tracking and every transport (I1), and an entry outlives its row (§6.2). For a state transition or a `TxDraft` send, `None` is unknown, read as `MaybeSent`. A txid and a transition hash look alike, so the host reads every `None` as unknown, and the asset-lock reading belongs to the engine's funding gates (`discard_registration`), which know the step's kind. A live `Unsent` entry and a `Committing` one read `MaybeSent`. | §4.6, H11, I16, §16.6 | `asset_lock_absent`: reading the entry alone, or skipping `Consumed` rows, funds twice after a journal rollback or deletion. `restart_retry`: after a restart a withdrawal reads `None` and gets no retry |
| Tombstone (N-1) | **Applied.** A state transition settled definitely unsent keeps a per-process `NotSent` tombstone instead of being forgotten. The J step that admits a later First of the same bytes under a live lease replaces it before any transport. After a restart the set is gone and the answer is `None`, which for a transition is unknown: fail closed. | §5.5, I4, I10, H16 | `rowless_tombstone`: forgetting the id leaves no retry; a tombstone that a later First does not replace says `NotSent` while the bytes may be out |
| `Committed{will_resend: false}` | **Removed.** It was rev1's `DispatchStatus` value for a committed artifact the engine would not resend, and it maps to `MaybeSent` (`will_resend: true` is `WillBeSent`). rev2 never used it. | — | — |
| `lease_revoked`, `lease_expired` (N-5) | **Mapped.** `LeaseError::Revoked{cause}` is `platform.lease_revoked{cause}`, and `LeaseError::Ended` (`end_flow`, the reaper) is `platform.lease_expired`. A passed `key_until` parks the lease, so it is `platform.needs_grant{purpose}`, never `lease_expired`: E0-08 drops "or its own key passed `key_until`" from its row. | §4.4, §16.4 | — |
| `WalletClosed` (N-4) | **Kept** as its own cause: one wallet's close (§8.6), while `Close` is the session's. E0-08 adds `RevokeCause::WalletClosed`. | §16.4 | — |
| `funds_committed` (N-3) | **Applied.** In Mode A it is true when an asset lock of the flow has its entry in `Committing`, `Dispatching`, `Ambiguous` or `PreFence`, or has a tracked row with no entry. It is false for an `Unsent` entry (live, or with a dead origin), a `Revoked` entry, and no entry with no row. In Mode B it is true from the J step that takes the funding call's permit (and its marker) until the funding reads `NotSent`. So a revoked lock and an uncommitted one both read false. E0-08 copies §16.5. | §2a.5, §4.6, §16.5 | — |
| A lease id's reach, the reaper (N-6) | Unchanged here. A lease id is accepted only by calls of its wallet whose purpose it carries, otherwise `platform.needs_grant{purpose}`; another wallet's id is `platform.grant_invalid`. E0-08 states it. | §16.1 | — |
| The retry gates (N-1, N-2) | `discard_registration` needs the funding to read `NotSent` or `None`, which also refuses a live `Unsent`. `resume_registration` acts on any state, and `finish_asset_locks` is not gated. E0-08 replaces its "`NotSent` … or absent" clause with these, and its refusal list's phase rule with the same check. | §16.6 | — |

## Changes in rev2

**Summary.** The core stands as both r2 reviews found it, and all seven r1 reproductions stay closed. rev2 fixes six
interleavings that GPT r2 found in rev1's fixes and in Mode B, applies DEC-67 everywhere, gives Mode B a source of
truth for its outcomes, and makes §16 the single source for names. Each new trace fails under rev1's rule and passes
under rev2's in the model:
- Part 2 holds `joined_lock`, `rebind_ceiling`, `repair_resolution` and `quickunlock_sum`;
- the new Part 5 holds Mode B funding and double pay, step 2's classification and its copy, and retry after a
  restart.

**Manager rulings for rev2** (2026-10-08), each applied:
1. All GPT majors are accepted, each in the model with a trace that fails under rev1 and passes under rev2.
2. DEC-67 everywhere:
   - a rebind after an unlock prompts again when "require authentication for every payment" is on (§4.3, §4.8, Q5,
     Q22, §16);
   - Touch ID may issue `PlatformOp` capped at the spend limit (§3.1, §3.7);
   - one combined cap over a grant set, so "Accept and pay" cannot reach twice the limit.
3. Mode B status derivation from the tracked rows and the call's end (§2a.5), with "no double pay" passing in the
   model (Part 5).
4. The design is authoritative over the contract. *Corrected by amendment 1 above: names and shapes follow
   c3c9bbd, and the safety semantics stay here.*
   - retry and discard are fail-closed; "absent means allowed" holds for asset locks only;
   - the `finish_asset_locks` gate is fixed;
   - §16 states the names, which are c3c9bbd's.
5. The UX minors (§4.6, §4.8, UX-SPEC §4.1).
6. The phase plan: P4 is L, and the Swift freeze is respected (§13).

**GPT r2 dispositions**

| # | Finding | Disposition | Where | Model |
|---|---|---|---|---|
| 1, 3 | r1 majors 1 and 3 | Closed in r2; unchanged. | — | the r1 replays still pass |
| 2 | r1 major 2: closed for one lock, open for joined locks | Closed by 8. | §8.1 | `joined_lock` |
| 4 | r1 major 4: closed in Mode A, accepted residual in Mode B | Unchanged. Its Mode B outcome rule is fixed by 10. | §4.4 | Part 4, Part 5 step 2 |
| 5 | r1 major 5: closed in Mode A; Mode B recovery conditional | Mode B's replacement-funding path is closed by 12. | §2a.5 | Part 5 funding |
| 6 | r1 major 6: refunds can undo the rebind minimum | Closed by 9. | §4.2, §4.3 | `rebind_ceiling` |
| 7 | r1 major 7: Mode B's definite-outcome rule | Closed by 10. | §2a.3 | Part 5 step 2 |
| 8 | A second Lock joins a completed gate and returns with the vault unlocked | **Fixed.** Every lock request runs its own vault gate after its freeze; only the drain is shared (H14). An unlock waits while a lock gate is pending, so a lock request is always ordered before an unlock issued after its call. The FFI lock runs its gate inline and marks it done. | §8.1, §8.3, H14 | `joined_lock`: rev1 redeems a pre-K2 grant after K2 returned; rev2 does not |
| 9 | A refund after a rebind restores signing above the fresh cap | **Fixed.** Each charge carries the authority generation it was made under. A refund restores only that generation's accounting. New signing is bounded by the current generation's ceiling, `min(available at rebind, fresh cap)`, through every later refund and rebind, with checked arithmetic. | §4.2, §4.3 | `rebind_ceiling` |
| 10 | Mode B lists step 2 as a sole-artifact call for `Locked → Cancelled` | **Fixed.** The engine's signer adapters count the signatures released during each call permit (host-visible and monotone). A call is `Cancelled` only if it failed with no signature released and no earlier marker of its step exists. Otherwise it is `MaybeSent`, and the flow is Parked when a further signature is needed. No call is special-cased by name. | §2a.3 | Part 5 step 2: rev1 reports Cancelled after S1 went out |
| 11 | Funded-flow copy promises more than Mode B provides | **Fixed.** One copy table (§16.10). While any library call of the flow runs, the line is DEC-67's: "Lock stops new signatures; a transaction already signed may still be sent". "You'll finish after you unlock" appears only when no call runs and a further signature is required. "Sent before the lock" needs the drain to have seen it. | §4.6, §16.10 | Part 5 step 2: rev1's copy promises a stop that does not happen |
| 12 | Mode B's absent-artifact discard allows duplicate funding after power loss | **Fixed.** In Mode B every funding call is preceded by a durable funding step marker, keyed by flow and step, not by artifact. `dispatch_status` is derived (§2a.5). With no marker the call never started, which is an asset lock's `None` (amendment 1). A marker with no row and no definite resolution is `MaybeSent`, which can last indefinitely, and blocks funding again until DP1-05 finds the lock or Repair's ChainLocked self-spend proves it dead. | §2a.5 | Part 5 funding: rev1 builds T2 beside a peer-held T1; rev2 cannot |
| 13 | Repair turns negative observations into definitely-unsent | **Fixed.** Repair keeps a row of unknown provenance `MaybeSent`. Its only resolutions are "Send it" (a First under a fresh lease) and "Cancel it": a self-spend of one of its inputs, after which the row is cleaned up only once that spend is ChainLocked. There is no negative-evidence discard. | §6.5 | `repair_resolution` |
| 14 | Q22 implements the policy DEC-67 rejected | **Fixed.** A silent rebind needs the setting off. With it on, the lease goes to `NeedsGrant` and the flow to `Authorize` (a prompt). Q22 is marked rejected, and §3.1 now admits Touch ID for `PlatformOp` (Q21). | §3.1, §4.3, §4.8, Q22 | — |

**Opus r2 dispositions**

| # | Finding | Disposition | Where |
|---|---|---|---|
| 1a | "No entry" means retry allowed in the contract; `finish_asset_locks` gated backwards | **Fixed.** Only `NotSent` allows a retry or a discard, plus an asset lock's `None` (amended in amendment 1: rev2 first answered `Unknown` and never none; c3c9bbd's `None` stands, read by the artifact's kind, §16.6). `NotSent` needs positive evidence. `finish_asset_locks` is not gated, since it resumes committed locks. A test covers a restart after a withdrawal's `broadcast_unknown`. | §4.6, H11, H16, §16 |
| 1b | Names and shapes disagree with c3c9bbd | **Fixed.** §16 is the single source and takes the contract's names where Opus suggested: `will_be_sent{artifact}`, `broadcast_unknown{artifact}`, `DashPay::dispatch_status(artifact)` (amended to c3c9bbd's `Option` of four values), `DispatchResolved{wallet_id, artifact, resolution}`, `grant_request(identity, GrantAction)`, "no more than quoted". It adds the contract's lease codes: `needs_grant` (rows move to `Unlock`/`Authorize`), `lease_expired` only for `end_flow` and the reaper, `lease_revoked{cause}`, `RevokeCause::WalletClosed`, `storage`. It also drops `FlowKind::Discovery` (restored by amendment 1, dropped again by amendment 2), makes the reaper skip running flows, and names the owners of `LeaseView` and Repair. | §16 |
| 2 | Mode B has no source for `dispatch_status`, `funds_committed`, `WillBeSent`, `DispatchResolved` | **Fixed.** §2a.5 derives them from `list_tracked_locks`, the funding marker and the call's end, and says how row-less "may have been sent" resolves after a restart (H16). | §2a.5 |
| 3 | Rebind skips the prompt against DEC-67 | Fixed with GPT 14. | §4.3 |
| 4a | Mode B step markers have no read rule; the J step does a FULL write | **Fixed.** At a resumable step's call-permit step, an earlier marker of the step under a dead lease means MaybeSent. The marker write follows the `Committing` pattern (spawned, outside J). | §2a.3 |
| 4b | Mode B's reason for step 2 | Superseded by GPT 10's rule. The one-call path is not used in Mode B. | §2a.3 |
| 4c | Library builds may pick possibly-sent inputs in Mode B | **Fixed.** No library funding build starts while a tracked `Built`/`Broadcast` row of the wallet has no proof and the catch-up has not resumed it. | §2a.3 |
| 4d | Mode A "all of Mode B" | **Fixed.** Mode A supersedes Mode B items 5 and 9's worst-case funding bound. | §2a.4 |
| 5a–f | UX: copy precedence, DEC-67 wording, "key held" (mixing-only, Cross text, tooltip), post-funding wording, auto lock armed only while the key is held, Touch ID combined sum, the FFI lock's thread | **Fixed.** | §3.7, §4.6, §4.8, §8.1, UX-SPEC §4.1 |
| 6 | P4 understated; P2a events and P5 against the Swift freeze | **Fixed.** P4 is L. P2a's new `EngineEvent` variants stay engine-side and dw-ffi forwards them from E0-13. P5 binds `send.cancelled` in the chosen stack (E0-13) and maps it onto an existing code for the frozen shells. | §13 |
| 7a–f | `funds_committed` defined three ways; `WillBeSent` copy; the §12 `payment_guard` line; `send.cancelled_by_lock`; Repair's mempool check; `DispatchScope`'s wallet type | **Fixed.** One definition (§16.5); cause-neutral "Will be sent"; the test line follows L2; `send.cancelled` everywhere; Repair rewritten (GPT 13); `[u8; 32]`. | §4.6, §12, §13, §5.2, §16 |

## Changes in rev1

**Summary.** The core stays as both reviews found it: the host-owned journal, the single compare-and-set under J,
revocation at the lock call, host-enforced deadlines and `lock_gen`. rev1 makes these changes:
- it specifies a design for each outcome of Q1 (§2a);
- it closes GPT's seven majors, each reproduced in the model, where it fails under rev0's rule and passes under
  rev1's;
- it adds the escaping mutation GPT found, and judges every definite verdict from the actual send history;
- it adds the "will be sent" outcome, a journal check before any retry or discard, and `DispatchResolved`;
- it fixes the Lock copy and the lock-state UX;
- it writes out the E0-08 contract changes (§16);
- it releases the build's in-broadcast pin on a cancelled artifact;
- it closes every Opus minor.

**Manager rulings for rev1** (2026-10-08), each applied:
1. All seven GPT majors are accepted. Each is fixed and added to the model, and so is GPT's missed mutation.
2. Q1 fallback: two modes with separate acceptance lists. E0-04 and DP1-02 can close in Mode B, and everything
   common to both modes lands first (§2a, §13).
3. No double pay through the UI: a "will be sent" outcome, retry and discard consult the journal, and an event
   fires when a provisional outcome resolves (§4.6, §5.7 H11–H12).
4. Lock UX: Lock "pauses" once funds are committed; a "key held" lock state with a working Lock control; the four
   departures from mobile conventions are fixed (§4.6, §4.8).
5. The E0-08 contract changes (§16).
6. Releasing the in-broadcast pin on cancel; the model splits reservation and pin (§5.5).
7. All Opus minors.

**GPT r1 dispositions**

| # | Finding | Disposition | Where | Model |
|---|---|---|---|---|
| 1 | A concurrent row-less resend outlives a definite refusal and released inputs | **Fixed.** Every attempt, a Resend included, holds an attempt guard tracked under J. A row-less artifact is settled definitely unsent only when no attempt still runs and none may have let it out. A Core send settled that way is tombstoned, so its inputs are released once and any later copy is refused. The release decision comes from that settlement, never from one attempt. | §5.5 "the row-less set" | `rowless-concurrent` scenario; GPT's trace replays as a violation under `rowless-rev0` and is not admitted under rev1 |
| 2 | Waiting for old permits does not close the interval before the vault gate runs | **Fixed.** The table owns a lock barrier that covers both the vault gate's completion and the drain. Every lease insert (H8) waits for the barrier and re-reads `lock_gen` at insert. The barrier is cleared by a detached coordinator task, so a dropped caller cannot leave it set, and concurrent locks join it. | §8.1, §8.3, H8 | Part 2 `gate_window`: rev0 admits a First after return; rev1 does not |
| 3 | Cross-process row-less refusal depends on a marker with no write-ahead rule | **Fixed.** A resumable row-less step writes a durable (`FULL`) write-ahead step marker, keyed by step and artifact hash, in its First's J step and before any transport. H10 consumes the marker, not the flow's phase. | §5.5, §6.2, §7.6, H10 | `rowless-resume` and `rowless-resigned` scenarios; `no-step-marker` replays GPT's trace |
| 4 | The own-key split still has an invisible ChainLock fallback, including in registration's second call | **Fixed in Mode A**: the PR adds `FallbackPolicy::Surface`, so every ChainLock fallback returns `ChainLockFallbackRequired` to the engine, which parks at that moment and continues only under a new grant. Step 2 runs only once the row holds a proof. **Mode B** keeps the documented residual (the key is bounded by `key_until`), and a signer `Locked` out of step 2 maps to Parked, never Failed. | §4.4, L17 | Part 4: rev0 shows the key held through a hidden wait and a funded registration Failed; Mode A shows neither; Mode B shows exactly the documented residual |
| 5 | The FULL journal can preserve a possible send that the load catch-up cannot fence or resume | **Fixed in Mode A.** `register` stores a recovery payload: the signed transaction, its input outpoints and the funding metadata. Load reconciles `Dispatching`/`PreFence` entries that have no row: it restores the row (L16), installs a pending-spend fence on the inputs (L12, cleared only by an observed spend), then resumes. **Mode B**: a documented residual (§2a). | §6.2, §7.2, H6 | `flow+power` scenario (power loss separate from a crash); `no-payload` replays GPT's trace |
| 6 | Rebind can restore signing above the new grant's cap | **Fixed.** A rebind sets each purpose's remaining budget to min(old remaining, fresh grant's cap); a purpose the fresh grant lacks drops to 0; charges and permits already made stand. A mixed grant set sums its caps per purpose. | §4.3 | Part 2 `rebind_cap`: rev0 admits a transition above the fresh cap; rev1 does not |
| 7 | "Nothing in flight" is not evidence of a cancelled flow | **Fixed.** Every lease keeps a monotone history of its artifacts' commits. The lock report derives each flow's outcome from that history: Sent, WillBeSent, MaybeSent, or Cancelled only when no artifact was ever committed (or every committed one is settled definitely unsent). Multi-artifact flows report per artifact. | §8.4 | GPT's trace replays as a violation under `snapshot-outcome`; the design reports Sent |
| 8 | The model's oracle can forget a successful send; its coverage claim is too broad | **Fixed.** The oracle judges "may send" from the immutable send history, the attempts still running and the journal. GPT's `forget-rowless-history` mutation is in the list and is caught. Power loss is a separate transition; the concurrent caller, row-less crashes, the post-freeze creation window and the lock outcome are all modelled. The header now states what the state space covers. | §10 | 29 mutations, all caught |

**Opus r1 dispositions**

| # | Finding | Disposition | Where |
|---|---|---|---|
| 1 | A declined Q1 has no end state | **Fixed.** Mode B: call permits around each library write call, the vault gate stops new signatures, and the journal's step markers and lock history apply. It has its own acceptance list, and E0-04 and DP1-02 can close against it. The mode-independent work lands first. | §2a, §13 |
| 2 | Outcomes the journal can still resend are reported as plain MaybeSent, inviting a second payment | **Fixed.** A new `WillBeSent` outcome (`platform.broadcast_pending` in rev1, renamed `platform.will_be_sent{artifact}` in rev2). `funds_committed` holds for any artifact not definitely unsent. Retries and `discard_registration` consult `dispatch_status`. `EngineEvent::DispatchResolved` fires when a provisional outcome resolves, including a reload that refuses and cleans up. | §4.6, H11, H12, §16 |
| 3 | "Lock to cancel" after funding; no Lock control while the vault is locked | **Fixed.** The copy is keyed on `funds_committed` ("Lock to stop; you'll finish after you unlock"). A fourth lock state, "Locked, key held by a flow", and a Lock action that stays enabled while an own-key lease exists. `Vault.lock()` on a locked vault still revokes leases. UX-SPEC is edited in this branch. | §4.6, UX-SPEC §4.1 |
| 4 | E0-08's m4 contract cannot carry the design | **Fixed:** the exact changes are in §16, passed to the E0-08 fix round. | §16 |
| 5 | The cleanup leaves the in-broadcast pin pending | **Fixed.** The cleanup and the drop guard settle the pin released; `Deferred`, `Committed` and L13 keep it pending. The model splits `reserved` from `pinned`, and the tests assert that the inputs can be selected again. | §5.5, L6, L14 |
| 6 | The PR's shape | **Fixed.** A fence-aware `SpvBroadcaster`, not a decorator; `admit` between subscribe and enqueue; a `DispatchContext{wallet, tracked_row, site}` passed by every F1 site; `DispatchScope{wallet, origin, step}` in rs-sdk; the DAPI broadcaster's retries clamped; P3 is L. | §5.1–5.3, §11 |
| 7 | P2 understated; the async FFI lock collides with the frozen Swift shells | **Fixed.** P2 is now P2a + P2b (L in all). The FFI `Vault.lock()` stays synchronous: it does the freeze and the vault gate before it returns, and reports the drain through `LockProgress` and `LockReport` events. E0-04 is L–XL. | §8.1, §13 |
| 8 | Stale DASHPAY §2.6 and ROADMAP DP1-02 text name the build-only API | **Fixed in this branch:** both now name the two-call split. | DASHPAY §2.6, ROADMAP DP1-02 |
| 9 | Registration's step 2 has hidden ChainLock fallbacks | **Fixed** with GPT 4. | §4.4 |
| 10 | dispatch.sqlite lifecycle gaps | **Fixed.** `remove_wallet` with a `Wipe` grant secure-erases the wallet's entries once it has no tracked row. Seeding goes through the library's loaded rows (`list_tracked_locks`), per wallet, so it no longer depends on PWS's schema. A journal schema newer than the build disables the fence rather than failing. The journal is per network and closed with the session. | §6.2, §6.5 |
| 11 | Ambiguous, L13 and Deferred rows are not driven again until the next launch | **Fixed.** H6's resume pass re-runs on every SPV peers 0 → >0 transition and after the journal recovers from a write failure. | H6 |
| 12 | L2 is broader than its reason | **Fixed.** L2 forbids the wallet-manager guard and `build_persist_serial` only across `register` and the `admit`s that do journal I/O. `payment_guard` is explicitly allowed. | L2 |
| 13 | The contact-payment cancelled code is scheduled only for P5 | **Fixed.** `send.cancelled` and its m1/m1-swift rows move to DP3-01. `DispatchRefused` maps to it; the release uses the finalized handle's owner-guarded path, only when the row-less settlement is definitely unsent, and keeps the reserved DIP-15 address. "Accept and pay" partial copy is added. | §4.6, §16 |
| 14 | Four departures from mobile conventions | **Fixed.** Rebind from the vault key without a prompt on `Unlocked(Full)`, within the remaining budget. Auto lock is `lock_vault`. The drain is non-modal. `QuickUnlock` issues `PlatformOp` under limits. | §3.7, §4.3, §4.8 |
| 15 | Rows with no journal entry leave the user stuck | **Fixed.** Tools ▸ Repair "Unrecorded asset locks": "Send it" (registers it under a fresh lease and makes a First) or "Discard" (after the inputs are seen unspent and the transaction is not on chain). | §6.5 |
| 16 | A refusal in rs-sdk skips the nonce refresh | **Fixed.** The refusal path calls `refresh_identity_nonce(owner)`, and the PR tests it. | §5.1, L19 |
| nits 1–6 | "five" vs "four"; the model's "Deferred after H" label; `WalletClosed`; H9/H10 order; the `funding_signer` cap; a third copy line | **Fixed.** | §4.3, §4.6, §4.7, §5.7, §10, A.1 |


## 0. Decisions at a glance

1. **Two modes.** Mode A has the platform PR (Q1, pasta's call); Mode B does without it (§2a). Everything common to
   both lands first. E0-04 and DP1-02 can close in either.
2. **The dispatch record moves out of platform-wallet and into a journal the host owns.** It is a small SQLite file,
   `<network>/dispatch.sqlite`, opened with `synchronous=FULL`, and only the engine's fence writes it. The asset-lock
   row, its changeset merge and its SQLite upsert stay exactly as at the pin.
   - In Mode A the journal holds registered artifacts with a recovery payload, and step markers.
   - In Mode B it holds step markers only.
3. **One step under one mutex decides every hand-off.** The fence's `admit` holds the lease table's mutex J and, in
   that step:
   - reads the artifact's entry;
   - checks the origin lease;
   - charges the budget;
   - either compare-and-sets `Unsent → Committing` (with a permit) or `Unsent → Revoked`.

   Nothing about the decision is read earlier. `admit` takes no library lock, and it waits on nothing but J and its
   own journal write.
4. **The commit is that compare-and-set** (or, for a row-less artifact, its permit's grant). A registered artifact,
   or a resumable row-less step, starts its transport only after its durable record (`Dispatching`, or the step
   marker) has returned. A failed or ambiguous write is `Ambiguous`: it is never cleaned up, and it is resent only
   after a later write succeeds.
5. **Registration before tracking** (Mode A). An asset lock is registered with its recovery payload (durable
   `Unsent`) before its `Built` row is tracked, and the row is durable before it is handed off.
   - Rows that predate the fence are seeded as `PreFence`, possibly sent.
   - A row with no entry is neither sent nor cleaned up; Tools ▸ Repair offers a way out.
   - A row lost to a power loss is restored from the payload, and its inputs are fenced.
6. **Row-less bytes are tracked per attempt.** The fence keeps every admitted row-less artifact and its running
   attempts. It settles one as definitely unsent only when no attempt still runs and none may have let it out. A
   settled artifact is tombstoned for its process: a Core send refuses any later copy, and a state transition
   answers `NotSent` until a later First of the same bytes replaces it.
7. **Revocation happens at the call, and a barrier closes the gate window.** `lock_vault` revokes every lease and
   snapshots the permits in one synchronous step under J.
   - A table-owned lock barrier, which covers both the vault gate's completion and the drain, keeps every lease
     insert out until both are done.
   - Every lock request runs its own vault gate, so a second lock never inherits a gate that an unlock has since
     undone. Only the drain is shared.
   - Permit deadlines (grant + H) are enforced by the host.
   - So `lock_vault` returns within `max(H, vault-gate wait)` of its call, for any number of leases. A dropped
     future, or a second call, changes nothing.
8. **Outcomes come from history, and "unknown" never allows a retry.** A flow's lock outcome derives from its
   artifacts' commit history, not from what was in flight:
   - `Sent`;
   - `WillBeSent`: committed, and the engine will resend it;
   - `MaybeSent`;
   - `Cancelled`, only for a flow that never committed anything.

   No retry or discard is offered unless `dispatch_status` is `NotSent`, which needs positive evidence. The one
   exception is the engine's funding gate, for an asset lock with no entry and no row, which was never registered.
   A state transition or a `TxDraft` send with no entry is unknown, and it never allows a retry. `DispatchResolved`
   fires when a provisional outcome resolves. Mode B derives all of this from the library's tracked rows, a durable
   funding marker and how each call ended (§2a.5).
9. **Exactly one party cleans up, nothing can drop it, and it frees the inputs.** Only the compare-and-set winner
   cleans up, as a spawned task (a drop guard covers a dropped build). It releases the reservation owner-guarded, and
   it settles the in-broadcast pin released, so the inputs are selectable again.
10. **A catch-up at load and on reconnect.**
    - Possibly-sent entries get a pending-spend fence on their inputs (and, in Mode A, a restored row if one was
      lost) before any build can run.
    - Every tracked row is resumed after SPV starts, and again on each peers 0 → >0 transition.
11. **Leases** are engine objects with a 128-bit random id, bound to one wallet and one flow.
    - A lease holds scoped signers and per-purpose budgets.
    - A rebind never raises its authority: each budget becomes the minimum of what remains and what the fresh grant
      caps, and later refunds of older charges never lift it.
    - The rebind prompts again when "require authentication for every payment" is on (DEC-67).
    - On a locked vault a lease holds one `KeyHold`. In Mode A it is dropped at the library's InstantSend window's
      end and at every ChainLock fallback. In Mode B it is bounded by `key_until`.
    - The facade carries a lease across "Accept and pay"'s two calls (§16).
12. **Grants:**
    - `PlatformOp{max_duffs, max_credits}`;
    - `IdentityScan`;
    - `authorize_set`;
    - `platform_signer` capped by its token;
    - `Vault::epoch()`;
    - `QuickUnlock` (Touch ID) for `PlatformOp`, capped at the spend limit by one combined value over the whole
      grant set (DEC-67).
13. **One upstream PR** in Mode A (B5). It makes these changes:
    - a fence-aware `SpvBroadcaster`;
    - the rs-sdk broadcast hook, with nonce refresh and clamped retries;
    - `register`, `abandon` and their drop guard;
    - the recovery payload and its restore;
    - the pending-spend re-fencing;
    - `proof_wait_started`;
    - the surfaced ChainLock fallback.

    In Mode B, flows driven by the library carry DEC-67's promise: "Lock stops new signatures; a transaction already
    signed may still be sent".

| Open issue (DASHPAY §2.6) | Resolution | Checked by |
|---|---|---|
| M-A read, fence and write not atomic; lock order | §5.4 single J step with CAS; §5.8 admit never blocks; cleanup by the CAS winner only (§5.5) | model Part 1 (`split-literal` caught, `split-cas` safe, M-A replay), Part 3 |
| M-B (a) `store` not durable | journal written with `synchronous=FULL` before the transport (§6.2); row durable before admit (L3) | `transport-first`, `no-flush` mutations |
| M-B (b) merge not monotone, resurrected rows | the record is not in the changeset; the journal never moves `Dispatching → Unsent`; a resurrected row meets its entry (§6.3) | Part 1 `flow+crash` |
| M-B (c) failed write is not a clean `Unsent` | `Ambiguous`: no cleanup, Deferred, retried before any transport (§6.4) | `clean-on-write-fail` mutation |
| M-B (d) rows from before the record | the journal is seeded with `PreFence` for them ⇒ Resend; no entry ⇒ kept, not sent (§5.4, §6.5) | `legacy` and `unknown` scenarios; `legacy-unsent`, `no-entry-resend` mutations |
| M-C H bound and drain end | deadline from the grant covers write and transport; host-side expiry; revoke at the freeze; `lock_gen` on lease creation (§8) | Part 2 (all five wrong timing rules fail) |
| m-1 lease ids across processes | 128-bit random ids, matched with the wallet (§4.1) | `lease-reuse` mutation |
| m-2 Part 2 cannot fail | Part 2 is now a tick simulation with per-lease states and the freeze as an event | Part 2 mutations |
| m-3 undocumented atomicity | stated (§10.1); the split variants show what breaks | `split-*` variants |
| nits | §A.2 | — |

## 1. What the pin does

These facts were checked at the pins. Everything below depends on them.

| # | Fact | Where |
|---|---|---|
| F1 | Every Core-transaction hand-off in platform-wallet goes through `TransactionBroadcaster::broadcast`. The sites: the asset-lock build `build.rs:1142`; resume, Built arm `recovery.rs:1176` and Broadcast arm `:1491`; `CoreWallet::dispatch_unexpired` `core/broadcast.rs:158` (finalized sends, ProUpServTx); the plain `broadcast_transaction` `broadcast.rs:288`; `reservations.rs:154`; the contact payment `payments.rs:1446`; the load replay `manager/load.rs:620`. None holds the wallet-manager guard at the call. | PW |
| F2 | platform-wallet builds only `SpvBroadcaster` (`broadcaster.rs:236`). dw also builds one of its own for PSBT broadcasts (`dw-engine/src/send/psbt.rs:464`), which bypasses anything installed in the manager. It calls `SpvRuntime::broadcast_transaction_and_wait` (`spv/runtime.rs:323-338`), which holds `client.read()` for the whole wait: 60 s acceptance + 5 s grace. That deadline starts only after lock waits with no bound (`client/transactions.rs:89`), so it is not a hard bound. `Rejected` comes back for "client not started", zero peers and `NotConnected`. | PW, dw |
| F3 | dash-spv's `broadcast_transaction` takes the network mutex, checks the peer count and enqueues the tx locally (`client/transactions.rs:32-50`). The mempool task then sends it and records it in `broadcasts` (`sync/mempool/manager.rs:363/465`), which it rebroadcasts every 600 s (`:48`, `:677-741`). That set is filled only by local `broadcast_transaction` and is not persisted. | rust-dashcore |
| F4 | The asset lock is tracked `Built` (`build.rs:1098`) and stored (`:1110`) before the broadcast at `:1142`. `queue_asset_lock_changeset` logs a `store` error and carries on (`asset_lock/manager.rs:220-236`). | PW |
| F5 | dw opens the persister with `SqlitePersisterConfig::new` (`dw-engine/src/session.rs:288`): `FlushMode::Immediate` (`store` commits inline), WAL, `synchronous=NORMAL` (`PWS/sqlite/config.rs:13, 148`). So a returned `store` survives a process crash but not a power loss. | dw, PWS |
| F6 | `resume_asset_lock` runs only when it is called: inside library flows, after an InstantSend timeout (`orchestration.rs:701`, 300 s) and on the ChainLock recovery paths (`registration.rs:197/454`, `orchestration.rs:566`); by the library's deferred-resume task, which waits up to 10 minutes for the transport (`recovery.rs:224, 592`) and can run beside a live build; and by a host's launch catch-up. iOS has one (`catchUpStuckAssetLocks`). **dw has none**: no dw code calls `resume_asset_lock`. Resume claims (`tracking.rs:82`) keep `untrack_asset_lock` from removing the row (`:172`). | PW, dw |
| F7 | Reservation tokens and the funding-account list are in memory only. At load the inputs of a `Built` row are **not** reserved again. The release paths are input-keyed and owner-guarded when they have the token (`reservations.rs:223-263`). Some paths have no token and release unconditionally (the generic send `reservations.rs:154-167`, `payments.rs:1446ff`). | PW |
| F8 | dw's SQLite backend returns an empty `unconfirmed_outgoing_txs`, so the load replay never re-dispatches anything in dw (`PWS/sqlite/persister.rs:1861`). | PWS |
| F9 | In rs-sdk, `put_to_platform_and_wait_for_response` is build and sign, then `StateTransition::broadcast` (`SDK/platform/transition/broadcast.rs:125`), then `wait_for_response` (`:172`). `broadcast_with_retries` (`:280`) re-sends the same bytes, possibly to the same node. The SDK's default is 3 retries, so at most 4 sends (`SDK/sdk.rs:89-95`). The wait has its own 30 s × 3 setting (`rs-dapi-client/src/transport/grpc.rs:438-446`). | SDK |
| F10 | `submit_with_cl_height_retry` (`orchestration.rs:353`) signs again with a higher `user_fee_increase`, so each retry is a new artifact. Re-running a registration from an existing lock (`orchestration.rs:701` → `registration.rs:227`) signs again with the same `user_fee_increase`, and since ECDSA signing is deterministic the bytes are identical. | PW |
| F11 | Shielded redrives store signed state transitions and send them again from the coordinator (`shielded/operations.rs:3187/3257`). Shielded is 1.1 (DEC-20). | PW |
| F12 | The key-wallet `Signer` used by `build_asset_lock_with_signer` signs digests. The blanket `TransactionSigner` impl (`transaction_builder.rs:1073`) gives the signer no view of the transaction, so the vault cannot check a cap before it signs a library-built transaction. | rust-dashcore |
| F13 | `SdkBuilder` cannot take a custom request executor (`SDK/sdk.rs:309, 778`), so the host cannot intercept a state-transition broadcast without an rs-sdk change. | SDK |
| F14 | The library holds `generation.payment_guard()`, a lifecycle read lock, across a broadcast (`manager/load.rs:605`, `signed_payment_registry.rs:423`). The DashPay registration path falls back to the ChainLock proof internally (`registration.rs:171-200`), where its caller cannot see it. | PW |
| F15 | `change_passphrase`, `encrypt`, `recover` and `destroy` reach the vault through the generic `vault_op` (`dw-engine/src/keys.rs:119-136`; `dw-ffi/src/api/vault.rs:519-531`). That emits `VaultLockState` only when the lock state changes (`keys.rs:202-210`), and a passphrase change ends the epoch without changing it. | dw |
| F16 | `AssetLockFunding::FromExistingAssetLock` takes an outpoint and a voucher flag, not a proof (`orchestration.rs:227-238`). Registration's step 2 resolves the proof again: on an InstantSend timeout it falls back to `upgrade_to_chain_lock_proof(&out_point, None)` inside the call (`registration.rs:169-197`). When Platform rejects an IS proof at submit it does the same (`:240-259`), and top-up does likewise (`:479-502`). `create_funded_asset_lock_proof` alone returns its 300 s timeout to the caller (`build.rs:1265-1300`). | PW |
| F17 | The build's `InBroadcastPin` is taken at build time (`build.rs:376`). Its `Drop` settles as a pending spend unless it was settled released (`core/generation.rs:634-656`), and a pending-spend fence clears only on an observed spend, with no timer (`:537-576`). Pending-spend fences are process-only (`:107-165`, `:242-252`). | PW |
| F18 | dw's `remove_wallet` calls `persister.delete_wallet` (`dw-engine/src/wallets.rs:223-232`), which cascade-deletes and secure-erases the wallet's rows (`PWS/sqlite/persister.rs:740-790`). | dw, PWS |
| F19 | The production broadcaster is a concrete per-wallet `Arc<SpvBroadcaster>`, monomorphized into `CoreWallet`, `IdentityWallet` and `AssetLockManager` (`platform_wallet.rs:312-325, 663-700`). `DapiBroadcaster` is one `sdk.execute` with the SDK's retries across nodes (`broadcaster.rs:131-149`). | PW |
| F20 | `StateTransition::broadcast` refreshes the identity nonce only on the failure arm after `broadcast_with_retries` (`SDK/platform/transition/broadcast.rs:161-168`). | SDK |

## 2. Terms

- **Artifact:** signed bytes that can leave the process: a Core transaction (id: txid) or a state transition (id:
  the hash of its serialized bytes).
- **Registered artifact:** an artifact that platform-wallet persists before its hand-off and may re-dispatch from
  another task or a later process. At the pin that means asset locks. Shielded redrives join them in 1.1 (§7.5).
  Every other artifact is **row-less**: it is handed off only inside the call that signed it.
- **Hand-off:** the call after which the bytes may leave the process. For a Core transaction that is dash-spv's local
  `broadcast_transaction` (an enqueue, F3), or one DAPI request; for a state transition, one
  `StateTransition::broadcast` including its retries.
- **First / Resend:** a First hand-off makes a possible dispatch that did not exist before. A Resend repeats one
  already recorded and commits nothing new.
- **Commit:** the point after which an artifact counts as possibly sent: the J step that grants its First permit.
  For a registered artifact that step is the compare-and-set `Unsent → Committing`, and its `Dispatching` write
  (spawned in the same step) must succeed before any transport call. For a row-less artifact it is the grant, which
  also records its id in the process's row-less set.
- **Origin:** the lease a First is made under: an `OriginTag` (lease id) or `Unleased(kind)` for the engine paths that
  have no lease (§5.2).
- **Permit:** the RAII proof that a First was admitted. It carries a deadline: the grant instant plus H. The lock
  drain waits for permits only, and stops waiting for each one at its deadline.
- **H:** the hand-off deadline, 10 s (Q6).
- **J:** the lease table's `std::sync::Mutex`, held only for in-memory updates, never across an await or any I/O.
- **Resumable step:** a row-less flow step that a later process may sign again with identical bytes (F10): the
  identity transition of a registration or top-up from an existing lock, and any step the engine declares
  resumable. Its First writes a durable step marker (§7.6).
- **Lock barrier:** the table-owned state of a running lock, set at its freeze and cleared once both its vault gate
  and its drain are done (§8.1).
- **Outcomes:** `Sent`, `WillBeSent` (committed, the engine will resend it), `MaybeSent`, `Cancelled` (never
  committed), and for a single attempt `NotSent` (definitely not sent by this attempt).

## 2a. Two modes: with and without the platform PR

Q1 (opening one platform PR and carrying its cherry-pick) is pasta's call (B5). Both outcomes are designs with their
own acceptance list, and E0-04 and DP1-02 can close in either.

### 2a.1 What both modes share

All of the following land first (P1, P2a; §13) and work without any library change:
- **The vault.** The grants of §3: caps, `IdentityScan`, `authorize_set`, `KeyHold`, `Vault::epoch()` and
  `QuickUnlock` for `PlatformOp`.
- **The lease table** (§4):
  - leases, their lifecycle and budgets;
  - the rebind minimum;
  - the facade's lease handle;
  - the background lease, `LeaseView` and the events.
- **The lock** (§8):
  - the freeze, the vault gate, the drain and the barrier;
  - deadlines and `lock_gen`;
  - the revoking session methods and `vault_op`'s epoch check;
  - close;
  - the synchronous FFI lock with its events;
  - outcomes from history.
- **Engine-owned hand-offs**, those whose bytes the engine builds itself (`TxDraft`: contact payments, and M1 sends
  in P5). The engine is the fence there: the row-less set with its attempt tracking and tombstone, the Spend charge
  bound to the txid, and the release and pin settling.
- **The journal's step markers** for resumable steps (§7.6), with H10's reading of them.
- **The UI and contract surface:**
  - `WillBeSent`, `funds_committed`, `dispatch_status` and `DispatchResolved`;
  - the Lock copy and states;
  - §16.
- **The catch-up's resume pass** (H6 part 2), through the public `resume_asset_lock`.

### 2a.2 Mode A: with the platform PR

Everything in §5 holds as written: per-artifact admission at every hand-off, registered artifacts with their
recovery payload, `PreFence` seeding, restore and pending-spend re-fencing at load, the exact key window
(`proof_wait_started`) and the surfaced ChainLock fallback. The theorem of §9.2 holds at hand-off granularity.

### 2a.3 Mode B: without it

- **Call permits.** The engine wraps each library write call in a call permit:
  - `create_funded_asset_lock_proof`;
  - registration and top-up `FromExistingAssetLock`;
  - contact requests and accepts, profile, contactInfo, DPNS, key updates, withdraw, transfer.

  The permit is taken in a J step that checks that the lease is live and charges the budget from the call's quote.
  Its deadline is the grant + H, and the drain waits for it as for a First permit. The one-call
  `register_identity_with_funding` path is not used in Mode B.
- **Durable markers first.** Before a funding call or a resumable step's call, the engine writes the step's marker
  (§7.6) with the `Committing` pattern: taken in the J step, written by a spawned task outside J, and awaited before
  the library call starts (review Opus r2 4a).
  - A funding step's marker (`registration/<draft>/funding`, `topup/<id>/funding`) carries the funding key, that is
    the funding type, identity index and account. It is keyed by flow and step, never by an artifact the engine
    cannot name yet (review GPT r2 12).
  - At a resumable step's call-permit step, an earlier marker of the same step under a lease that is no longer live
    means the step's outcome is `MaybeSent`, not `Cancelled`.
- **Signatures.** The vault gate ends the epoch at the lock, so every signer issued before fails `Locked` after it.
  The engine's signer adapters count the signatures they release during each call permit. The count is host-visible
  and only grows (review GPT r2 10).
- **Outcome per call:**
  - `Sent` when the call returned Ok;
  - `Cancelled` only when the call failed (a signer `Locked`, or refused before it started), **released no signature
    at all**, and no earlier marker of its step exists. No signature means no artifact could be formed, whatever the
    call does internally;
  - `MaybeSent` otherwise, including a call still running when the drain ends, and a call whose final signature
    failed `Locked` after an earlier one was released. Step 2's ChainLock-height retry and its IS→CL retry both
    re-sign inside one call (F10, F16), so an earlier attempt may execute;
  - a call that failed `Locked` when a further signature was needed parks its flow (`Unlock`/`Authorize`), whatever
    its outcome.
- **Status.** `dispatch_status`, `funds_committed`, `WillBeSent` and `DispatchResolved` are derived as §2a.5 says.
- **Funding cap.** The engine refuses a call whose `amount + fee_bound_worst` exceeds the funding budget, where
  `fee_bound_worst` is the fee of spending every spendable UTXO, a true upper bound. The exact debit is checked only
  in Mode A (at `register`).
- **No funding build over a possibly-sent row** (review Opus r2 4c). No library funding call starts while the wallet
  has a tracked `Built` or `Broadcast` row without a proof that the catch-up has not yet resumed. Otherwise the
  library's coin selection could pick that row's inputs.
- **Re-dispatch keeps the pin's semantics.** A resume, the deferred task or the catch-up may resend a `Built` row
  that was signed before a lock. That is the r4 M1 shape, and it is allowed under Mode B's weaker promise.
- **Weaker theorem.** After `lock_vault` returns:
  - nothing is signed under a lease it revoked;
  - no library call under such a lease starts.

  A call already running, or a row it left, may still hand off bytes signed before the lock; it reports MaybeSent.
- **Promise, in DEC-67's words:** "Lock stops new signatures; a transaction already signed may still be sent". While
  any library call of a flow runs, that is the line the UI shows, funded or not (§16.10).
- **Documented residuals** (each closed by Mode A):
  - a signed-but-unsent `Built` row can go out after a lock (r4 M1);
  - a power loss can orphan a possible send (GPT r1 5). Its funding marker keeps it `MaybeSent` and blocks funding
    again (§2a.5), possibly indefinitely, until DP1-05 finds the lock on the chain or Repair's self-spend is
    ChainLocked;
  - a possibly-sent row's inputs are not fenced at load (F7). The engine mitigates this for its own builds: `TxDraft`
    coin control excludes the outpoints of tracked `Built` and `Broadcast` rows, and no library funding build starts
    over them (above);
  - the own key is bounded by `key_until`, not by the fallback moment. A hidden ChainLock wait (F16) may keep it
    until `key_until`, and a later signer `Locked` maps to Parked, never Failed (model Part 4's "Mode B residual");
  - there is no exact key window: the key is held for the whole `create_funded_asset_lock_proof` call, at most
    300 s + `A` (§4.4).

### 2a.4 Acceptance

**Mode B** (closes E0-04 if Q1 is declined or still undecided at P4; DP1-02 closes against it):
1. The vault tests (§12 "dw-vault").
2. The lease table tests (§12), the rebind minimum and the facade lease handle included.
3. The lock tests (§12 "Bound" and "Barrier"):
   - zero permits, a delayed blocking pool, a gate wait longer than H, dropped callers and double locks;
   - the synchronous FFI lock with `LockProgress`.
4. Engine-owned `TxDraft` hand-offs (§12 "Fence conformance", Core variants), with:
   - the row-less repeat and the concurrent caller;
   - inputs that are selectable again after a cancel.
5. Call permits:
   - paused before the call: Cancelled, nothing recorded;
   - running at the lock: `lock_vault` waits at most H; Sent if it returned, MaybeSent if it outran H;
   - a signer `Locked` before the call's first signature: Cancelled;
   - a signer `Locked` between step 2's ChainLock-height attempts, after an IS submission, or with an earlier marker
     of the step present: MaybeSent, and the flow Parked (Part 5 step 2).
6. Step markers: the kill matrix for resumable steps (kill before and after the broadcast, before the response,
   before the phase write); a later refusal reads MaybeSent.
7. **No double pay** (Part 5 funding; engine tests against the fake library):
   - outcomes from history, `WillBeSent`, and `dispatch_status` derived as §2a.5 says;
   - retry and discard allowed only on `NotSent`;
   - the withheld-T1 power-loss trace: the row is lost, DP1-05 finds nothing, discard is refused, and no T2 is
     built;
   - a top-up's `broadcast_unknown` offers no retry;
   - a restart after a withdrawal's `broadcast_unknown` offers no retry (H16);
   - `DispatchResolved` from row transitions, from DP1-05's adoption and from Repair's ChainLocked self-spend;
   - a definitely-unsent funding (a Lock before signing, a not-ready rejection, a self-spend) allows funding again.
8. The Lock copy of §16.10 with its precedence:
   - the signed-before-broadcast barrier through the UI shows DEC-67's line, never a stop promise;
   - the "key held" lock state, with its mixing-only and Cross forms, and its Lock control (§4.6).
9. For DP1-02, with pin re-dispatch semantics:
   - the kill matrix at every transition;
   - the funding cap holds, by the worst-case fee bound;
   - a testnet registration;
   - a registration Lock during the InstantSend wait parks keyless.

**Mode A** (closes E0-04 when the PR is in the pin):
1. All of Mode B's list, except item 5 (call permits) and item 9's worst-case funding bound. Mode A replaces both
   with per-artifact admission and `register`'s exact debit, site by site in P4 (review Opus r2 4d).
2. Fence conformance at hand-off granularity for Core transactions and state transitions (barriers 1, 2 and 2 with
   a stall).
3. The dispatch-record cases 1–10 (§12).
4. The power-loss recovery tests (payload restore, pending-spend fences).
5. The surfaced-fallback tests (§4.4).
6. The SPV split and the rs-sdk hook tests (nonce refresh and clamping included).
7. The stress run and the kill matrix at hand-off granularity.
8. The PR's L-tests.

### 2a.5 Mode B: where its statuses come from (review Opus r2 2)

Mode B has no registered journal entries, so it derives every status from three sources, all available at the pin:
- the library's tracked rows (`AssetLockManager::list_tracked_locks`, `PW/wallet/asset_lock/manager.rs:263-271`),
  matched to a flow by a funding key (funding type, identity index and account): the one in its funding marker,
  **and the draft's own**, which `app.sqlite` keeps (review Opus r3 F-2). So a deleted or rolled-back journal still
  finds the row. A row that matches several drafts counts for each, which fails closed;
- the funding marker itself, and the resolution recorded with it;
- how each call ended, including its released-signature count.

| Evidence | `dispatch_status` of the funding |
|---|---|
| Repair's self-spend of one of the lock's inputs is ChainLocked | `NotSent` (it can no longer confirm) |
| a matching row is `InstantSendLocked`, `ChainLocked`, `Consumed` or `RecoveredFromChain` | `Sent` |
| a matching row is `Built` or `Broadcast`, with or without a marker | `WillBeSent` (the catch-up and the deferred resume resend it) |
| no row, and the marker records a definite resolution: the call released no signature, or the pin's `Rejected` arm untracked the row before any send | `NotSent` |
| no marker and no matching row: the call never started | `None`: an asset lock never registered, so a second funding is allowed (§16.6) |
| anything else: a marker, no row, no definite resolution | `MaybeSent`, possibly indefinitely |

- **Definite resolutions are written with the marker** (`FULL`), so they survive a restart.
- **The table is applied to every flow with a funding marker or a matching row**: at load, on every
  `list_tracked_locks` change and on each call's end.
- **`funds_committed`** (§16.5) is true from the J step that takes the funding call's permit, and with it the
  marker, until the funding's status is `NotSent`. A matching row with no marker sets it too.
- **`DispatchResolved`** fires when the status moves to `Sent` (a row reaches a proof, or DP1-05 adopts a
  `RecoveredFromChain` lock) or to `NotSent` (a definite resolution, or the ChainLocked self-spend).
- **An orphan can stay unresolved indefinitely.** A lock that lost its row and is withheld by a peer stays
  `MaybeSent` with no time bound. The user can force a resolution with Repair's "Cancel it" (§6.5): a self-spend of
  every coin. Once that spend is ChainLocked, any lost lock whose inputs it consumed can no longer confirm.
- **Row-less state transitions after a restart, in both modes (H16).** The per-process set and its tombstones are
  gone, so `dispatch_status` answers `None`, which for a transition is unknown. Every resolution needs positive
  evidence, and none is offered a retry before it:
  - a resumable step has its marker, and §7.6 resolves it;
  - any other transition has no record, so it stays "may have been sent" for as long as the host shows it, and the
    engine never answers `NotSent` for it. In its own process the row-less entry keeps the transition's identity,
    nonce space and nonce, and H16's evidence settles it.

## 3. Grants (dw-vault)

### 3.1 `PlatformOp{max_duffs, max_credits}`

- `GrantPurpose::PlatformOp` gets `max_duffs: u64` (Core funding: asset locks for registration and top-up) and
  `max_credits: u64` (state-transition spending, §4.2). Either can be 0, which means the purpose is not granted.
- `requires_credential` is unchanged: `PlatformOp` follows the `Spend` column of the m1-engine §2.2 credential
  table.
- `QuickUnlock` (Touch ID) may issue `PlatformOp`, capped at the spend limit (DEC-67, Q21), by the combined value of
  §3.7.

### 3.2 `IdentityScan`

- A new purpose, wallet-scoped, uncapped and not `requires_credential`.
- `Vault::scan_key` accepts only an `IdentityScan` token and refuses `PlatformOp` with `GrantPurposeMismatch`. So a
  capped flow token cannot release the master key, as m1-engine §2.2 demands.
- The unattended bring-up asks for `IdentityScan` with `Credential::None`. That works in the prompt-free states only,
  as today.

### 3.3 `authorize_set`

```rust
pub fn authorize_set(&self, purposes: &[GrantPurpose], wallet: Option<&WalletId>, credential: Credential<'_>)
    -> Result<Vec<AuthGrant>, VaultError>;
```

- One credential check (one Argon2id run, one throttle count) issues one grant per purpose, all with the same
  wallet binding and TTL.
- If the vault holds no full-scope key, each grant carries its own copy of the unwrapped key, as single grants do.
- Every purpose must be allowed for the credential, or nothing is issued.
- "Accept and pay" (DASHPAY §2.3) asks for `[PlatformOp{0, accept_cost}, Spend{amount + fee}]` and gets one prompt.

### 3.4 Caps the vault enforces

- `platform_signer(wallet, token, PlatformFunding{max_duffs})` is refused unless `max_duffs ≤ token.max_duffs`, as
  m1-engine §2.2 requires. The engine passes the lease's remaining funding budget.
- Requesting `PlatformIdentity` from a token with `max_credits = 0` is refused.
- Requesting `PlatformFunding` from a token with `max_duffs = 0` is refused.
- The vault still cannot see the transaction it signs for (F12), so the debit check stays in the engine (§4.2).

### 3.5 `KeyHold`: an own key the lease can drop

Today a signer that carries a grant's own key holds an `Arc<Key32>`. Every clone of the signer keeps the key alive,
including clones held inside a library call, so "the lease drops its key" would not erase anything.

- E0-04 changes own-key signers to hold a `Weak<Key32>`.
- `Vault::hold_key(tokens: &mut [GrantToken]) -> Option<KeyHold>` builds the one strong `Arc` from the first own key
  among the tokens and erases every token's copy. Every own key of a grant set is the same data key, so one hold
  serves the whole set, including "Accept and pay"'s two grants.
- `Vault::platform_signer_held(wallet, &KeyHold, token, scope)` and `signer_held(…)` issue signers that reference it.
- When the `KeyHold` is dropped, the key is erased. A signer operation in progress keeps it only until that operation
  ends (≈1 ms), because it upgrades the `Weak` inside the gate. Every later call fails `Locked`.
- Vault-key signers (vault unlocked with scope Full) are unchanged and still depend on the vault's epoch.
- `Vault::epoch() -> u64` (new, public, not secret) lets the engine detect every epoch change, whichever call made it
  (§8.6).

### 3.6 Contract changes (m1-engine §2.2)

- The purpose list adds `PlatformOp{max_duffs, max_credits}` and `IdentityScan`.
- The credential table puts `IdentityScan` in the `Spend` column. `QuickUnlock` gains `PlatformOp` under §3.7's
  limits.
- `authorize_set` gets a row.
- `Vault.lock()` stays synchronous. It returns after the freeze and the vault gate, revokes every lease even on a
  vault that is already `Locked`, and reports the drain through `LockProgress` and `LockReport` (§8.1).
- The `PlatformFunding` note changes from "advisory today" to "capped by the token; the debit is checked at
  `register` (Mode A) or against the worst-case fee bound (Mode B)".

### 3.7 `QuickUnlock` for `PlatformOp` (review Opus 14, DEC-67)

iOS uses biometrics for DashPay writes, while rev0 made every one of them, and "Accept and pay", need the typed
passphrase on a Mac with Touch ID. DEC-67 accepted Q21: `QuickUnlock` may issue `PlatformOp{max_duffs, max_credits}`,
**capped at the spend limit**.
- **One combined value.** A grant's value is `max_duffs + ceil(max_credits / CREDITS_PER_DUFF)`, where
  `CREDITS_PER_DUFF` is Platform's fixed rate of 1000. It must be at most `spend_limit_duffs`.
- **Over the whole set** (review Opus r2 5e). `authorize_set` with `QuickUnlock` sums the values of every grant in
  the set, the `Spend` grants' `max_duffs` included, and checks that one sum against the limit. So "Accept and pay"
  (a `PlatformOp` for the accept plus a `Spend` for the payment) can reach the limit once, not about twice (model
  `quickunlock_sum`).
- **Freshness.** The passphrase must have been entered within `PASSPHRASE_MAX_AGE_SECS`, as for `Spend`.

Anything above the limit asks for the passphrase. `IdentityScan` stays prompt-free only in the prompt-free states, as
before.

## 4. Leases (dw-engine `platform/lease.rs`)

### 4.1 What a lease holds

```rust
pub struct Lease {                       // Arc<Lease>; owned by its flow
    id: LeaseId,                         // 128-bit, OS RNG (m-1); never persisted as "live"
    wallet: WalletId,
    flow: FlowKind,                      // §16.1's closed list
    table: Arc<LeaseTable>,
}
struct LeaseEntry {                      // inside LeaseTable, under J
    state: LeaseState,
    purposes: Vec<PurposeBudget>,        // §4.2: per purpose, the generation ceilings and the charges
    grants: Vec<GrantMeta>,              // purpose and caps; the tokens are dropped once the signers exist
    key: Option<KeyHold>,                // own-key leases only, one per lease (§3.5, §4.4)
    key_until: Option<Instant>,
    signers: LeaseSigners,               // scoped VaultSigners issued from the tokens
    created_gen: u64,                    // lock_gen read before the grants were redeemed (§8.3)
    flow_task: Option<AbortHandle>,      // so close can stop the flow (§8.5)
}
```

- A lease is created from one or more grants with `NetworkSession::begin_lease(wallet, flow, grant_ids)`, in three
  steps:
  1. wait until no lock barrier is set (§8.1), then read `lock_gen`. A barrier covers a lock's vault gate as well as
     its drain, so a grant issued before a lock is dead (cleared by that gate) before anyone can redeem it here
     (review GPT 2);
  2. redeem every grant (so the 120 s grant lifetime cannot run out mid-flow, DASHPAY §2.3), build the one `KeyHold`
     and issue every signer the lease's purposes need, then drop the tokens;
  3. insert into the table, in a J step that refuses if a barrier is set or `lock_gen` has changed since step 1. If
     only the vault's epoch moved since step 2 (an unlock or a scope change, not a lock), the lease is inserted as
     `NeedsGrant`, since its tokens are already dead.

  If step 3 refuses, the hold and the signers are dropped and the call fails `lease.locked`
  (`platform.cancelled`, §16); the flow asks again.
- Every token must be for the lease's wallet.
- A lease is ended by `Lease::end()`, or by dropping its last owning handle (review P2a r1 F6). There are two
  handle kinds. `begin_lease` returns the owning RAII handle; its clones share one owner, and dropping the last of
  them ends authority exactly as `end()` does. A handle looked up by id (`lease`, `lease_for`) borrows: dropping it
  never ends the lease. The facade keeps an owner per `begin_flow` lease in the session (§4.7). The idle reaper
  stays as a backstop. The table keeps its entry until its last permit has dropped.
- `LeaseId` values are random, so an origin recorded by an earlier process never matches a lease of this one. The
  fence also compares the wallet.

### 4.2 Purposes and budgets

| Purpose | From grant | Signers it yields | Charged at First admit |
|---|---|---|---|
| `Funding{max_duffs}` | `PlatformOp.max_duffs` | `PlatformFunding{remaining}` | wallet debit of the Core transaction |
| `Credits{max_credits}` | `PlatformOp.max_credits` | `PlatformIdentity` | credit cost of the state transition |
| `Spend{max_duffs}` | `Spend` | the `Spend` signer, for `TxDraft` (DP3-01) | wallet debit of the Core transaction |
| `Crypto` | any `PlatformOp` | `DashPayCrypto` | never hands off |

- **Wallet debit:** the sum of the wallet-owned input values minus the outputs that pay the wallet, which is m1's
  `total_debit = external_sent + fee`. The fence never computes it itself, because that would take the wallet-manager
  guard inside `admit` (§5.8):
  - for an asset lock, the library passes the debit to `register`, from the inputs it just selected; `register`
    charges the `Funding` budget and fails if it does not fit;
  - for a row-less Core send under a lease, which is always engine-built (`TxDraft`), the engine charges the `Spend`
    budget at `prepare`, before signing. The fence charges nothing more.
- **Mode B** has no `register`. Its call permit charges the call's quote: for funding, `amount + fee_bound_worst`
  (§2a.3), corrected to the actual debit from the tracked row once the call returns.
- **Credit cost:** the explicit credits the transition moves out of the identity (transfer, withdrawal, top-up from
  credits) plus `fee_bound(st)`. `fee_bound` comes from DP1-06's cost table; until that lands, it is a conservative
  constant per transition type, documented next to the table. See Q7.
- **When the charge happens:**
  - Funding: at `register`, under J;
  - Spend: at `TxDraft::prepare`;
  - Credits: at First admit, under J, together with the permit.

  If a charge does not fit, the operation is refused before anything is committed. A charge equal to the remaining
  cap fits. A zero charge still needs its purpose (review P2a r1 F8): a transition with zero credit cost needs a
  lease carrying `Credits`, and `register` with zero debit one carrying `Funding`, otherwise `needs_grant`.
- **Refunds:**
  - a `register` whose artifact ends `Revoked` (refused or abandoned) refunds its Funding charge, because it was
    never sent;
  - a row-less artifact refunds its charge when it settles `DefinitelyUnsent` (every attempt definitely rejected,
    none possibly out, §5.5);
  - a committed registered artifact keeps its charge.
- **Authority generations** (review GPT r2 9).
  - Each purpose keeps a list of generations `g = 0, 1, …`, one per grant set the lease has been bound to.
    Generation 0's ceiling is the original grant's cap. Each rebind opens a new generation (§4.3).
  - Every charge records the generation it was made under.
  - A refund restores only its own generation's accounting. It never adds to a later generation's available
    amount.
  - A new charge is checked against the **current** generation only:
    `available_g = ceiling_g - Σ(charges made under g, not refunded)`.
  - All of this uses checked arithmetic: an overflow refuses the charge.
- **Pre-sign check.** Where the engine builds the transaction itself (`TxDraft`), the debit is checked against the
  remaining budget before signing, as m1 does today. The test "a lease cannot sign a BIP44 spend above its cap"
  applies to that path. The library builds asset locks and signs them in the same call (F12). For them:
  - the engine refuses to start a build whose `amount + fee_bound` exceeds the funding budget;
  - `register` refuses a built transaction whose debit exceeds it. The build aborts and releases its inputs, so an
    over-cap asset lock is signed but never tracked and never leaves the process (Q8).

### 4.3 Lifecycle

```
            begin_lease
                │
                ▼
   ┌──────── Active ──────────────────────────────┐
   │   (signs; admits Firsts)                     │
   │      │ funding handed off, own key            │ epoch change other than lock
   │      ▼                                       ▼ (unlock, scope change)
   │  AwaitingProof{key_until}                NeedsGrant
   │      │ key_until passes / park(ProofWaiting) │ rebind(grant) ──► Active
   │      ▼                                       │
   │  Parked{ProofWaiting}  ── (no key; admits Firsts of artifacts already signed)
   │                                              │
   └─► lock_vault / close / change_passphrase / remove_wallet ──► Revoked{cause}   (terminal)
       end() ──► Ended (terminal)
```

| State | Signs | Admits a First | Holds an own key |
|---|---|---|---|
| `Active` | yes | yes | if it was issued on a locked or mixing-only vault |
| `AwaitingProof` | yes, until `key_until` | yes | yes, until `key_until` |
| `Parked{ProofWaiting}` | no (`LeaseError::Parked`) | yes | no |
| `NeedsGrant` | no; `rebind` takes fresh authority and keeps the id, the charges made and the permits, with each budget capped (below) | yes | no |
| `Revoked{Lock \| Close \| PassphraseChange \| WalletRemoved \| WalletClosed}` | no | no (refused) | no; dropped in the revoking J step |
| `Ended` | no | no | no |

- **Firsts while parked.** `Parked` and `NeedsGrant` still admit Firsts, because nothing in them needs a key. A
  parked flow has signed nothing new, and an artifact it signed before parking is the same as one an `Active` lease
  holds. Only revocation stops dispatch.
- **Signers stop with their tokens.** A vault epoch change ends every token (E0-03), so every lease's signers fail
  `Locked` after any epoch change.
  - The revoking calls of §8.6 go through dedicated session methods, which freeze before they call the vault.
  - Every other vault call runs through `vault_op`, which compares `Vault::epoch()` before and after. A changed epoch
    moves every lease to `NeedsGrant`, drops every `KeyHold` and re-creates the background lease, whatever the lock
    state did (F15).
  - A `begin` whose issue straddles an unlock sees the epoch change too, so its lease starts `NeedsGrant` and asks
    for a grant the user just gave. That is fail-safe and accepted (review Opus high O-9).
- **Rebind never raises authority** (reviews GPT r1 6, GPT r2 9).
  - In one J step, `rebind` checks the wallet, then opens a new generation for each of the lease's purposes. Its
    ceiling is `ceiling_{g+1} = min(available_g, the fresh grants' cap for that purpose)`, summing a mixed set's caps
    per purpose. A purpose the fresh grants do not cover gets ceiling 0.
  - Later refunds of charges from generation g or earlier never lift `ceiling_{g+1}`. A second rebind takes the
    minimum again, so the authority for new signing is the minimum over every grant the lease has held.
  - Charges already made and permits already granted stand. Artifacts signed before the epoch change keep
    dispatching under the lease as before (Q5). Only new signatures need the fresh authority, and they are capped by
    it.
  - A purpose at 0 refuses its signer (`platform.needs_grant{purpose}`).
- **When the rebind prompts** (DEC-67; Q22 rejected).
  - **"Require authentication for every payment" on** (the default): the lease goes to `NeedsGrant`, and the flow
    waits for `RegistrationWait::Authorize` (vault `Unlocked`) or `Unlock` (locked or mixing-only). The user
    authenticates again, and the rebind takes that fresh grant, still within the generation rule.
  - **The setting off**, on a vault now `Unlocked` with scope Full: the engine may rebind silently from the vault
    key, with an internal `Credential::None` grant capped at exactly the available amounts. That grants no new
    authority.
  - On a locked or mixing-only vault the rebind always needs a credential.

### 4.4 Key hold on a locked vault

An own-key lease, one issued on a `Locked` or `UnlockedMixingOnly` vault, holds a `KeyHold` (§3.5).

- **Which flows may hold one.** An own-key registration or top-up runs as two library calls, so that the end of the
  InstantSend wait is visible to the engine:
  1. `AssetLockManager::create_funded_asset_lock_proof` (`build.rs:806`). It builds, registers (Mode A), tracks and
     broadcasts the lock, then waits 300 s for a proof inside the library (`build.rs:1265-1300`), and returns its
     timeout rather than falling back.
  2. `AssetLockFunding::FromExistingAssetLock{out_point}`, which signs and submits the identity transition.
     - It takes no proof (F16). It resolves the row's proof again, so it is called **only once the row holds a
       proof**: status `InstantSendLocked` or `ChainLocked`. Then that resolution short-circuits to the stored proof
       and waits for nothing.
     - The flow checks the tracked status before it calls step 2 (Opus 9).

  The one-call path (`register_identity_with_funding` on Core balance) falls back inside the library (F16), so it
  may run only under a vault-key lease.
- **Hidden fallbacks in step 2 (F16, reviews GPT 4 and Opus 9).**
  - Even with a proof on the row, step 2 falls back to an unbounded ChainLock wait inside the call when Platform
    rejects the InstantSend proof at submit (`registration.rs:240-259`, top-up `:479-502`).
  - **Mode A:** the PR adds `FallbackPolicy::Surface` (L17), passed by the engine on both calls. Every place that
    would call `upgrade_to_chain_lock_proof(…, None)` returns `ChainLockFallbackRequired{out_point}` instead.
    - The engine parks the lease at that moment (`lease.park(ProofWaiting)` drops the key), and the row records
      `ProofWaiting{CL}`.
    - When the ChainLock proof is on the row, the continuation runs step 2 again under a **new grant and a new
      lease**.
    - No signature is ever made with the old authority after a fallback.
  - **Mode B:** the fallback stays hidden.
    - The `KeyHold` timer drops the key at `key_until` whatever the library is doing, so the key is bounded but may
      be usable into the hidden wait.
    - A signer `Locked` out of step 2 under an expired or revoked lease is mapped to `Parked` (§4.6's line then adds
      "unlock to finish"), never `Failed`: the funds are committed and the flow can finish.
  - Model Part 4 checks both modes and rev0.
- **What a leased flow must never use.**
  - `build_asset_lock_transaction` is build-only. It returns the transaction unsent, untracked and without its
    reservation token (`build.rs:91-117`, `:144`), and the pin has no public API to track a prebuilt lock.
  - The fence enforces this in Mode A: an asset-lock transaction (special type 8) arriving as a row-less First is
    refused (§5.4).
- **Without a proof wait:** `key_until = created + 120 s`, the grant TTL.
- **The InstantSend window.**
  - **Mode A:** `create_funded_asset_lock_proof` calls `DispatchFence::proof_wait_started(wallet, txid, timeout)`
    right before its 300 s wait (L15). The fence sets the origin lease's `key_until = now + timeout` and moves it to
    `AwaitingProof`. So the key lasts exactly as long as the library's own window: DASHPAY §2.6's window, from an
    in-memory instant that nothing persisted moves.
  - **Mode B:** the floor is the call permit's grant + 300 s + `A`, where `A` is the acceptance wait's configured
    bound (65 s at the pin, F2). The key outlives the window by at most `A`.
- **Expiry:** a timer task drops the `KeyHold` at `key_until` and moves the lease to `Parked{ProofWaiting}`. That
  is a park, not an end. A call that then needs the key gets `platform.needs_grant{purpose}`, and a registration
  row moves to `Unlock` or `Authorize` when its proof arrives, so the host never prompts during the wait.
  `platform.lease_expired` is reserved for a lease ended by `end_flow` or the idle reaper, never a passed
  `key_until` (§16.4).
- **InstantSend timeout of step 1:** when `create_funded_asset_lock_proof` returns its timeout, the flow calls
  `lease.park(ProofWaiting)`, which drops the key at once. The row records `ProofWaiting{CL}` and waits for the
  ChainLock without any key.
- **Resuming:** a parked registration resumes with a new grant and a new lease ("Finish registering @alice"), through
  step 2 once the row holds the ChainLock proof.
- **Unlocked vault (vault-key leases):** no `KeyHold`. The lease lives until the flow ends or a revoking call ends it.

### 4.5 The background `DashPayCrypto` lease

- The session owns at most one per wallet with a seed.
- **It exists** while the vault is `Unlocked` with scope Full, `Unencrypted` or `NoKeys`. It is created on the
  `VaultLockState` event, or at bring-up if the vault is already in one of those states.
- **It is dropped** in the freeze step of `lock_vault` and on any move to `Locked` or `UnlockedMixingOnly`.
- **It is created, and re-created, under the same `lock_gen` rule as `begin_lease` (H8).** Its insert refuses if a
  freeze happened since the creation began, so a re-creation racing `lock_vault` can never leave a background lease
  behind the freeze.
- **It is re-created** after an epoch change that keeps the vault prompt-free: a passphrase change, or a scope change
  back to Full. The engine notices those through `Vault::epoch()` in `vault_op` and in the dedicated methods (§4.3),
  not through `VaultLockState`, which a passphrase change does not emit.
- **Its contents:** one `Vault::dashpay_crypto_signer`, no tokens, no budgets.
- **No hand-off, ever.** It has no purpose that hands off, so the fence refuses any `admit` with its origin.
- **What it serves:** the sweeps (contact accounts, decrypted contact xpubs). A crypto product meant for a
  submission (the encrypted xpub of a contact request) is made in the flow whose lease covers that submission
  (DASHPAY §2.6).

### 4.6 What the UI sees

```rust
pub struct LeaseView {                 // owned by m4 §2.11 (flows.rs), §16
    pub id: String,                  // first 8 hex digits; for logs and the UI only
    pub wallet_id: String,
    pub flow: FlowKind,
    pub state: LeaseStateView,       // Active | AwaitingProof{key_until} | Parked{reason}
                                     //   | NeedsGrant | Revoked{cause} | Ended
    pub own_key: bool,
    pub key_expires_in_secs: Option<u64>,
    pub funds_committed: bool,       // §16.5's definition
    pub budgets: Vec<BudgetView>,    // {purpose, ceiling, spent} of the current generation
    pub in_flight: u32,              // permits held now
    pub call_running: bool,          // a library call of the flow is running (selects the copy, §16.10)
}
```

- `NetworkSession::leases() -> Vec<LeaseView>`.
- **`DashPay::dispatch_status(artifact) -> Result<Option<DispatchState>, PlatformError>`** (§16.6), where
  `DispatchState` is one of
  `WillBeSent`, `MaybeSent`, `Sent` and `NotSent`, and `None` means the engine has no entry.
  - `None` depends on the artifact's kind. For an asset lock with no tracked row either, it means never
    registered. For a state transition or a `TxDraft` send it means unknown, and it behaves exactly like
    `MaybeSent`: no retry, no discard (review DW-E0-08 r2 N-1). The host cannot tell the kinds apart, so it reads
    every `None` as unknown; the engine's funding gates apply the asset-lock reading (§16.6).
  - A live `Unsent` entry and a `Committing` one read `MaybeSent`: the flow can still commit it.
  - `NotSent` allows a retry or a discard, and it always rests on positive evidence:
    - a `Revoked` entry, or an `Unsent` one whose origin is dead (I1: its transport was never called);
    - a row-less settlement `DefinitelyUnsent`, kept as a tombstone in its process (§5.5);
    - Mode B's definite resolutions (§2a.5);
    - a ChainLocked conflicting spend;
    - a different transition this engine signed, proved executed in the transition's own nonce slot (H16).
- **`EngineEvent::DispatchResolved{network, resolved: DispatchResolved{wallet_id, artifact, resolution: Sent |
  NotSent}}`** (§16.7) fires when a provisional outcome settles:
  - a Resend is accepted, or the wallet sees the transaction;
  - a reload refuses and cleans up an `Unsent` row;
  - a row-less artifact settles `DefinitelyUnsent`;
  - Mode B's derived status moves (§2a.5);
  - H16's evidence settles a row-less transition.
- **Engine-side events:** `EngineEvent::LeaseChanged{network, lease}` and
  `EngineEvent::LockProgress{network, phase: Draining{in_flight, deadline_in_ms} | Done(LockReport)}`. dw-ffi does
  not forward the new variants until E0-13 (the Swift shells are frozen, §13).

**Outcomes** (reviews GPT 7, Opus 2). Every flow outcome is one of four, with the copy of §16.10:

| Outcome | Meaning | Retry or discard |
|---|---|---|
| `Sent` | handed off and seen accepted, or handed off before the lock | — |
| `WillBeSent` | committed, with a tracked row; the engine resends it on resume, catch-up and reconnect | **never**; the flow waits for `DispatchResolved` |
| `MaybeSent` (and a transition's `None`) | handed off, outcome unknown | only after `DispatchResolved(NotSent)` or a `dispatch_status` of `NotSent` |
| `Cancelled` | nothing of the flow was ever committed, or every committed row-less artifact settled unsent | allowed |

- **Two rules prevent paying twice:**
  - `funds_committed` (§16.5) holds from the funding's commit (Mode A: its entry reaches `Committing`; Mode B: the
    funding call takes its permit) until the funding reads `NotSent`;
  - no UI path builds a second asset lock, or a second payment, for a step whose artifact is anything but `NotSent`
    (the engine's funding gate also accepts an asset lock's `None`; H11, §16.6).
- **Which line shows** (reviews GPT r2 11, Opus r2 5a). Exactly one line of §16.10 shows at a time, chosen in order:
  1. a library call of the flow is running (Mode B), or a committed First of the flow is in flight (Mode A):
     **C2**, DEC-67's line. This covers the whole funding call and a funded step 2;
  2. otherwise, `funds_committed` and a further signature is needed (waiting for a proof): **C3**;
  3. otherwise: **C1** ("Lock to cancel").

  **After a lock** the line is C4 (a manual lock) or C5 (an auto lock: "Locked automatically" followed by C4's
  line). It is chosen again on every change of the flow's calls and outcomes, first match (review GPT r3 F2):
  1. a library call of the flow still runs (Mode B), or a committed First of it is in flight (Mode A): DEC-67's
     promise, C2's string. Lock has stopped new signatures, and one already released may still be sent;
  2. nothing of the flow was committed (outcome `Cancelled`): "Cancelled";
  3. an artifact of the flow is pending, that is `MaybeSent` (or a transition's `None`) or `WillBeSent`: "May have
     been sent" or "Will be sent", the first if both. A parked flow adds "; unlock to finish" (vault locked) or
     "; confirm to finish" (unlocked since);
  4. the flow is parked with nothing pending: "Unlock to finish" or "Confirm to finish". This is the only
     unqualified stop promise: every committed artifact is settled, and what is left needs a new signature;
  5. otherwise: "Sent". "Sent before the lock" shows only when the drain saw the permit or the call finish `Sent`;
     a success seen later is plain "Sent".
- **The drain is not a modal** (review Opus 14).
  - The vault is locked by the lock's own gate before the drain ends, so the lock screen shows at once.
  - While the drain has permits in flight, a non-blocking status line under it reads C6 and disappears at
    `LockProgress::Done`.
- **The "key held" lock state** (reviews Opus 3, Opus r2 5b; UX-SPEC §4.1).
  - While any own-key lease is `Active` or `AwaitingProof`, the status bar shows the vault's own lock icon with a
    badge. On a `Locked` vault that is an orange `lock.fill` with a badge; on a mixing-only vault, the mixing-only
    icon with the same badge.
  - Cross's short text is "Locked · key held" or "Mixing only · key held".
  - The tooltip is C7: it names the flow (registration or top-up) and the seconds left, and says "Lock to cancel"
    or "Lock to stop" by `funds_committed`.
  - The toolbar and menu "Lock" action stays enabled in that state. It runs `lock_vault`, which revokes the lease
    even though the vault is already locked.
- **Contact payments** (review Opus 13, DP3-01).
  - A `TxDraft` contact payment refused by Lock reports `send.cancelled`, not `send.broadcast_rejected`.
  - Its reservation is released through the finalized handle's owner-guarded path, only when the row-less
    settlement is definitely unsent. The reserved DIP-15 address stays on the draft (DASHPAY §2.3 step 5).
  - "Accept and pay" with the accept `Sent` and the payment `Cancelled` shows C8.

### 4.7 API sketch

```rust
impl NetworkSession {
    pub async fn begin_lease(self: &Arc<Self>, wallet: WalletId, flow: FlowKind, grants: &[String])
        -> Result<Arc<Lease>, EngineError>;        // waits while a drain is in progress (§8.3)
    pub async fn lock_vault(self: &Arc<Self>) -> Result<LockReport, EngineError>;   // §8
    pub fn leases(&self) -> Vec<LeaseView>;
}
impl Lease {
    pub fn identity_signer(&self, identity_indices: &[u32]) -> Result<VaultIdentitySigner, LeaseError>;
    pub fn funding_signer(&self) -> Result<VaultSigner, LeaseError>;   // PlatformFunding, cap fixed at issue;
                                                                      // the remaining budget is charged at register
    pub fn contact_crypto(&self) -> Result<VaultContactCrypto, LeaseError>;
    pub fn spend(&self) -> Result<LeaseSpend<'_>, LeaseError>;        // for TxDraft::prepare_with_lease
    pub fn scope<F: Future>(&self, f: F) -> impl Future<Output = F::Output>;  // DispatchScope::lease(self.id)
    pub fn park(&self, reason: ParkReason);
    pub fn rebind(&self, grant_ids: &[String]) -> Result<(), LeaseError>;   // §4.3: budgets capped by the fresh grants
    pub fn end(self: Arc<Self>);
}
```

The facade carries one lease across several calls (review Opus 4). `NetworkSession::begin_flow(wallet, flow,
grants) -> String` returns a lease id, which every facade call accepts wherever it takes a `grant: String`, and
`end_flow(id)` ends it. The session holds the lease's owning handle from `begin_flow` until `end_flow` or the
reaper ends it, so looking a lease up by id and dropping that handle never ends it (§4.1). "Accept and pay" takes one prompt (`authorize_set`), opens one flow, and passes its id to
`accept_request` and then to the payment's `TxDraft.prepare`.

An idle reaper ends a vault-key lease after 10 minutes with no call, no permit and **no running flow task**. A
vault-key registration waiting an hour for a ChainLock makes no call and holds no permit, so it must not be reaped.
An own-key lease's key already ends at `key_until`. So a host that abandons a sheet does not leak entries. The
reaper ends a lease a lock or removal revoked the same way, an idle period after the revocation (or a later use),
so its cause stays readable until then and a host that never calls `end_flow` does not leak it either (review Opus high O-3); the
facade drops its owner at the next `begin_flow`. A reaped lease's later use is `platform.lease_expired`.

### 4.8 Auto lock and the other mobile conventions (reviews Opus 14, Opus r2 5d)

- **Auto lock** (UX-SPEC §4.13) runs `lock_vault`, exactly like the Lock button.
  - Its inactivity timer is armed **only while the vault holds its key**, that is while it is `Unlocked` or
    `UnlockedMixingOnly`. It never fires on a vault that is already `Locked`. Otherwise it would revoke every
    own-key flow after a few minutes of watching its progress screen.
  - A visible flow-progress screen does not count as activity, so a long ChainLock wait does not keep the wallet
    unlocked (Q23).
  - An auto lock during a flow shows C5: "Locked automatically" followed by the line §4.6 chooses after a lock.
    While a call of the flow still runs, that is DEC-67's promise; afterwards it follows the observed outcome, and
    "unlock to finish" stands alone only when nothing of the flow is pending (review GPT r3 F2).
- **A second prompt after an unlock** happens exactly when "require authentication for every payment" is on
  (DEC-67, §4.3).
- **The lock drain** is not a modal (§4.6).
- **Touch ID** issues `PlatformOp` within the combined spend limit (§3.7).

## 5. The dispatch fence

### 5.1 Where the library calls it (Mode A)

**Core transactions.** The broadcaster is a concrete per-wallet `Arc<SpvBroadcaster>`, monomorphized into three
wallet types (F19). A decorator around `broadcast(&tx)` would hold the permit through the whole acceptance wait, so
the PR makes `SpvBroadcaster` itself fence-aware (review Opus 6). Its `broadcast` takes a
`DispatchContext{wallet, tracked_row, site}`, which every F1 site passes, and runs four steps:
1. check the transport is ready (client started, peers > 0); if not, `Rejected` before any `admit` (L8);
2. subscribe to dash-spv's events (no permit);
3. `fence.admit(…)`, then, for `First`, `DashSpvClient::broadcast_transaction`, the local enqueue of F3, under the
   permit and bounded by `permit.deadline()`, checked before each poll of the library;
4. wait for the acceptance event (no permit).

`client.read()` is held only for step 3, which also stops a pending wait from blocking `stop()` (an E0-05 concern).

`DapiBroadcaster` is one `sdk.execute` with the SDK's retries across nodes (F19). Its retries are clamped exactly like
rs-sdk's: each attempt's timeout is `min(configured, deadline - now)`, and no attempt starts after the deadline.

dw's PSBT path builds its own `SpvBroadcaster` (F2). P4 passes it a `DispatchContext` with `Unleased(External)`, so
every Core hand-off in dw has one path.

**State transitions.** `Sdk::with_dispatch_fence` (new), called by `StateTransition::broadcast`
(`SDK/platform/transition/broadcast.rs:125`).
- Every `put_to_platform*` path and every `broadcast_and_wait` reaches it, so all the platform-wallet call sites
  the inventory found (identity, documents, DPNS, contactInfo, contact requests, transfers, withdrawals, tokens,
  platform addresses, masternode withdrawal) are covered by one call.
- The hook sits **inside** the retry closure of `broadcast_with_retries`. That has two effects:
  - one permit covers all of the attempts. Each attempt's timeout is clamped to the time left before the permit's
    deadline, and no retry starts after it;
  - a refusal returns through the existing failure arm, which calls `refresh_identity_nonce(owner)`
    (`broadcast.rs:161-168`, F20). The bumped nonce a refused transition took is therefore never left as a gap in
    the cache (review Opus 16, L19).

**Registration and fallbacks.**
- At the asset-lock build (`build.rs`, before `track_asset_lock` at `:1098`), platform-wallet calls
  `fence.register` with the recovery payload (§6.2).
- It calls `fence.abandon` on every exit that would untrack a row that was never handed off (§5.5).
- It honours `FallbackPolicy::Surface` in `create_funded_asset_lock_proof`, in step 2 and in top-up (L17, §4.4).

### 5.2 Origin: `DispatchScope`

- The task-local `DispatchScope{wallet: [u8; 32], origin, step}` lives in **rs-sdk's** dispatch module (rs-sdk
  cannot name platform-wallet's `WalletId`), so that both rs-sdk's
  hook and platform-wallet can read it (rs-sdk cannot depend on platform-wallet), and platform-wallet re-exports it
  (review Opus 6).
  - `origin` is `Lease(OriginTag)` or `Unleased(UnleasedKind)`.
  - `step` is `Option<StepId>` for a resumable step (§7.6).
  - A state transition carries no wallet id at `StateTransition::broadcast`, so the scope supplies it.
- The engine wraps every library call that can hand off, either in `lease.scope(…)` (with the step for a resumable
  one) or in `DispatchScope::unleased(kind, …)`.
- The library reads the scope at `register` and `admit`. Where it spawns its own task to finish a hand-off it began
  in a scope, it captures the scope and re-installs it in that task (L11). The engine does the same for its own
  spawns: `tx_actions.rs:414` hands off inside `tokio::spawn`, so the scope is installed inside that future (H7).
- A hand-off with no scope gets no origin. The journal then decides for a registered artifact. A row-less artifact
  gets `Deferred` and a `Notice`: it **fails closed** with no false verdict (Q12).

| `UnleasedKind` | Paths (dw-engine) | Drained by `lock_vault` |
|---|---|---|
| `Send` | `TxDraft::broadcast` for M1 sends, both the mixed and the finalized paths (`send/mod.rs:1072-1079`), until P5 leases them (Q2) | no |
| `Mixing` | CoinJoin denomination and collateral transactions (`coinjoin.rs:1539, 2255`) | no; mixing stops on lock anyway |
| `External` | PSBT broadcast of bytes signed elsewhere (`send/psbt.rs:469`) | no |
| `Rebroadcast` | `tx_actions` resend of a transaction already in the wallet's records (`tx_actions.rs:415`) | no |

- **Registered artifacts:** an unleased origin is refused at `register`. Every registered artifact belongs to a lease.
- **Unleased Firsts:** admitted without a permit, as today.

### 5.3 Types

```rust
pub struct DispatchContext { pub wallet: WalletId, pub tracked_row: bool, pub site: DispatchSite }
pub struct DispatchRequest<'a> {
    pub ctx: DispatchContext,
    pub artifact: ArtifactRef<'a>,       // CoreTx{tx} | StateTransition{st, hash}
    pub scope: Option<DispatchScope>,    // the task-local; None when unscoped
}
pub enum Verdict {
    First(DispatchPermit),               // hand off now, under the permit, until permit.deadline()
    FirstUnleased(AttemptGuard),         // Unleased origin: hand off now, no permit
    Resend(AttemptGuard),                // a recorded possible dispatch: hand off now, no permit
    Refused { cleanup: bool, step_possibly_dispatched: bool },
                                         // this artifact provably never sent; `cleanup` only for the CAS winner;
                                         // `step_possibly_dispatched`: an earlier artifact of the same step may
                                         // have been (H10), so the flow reports MaybeSent, not Cancelled
    Deferred,                            // not now; outcome unknown (MaybeSent): keep the row and the reservation
                                         // (also a tracked row with no entry: unknown provenance, with a Notice)
}
#[async_trait] pub trait DispatchFence: Send + Sync {
    /// Durable Unsent{origin, payload}, after charging `payload.debit_duffs` to the origin lease's Funding budget.
    async fn register(&self, wallet: WalletId, payload: AssetLockPayload) -> Result<(), FenceError>;
    async fn admit(&self, req: DispatchRequest<'_>) -> Verdict;
    /// One J step, no I/O; callable from Drop.
    fn abandon(&self, wallet: WalletId, txid: Txid) -> Abandon;   // Revoked{cleanup} | Committed
    /// Informational: the library's proof wait for this asset lock starts now (§4.4).
    fn proof_wait_started(&self, wallet: WalletId, txid: Txid, timeout: Duration);
    /// Informational: a surfaced ChainLock fallback for this asset lock (§4.4, L17).
    fn chainlock_fallback(&self, wallet: WalletId, txid: Txid);
}
pub struct AssetLockPayload {            // everything needed to rebuild the Built row (GPT 5)
    pub tx: Transaction, pub inputs: Vec<OutPoint>, pub out_point: OutPoint, pub debit_duffs: u64,
    pub funding_type: AssetLockFundingType, pub account_index: u32, pub identity_index: u32, pub amount: u64,
}
impl DispatchPermit {
    pub fn deadline(&self) -> Instant;
    pub fn finish(self, outcome: Outcome) -> Settlement;   // Drop = finish(MaybeSent)
}
impl AttemptGuard { pub fn finish(self, outcome: Outcome) -> Settlement; }   // Drop = finish(MaybeSent)
pub enum Settlement { DefinitelyUnsent, MaybeOut, Sent }   // for the whole artifact, not this attempt
```

`tracked_row` is a fact about the artifact: it has a persisted row. It does not say First or Resend. Only the journal
decides that for such an artifact. A tracked row the journal does not know is `Deferred`, never resent because of
`tracked_row` alone (Q16).

The library derives its error and its release decision from the `Settlement`, never from its own attempt (L6):
- only `DefinitelyUnsent` is a definite rejection, which releases inputs;
- `MaybeOut` is the unknown outcome.

### 5.4 `admit`: the one decision

```
admit(req):
  if req is a tracked row and the journal is not loaded: → Deferred   // never waits (§5.8)
  if req is unscoped: debug assertion and Notice{UnscopedDispatch}, whatever the verdict (H7)
  lock J
    e = journal_mem.get(req.wallet, artifact_id)
    match e:
      Revoked                → Refused{cleanup: false}
      Dispatching | PreFence → Resend(guard)
      Committing(other)      → Deferred                            // the other caller resolves it; no waiting
      Ambiguous              → e := Committing(retry), spawn the Dispatching write; unlock; await it
                                 ok: e := Dispatching → Resend    fail: e := Ambiguous → Deferred
      Unsent{origin: L}      → if live(L) and L.wallet == req.wallet:
                                   e := Committing(me); p := new permit(L, now + H); record L's commit;
                                   spawn the Dispatching write (it owns e from now on); unlock; await it
                                     ok, now < p.deadline: e := Dispatching → First(p)
                                     ok, deadline passed:  e := Dispatching → Deferred
                                     fail (either way):    e := Ambiguous   → Deferred
                               else: e := Revoked; refund L's Funding charge → Refused{cleanup: true}
      none, tracked_row      → Deferred + Notice{DispatchRecordMissing}
                               // unknown provenance: never sent, never cleaned up (Q16)
      none, row-less         → r = rowless[id]
                               if r is Revoked:  → Refused{cleanup: false}            // a settled Core send
                               if r is Admitted: r.attempts += 1 → Resend(guard)     // the same bytes again
                               if req is an asset-lock tx: → Refused{cleanup: false}   // must be registered
                               match req.scope.origin:
                                 Lease(L) live, and a ST whose Credits fit, or a Core tx with
                                 a Spend charge recorded for this txid (TxDraft) →
                                   if req.scope.step is a resumable step:              // §7.6, GPT 3
                                     e := Committing(me) for (step, id); p := new permit;
                                     spawn the step marker's write; unlock; await it
                                       ok, in time: e := Dispatching; rowless[id] := Admitted{1} → First(p)
                                       otherwise:   e := Ambiguous or Dispatching          → Deferred
                                   else: p := new permit; rowless[id] := Admitted{1}; record L's commit → First(p)
                                 Lease(L) otherwise → Refused{cleanup: rl_inputs and first refusal,
                                                              step_possibly_dispatched: marker(step) exists}
                                 Unleased(_)        → rowless[id] := Admitted{1} → FirstUnleased(guard)
                                 None               → Deferred + Notice   // fail closed, no false verdict
  unlock J
```

- **`live(L)`:** `L` is in this process's table, not `Revoked` or `Ended`, and is a lease that hands off (not the
  background lease).
- **Origins at load:** an origin read from the journal at load is never live (§4.1).
- **One step:** every read of `e` and every change to it happens under J in one critical section. The durable write
  happens outside J, but `Committing` excludes every other decision on that artifact while it runs, so nothing can
  decide on an earlier read (M-A).
- **The write is spawned inside the J step**, on the blocking pool, and that task owns the resolution of
  `Committing`. A dropped `admit` future therefore never leaves an entry in `Committing` without a writer; the
  orphaned write still resolves it to `Dispatching` or `Ambiguous` (the model's `W` steps).
- **Every grant records the commit on its lease's history** (§8.4). The history only grows. A lock outcome is read
  from it, never from what was in flight (review GPT 7).
- **Fail closed without a false verdict.** An unscoped row-less call that is not in the set gets `Deferred`, not
  `Refused`. Nothing is handed off, and nothing is called definitely not sent either, since its history is unknown (a
  load replay, say). A missing scope is an engine bug, and the debug assertion and the `Notice` make it visible in
  tests, even when the answer is a `Resend`.
- **No waiting on another caller.** A caller that finds `Committing(other)` gets `Deferred` at once and keeps
  everything. The committer resolves the entry within H. A resume that was deferred simply ends, and the next resume
  finds `Dispatching`.

### 5.5 `register`, `abandon`, cleanup, row-less attempts and `finish`

- **`register(wallet, payload)`:**
  - Its origin comes from the scope, and it must be `Lease(L)` with `L` live and no lock barrier set, or it fails.
  - It charges `payload.debit_duffs` to `L`'s Funding budget, under J, and fails if that does not fit.
  - It writes `Unsent{origin: L.id, process: P, payload}` to the journal durably, in one `FULL` transaction, then
    puts it in memory. The payload is what the load catch-up needs to rebuild a row that `wallet.sqlite` lost (GPT
    5): the signed transaction, its input outpoints, the lock's outpoint and the funding metadata.
  - Registering the same txid again with the same origin is a no-op. With another origin it fails. That is safe
    because the credit output's key index is consumed at signing (`asset_lock_builder.rs:578-620`), so a rebuild of
    the same flow gets a new txid.
  - If `register` fails, the library aborts the build before it tracks anything:
    - it releases the reservation with the build's token;
    - it settles the build's in-broadcast pin released (F17);
    - nothing is left behind, and it reports a definite not-sent.
- **`abandon(wallet, txid)`:**
  - It is synchronous: one J step with no I/O, since `Revoked` is never persisted.
  - The build calls it on every exit that would untrack a row never handed off: a failed store or flush, a transport
    that is not ready (L8). A **drop guard** calls it too, held from `track` until `admit` has decided, so a build
    future dropped in that window (a host-cancelled call) still abandons (L14).
  - `Unsent` becomes `Revoked`, the Funding charge is refunded, and the answer is `Revoked{cleanup: true}`.
  - `Revoked` is answered `Revoked{cleanup: false}`.
  - `Committing`, `Dispatching` or `Ambiguous` is answered `Committed`: the build keeps its row, its reservation
    and its pin pending, and reports MaybeSent (WillBeSent).
- **Cleanup:**
  - **Who runs it:**
    - It is done only by the caller that got `cleanup: true`, so it happens at most once per artifact per process.
      On a reload the catch-up (H6) refuses the row again.
    - The library runs it as a **spawned task** (`shared_handle()`), not inline in the caller's future (L6). So a
      caller dropped right after the verdict, or a drop guard (which cannot await), still gets its row cleaned up.
  - **What it does:**
    - It untracks the row, overriding resume claims, since `Revoked` proves the bytes never left and never can.
    - It releases the funding reservation, owner-guarded by the build's token. The PR keeps the token, the funding
      accounts and the in-broadcast pin with the in-memory row, so a resume that wins the compare-and-set can
      release them too.
    - It **settles the in-broadcast pin released** (`settle_released`; review Opus 5, F17). Otherwise the pin settles
      as a pending spend on drop and the inputs stay unselectable for the session. The drop guard and the cleanup
      task own the pin with `settle_released_on_drop()` set as soon as `abandon` or `admit` answers
      `cleanup: true`. `Deferred`, `Committed` and L13 keep it pending, as the pin does.
    - It queues the row's removal.
  - **After a reload** there is no token, pin or reservation of this row left (F7, F17). The catch-up installs
    fences only for possibly-sent rows (L12), so the cleanup of an `Unsent` row only untracks.
  - **Other refused callers** drop their claims and report Cancelled.
- **Row-less attempts** (review GPT 1). The fence keeps, in memory and per process, an entry for every row-less
  artifact it admitted:
  - **The entry:** `Admitted{attempts, possibly_out}`, whose state transitions also keep their identity and nonce
    (H16); `Revoked`, a Core send's tombstone; or `NotSent`, a state transition's tombstone.
  - **Attempt guards.**
    - Every attempt (the First, and every Resend of the same bytes) holds a permit or an `AttemptGuard`.
    - `possibly_out` becomes true, and stays true, when any attempt finishes `Sent` or `MaybeSent`, or is dropped.
    - `finish` returns the **artifact's** `Settlement`. It is `DefinitelyUnsent` only when this attempt was a
      definite rejection, no other attempt is still running, and `possibly_out` is false; otherwise it is `MaybeOut`
      (or `Sent`).
  - **On `DefinitelyUnsent`:**
    - the budget is refunded;
    - a state transition's entry becomes the tombstone `NotSent`, so `dispatch_status` still answers `NotSent`
      (review DW-E0-08 r2 N-1; rev2 first forgot the id, which left the host nothing to retry on). It has nothing to
      release. An identical later signature is a fresh First under a live lease: the J step that admits it replaces
      the tombstone with its attempt before any transport, so the answer is `MaybeSent` from then on. A refusal
      leaves the tombstone (model `rowless_tombstone`);
    - a Core send's entry becomes `Revoked`, and its engine releases the draft's reservation and pin exactly once
      (the release runs only for that `Settlement`). Any later copy of those bytes is refused.
  - **A refusal of an id never admitted** is definite. For a Core send it makes the entry `Revoked`, and only that
    refusal's caller releases. For a state transition it writes the tombstone `NotSent`.
  - **What it covers:**
    - a `TxDraft` repeat (`send/mod.rs:1052-1080`);
    - a user resend (`tx_actions.rs:415`);
    - a registration re-run that signs identical bytes (F10);
    - a second holder of the same bytes running concurrently.
  - **Per wallet, one transaction** (review Opus high O-1, which reverses P2a's first keying by artifact alone).
    Entries and `TxDraft` Spend bindings are keyed by (wallet, artifact), so one wallet's admission, refusal or
    settlement never replaces or refunds another's. The same bytes under two wallets are still one transaction: while
    another wallet may have them out or about to go out (its entry is `Admitted`, running or possibly out, or
    `Resolving`; or it holds a step marker or a `Sent` row of them, a copy's marker write included), a copy for this
    wallet answers `Deferred`, in `decide` and again before a resend that waited for its markers. So no entry of this
    wallet runs alongside the other's attempt and settles `DefinitelyUnsent`. Another wallet's tombstone does not
    hold a copy back. Meanwhile this wallet's own tombstone reads `MaybeSent` (`dispatch_status`, the lock report),
    so the host offers no retry on it. The hold can last the session: a possibly-out entry clears only when the
    bytes are seen (BL-76), and a `Resolving{failed}` one only when its own wallet retries. That fails safe: the
    deferred send reports `broadcast_unknown` with its inputs pinned. The set of artifacts seen `Sent` stays keyed by
    artifact: identical bytes on the wire are sent for every wallet. Unreachable in P2a (one wallet signs a
    `TxDraft`'s inputs), this is for the producers that follow (`External`, `Rebroadcast`, P2b/P4 hand-offs).
  - **It is not persisted.** Row-less bytes are handed off only in the process that signed them (dash-spv's broadcast
    set is not persisted either, F3, and dw has no load replay, F8). The exception is resumable steps, which carry a
    durable marker instead (§7.6). Nor are the tombstones: after a restart `dispatch_status` answers `None`, which
    for a row-less artifact is unknown (§16.6).
- **`finish(outcome)`:**
  - It records `Sent`, `MaybeSent` (also the drop default) or `NotSent`.
  - It feeds the lock report and, for a funding permit, the floor of the IS window (§4.4).
  - For a registered artifact, a definite transport rejection after `Dispatching` does **not** untrack or release
    (L13). The row stays and is resent later (Q13): its outcome is `WillBeSent`. The library runs its readiness
    check (client started, peers > 0) **before** `admit`, and a not-ready transport is handled through `abandon`,
    so this case reduces to a peer loss in the moment between the check and the enqueue.

### 5.6 What the library must do (the PR's obligations, Mode A)

| # | Obligation |
|---|---|
| L1 | Call `admit` at every hand-off of F1 and F9. Never decide First or Resend itself. |
| L2 | Hold no wallet-manager guard and no `build_persist_serial` while it awaits `register` or a registered artifact's `admit`, the calls that do journal I/O. The fence takes neither, so this is not needed to keep the drain deadlock-free. It is needed so that an fsync never stalls every wallet reader. `payment_guard` is explicitly allowed across an `admit`: those sites are row-less, and a non-resumable row-less `admit` does no I/O and takes only J, a leaf. The pin holds it on purpose, to linearize wallet teardown against a payment (review Opus 12). |
| L3 | Make the row durable before `admit` for a tracked row: propagate the `store` result (no log-and-continue as in `queue_asset_lock_changeset`) and `flush` when `!store_commits_inline()`. |
| L4 | Call `register` with the recovery payload, including the debit of the inputs it selected, before tracking an asset lock, and abort if it fails. |
| L5 | `First(p)`: call the transport only if `now < p.deadline()`, bounded by `p.deadline()`; then `finish`. A timeout is MaybeSent. Every wait before the transport (the wallet, the SPV client) is bounded by the deadline too, and the deadline is checked last, immediately before the library is entered: a First past it never dispatches and is definitely unsent (review P2a r1 F3). The bound is checked before each poll of the library, so nothing of it runs at or past the deadline. |
| L6 | `Refused{cleanup: true}` and `Abandon::Revoked{cleanup: true}`: spawn the cleanup of §5.5 as a library task, overriding resume claims, releasing the reservation owner-guarded and settling the in-broadcast pin released. Then report the definite not-sent error (`DispatchRefused`). `cleanup: false`: drop the claim and report the same error. Derive every release from the `Settlement`, never from the attempt, and never map `Refused` onto a release path without a token (F7). |
| L7 | `Deferred` and `Abandon::Committed`: keep the row, the reservation and the in-broadcast fence; report the unknown outcome (`TransactionBroadcastUnconfirmed`). |
| L8 | Run the transport readiness check before `admit`; if it fails, `abandon` (for a tracked row) and report the definite rejection. |
| L9 | Never insert an outgoing transaction into the wallet's records, or into dash-spv's broadcast set, before its admitted hand-off. A test checks this. |
| L10 | Split the SPV broadcast as §5.1; in rs-sdk and `DapiBroadcaster`, clamp each attempt to the permit's deadline. |
| L11 | Capture `DispatchScope` across the library's own spawns of hand-off work. |
| L12 | Provide a load-time **pending-spend fence** for named outpoints. It is not a key-wallet reservation, which expires after 24 blocks: it clears only on an observed spend of the outpoint, or when the row reaches a terminal status (review GPT, closure note). The host calls it for every possibly-sent entry's inputs in its catch-up (H6). |
| L13 | After `admit` returned `First` for a tracked row (its `Dispatching` record is durable), a transport rejection neither untracks the row nor releases its inputs. The pin's `Rejected` arm (`build.rs:1144-1233`) applies only before `admit`, through `abandon`. |
| L14 | Hold a drop guard from `track` until `admit` has decided, owning the build's in-broadcast pin. Its `Drop` calls `abandon`, spawns its cleanup, and settles the pin released on `cleanup: true`. |
| L15 | `create_funded_asset_lock_proof` calls `fence.proof_wait_started` right before its proof wait (§4.4). |
| L16 | Provide `AssetLockManager::restore_tracked_lock(payload)`, which re-tracks a `Built` row from a journal payload, for an entry whose row `wallet.sqlite` lost (GPT 5). |
| L17 | `FallbackPolicy::Surface` for `create_funded_asset_lock_proof`, registration's step 2 and top-up: every place that would call `upgrade_to_chain_lock_proof(…, None)` returns `ChainLockFallbackRequired{out_point}` and calls `fence.chainlock_fallback` instead (§4.4, GPT 4). |
| L18 | A fence-aware `SpvBroadcaster` (§5.1) and a `DispatchContext` at every F1 site; `DispatchScope{wallet, origin, step}` in rs-sdk. |
| L19 | The rs-sdk hook sits inside the retry closure, so a refusal reaches the failure arm's `refresh_identity_nonce` (F20, review Opus 16). |

With no fence installed, the library behaves exactly as at the pin, so other hosts (iOS) are unaffected.

### 5.7 What the host must do (both modes, unless marked)

| # | Obligation |
|---|---|
| H1 | J is never held across an await, I/O or any library lock. The fence takes no library lock at all, so J is a leaf. |
| H2 | The durable writes happen outside J, on the journal's own connection, with `synchronous=FULL` (§6). |
| H3 | Every permit has a deadline of grant + H, and the drain treats a permit past its deadline as ended, whether or not the library dropped it. |
| H4 | Revocation (the freeze) and the permit snapshot happen in one J step, before `lock_vault`'s first await. |
| H5 | Lease ids are 128-bit random. The journal stores the id, the wallet and the process nonce, and every check compares the wallet too. |
| H6 | **Catch-up at load and on reconnect**, in two parts. (1) Synchronously, after the journal is loaded and before any user flow or build can run: for every entry at `Dispatching` or `PreFence`, restore its row from the payload if `wallet.sqlite` lost it (Mode A, L16) and install a pending-spend fence on its inputs (L12; in Mode B, `TxDraft` coin control excludes tracked rows' outpoints). (2) After SPV has started (E0-05's bring-up), spawn, and do not await, an unscoped resume of every tracked `Built` or `Broadcast` row, so the journal decides (pin semantics in Mode B). Part 2 runs again on every SPV peers 0 → >0 transition and after the journal recovers from a write failure, so `Ambiguous`, `Deferred` and L13 rows are driven within the session (review Opus 11). Both parts run again when a closed wallet is opened. |
| H7 | Every engine call site that can hand off runs inside a `DispatchScope`, including hand-offs inside a task the engine spawns (`tx_actions.rs:414`). |
| H8 | Every way a lease enters the table (`begin_lease`, the background lease's creation and re-creation, `rebind`) first waits until no lock barrier is set, then reads `lock_gen` before it takes a token or signer. Its insert refuses if a barrier is set or `lock_gen` changed. An insert that finds only the vault epoch moved inserts `NeedsGrant` (§4.1, §8.3; GPT 2). |
| H9 | Every vault call that can end the epoch either is a revoking call with its own session method (§8.6), or runs through `vault_op`, which compares `Vault::epoch()` before and after (§4.3). |
| H10 | A resumable step's First writes its durable step marker before any transport (§7.6). A flow resumed in a later process reads `Refused{step_possibly_dispatched: true}` as MaybeSent, never Cancelled. The marker, not the flow's own phase, is the evidence (GPT 3). |
| H11 | **Fail closed.** No retry, no discard and no second funding of a step is offered unless `dispatch_status` of its artifact (or, in Mode B, of its funding, §2a.5) is `NotSent`. The one exception is the engine's funding gate (`discard_registration`), which also allows an asset lock's `None`: no entry, no marker and no tracked row (§16.6). The host reads every `None` as unknown, and a state transition's or a `TxDraft` send's `None` never allows a retry (reviews Opus r2 1a, DW-E0-08 r2 N-1). `discard_registration` consults it for the registration's funding; a top-up retry, a payment retry and "Register again" do too. `finish_asset_locks` is **not** gated: it resumes committed locks, which are never `NotSent` (review Opus r2 1a). |
| H12 | `DispatchResolved` (§16.7) is emitted for every provisional outcome when it settles, from the sources of §4.6. DP1-02 moves a row back to retryable only on `NotSent`; the payment and top-up UIs clear "may have been sent" on either outcome. P2a provides `note_seen` and `note_executed` but no production caller feeds them yet (the wallet's transaction-seen and confirmation paths, proved execution results): that wiring is **BL-76**, owned by P2b/E0-05 (review Opus high O-2). Until then a cut First stays `MaybeSent`, which fails safe. |
| H13 | Mode B: each library write call runs under a call permit, taken in a J step that checks the lease and charges the call's quoted budget. A funding call or a resumable step first has its marker written, with the `Committing` pattern (§2a.3). The engine's signer adapters count the signatures released per call permit. |
| H14 | **Every lock request runs its own vault gate** (review GPT r2 8). Each `lock_vault` call (async, the FFI's sync lock, the relock timer, auto lock) runs `vault.lock()` after its own freeze, and returns only after that gate. Concurrent requests share only the drain. An unlock (`NetworkSession::unlock`) waits while any lock gate is pending, so a lock request is always ordered before an unlock issued after its call. The epoch that a lock ends is always one that existed at or after its call. |
| H15 | Mode B: the statuses of §2a.5 are derived on every `list_tracked_locks` change, at load and at each call's end. Definite resolutions are written with their marker (`FULL`). |
| H16 | A row-less transition h's `broadcast_unknown` is settled only by positive evidence, in its own process, where the row-less entry keeps h's identity, nonce space and nonce n (review Opus r3 F-1). `Sent` needs h's proved execution result. `NotSent` needs both: (a) h's nonce slot is used by a different transition: this engine signed h′ ≠ h for the same identity, in the same nonce space (the identity nonce for withdrawals, transfers and key updates; the identity-contract nonce of the same contract for DashPay and DPNS documents) and with the same n, and holds h′'s proved execution result; (b) h did not execute, which (a) proves, since one slot admits one transition (a second use is `NonceAlreadyPresentAtTip` or `NonceAlreadyPresentInPast`). A fetched nonce past n is no evidence: at the pin n still executes while its slot is marked missing, up to 24 below the current value (`rs-dpp/src/identity/identity_nonce.rs:17`, `:99-176`; the identity-contract nonce uses the same window). A closed slot alone is none either, since h may have closed it. Without both, h stays `MaybeSent`. After a restart a resumable step has its marker (§7.6); any other transition has no record, so `dispatch_status` answers `None`, which is unknown. A retry is never offered on `None` or `MaybeSent` (reviews Opus r2 1a, DW-E0-08 r2 N-1; Part 5 `restart_retry`, `nonce_notsent`). |

### 5.8 Lock order and why nothing can deadlock

- The draft's design used a FIFO `RwLock` per lease. A task that held the wallet-manager guard G and then asked for a
  shared permit queued behind the drain's exclusive request, while the permit holder waited for G. That is a
  deadlock (model Part 3 finds it).
- In this design nothing in the fence waits on anything another task holds, except J:
  - admission is a non-blocking decision under J. `admit` does not wait for another caller's commit (it answers
    `Deferred`), and it does not wait for the journal load (it answers `Deferred`);
  - the debit is supplied by `register`'s caller, so `admit` never takes the wallet guard;
  - the only thing `admit` awaits is its own spawned journal write, which holds no lock;
  - the drain takes no lock at all, and only waits for permits to end or expire;
  - J is a leaf (H1).
- A permit holder that waits for G (dash-spv's mempool task takes it, F3), or for `payment_guard` (F14), delays only
  itself, and its deadline cuts it. So no wait-for cycle can include the drain or `admit`.
- Model Part 3 confirms that the design has no deadlock. It also shows that an `admit` which took the wallet guard
  would deadlock a caller that holds G behind a queued writer (tokio's `RwLock` is fair). That is why the debit moved
  to `register`.

## 6. The dispatch journal (persistence medium)

### 6.1 Why the host owns it

The draft kept the record in the asset-lock row. Each fix then needed a matching change in three places
(`AssetLockChangeSet::merge`, the SQLite upsert guard in `PWS/sqlite/schema/asset_locks.rs:140-150`, and Swift's
`persistAssetLocks`), plus a flush contract and a tombstone (M-B a–d). The row's wire format is positional bincode
that rejects trailing bytes, so even one new field needs a migration and a versioned blob.

A journal that only the fence writes avoids all of that:
- its writes are single statements the host controls;
- it is monotone by construction;
- it is durable on the host's own terms;
- it never travels through a changeset.

The library keeps its row exactly as at the pin. That row now serves funds tracking only, not the cancel promise.

### 6.2 Medium

- **File:** `<network dir>/dispatch.sqlite`, mode 0600, beside `app.sqlite`. It is a separate file because:
  - it needs a stricter durability setting than `app.sqlite`;
  - it must not take part in `.dwbackup` row export, which takes every table with a `wallet_id` column
    (`dw-appdb/src/rows.rs:69-89`);
  - its lifecycle differs: it is per network, opened at session open and closed with the session (§8.5), and its
    rows are erased per wallet only by a wiping `remove_wallet` (§6.5).
- **Code:** a module of dw-appdb (`dispatch.rs`) with its own embedded migrations. One connection behind a mutex,
  used only from the blocking pool.
- **Pragmas:**
  - `journal_mode=WAL`, `synchronous=FULL`, `busy_timeout=2000`;
  - `secure_delete=ON`, so an erased wallet's rows are overwritten (§6.5);
  - on Apple platforms, `fullfsync=ON` and `checkpoint_fullfsync=ON`, because macOS `fsync` does not flush the disk
    cache.
- **Version gate:** `meta.schema` is read at open. A schema newer than the build disables the fence (leased Firsts
  refused, `Notice{DispatchJournalUnavailable}`) rather than failing a CHECK on an unknown state (review Opus 10).
- **Schema:**

```sql
CREATE TABLE meta (k TEXT PRIMARY KEY, v BLOB NOT NULL) WITHOUT ROWID;
  -- schema, created_at, and seeded:<wallet hex> per wallet (§6.5)
CREATE TABLE dispatch (                                      -- registered artifacts (asset locks), Mode A
  wallet       BLOB NOT NULL CHECK (length(wallet) = 32),
  txid         BLOB NOT NULL CHECK (length(txid) = 32),
  origin_lease BLOB CHECK (origin_lease IS NULL OR length(origin_lease) = 16),  -- NULL for PreFence
  process      BLOB CHECK (process IS NULL OR length(process) = 16),           -- per-session nonce, diagnostics
  state        INTEGER NOT NULL CHECK (state IN (0, 1, 2)),  -- 0 Unsent, 1 Dispatching, 2 PreFence
  payload      BLOB NOT NULL,                                 -- AssetLockPayload, versioned (GPT 5)
  registered_at INTEGER NOT NULL,
  dispatched_at INTEGER,
  PRIMARY KEY (wallet, txid)
) WITHOUT ROWID;
CREATE TABLE step_log (                                      -- resumable steps' markers, append-only (DEC-154)
  seq       INTEGER PRIMARY KEY AUTOINCREMENT,
  wallet    BLOB NOT NULL CHECK (length(wallet) = 32),
  artifact  BLOB NOT NULL CHECK (length(artifact) = 32),     -- ST hash or txid (Mode A); call id (Mode B)
  kind      INTEGER NOT NULL CHECK (kind IN (0, 1, 2, 3)),   -- 0 marker, 1 NotSent, 2 Sent, 3 MaybeSent
  step_id   TEXT CHECK ((kind = 0) = (step_id IS NOT NULL)), -- e.g. "registration/<draft>/identity"
  at        INTEGER NOT NULL
);
CREATE INDEX step_log_artifact ON step_log (wallet, artifact, seq);
```

Schema 2 (DEC-154, review P2a r2 R2-F1). Schema 1 had a `step` table whose rows a definite resolution deleted;
it holds no resolution, so the migration at open copies every row into `step_log` as a marker, all standing, and
drops `step`, in one transaction.

**Reading the log.** A marker stands unless a later `NotSent` row of its artifact supersedes it. A `Sent` (or
`MaybeSent`) row is final evidence: every marker of its artifact stands whatever follows, and no row revokes it.
Recovery takes the rows in `seq` order. P2a writes no `MaybeSent` row; the kind is reserved for the funding marker
of Mode B (§2a.5).

- **Statements:**
  - `register`: `INSERT … ON CONFLICT DO NOTHING`, then a read-back. A row with another origin is an error.
  - `Dispatching`: `UPDATE dispatch SET state = 1, dispatched_at = ? WHERE wallet = ? AND txid = ? AND state = 0`,
    or a no-op if the row is already 1.
  - Step marker: an appended kind-0 row, `FULL`, before the step's transport (§7.6). A marker of the same step that
    already stands is not appended again.
  - Resolution: an appended `NotSent`, `Sent` or `MaybeSent` row (§7.6). A `NotSent` row is not repeated while it
    is the artifact's latest row; a `Sent` row is written once.
  - **The Sent bundle** (DEC-163, review P2a r4 R4-F1). Every `Sent` write of a marked artifact (P2a writes no
    `MaybeSent`) is
    one backend call, `resolve_sent`: in one `IMMEDIATE` transaction it appends each marker of the artifact's
    complete current set (those the engine holds durable included; the marker statement skips one that stands)
    and then the final row. No `Sent` row is ever written by itself for a marked artifact.
  - No statement sets `state` to 0, and no statement updates a `step_log` row.
  - **Compaction** (DEC-160, review P2a r3 R3-F1). There is one predicate: an artifact's `step_log` rows may be
    deleted only if it has no standing marker and no `Sent` or `MaybeSent` row (terminal: settled `NotSent`, and
    no copy can resend on them). Two statements apply it, each as SQL inside its own `DELETE … RETURNING`. The
    wiping `remove_wallet` of §6.5, over one wallet, runs it in its own `IMMEDIATE` transaction. The sweep, over
    every wallet, runs inside the open's transaction (SQLite's default, deferred, the one that also checks and
    migrates the schema), before the journal is handed to the engine, so no write of this session can race it
    (review P2a r4 R4-N1). Nothing checks the predicate in memory first, so an append is serialized with the
    delete: either it lands first and the predicate keeps the artifact, or the delete runs first and it is a new
    row. Because a `Sent` row lands only in one transaction with its markers (the bundle above), either order
    leaves the markers and the `Sent` row together.
    `dispatch` rows are deleted only by `remove_wallet`, under its own entry check (§6.5). So an entry outlives its
    row, which is why an asset lock's `None` means "never registered" (§16.6).

### 6.3 States

| In memory | On disk | Meaning |
|---|---|---|
| none | none | row-less artifact; or a row of unknown provenance (kept, not sent, `Notice`) |
| `PreFence` | 2 | the row existed when the journal was created; possibly sent, so Resend only |
| `Unsent{origin}` | 0 | registered, never possibly sent |
| `Committing(owner)` | 0 or 1 | a `Dispatching` write is in progress |
| `Dispatching` | 1 | possibly sent; only Resends from now on |
| `Ambiguous` | 0 or 1 | a write failed; it may still have reached disk |
| `Revoked` | 0 | refused or abandoned in this process; never possibly sent |

- **Monotone:** on disk, 0 → 1 only, and 2 is written only by the seeding. In memory, `Unsent` goes to `Committing`
  or `Revoked`, `Committing` goes to `Dispatching` or `Ambiguous`, and `Ambiguous` goes to `Committing` (retry). None
  of them goes back.
- **`Revoked` is not persisted** and does not need to be. At load every origin is dead, so an on-disk 0 is refused on
  its first `admit`, which is exactly what `Revoked` would do. A row brought back by the library's merge or upsert
  (M-B b) meets the same entry and is refused again. The journal decides, not the row.

### 6.4 Write failures

- **`register` fails:** the build aborts (L4). An entry that reached disk anyway, with no row, is harmless.
- **`Dispatching` fails** (including an fsync error, after which the frames may still be durable): the entry becomes
  `Ambiguous` and `admit` answers `Deferred`.
  - The transport is not called, and the flow reports MaybeSent.
  - In this process the artifact is resent only after a later write succeeds (`Ambiguous` → `Committing(retry)` →
    `Dispatching` → Resend). That retry needs no lease: the commit already happened, under its permit, before any
    lock.
  - At the next load the file decides. 1 means Resend. 0 means the transport was never called, because I1 guarantees
    that, so the row is refused and cleaned up.
- **A step marker's write fails:** like `Dispatching`. The step is `Ambiguous`, the transport is not called, and
  the flow reports MaybeSent. A retry first makes the marker durable.
- **The journal cannot be opened, or its schema is newer:** the fence refuses every registered-artifact `register`
  and every leased First, and raises a `Notice{DispatchJournalUnavailable}`, which maps to the common `storage`
  code (§16.4). Tracked rows get `Deferred`, so nothing is sent or cleaned up without the journal. Unleased paths
  still work.
- **After a write failure recovers** (the next write succeeds), H6's resume pass runs again (review Opus 11).

### 6.5 Load, loss, rollback, GC

- **Seeding** (Mode A), per wallet, through the library rather than PWS's schema (review Opus 10).
  - When a wallet is loaded and `meta` has no `seeded:<wallet>` row, the fence asks platform-wallet for the wallet's
    tracked asset locks (`list_tracked_locks`). That happens on the first session with the fence, after a deleted
    journal, and when a closed wallet is opened for the first time since.
  - For each lock whose status is not `Consumed`, it inserts a `PreFence` entry with its payload, built from the
    row's transaction. Then it sets `seeded:<wallet>`, all in one transaction.
  - No fence-era row of that wallet can exist yet: the fence admits nothing for a wallet before its seeding is done.
  - This depends only on the library's public row type, not on the `asset_locks` table's columns, labels or outpoint
    encoding.
- **Load:** at session open, after the seeding of the loaded wallets and before `start_wallet_subsystems` or any
  `admit`, the fence reads every row into memory. Until then `admit` answers `Deferred` and `register` fails (in
  practice no flow can run that early). The catch-up (H6) runs right after.
- **A row lost to a power loss** (GPT 5). `wallet.sqlite` runs `synchronous=NORMAL` (F5, DEC-65), so a power loss can
  drop a `Built` row whose `Dispatching` entry the `FULL` journal kept. The catch-up's part 1 then:
  1. restores the row from the entry's payload (L16);
  2. installs a pending-spend fence on its inputs (L12) before any build can select them;
  3. hands the row to part 2's resume, which resends it.

  An `Unsent` entry with no row needs nothing: its transport was never called (I1).
- **Wallet removal** (review Opus 10, F18).
  - dw's `remove_wallet` secure-erases the wallet's rows in `wallet.sqlite`.
  - Once the wallet has no tracked row left, a `remove_wallet` with its `Wipe` grant also deletes the wallet's
    `dispatch` rows and its `seeded:` mark, and the `step_log` rows that the compaction predicate (§6.2, DEC-160)
    selects, in one transaction with `secure_delete=ON`, then a WAL checkpoint (TRUNCATE).
  - Its `step_log` rows that the predicate keeps (a standing marker, or a `Sent`/`MaybeSent` row) survive the
    removal: they are evidence of something possibly on the wire, which the chain shows anyway. The engine forgets
    only the artifacts the delete returned; the standing steps, the evidence and the possibly-out row-less state of
    the rest stay, in memory as on disk.
  - **No erase window for evidence** (DEC-163, review P2a r4 R4-F1). The delete needs no pause around it: every
    `Sent` row is the bundle of §6.2, so whichever of the delete and the bundle commits first, the log ends with the
    markers and the `Sent` row together. Sightings, `NotSent` writes and row-less copies run during the delete as
    at any other time. An erase that commits and then reports an error changes nothing either: a later sighting's
    bundle rewrites the whole marker set, durable ones included, so the reopened log stands before any copy
    repairs it.
  - **What the removal keeps** (memory and lifecycle only). The removal barrier, and the join of in-flight
    `register` calls (DEC-134). The blocking delete is counted as a journal write, so closing the journal waits
    for it, and it applies its own outcome under J, so a removal dropped meanwhile still forgets what was deleted.
    The WAL checkpoint after the commit is best effort.
  - **The forgetting guard.** The engine forgets an artifact the delete returned only if nothing of it is newer
    than the delete: no `Sent` evidence (pending, owed or durable), no step marker in memory, and no row-less
    state that is `Admitted` or `Resolving`. Any of those may mean a write or a decision began after the delete read
    the log, and its state stays. A failed delete forgets nothing.
  - **Why copies do not defer during the delete.** A row-less copy resends only on marks memory holds durable,
    and those stand on disk unless a `NotSent` row superseded them; the predicate keeps every artifact with a
    standing marker. So the artifacts the delete returns are settled `NotSent` ones, which a copy cannot resend
    on, or `Resolving` ones (a `NotSent` write running, or failed and possibly committed), whose copies defer
    anyway. A copy's marker write, appended after the delete read the log,
    is a new row the guard then protects in memory.
  - A wallet removed while it still tracks a possibly-sent lock keeps those entries until the lock is consumed, and
    the removal says so.
  - If PWS's auto-backup is later restored, its rows meet no entry: kept, not sent, with a `Notice` (safe).
- **The journal is deleted or reset:** the next session seeds it again, so every current row becomes `PreFence` and
  is resent. That is safe for funds (nothing possibly sent is cleaned up). It can send an asset lock the user had
  cancelled, but only after someone deletes a file by hand.
- **The journal is rolled back** to an older copy by hand, with `wallet.sqlite` newer:
  - a row registered after the copy has no entry, so it is kept and not sent, with a `Notice` (safe);
  - a row dispatched after the copy reads `Unsent` and is refused, so its row is untracked even though it may be on
    the wire. DP1-05's asset-lock reconstruction recovers that lock from the chain (`RecoveredFromChain`).

  dw never restores this file, so this is the only residual, and it needs manual file surgery.
- **`wallet.sqlite` is restored or rolled back:** every row meets its entry, or has none and is kept. Safe both ways.
- **A way out for rows with no entry** (reviews Opus 15, GPT r2 13). Tools ▸ Repair "Unrecorded asset locks" lists
  them; they read `MaybeSent` throughout. Repair offers two actions:
  - **"Send it"** registers the row's transaction under a fresh lease (a prompt). The lock's owner is the user, so it
    commits as a First like any other.
  - **"Cancel it"** builds an engine `TxDraft` that spends one of the row's inputs back to the wallet (a prompt) and
    sends it.
    - Only once that spend is **ChainLocked** can the old transaction never confirm. Only then does the engine record
      the row `NotSent` (as `Revoked`) and run the ordinary cleanup.
    - Until then the row stays `MaybeSent`: a peer may still hold the old bytes. Negative observations, such as
      unspent inputs or a transaction absent from the local chain or mempool, prove nothing (F17's withheld-peer
      case).

  There is no "Discard" from negative evidence, and no force-forget that would produce `NotSent`, `Cancelled` or a
  retryable status. The same self-spend resolves a Mode B funding that lost its row (§2a.5): spending every coin
  conflicts with any inputs such a lock consumed.
- **GC:** none in 1.0 (Q10). A row is about 120 bytes plus its payload, one per asset lock ever built.

## 7. Commit points and crash consistency

### 7.1 Per artifact class

| Class | Commit | Possibly sent from | Resent by |
|---|---|---|---|
| Registered (asset lock) | start of the `Dispatching` write, under a First permit | that write | resume, deferred resume, load catch-up, dash-spv's rebroadcast (within the process) |
| Row-less Core tx (contact payment, M1 send) | grant of the First permit (or `FirstUnleased`) | the enqueue into dash-spv | dash-spv's rebroadcast set only; there is no load replay in dw (F8) |
| State transition | grant of the First permit | the first broadcast attempt | rs-sdk's retries inside the same permit only; a re-signed retry (F10) is a new artifact and a new First |

### 7.2 An asset lock, step by step

| Step | Durable afterwards | Crash right after: what the catch-up at load does (H6) |
|---|---|---|
| 1. sign (lease's funding signer) | nothing | nothing to do; inputs are free |
| 2. `register` (entry 0) | entry 0 | orphan entry; no row; nothing to do |
| 2a. (the payload is part of step 2's transaction) | entry 0 with payload | as 2 |
| 3. track `Built`, `store` inline (L3) | entry 0, row | row with entry 0 → refused → untracked (nothing of it reserved after load) |
| 4. readiness check; on failure `abandon` → cleanup | as 3, or removal queued | as 3 (a removal that did not land is refused again) |
| 5. `admit`: `Committing`, permit, charge (under J) | as 3 | as 3 |
| 6. `Dispatching` write in progress | entry 0 or 1 | 0 → refused (transport never called, I1); 1 → Resend |
| 7. write done | entry 1, row | inputs reserved again (L12), then Resend (MaybeSent) |
| 8. transport call (enqueue), bounded by the deadline | as 7 | as 7; the bytes may already be out |
| 9. transport returns or times out; `finish` | as 7 | as 7 |
| 10. status `Built → Broadcast` (`store`) | entry 1, row `Broadcast` | as 7, through the Broadcast arm (a Resend) |
| 11. proof wait (no permit), IS window (§4.4) | — | platform-wallet's tracking; DP1-02 resumes its flow |

**Power loss** is a separate case (GPT 5). `wallet.sqlite` runs `synchronous=NORMAL` (F5), so it can lose the row of
step 3, or its later status, while the `FULL` journal keeps entry 1:
- **Mode A:** load restores the row from the payload, fences its inputs and resends it (§6.5), whether or not the
  bytes ever left.
- **Mode B:** there is no payload. Once the lock reaches the chain, DP1-05's reconstruction finds it as
  `RecoveredFromChain`. Before then its inputs are not fenced; that is Mode B's documented residual (§2a.3).

An `Unsent` entry whose row was lost is a lock that was never sent (I1).

### 7.3 Row-less Core transactions

- `admit` decides from the origin (§5.4). There is no journal entry and nothing to persist.
- A First permit covers the enqueue, and its grant records the id in the row-less set.
- After a lock the lease is revoked. A first hand-off of new bytes under it is refused, and since nothing was
  enqueued, nothing can be rebroadcast. A repeat of bytes already admitted is a Resend (§5.5).
- A definite rejection settles the artifact unsent only when no other attempt of the same bytes is running and none
  may have let them out (§5.5). Only then are the inputs and the pin released, and the artifact is tombstoned.

### 7.4 State transitions

- The permit covers `StateTransition::broadcast` and its retries, all clamped to the deadline. An identical
  re-signed transition (F10) is a Resend through the row-less set.
- `wait_for_response` runs without the permit.
- A timeout is MaybeSent, and the flow keeps waiting for the proof to learn the outcome.

### 7.5 Re-dispatch paths

| Path | Reaches the fence as | Decided by |
|---|---|---|
| `resume_asset_lock`, Built arm (`recovery.rs:1176`) and Broadcast arm (`:1491`), all callers (F6) and dw's catch-up at load (H6) | CoreTx, `tracked_row`, usually unscoped | the journal (`PreFence` for rows older than the journal) |
| deferred-resume task (`recovery.rs:592`) | same; spawned by the library, unscoped | the journal |
| load replay (`load.rs:620`) | CoreTx, row-less, unscoped | never runs in dw (F8); if it did, it would get `Deferred` and a `Notice`: nothing sent now, and no false "definitely not sent" for bytes that were probably sent before |
| dash-spv's 600 s rebroadcast (F3) | not at all | an entry exists only after a fenced hand-off, so membership is the record |
| shielded redrives (F11, 1.1) | state transition, persisted before dispatch | must be registered artifacts, keyed by ST hash, before the shielded work ships (X-phase) |
| a resumed resumable step (§7.6) | the step's transition, scoped with its step id | the step marker: identical bytes are a Resend; other bytes under a revoked lease are `Refused{step_possibly_dispatched}` |

### 7.6 Resumable row-less steps (review GPT 3)

A registration's or top-up's identity transition from an existing lock is row-less, but a later process signs it
again (F10). The per-process set dies with the process, and the flow's own phase is written only after the library
returns, too late to record a hand-off. So:
- **Declaring.** The engine scopes such a step as resumable:
  `DispatchScope{step: Some("registration/<draft>/identity")}`.
- **Writing.** In Mode A, the step's First takes `Committing` for `(step, artifact hash)` in its J step, and spawns
  the marker's durable write (`FULL`). Its transport starts only after the write returned. In Mode B the engine
  writes the marker under the call permit, before it calls the library.
- **Every copy** (review P2a r1 F1). The durable marker is the precondition of transport for every copy of the
  transition, joined or not, whatever its scope. A copy waits (`Deferred`) while any marker of the artifact is
  `Committing`; a missing marker of its own step, or an `Ambiguous` one, is written first, and the copy is handed
  off only after every marker of the artifact is durable. The check is repeated after the copy's own write
  returns: a marker another copy began meanwhile is owed too.
- **Definite resolution** (review P2a r1 F2; DEC-154 after r2 R2-F1). The journal is append-only (§6.2). A
  definite not-sent outcome (refunded, or definitely unsent) appends a `NotSent` row of the artifact, before the
  refund and the `NotSent` tombstone. It supersedes the artifact's earlier markers: a later Lock, reload or resume
  finds none standing, so the step needs a fresh, charged First, whose marker is a new row. The row is begun in the
  J step that decides the resolution and written outside J. While it is written, and after the journal refuses it,
  the artifact is `Resolving`: it stays charged and possibly out (`MaybeOut`), and no copy resends on its old
  marker. **A write that fails or panics counts as possibly committed** (it may have landed and then errored):
  each later admit retries it first and defers meanwhile; once it lands, the artifact is refunded and `NotSent`,
  and a fresh First may follow. A settlement that waits on another copy's marker write (every attempt ended
  definitely unsent, but a write is `Committing`) proceeds when that write returns, whether it landed or not.
- **Sent evidence.** An artifact seen sent (a sighting, or an executed result) is never settled or refunded. At its
  first sighting the fence begins a `Sent` row for every marked artifact, in the sighting's J step, and holds the
  evidence `Committing` until it lands. The row is the bundle of §6.2 (DEC-163): the J step gathers the artifact's
  whole marker set, and one transaction outside J appends those markers and the `Sent` row. Every copy owes it like a marker: it defers while the row is `Committing`,
  and writes it first when an earlier write failed. A sighting during a `NotSent` write does not undo the write; the
  `Sent` row that follows wins over it, so no marker is deleted or restored. On disk the `Sent` row makes every
  marker of the artifact that was written stand again; in memory the markers the `NotSent` row superseded come back
  at the sighting, each as its own write left it (review r2 H1): one whose write returned ok is durable, any other
  is `Ambiguous`. The sighting's bundle writes each of them with the `Sent` row (a no-op for one that had landed),
  and once it lands they are durable. Copies are gated by the `Sent` row meanwhile. A `NotSent` row after a `Sent` row changes nothing.
- **A `Sent` row that failed** (lost, landed and then an error, or panicked) stays owed: a copy writes its bundle first,
  and without any copy it is begun again by the next sighting or the next journal write of the wallet that lands
  (review r2 M2). Only a journal that keeps failing until a crash can lose it; the next process then reads the
  markers as superseded (an accepted residual: DEC-154 (3) asks for the in-memory obligation).
- **Close** (review r2 L1). Closing the journal gives each owed `Sent` row a last try, stops new writes and waits
  for the running ones, `register` writes included (review Opus high O-5), so none of this session lands after the
  next session's sweep. A flow task registered after close aborted the tasks is aborted at once (O-8).
- **Wallet removal** (DEC-160, DEC-163). The removal compacts the wallet's log with the sweep's predicate inside
  its delete and forgets only what it erased and nothing newer (the guard of §6.5). An idle step that ended
  possibly sent keeps its standing marker through the removal, and a copy admitted during the erase Resends on
  that marker.
- **Reading, in a later process:**
  - identical bytes find their marker: a Resend;
  - different bytes for the same step, under a revoked lease, get `Refused{step_possibly_dispatched: true}`, which
    the flow reports as MaybeSent;
  - no marker means the step was never handed off: its write-ahead precedes any transport.
- **Failure.** A marker write that fails or is ambiguous behaves like a `Dispatching` failure (§6.4).

| Kill point | Durable afterwards | Next process |
|---|---|---|
| before the marker write | nothing | a fresh First, needs a live lease; refused → Cancelled (truly never sent) |
| marker write in progress | marker or nothing | as the row above, or as the row below |
| after the marker, before or after the broadcast | marker | identical bytes: Resend; otherwise MaybeSent |
| after the response, before the phase write | marker | as above; the flow's re-query then finds the identity |
| `NotSent` write in progress | marker, or marker and `NotSent` | `Resolving` in the old process; the next reads either |
| after a definite not-sent resolution | marker superseded by `NotSent` | a fresh First, charged again |
| after a sighting's `Sent` row | marker and `Sent`, whatever `NotSent` rows sit between | identical bytes: Resend; otherwise MaybeSent |

## 8. Lock, close and other revocations

### 8.1 `lock_vault`

```rust
pub async fn lock_vault(self: &Arc<Self>) -> Result<LockReport, EngineError> {
    let _op = self.try_enter()?;               // as today; a closing session handles leases itself
    self.cancel_relock();
    let done = self.leases.lock(Cause::Lock);  // (1) freeze + barrier, sync; spawns the coordinator
    Ok(done.await?)                            // (2)+(3) gate and drain, owned by the coordinator
}
// LeaseTable::lock (sync):
//   J { revoke every lease, drop every KeyHold and the background lease, S := permits,
//       lock_gen += 1, barrier := Barrier{gen, gate_done: false, drain_done: false} }
//   spawn coordinator { spawn_blocking(vault.lock()) → J{barrier.gate_done}; drain(S) → J{barrier.drain_done};
//                       when both: J{clear barrier}, notify; emit LockProgress::Done(report) }
```

1. **Freeze and barrier** (synchronous, before the first await). In one J step:
   - every lease that is not `Ended` becomes `Revoked{Lock}`, and every `KeyHold` is dropped;
   - the background lease is dropped;
   - the in-flight permits are snapshotted into `S`, and `deadline_max = max(p.deadline for p in S)`;
   - `lock_gen` is incremented;
   - the **lock barrier** is set.
2. **Vault gate.** `vault.lock()` ends the epoch and every grant issued before it, and waits for gated vault
   operations already running. It runs on the blocking pool under the coordinator.
3. **Drain.** The coordinator waits until every `p ∈ S` has dropped or is past its deadline. It wakes on the permits'
   notify or a timer, so it is not polling, and emits `LockProgress::Draining` while it waits.

**The barrier** (review GPT 2).
- It is cleared only when both the gate and the drain are done. Until then no lease enters the table (H8): not
  `begin_lease`, not the background lease, not `rebind`.
- So nobody can redeem a grant issued before the lock in the window after the freeze and before the gate ends its
  epoch. The coordinator is a spawned task, so a dropped `lock_vault` caller cannot leave the barrier set.
- **Every request runs its own gate** (H14, review GPT r2 8). A second `lock` while a barrier is set makes its own
  freeze, which revokes nothing new since no lease can have entered, and then **runs its own vault gate**. It shares
  only the drain. Without its own gate, a second lock that arrived after an unlock would return with the vault
  unlocked and a grant from before its call still redeemable (model `joined_lock`). The barrier clears when every
  pending gate and the drain are done.

**Callers.**
- `relock_after`'s timer and auto lock (§4.8) run the same steps.
- The FFI's `Vault.lock()` stays **synchronous** (review Opus 7; the Swift shells are frozen).
  - UniFFI runs it on the caller's thread (`dw-ffi/src/api/vault.rs:513`, review Opus r2 5f). It runs step 1, then
    runs `vault.lock()` **inline** as its own gate, and marks that gate done in the barrier. It never block-waits on
    the coordinator's `spawn_blocking` from a thread that may belong to the runtime.
  - It then returns the `VaultStatus`. The drain's end arrives as `LockProgress::Done(LockReport)` engine-side.
- The engine's async `lock_vault` (dwcli, tests, the chosen stack's binding in E0-13) returns after both.

### 8.2 The bound

Let `t0` be the moment of the freeze, which happens before `lock_vault`'s first await, so it is its call time.

- Every permit in `S` was granted before `t0` (J orders them), so its deadline is less than `t0 + H`.
- No permit is granted after `t0` under a lease that existed at `t0`, because all of them are revoked at `t0`.
- No lease enters the table while the barrier is set (H8), so no permit outside `S` can appear for the drain to wait
  on, and no grant issued before the lock can be redeemed after the freeze.
- The drain ends at the first instant at which every `p ∈ S` has dropped or passed its deadline. That is at most
  `max_{p∈S} deadline < t0 + H`. The host enforces this itself (H3), so a library that ignores its deadline cannot
  stretch it.
- The vault gate ends at `t0 + T_gate`.

So **`lock_vault` returns, and the barrier clears, by `t0 + max(H, T_gate) + ε`**, where ε is the timer's wakeup
latency (tokio's 1 ms granularity plus scheduling). This holds for any number of leases and permits, and for a gate
wait longer than H. The synchronous FFI `Vault.lock()` returns by `t0 + T_gate`.

- `T_gate` is the E0-03 gate wait: about 1 ms per in-flight signature, or one vault-file write and its read-back.
  H does not bound it.
- The tests assert the bound with paused tokio time, where ε = 0, and with real time at a tolerance of 250 ms.
- In ROADMAP and DASHPAY, "within H of the freeze" and "H plus one poll" both become "within `max(H, T_gate)` of the
  call, plus timer slack".

### 8.3 How the drain ends, and what happens around it

- **Every permit ended or expired:** the normal end. Permits that expired are reported MaybeSent.
- **The future is dropped mid-drain:** nothing changes. Revocation already happened at the freeze; the permits keep
  their deadlines; the vault gate finishes on the blocking pool. There is no admission state to reopen (M-C).
- **A second `lock_vault` during a drain:**
  - its freeze revokes nothing new (no lease can have been created, see the next item);
  - its snapshot is the subset of `S` still in flight, so it returns by the same `t0 + H`;
  - the second call therefore joins the first.
- **`begin_lease` across a freeze.** `begin_lease` reads `lock_gen` before it redeems anything, and its insert
  refuses if `lock_gen` changed (H8). So a lease whose redemption began before a lock's call and whose insert would
  come after it never exists. Its tokens were redeemed under the old epoch and are dropped unused, and the flow
  asks again.
- **`begin_lease` after a freeze and before the gate** (review GPT 2). It waits for the barrier, which clears only
  after the gate has ended the epoch. A grant issued before the lock is then dead, so its redemption fails. The
  barrier is table-owned and cleared by the coordinator, so a dropped future cannot wedge it. The wait is at most
  `max(H, T_gate)`.
- **Unlocking during a lock** waits while any lock gate is pending (H14). Taking the same exclusive vault gate
  would not by itself order an unlock queued before the lock's worker reached the gate, so the engine orders it
  explicitly. After the gates, an unlock proceeds.
  - A grant issued in the new epoch is usable, but only the lease made from it, and only after the barrier.
  - A later lock request runs its own gate and ends that epoch again.

  So "lock_vault returned" is a clean boundary for the UI (nit 4).

### 8.4 Outcomes (review GPT 7)

```rust
pub struct LockReport {
    pub status: VaultStatus,
    pub flows: Vec<FlowReport>,                  // every revoked lease
}
pub struct FlowReport {
    pub lease: LeaseSummary,                     // {id, flow, wallet}
    pub outcome: Outcome,                        // Sent | WillBeSent | MaybeSent | Cancelled
    pub artifacts: Vec<(ArtifactId, Outcome)>,   // per artifact, for multi-artifact flows
}
```

- **A permit says only what the drain waits for.** Every lease keeps a monotone history of its artifacts. Each grant
  of a First, each `Dispatching` write and each settlement is recorded under J, and nothing removes an entry.
- **Each artifact's outcome comes from that history and the journal, never from the permit snapshot:**
  - `Sent`: an attempt finished `Sent`;
  - `WillBeSent`: a registered artifact at `Dispatching`, `PreFence` or `Ambiguous` with its row tracked, which the
    engine resends. Every rejected-after-commit attempt (L13) lands here;
  - `MaybeSent`: committed, outcome unknown (an expired permit, a row-less `MaybeOut`);
  - `Cancelled`: never committed, or a row-less artifact settled `DefinitelyUnsent`.
- **The flow's outcome is the strongest of its artifacts'**, in the order `Sent` > `WillBeSent` > `MaybeSent` >
  `Cancelled`, so a flow is `Cancelled` only when every artifact is. Example: a registration whose funding lock is
  `Sent` and whose identity transition was never committed reports `Sent`, with the artifacts listed. The UI says
  C3, "Funds committed — finishing. Lock to stop; you'll finish after you unlock".
- A registration waiting for its proof, with no permit in flight, is therefore `Sent` or `WillBeSent`, never
  `Cancelled`.

### 8.5 Close

`NetworkSession::close` runs these steps in order, before its `gate.close()`:
1. the freeze with `Cause::Close`;
2. the drain (at most H);
3. abort every flow task registered with a lease (`flow_task`);
4. then the existing steps (gate, pump, manager shutdown).

Flows hold the session's `OpGuard` while they run, so the abort must come before `gate.close()`, which waits for
guards. A flow aborted after the drain is outside every permit, or inside an expired one. The library's drop handling
(sticky claims, `InBroadcastPin::drop`) then settles it as pending, and a committed asset lock is resent at the next
session. The journal is closed last, after the manager's shutdown: it is per network, so switching networks closes
one journal and opens the other. E0-05 owns the cancelation of the bring-up task. This design adds only the lease
steps.

### 8.6 Other revocations and epoch changes

| Event | Leases | Drain | Background lease |
|---|---|---|---|
| `lock_vault`, relock timer, auto lock (§4.8), FFI `Vault.lock()` (also on a vault already `Locked`) | all `Revoked{Lock}` | yes | dropped |
| close | all `Revoked{Close}` | yes, then abort flows | dropped |
| `change_passphrase` (vault stays unlocked; epoch ends) | all `Revoked{PassphraseChange}` (Q4) | yes | re-created |
| `remove_wallet` | that wallet's, `Revoked{WalletRemoved}` | yes (that wallet's permits) | that wallet's dropped |
| close a wallet (dash-qt "Close Wallet", QT-101) | that wallet's, `Revoked{WalletClosed}` | yes (that wallet's permits) | that wallet's dropped; H6 runs again when it is opened |
| unlock or scope change (epoch ends) | `NeedsGrant`, `KeyHold` dropped (Q5) | no | created or dropped by the new state |
| encrypt, recover, destroy | as `lock_vault` | yes | dropped |
| any other call that ends the epoch (caught by `vault_op`'s epoch check, H9) | `NeedsGrant` | no | re-created if the vault is prompt-free |

The revoking rows (lock, close, passphrase change, wallet removal, encrypt, recover, destroy) each get a session
method that freezes first: `lock_vault`, `close`, and new `change_passphrase`, `encrypt_vault`, `recover_vault`,
`destroy_vault` and `remove_wallet`. dw-ffi's `Vault` calls those instead of the generic `vault_op` (F15).

## 9. Why it is correct

### 9.1 Invariants

- **I1. Commit before hand-off.** The transport of a registered artifact is called only after its `Dispatching`
  write has returned success, in this process or an earlier one (`admit` returns `First` or `Resend` only from
  `Dispatching`).
- **I2. Commits only under a live lease.** `Unsent → Committing` and every row-less `First` happen in a J step that
  finds the origin lease live. Every lease that exists when `lock_vault` is called is revoked in that call's freeze, a
  J step. So no commit is ordered after a lock's call under a lease that lock revoked.
- **I3. Exclusive outcomes.** `Revoked` and `Dispatching` (or `Committing`, or `Ambiguous`) are reached only from
  `Unsent`, each by a compare-and-set under J. On disk, 0 → 1 only. At load, `Unsent` with a dead origin is refused,
  as `Revoked` would be. `PreFence` is never `Unsent`: it counts as `Dispatching` and is never cleaned up. A tracked
  row with no entry is neither sent nor cleaned up.
- **I4. A refusal is definite.** `Refused` or `Revoked{…}` ⇒ the entry was `Unsent` (I3) ⇒ no `Dispatching` write
  ever succeeded ⇒ no transport call ever happened (I1) and none can (I3). So the cleanup, which happens only on
  `cleanup: true`, never touches bytes that may be on the wire. For a row-less artifact a refusal is given only to an
  id that was never admitted in this process, or to a tombstoned one whose every attempt settled definitely unsent:
  a Core send's `Revoked`, or a state transition's `NotSent` under a dead lease (§5.5). A resumable step's earlier
  hand-off in an earlier process is visible through its marker, and its refusal says `step_possibly_dispatched`
  (§7.6).
- **I5. The drain is complete.** `lock_vault` returns only after every permit granted before its freeze has dropped or
  expired (H3, H4).
- **I6. One owner-guarded cleanup that cannot be dropped, and that frees the inputs.** Only the CAS winner cleans
  up, at most once per process. The cleanup is a spawned task, so dropping the caller cannot lose it, and a build
  dropped before `admit` abandons through its drop guard. The release takes the build's token, so it never frees
  another build's hold, and it settles the in-broadcast pin released, so the inputs are selectable again.
- **I7. Origins die with the process.** Lease ids are random and in memory only.
- **I8. The row is durable before its hand-off** (L3). That is for funds tracking, not for the cancel promise.
- **I9. No lease outlives the lock it raced.** Every lease in the table at a freeze is revoked by it. A lease whose
  creation began before a freeze is never inserted after it, and no lease is inserted while a lock's barrier (gate
  and drain) is set (H8).
- **I10. A row-less settlement is aggregate.** A row-less artifact is settled definitely unsent only when no attempt
  is running and none may have let it out. Every artifact settled so is tombstoned for its process: a Core send
  as `Revoked`, a state transition as `NotSent` (§5.5).
- **I11. Resumable steps are write-ahead.** Their transport follows a durable step marker (§7.6).
- **I12. A possible send survives a power loss** (Mode A). Every `Dispatching` or `PreFence` entry carries a payload
  that rebuilds its row and fences its inputs at load (§6.5).
- **I13. A rebind never raises authority.** Each generation's ceiling is the minimum of the previous generation's
  available amount and the fresh grants' cap, and refunds never lift a later generation (§4.2, §4.3).
- **I15. Every lock request ends an epoch that existed at or after its call** (H14). So no grant issued before a
  lock's call survives that lock.
- **I16. No retry without positive evidence.** A retry, discard or second funding needs `NotSent`, and `NotSent` is
  never inferred from negative observations or from a row-less artifact's absence. Only an asset lock's absence (no
  entry and no tracked row) passes the engine's funding gate, because its entry precedes every transport (I1) and
  outlives its row (H11, H16, §6.2, §16.6).
- **I14. Lock outcomes come from history.** `Cancelled` requires that nothing of the flow was ever committed, or that
  every committed artifact settled definitely unsent (§8.4).

### 9.2 The ordering theorem

> **Mode A.** After `lock_vault` returns, no artifact signed under a lease it revoked makes a hand-off whose commit
> was not ordered before the `lock_vault` call. Every hand-off whose commit was ordered before the call has ended
> (Sent or NotSent) or is reported MaybeSent or WillBeSent.
>
> **Mode B.** After `lock_vault` returns, nothing is signed under a lease it revoked, and no library call under such
> a lease starts. A call already running, or a row it left, may still hand off bytes signed before the call. Every
> such call is reported MaybeSent unless it returned.

- **Proof.** By I2, every commit under a revoked lease precedes the freeze in J's order, and the freeze happens at the
  call. A First's commit is the grant of its permit. If that permit's hand-off had not ended by the return, I5 means
  it was past its deadline, so it is reported MaybeSent. A registered artifact's later Resends (resume, load) all
  follow its `Dispatching` write (I1), which belongs to that same commit.
- **Mode B proof.** I2 and I9 hold for call permits as for First permits. The vault gate ends the epoch, so every
  signer issued before fails `Locked` (E0-03). A call already running is either done or past its permit's deadline
  when the drain ends.
- **Corollary (Lock to cancel).** A flow whose First (Mode A) or call (Mode B, before it signed) is refused reports
  Cancelled. By I4 nothing of it ever leaves, and its row, reservation and pin are released once, by a cleanup
  nothing can drop (I6). Once funds are committed, Lock pauses the flow; it cannot cancel it (§4.6).

### 9.3 Liveness

- **A genuine possible dispatch is never lost.** An entry at 1 or 2 is resent by every resume, and by the catch-up
  at every load (H6), with its inputs reserved again (L12). Cleanup, which needs `Unsent`, can never reach it (I3).
- **A never-dispatched row is cleaned up** at once by the CAS winner in this process, or by the catch-up at the next
  load (H6), since its lease died with the process.
- **Provisional outcomes resolve within the session.** H6's resume pass re-runs on every peers 0 → >0 transition and
  after a journal write recovers, and every resolution emits `DispatchResolved` (H12).
- **No deadlock** (§5.8).

## 10. The model check

### 10.1 What it models, and its limits

`docs/design/checks/e0_04_design_model.py` (86 checks):
- **Part 1** explores every interleaving of these actors:
  - O, the original flow, which may be dropped between track and admit, and may repeat its hand-off;
  - R, a resume of the same row, or in the row-less scenarios a second holder of the same bytes; after a crash, R
    is the catch-up at load (H6);
  - K, a spawned cleanup task;
  - W, an orphaned journal write;
  - L, `lock_vault`;
  - B, another build that may take freed inputs after a reload;
  - one crash, or one power loss, with every partial outcome, and the reload.

  The twelve scenarios:
  - `flow`;
  - `flow+crash`;
  - `flow+power`, where `wallet.sqlite` may lose rows the journal kept;
  - `restart`;
  - `ambiguous`;
  - `legacy`;
  - `rowless`, a state transition with a repeat;
  - `rowless-resume` and `rowless-resigned`, a resumable step across a crash, with identical or new bytes;
  - `rowless-core`, a `TxDraft` with inputs;
  - `rowless-concurrent`, GPT 1's second holder;
  - `unknown`.
- **Part 2** is a tick simulation of the bound with up to three leases. It also checks the lease-creation race, the
  window between the freeze and the gate (GPT 2), and the rebind cap (GPT 6).
- **Part 3** explores the lock order: the draft's permit lock, the design's non-blocking admit, and an admit that
  would take the wallet guard.
- **Part 4** explores an own-key registration's key hold against the ChainLock fallback, under rev0, Mode A and Mode B.
- **Part 5** is Mode B. It covers:
  - one funding call with power loss, a withholding peer, the catch-up's resend, DP1-05's discovery, Repair's
    self-spend and the user's choice to fund again, under rev1's "no entry means retry" and rev2's status
    derivation;
  - registration step 2's ChainLock-height retry with its `Locked` classification and its copy, after a manual
    lock and after an auto lock (C5);
  - a retry offered after a restart. `restart_retry` fixes the answer at `None` and checks how the host reads it,
    not how the answer is derived;
  - the row-less tombstone, the asset-lock reading of `None` in both modes, and the nonce evidence for `NotSent`.

  It also checks that rev2 never strands a definitely-unsent funding.
- **Part 6** is the step-marker journal (DEC-154, review P2a r2 R2-F1): one marked artifact whose First ended
  definitely unsent, its `NotSent` write, a sighting that may overtake it, one copy and a crash; every journal
  write lands, is lost, or lands and then errors. In two of its three starts a second step's marker was begun
  before the settlement and its write failed (lost, or landed then an error), and the copy is of that step
  (review r2 H1). The oracle reads the log as recovery does, where a `Sent` row makes only written markers stand:
  a copy's transport needs its step's marker standing on disk, and after a crash the copy's marker must stand if
  it went out, and the first marker if the process held the evidence durable. rev1's delete-and-restore fails
  both. So do five rejected rules: a copy that skips the `Sent` row, a failed `NotSent` write taken as
  uncommitted, a sighting kept only in memory, "latest row wins" letting `NotSent` revoke `Sent`, and superseded
  markers brought back durable whatever their write did. DEC-154 passes.

**The oracle** (review GPT 8). Every definite verdict (`Cancelled`, `Failed`, `NotSent`) is judged against the
immutable send history, the attempts still running and the journal, never against the bookkeeping under test. The
`forget-rowless-history` mutation that escaped rev0 is caught.

Atomic steps:
- each J critical section is one step: `admit`'s decision, `abandon`, the freeze, a permit drop;
- each durable write is one step with three outcomes: ok, failed, and failed but durable;
- each transport call has its outcomes: sent, rejected, deadline passed, then bytes leaving late or never.

Limits. What the model does **not** cover:
- more than one registered artifact, lease or crash;
- more than two concurrent row-less callers;
- the library's internals (claims; the in-broadcast pin is modelled only as "pinned");
- budgets and caps, beyond the rebind check: they are plain arithmetic under J, covered by unit tests (§12);
- epoch changes other than the lock, the background lease and the facade's lease handle (unit tests);
- Mode B's call permits beyond Parts 4 and 5. They follow the same freeze and drain rules as First permits, which
  Part 2 covers;
- the vault gate's own ordering, which E0-03's lock-race tests cover.

The claim is therefore: within these state spaces, every property of §10.2 holds and every listed wrong rule is
caught. It is not a proof about the code.

Some checks are confirmations rather than discoveries, and they are labelled as such. In Part 2, "no First after
the call", "a second call joins", the creation race and GPT 2's gate window follow from revocation at the freeze and
from H8, so the design passes them by construction; their teeth are the rejected rules, which fail them. Part 1's
"deadlock" check ignores only the step that drops `lock_vault`'s future.

It is a model of this design's decisions, not a test of the code.

### 10.2 Property → check

| Property | Check (violation text) |
|---|---|
| I2, theorem | "a commit after lock_vault was called"; "a first actual send after lock_vault returned" |
| I1 | "a hand-off without a durable Dispatching record" |
| I8 | "a hand-off before its row was durable" |
| I4 | "a possibly-sent row cleaned up"; "reported not sent, but sent or still sendable" (also for a row-less repeat) |
| I6 | "the cleanup ran twice in one process"; "another build's reservation released"; "a never-dispatched row left reserved" (a dropped build or caller) |
| I3 | "an artifact both Revoked and Dispatching" |
| I5 | "lock_vault returned while a hand-off it waits for ran". It is judged from the hand-offs running at the freeze, recorded at the freeze itself (a First still in its write or transport and not past its deadline), not from the permit set. |
| I7 | "a commit under an origin from an earlier process" |
| I9 | Part 2: a lease begun before the call and inserted after it |
| H6, L12 | "inputs of a possibly-sent row left selectable after load"; "a genuine possible dispatch was not resent" |
| liveness | "a never-dispatched row left reserved"; "a genuine possible dispatch was not resent" (ambiguous and pre-fence rows); "deadlock" |
| unknown provenance | "a hand-off without a durable Dispatching record"; "a row of unknown provenance cleaned up" |
| I10, I11 | "reported not sent, but sent or still sendable" in the row-less and resumable scenarios |
| I12 | "inputs of a possibly-sent row left selectable after load" (with or without its row); "a genuine possible dispatch was not resent" after a power loss |
| I13 | Part 2 `rebind_cap` |
| I14 | "the lock report says Cancelled for a possibly-sent flow" |
| I6 (the pin) | "inputs of a definitely-unsent artifact left unselectable" |
| bound | Part 2: the drain ends within H for 1–3 leases; the rejected timing rules fail |
| lock order | Part 3: no reachable state without a step, for the design's admit |

### 10.3 Replays

- **The r4 M1 counterexample.** Under the r3 rule (kind from the call path, the pin's claim exclusion), the model
  admits it and flags "a first actual send after lock_vault returned". The design admits no such trace. Its
  counterpart ends with nothing sent, the row gone, the reservation released once and O reporting Cancelled.
- **The M-A interleaving.** O dispatches, the lock comes, and only then does the resume act. Under the design the
  resume's admit reads `Dispatching` and resends, and the inputs stay reserved.
- **Barrier 1.** Nothing handed off; Cancelled; row gone.
- **Barrier 2.** `lock_vault` cannot return before the hand-off; Sent.
- **Barrier 2 with a stall.** Returns at the deadline; MaybeSent; late bytes are allowed.

### 10.4 Mutations (each must be caught)

| Mutation | Stands for | Caught as |
|---|---|---|
| `r3` | kind from the call path; claim exclusion | hand-off without a durable record (restart) |
| `split-literal` | read, fence and write as separate steps (M-A) | possibly-sent row cleaned up |
| `no-override` | a refusal does not override another party's claim | never-dispatched row left reserved |
| `no-cleanup` | a refusal leaves the row | never-dispatched row left reserved |
| `strict-resend` | a Resend needs a live lease | ambiguous dispatch not resent |
| `check-then-act` | lease check and permit in separate steps | commit after lock_vault was called |
| `transport-first` | the transport before the record is durable | hand-off without a durable record |
| `clean-on-write-fail` | a failed write leaves a clean `Unsent` (M-B c) | reported not sent, but still sendable |
| `legacy-unsent` | a `PreFence` row treated as never sent (M-B d) | possible dispatch not resent; row cleaned up |
| `no-entry-resend` | a row with no entry resent (trusting the call site's `tracked_row`) | hand-off without a durable record |
| `abort-direct` | an aborted build releases without the fence | possibly-sent row cleaned up |
| `no-flush` | hand-off before the row is durable | hand-off before its row was durable |
| `revoke-in-drain` | revocation at the drain's end; a drop reopens (M-C) | commit after lock_vault was called |
| `no-drain-wait` | return without waiting for the snapshot | lock_vault returned while a hand-off ran |
| `lease-reuse` | per-process lease ids (m-1) | commit under an earlier process's origin |
| `unguarded-release` | release without the build's token | another build's reservation released |
| `rowless-no-memory` | a row-less repeat under a revoked lease is refused | reported not sent, but sent |
| `no-rereserve` | the catch-up leaves a possibly-sent row's inputs free | inputs left selectable after load |
| `no-drop-guard` | a build dropped before admit keeps its row | never-dispatched row left reserved |
| `inline-cleanup` | the refused caller cleans up inline | never-dispatched row left reserved (caller dropped) |
| `every-refuser-cleans` | every refused caller cleans up | the cleanup ran twice in one process |
| `permit-ends-at-first` | the permit is dropped when the transport starts | lock_vault returned while a hand-off ran |
| `rowless-keep-on-notsent` | a row-less id stays in the set after a definite not-sent | a first actual send after lock_vault returned |

| `forget-rowless-history` | GPT r1's escaping mutation: a rejected row-less repeat forgets earlier hand-offs | reported not sent, but sent |
| `rowless-rev0` | rev0: a row-less id is forgotten on one attempt's own rejection (GPT 1) | reported not sent, but still sendable |
| `pin-left-pending` | rev0: the cleanup leaves the in-broadcast pin pending (Opus 5) | inputs of a definitely-unsent artifact left unselectable |
| `snapshot-outcome` | rev0: Cancelled for a flow with nothing in flight (GPT 7) | the lock report says Cancelled for a possibly-sent flow |
| `no-step-marker` | rev0: no write-ahead marker for a resumable step (GPT 3) | reported not sent, but sent |
| `no-payload` | rev0: no recovery payload for a lost row (GPT 5) | inputs of a possibly-sent row left selectable after load |

- `split-cas`, a split read and act whose act is a compare-and-set, passes. It is what the single J step guarantees.
- Part 2 fails each of `sequential`, `transport-deadline`, `host-waits`, `drop-reopens` and `no-gen-check`, and
  checks GPT 2 (`gate_window`) and GPT 6 (`rebind_cap`) under rev0 and rev1.
- Part 3 finds the draft's deadlock, and the one an `admit` taking the wallet guard would cause.
- Part 4 (own-key registration and the ChainLock fallback) finds rev0's key held through a hidden wait and its failed
  funded registration. It finds nothing in Mode A, and exactly the documented residual in Mode B.

**rev2's reproductions.** Each finding has a check that fails under rev1's rule and passes under rev2's:

| Finding | Check |
|---|---|
| GPT r2 8 | Part 2 `joined_lock` |
| GPT r2 9 | `rebind_ceiling` |
| GPT r2 13 | `repair_resolution` |
| DEC-67 | `quickunlock_sum` |
| GPT r2 10, 11 | Part 5 `step2_findings` |
| GPT r2 12, Opus r2 2 | `f_findings` |
| Opus r2 1a | `restart_retry` |

**GPT r1's reproductions** (majors 1, 3, 5 and 7) replay the reviewer's exact traces. Under the rev0 rule each one
breaks, each is admitted and ends in a violation; under rev1 it is not admitted, and its scenario explores clean.
Majors 2, 4 and 6 are the Part 2 and Part 4 checks above.

The three draft inputs stay unchanged as history. The design check replaces `e0_04_dispatch_model.py` as the spec
check DASHPAY §2.6 names.

## 11. The upstream PR (B5, Mode A)

- **Content:** one PR against platform `v5.1-dev`, in two commits.
  1. **rs-sdk:**
     - the `DispatchFence` trait and types, and `DispatchScope{wallet, origin, step}`, in a small module that
       platform-wallet re-exports;
     - `Sdk::with_dispatch_fence`;
     - the hook inside `broadcast_with_retries`' closure, with per-attempt clamping and the nonce refresh on refusal
       (L19).
  2. **platform-wallet:**
     - the fence-aware `SpvBroadcaster` with `DispatchContext` at every F1 site, and the SPV split (L18);
     - `DapiBroadcaster`'s clamped retries;
     - `register` with the recovery payload;
     - the synchronous `abandon` and its drop guard owning the pin;
     - the spawned cleanup with the token and pin kept on the in-memory row;
     - the load-time pending-spend fence (L12) and `restore_tracked_lock` (L16);
     - `proof_wait_started`, and `FallbackPolicy::Surface` (L17);
     - the obligations L1–L19 with their tests.

  With no fence installed, behaviour is unchanged. The PR is L (review Opus 6).
- **Carriage:** the desktop carries it as a cherry-pick onto its pin branch (DEC-18), next to E0-11's carried branch.
  The PR is opened only with pasta's go-ahead (B5).
- **Without it** the design runs in Mode B (§2a.3), which is a complete, closable design, not an interim.

## 12. Test plan

All tests follow the vault race tests' style. They use a recording fake transport, and real-time stress runs use the
abortable rendezvous from E0-03 `a79b3a9`. Each group is marked **[A+B]** (both modes) or **[A]** (Mode A only).

**[A+B] Lock barrier and outcomes (rev1)**
- **Barrier** (GPT 2):
  - with zero permits, a delayed blocking pool, a gate wait longer than H, a dropped `lock_vault` caller and a
    second lock, a `begin_lease` started after the freeze and before the gate waits for the barrier, and its
    pre-lock grant then fails to redeem;
  - no First under it after `lock_vault` returned;
  - the background lease's re-creation and `rebind` wait the same way.
- **Outcomes from history** (GPT 7): a registration with its funding `Sent` and its identity transition uncommitted,
  locked during the proof wait, reports `Sent` (with the artifacts listed), never `Cancelled`. A rejected-after-commit
  asset lock reports `WillBeSent`.
- **Rebind** (GPT 6, GPT r2 9):
  - smaller, zero, wrong-purpose and partial fresh grants: each purpose's ceiling becomes the minimum; charges and
    permits stand; a new 100-credit transition under a fresh 1-credit grant is refused;
  - an older charge refunded after the rebind does not lift the new ceiling, through a second rebind and with mixed
    purposes.
- **DEC-67:** with "require authentication for every payment" on, an unlock mid-flow moves the row to `Authorize`
  and the rebind takes a fresh grant; with it off, the rebind is silent and capped. Touch ID: an "Accept and pay" set
  at the limit is issued, and one duff above the combined value asks for the passphrase.
- **Joined locks** (GPT r2 8, H14):
  - K1, its gate, an unlock, a grant, K2 during the drain: K2 runs its own gate, and the grant fails to redeem
    after K2 returns;
  - the same with the synchronous FFI lock as K2, and with K1's caller dropped;
  - an unlock queued before K1's gate runs after it.
- **No double pay** (Opus 2, Opus r2 1a, 2):
  - a restart after a withdrawal's `broadcast_unknown`: `dispatch_status` is `None`, and no retry is offered (H16);
  - in one process, a withdrawal definitely rejected: `dispatch_status` is `NotSent` (the tombstone) and a retry is
    offered. The retry signs identical bytes and is admitted, and from then on the answer is `MaybeSent`, then
    `Sent`;
  - an asset lock with no entry: `None` allows a second funding only with no tracked row. With a `Consumed` row
    after a deleted journal it is `Sent`; with a `Built` row after a rolled-back journal it is `MaybeSent`;
  - `funds_committed` is false for a live `Unsent` funding (which reads `MaybeSent`) and for a `Revoked` one (which
    reads `NotSent`), and true from `Committing` on;
  - Mode B, a deleted `dispatch.sqlite` under a registration whose lock is `Broadcast`: the row matches the
    draft's own funding key, the funding reads `WillBeSent`, `funds_committed` holds and "Register again" is
    refused (Opus r3 F-2);
  - a withdrawal W1 (nonce n) ends `broadcast_unknown`, then a later transition takes n + 1: W1 stays `MaybeSent`
    and no retry is offered; only a transition this engine signed with nonce n, proved executed, makes W1 `NotSent`
    (Opus r3 F-1);
  - Mode B: a top-up's `broadcast_unknown` with its row `Built`: `WillBeSent`, no retry. With its row lost to a
    power loss and the lock withheld by a peer: `MaybeSent`, discard refused, until Repair's self-spend is
    ChainLocked;
  - `finish_asset_locks` resumes committed locks without a journal gate;
  - a top-up whose lock is `Ambiguous`, then a re-query that shows nothing: no retry offered until `DispatchResolved`;
  - a `MaybeSent` registration before `FundingSent`: `discard_registration` refused until `dispatch_status` says
    `NotSent`;
  - a reload that refuses and cleans up a never-sent row emits `DispatchResolved(NotSent)` and the row becomes
    retryable.
- **Mode B step 2** (GPT r2 10, 11): a signer `Locked` before the first signature is Cancelled. Between ChainLock-height
  attempts, after an IS submission, and with an earlier marker of the step present, it is MaybeSent and the flow
  parks. While the call runs the UI shows C2, through a signed-before-broadcast barrier.
- **Repair** (GPT r2 13): a withheld peer holds the row's transaction. "Cancel it" leaves the row `MaybeSent` until
  the self-spend is ChainLocked, and only then cleans it up; nothing is ever declared `NotSent` from negative
  observations.
- **Auto lock** (Opus r2 5d, GPT r3 F2):
  - it is not armed on a `Locked` vault, so an own-key flow survives inactivity there;
  - during funded step 2, between signing and submitting S1: C5 shows DEC-67's promise, then "Locked automatically
    — sent" once S1 executes;
  - after a ChainLock-height back-off with S2's signer `Locked`: "Locked automatically — may have been sent; unlock
    to finish", never a plain "unlock to finish", and S1 executing late moves it on.
- **Lock UX** (Opus 3, Opus r2 5):
  - copy keyed on `funds_committed`;
  - the "key held" lock state with an enabled Lock action;
  - `Vault.lock()` on a `Locked` vault revokes an own-key lease;
  - the lock screen shows before the drain ends.
- **The synchronous FFI lock:** it returns after the gate; `LockProgress::Done` carries the report.
- **The facade lease handle** (Opus 4): "Accept and pay" through `begin_flow` across `accept_request` and `prepare`,
  with the Spend part used after a 90 s accept; the idle reaper ends an abandoned vault-key lease.
- **Lease ownership** (review P2a r1 F6): dropping the sole owning handle ends the lease, and a new First under it
  is refused; a handle looked up by id and dropped leaves the lease `Active` while its owner lives; a `begin_flow`
  lease stays `Active` with no host handle until `end_flow`.
- **QuickUnlock for `PlatformOp`:** within and just above the limits, and with a stale passphrase.
- **Marker log** (DEC-154, review P2a r2 R2-F1), against a file-backed `dispatch.sqlite` with gated writes: a copy
  defers while a sighting's `Sent` row is held (Sol's blocked restore); a lost `Sent` row is written by the copy
  before its transport (failed restore); a `NotSent` row that lands and then errors, overtaken by the sighting,
  loses nothing and the evidence survives a reopen (delete committed then error); the reopened journal reads
  `Sent`, Resends identical bytes and refuses others with `step_possibly_dispatched`. A failed `NotSent` write
  against the fake journal keeps the artifact `Resolving{failed}` and copies defer until a retry lands. Review r2:
  a superseded marker whose write failed comes back `Ambiguous` and the copy of its step writes it (H1); a failed
  `Sent` row (lost, landed then an error, panicked) is begun again with no copy (M2); another wallet's sighting is
  not this one's (L2); closing the journal waits for a running `Sent` write (L1). `dw-appdb` checks supersession,
  finality of `Sent`, the sweep and the v1 migration. The stress checker logs supersession and evidence, carries
  which markers were ever written, and flags a copy on superseded markers (I11); after the checker, a step copy of
  a row-less artifact is checked against the fake journal's own `standing()` at each transport. The run sights
  artifacts during and after their settlement. The `SkipEvidence`, `FailedUncommitted` and `NoSentRow` mutations
  are each caught by the stress.
- **Wallet removal** (DEC-160, DEC-163), against the file-backed journal with a held erase: Sol's r3 and r4 probes
  are regressions. An idle possibly-out step keeps its log rows and its standing marker through removal and a
  reopen; a copy admitted while the erase runs Resends with its marker standing at the transport and after a
  reopen. A sighting's bundle held across the erase lands its marker again with the `Sent` row, and the engine
  keeps its state. A bundle overtaken by its own `NotSent` row and then the erase still lands its marker. After an
  erase that commits and then reports an error, a sighting's bundle alone makes the step stand: the journal is
  reopened before any copy, and identical bytes Resend while different ones are refused with
  `step_possibly_dispatched`. Closing the journal waits for an erase whose removal was dropped. `dw-appdb` checks
  that the bundle never lands without its markers over every interleaving of a `NotSent` row, the erase and the
  bundle, and that splitting it into statements can. `dw-appdb` also checks that the removal returns and erases exactly the
  predicate's set, and compares the SQL predicate with the Rust one over 300 random logs. The stress removes the
  flows' own wallet as well as the second one while row-less step copies run; the checker flags an erased artifact
  whose marker it still reads durable or whose `Sent` row was durable before the erase began (I11); the disk oracle checks each step copy's marker against the fake
  journal, and at the end that no `Sent` row lies on disk with the markers it had erased. `EraseAll` (the
  unconditional delete) is caught by the stress and by five of the regressions, Sol's probes among them. Three
  mutations whose races the stress hits too rarely are caught by the regressions: `BundleOwedOnly` (the bundle
  skips markers the engine holds durable, the 92d676b behaviour) by three, Sol's r4 probe among them;
  `SplitBundle` (markers and `Sent` row as separate statements) by two; `ForgetNewer` (no guard) by two.
- **Opus high review**: Opus's O-1 probe, inverted, is a regression. The same bytes admitted for a second wallet
  while the first wallet's attempt runs (or after it ended possibly sent) are `Deferred`; each wallet's Spend charge
  is refunded once, by its own settlement, and another wallet's lease cannot unbind it. Another wallet's standing
  marker from an earlier process, and a copy's marker write before its resend, hold the bytes back too (validator
  N-1). `ArtifactKeyed` (row-less state keyed by artifact alone, so a second wallet's copy joins the first's entry)
  is caught by both; the stress draws artifacts per wallet. The reaper ends a lock-revoked lease an idle period after
  the revocation, even one untouched long before it, and forgets it after another (O-3); closing the journal waits for a running
  `register` write (O-5); a task registered after close is aborted (O-8). DEC-134 (3)'s engine test makes the
  vault's deletion fail and checks that the wallet stays listed and usable, its grant is consumed and its flow's
  lease is revoked.

**[A+B] Row-less attempts and step markers (rev1)**
- **Concurrent holders** (GPT 1): O's First and R's Resend of the same bytes; O definitely rejected, the lock comes,
  O repeats. That repeat is a Resend, nothing is released, and R's send is never reported Cancelled. When both are
  definitely rejected the artifact settles unsent once and the inputs are selectable again.
- **Resumable steps** (GPT 3), for each kill point of §7.6: the next process's identical signature is a Resend; a
  different signature under a revoked lease reads MaybeSent.

**[A] Power loss** (GPT 5), as a separate harness from the kill matrix: drop `wallet.sqlite`'s WAL tail after a
`Dispatching` commit. Load restores the row from the payload, fences its inputs before a conflicting payment can
select them, and resends; a withheld-then-released lock confirms once.

**[A] Surfaced fallback** (GPT 4): a fake library whose step 2 gets an IS-proof rejection 20 s into the window
returns `ChainLockFallbackRequired`. The engine parks at once, the key is dropped, and the continuation signs only
under a new lease. In Mode B the same fake keeps the hidden wait; the key drops at `key_until`, and a signer
`Locked` maps to Parked.

**dw-vault (P1)**
- `PlatformOp` caps:
  - `platform_signer` refuses `PlatformFunding{max_duffs + 1}`;
  - a token with `max_credits = 0` gets no `PlatformIdentity`;
  - a token with `max_duffs = 0` gets no `PlatformFunding`.
- `scan_key` refuses a `PlatformOp` token and accepts `IdentityScan`, in every vault state of the credential table.
- `authorize_set`:
  - one passphrase check (counted by the throttle as one);
  - one grant per purpose;
  - all or nothing when a purpose is not allowed for the credential.
- `KeyHold`: dropping it erases the key while a signer clone is held. The clone then fails `Locked`, and an operation
  in flight finishes. Lock-race style: 16 signers, 300 drops.

**Lease table (P2)**, with paused tokio time unless noted:
- The state machine of §4.3, transition by transition, including `NeedsGrant` and `rebind`.
- Budgets:
  - an engine-built BIP44 spend over the remaining `Spend` budget is refused before signing ("a lease cannot sign a
    BIP44 spend above its cap");
  - a fenced Core First over the `Funding` budget is refused, with Revoked and cleanup;
  - a row-less artifact settled `DefinitelyUnsent` is refunded; one attempt's `NotSent` while another attempt runs
    is not.
- `DashPayCrypto` signs no transaction and no state transition: every hand-off purpose is refused, and the scope
  refuses signing (E0-03 tests stay).
- Background lease:
  - created in `Unlocked` and `Unencrypted`, absent in `UnlockedMixingOnly` and `Locked`;
  - dropped in the freeze;
  - re-created after `change_passphrase`.
- Key hold:
  - 120 s without funding;
  - 300 s after the funding `finish`;
  - `park` drops it at once;
  - the signer then fails `Locked`, and the lease is `Parked{ProofWaiting}`.
- Bound:
  - one, two and eight leases with stalled permits: `lock_vault` returns at most `max(H, T_gate)` after the call;
  - a First at 0.9 H is refused;
  - a second `lock_vault` joins;
  - a dropped `lock_vault` future leaves every lease revoked and `begin_lease` unblocked at the deadline;
  - `begin_lease` during a drain waits, then succeeds;
  - `begin_lease` paused between its redemption and its insert while `lock_vault` runs: the insert is refused
    (`lease.locked`), and the redeemed signers fail `Locked`.
- Epoch wiring: `change_passphrase`, `encrypt`, `recover` and `destroy` through dw-ffi revoke and drain. A vault call
  made through `vault_op` that ends the epoch moves leases to `NeedsGrant` and re-creates the background lease (H9).
- Key hold:
  - `authorize_set` on a locked vault yields one `KeyHold`, and no token copy of the key survives (the erased
    buffers are checked under the test allocator);
  - `proof_wait_started` starts the 300 s window, and `park` on the proof wait's timeout drops the key.
- Real time: the same bounds, within 250 ms.

**Journal (P2)**
- The CAS statements. No `1 → 0` statement exists (schema test). Register is idempotent; the same txid with another
  origin is refused.
- Fault injection through a `JournalBackend` trait with a failing implementation: a `register` failure aborts the
  build.
- A `Dispatching` failure, in both forms (lost, and durable despite the error), gives `Ambiguous` and Deferred, then
  a retry gives Resend; across a reopen, 0 means refused and 1 means Resend.
- An unopenable journal refuses leased Firsts and raises the `Notice`.
- `synchronous=FULL` (and `fullfsync` on Apple) are read back after opening.

**Fence conformance (P2 against a fake library, P4 against platform-wallet)**. Each case runs for a Core transaction
and for a state transition.
- **Barrier 1:** a flow holding a released signature pauses before `admit`; `lock_vault` runs and returns; the flow
  resumes. Nothing is recorded, it reports Cancelled, and the `Built` row is gone with its inputs released once and
  selectable again by a new build.
- **Barrier 2:** the flow pauses after `admit` returned `First`, before the transport. `lock_vault` on another thread
  has not returned after 200 ms; the flow hands off once, and only then does `lock_vault` return. It reports Sent.
- **Barrier 2 with a stalled transport:** `lock_vault` returns within `max(H, T_gate)`; the flow reports MaybeSent,
  never Cancelled.
- **Barrier 2 with a stalled `Dispatching` write:** the same bound; the flow reports Deferred and MaybeSent, and no
  transport call starts after the deadline (L5).
- **A row-less repeat:** a `TxDraft` under a lease times out (MaybeSent), `lock_vault` returns, and the repeat is
  admitted as a Resend. It is never reported Cancelled, and the inputs stay reserved. In a variant the first
  attempt is a definite `NotSent`: the id leaves the set, and the repeat after the lock is refused.
- **The fence's row-less guards:** a row-less asset-lock transaction is refused. A leased row-less Core send with no
  `TxDraft` Spend charge for its txid is refused. An unscoped call gets `Deferred` and a `Notice`, and an unscoped
  repeat gets `Resend` and a `Notice`.
- **The key window:** `proof_wait_started` sets `key_until` to its timeout. A fake library that delays the proof wait
  by the acceptance bound keeps the key until the wait's own end, and a timeout from
  `create_funded_asset_lock_proof` drops it at once.
- **A dropped build:** the build future is dropped after `track` and before `admit` decides. The drop guard
  abandons, the spawned cleanup untracks the row, releases its reservation once and settles its pin released, and
  nothing is recorded. **A new build can select the same inputs at once** (review Opus 5). The same holds when the
  CAS winner's future is dropped right after its verdict.
- **Mutation build:** a fence that checks the lease outside J (check-then-act) fails barrier 2.

**Dispatch-record cases (P4, asset locks through platform-wallet)**
1. **A resume of an unsent row.** The flow tracks `Built` (entry 0) and pauses before `admit`. A resume claims the
   row and pauses before its `admit`. `lock_vault` returns. Either one is refused first (both orders); the winner
   cleans up once, and the other reports Cancelled or drops its claim. Nothing is recorded.
2. **An unsent row at load.** A row with entry 0 is refused by recovery and untracked; nothing is reserved after
   load (F7), and nothing is recorded.
3. **A genuine ambiguous dispatch.** Entry 1 and a stalled transport give MaybeSent; `lock_vault` returns; a resume
   then sends a Resend, which the fake records.
4. **M-A order.** The resume admits after O's commit and after the lock: Resend; the inputs stay reserved.
5. **A pre-fence row** (`PreFence`, from the seeding): Resend at load, never cleaned up. The seeding covers closed
   wallets and skips `consumed` rows.
6. **A deleted journal:** the next session seeds it again; every `Built` row is resent; nothing is cleaned up.
7. **A failed `Dispatching` write:** no hand-off now; the row and reservation are kept.
8. **A row with no entry** (an older `wallet.sqlite` copied in by hand): not sent, not cleaned up, `Notice`.
9. **The catch-up at load (H6),** run with SPV deliberately slow to start: the re-reservation happens before any
   build can run, the resumes start only after SPV, and bring-up does not wait for them.
10. **Its outcomes:** after a kill at each step of §7.2, the next session's catch-up does what the
   table's last column says. Entry 1 rows get their inputs reserved again before any other build can select them,
   and a build started right after the catch-up cannot spend them.

**Library obligations (P3, in the platform PR)**
- At every F1 and F9 site, `register` and a registered artifact's `admit` run with no wallet-manager guard and no
  `build_persist_serial` held (a test-only guard-depth probe); `payment_guard` is allowed, per L2.
- `register` comes before `track`, and the row is durable before `admit` (a fake persister in Manual mode).
- `Refused{cleanup: true}` overrides a resume claim, and the release is owner-guarded (a newer build's reservation
  survives).
- `Deferred` keeps the row and the in-broadcast fence.
- The readiness check comes before `admit`.
- **Nothing enters `unconfirmed_outgoing_txs` or dash-spv's broadcast set before its admitted hand-off (L9, nit 3).**
- rs-sdk: one `admit` per `broadcast`, however many retries; the per-attempt timeout is clamped; no attempt starts
  after the deadline.

**Stress (P4)**
- 16 flows (asset locks and state transitions mixed), 4 resume tasks, and 300 `lock_vault`, unlock and `begin_lease`
  cycles.
- Random pauses at every step, and random write and transport faults.
- The fence and the fake transport log every J step and every transport call in J order. A checker replays the log
  against I1–I6:
  - each recorded hand-off's commit precedes the freeze of every lock that revoked its lease;
  - no refused artifact is ever recorded;
  - each lock returns within `max(H, T_gate)` + 250 ms;
  - each reservation is released at most once.
- Mutation runs: the stress must fail with check-then-act, revoke-in-drain and transport-first builds.

**Kill matrix (P4, T1)**
- A subprocess harness with fail points, env-selected, at every step of the §7.2 table, run against regtest dashd.
- Kill -9, restart, and check the outcome column. This is the base DP1-02's kill matrix grows from.

**FFI and integration (P4)**
- `Vault.lock()` stays synchronous in dw-ffi (the Swift shells are frozen); `LockProgress::Done(LockReport)` maps to
  the UI copy, and the chosen stack binds the async variant in E0-13.
- Regtest: a registration with lock during the IS wait gives MaybeSent or Sent, then a keyless park. Lock before the
  funding hand-off gives Cancelled with no transaction in the mempool.
- Devnet (T2): DAPI blackholed during an identity create gives MaybeSent at H; the proof arrives later or not.

## 13. Phase plan

| Phase | Content | Mode | Needs | Size | Done when |
|---|---|---|---|---|---|
| P0 | this design, and its DEC-57 closure check | both | — | — | review closed |
| P1 | dw-vault: §3 (caps, `IdentityScan`, `authorize_set`, cap from token, `KeyHold`, `epoch()`, `QuickUnlock` for `PlatformOp`); m1-engine §2.2 updated | both | P0 | S | P1 tests green; second-agent review |
| P2a | dw-engine, mode-independent: lease table (barrier, `lock_gen`, generation ceilings, history), background lease, `LeaseView`, the lock coordinator with a gate per request (H14) and the unlock ordering, the synchronous FFI lock (its `VaultStatus` return unchanged), close steps, the revoking session methods and `vault_op`'s epoch check (H9); `dispatch.sqlite` with its step markers; the engine fence for engine-owned hand-offs (row-less attempts, tombstone, Spend charge bound to the txid, release and pin settling); `dispatch_status`, `DispatchResolved`, outcomes, H16; the facade lease handle and §16's surface. **New `EngineEvent` and `NoticeCode` variants stay engine-side; dw-ffi forwards them from E0-13** | both | P1, E0-08 conforming to §16 | L | the [A+B] tests green, the stress checker on the fake library included; model check passes |
| P2b | Mode B wiring: call permits around every library write call (H13) with released-signature counting, funding and step markers written before them, §2a.5's status derivation (H15), the worst-case funding bound, no library funding build over a possibly-sent row, the catch-up's resume pass (H6 part 2, pin semantics), `TxDraft` coin control excluding tracked rows' outpoints, the C2 copy, Repair's self-spend resolution | B (and the interim for A) | P2a, E0-05 | M | Mode B's acceptance list (§2a.4) green; **E0-04 can close here** if Q1 is declined or undecided |
| P3 | the platform PR (§11) and its tests; the cherry-pick on the dw pin branch | A | P0, B5 | L | PR open upstream; cherry-pick builds with dw; the PR's L-tests green |
| P4 | Mode A wiring: install the fence on the manager and the SDK; the journal's registered artifacts, seeding, payload restore and pending-spend fences (H6 part 1); the surfaced fallback; PSBT through the fenced broadcaster; a new power-loss harness, the dispatch-record cases, stress and kill matrix at hand-off granularity | A | P2b, P3 | **L** | Mode A's acceptance list (§2a.4) green |
| P5 | `TxDraft` leases for M1 sends, both the mixed and the finalized paths (DEC-65 Q2); `send.cancelled` in the m1 contract, bound in the chosen stack in E0-13; the frozen Swift shells see it mapped onto an existing send code | both | P2a | S | send flow tests updated; `send.cancelled` |

- **Sizes.** E0-04 as a whole is L–XL. ROADMAP's row is updated (§15).
- **DP1-02** builds against P2a's API and the fake library at once, and closes against P2b (Mode B) or P4 (Mode A).
- **Order of work.** P2b also serves as Mode A's interim, so it lands whatever pasta decides; P4 then replaces call
  permits with per-artifact admission site by site.

## 14. Open questions

DEC-65 accepted Q2–Q20 as recommended below. Q1 is still pasta's (B5). rev1 added Q21–Q23, which DEC-67 settled:
it accepted Q21 and rejected Q22. Q23 stands as recommended.

Each question has a recommendation. Q1 is pasta's (B5). The rest are the manager's (DEC-22 practice) or the review's.

| # | Question | Recommendation |
|---|---|---|
| Q1 | B5: may we open the one platform PR of §11, an rs-sdk hook plus platform-wallet wiring, against `v5.1-dev`, and carry its cherry-pick? | **Yes.** Without it, library flows never get Lock to cancel. The PR is opt-in (no fence, no change), and smaller than the draft's, since it no longer touches the changeset merge, the SQLite upsert or the row format. |
| Q2 | Should M1 `TxDraft` sends be leased too, so Lock cancels a prepared but not yet broadcast payment? | **Yes, as P5.** It costs one per-send lease from the `Spend` token and the `send.cancelled` code DP3-01 introduces. It makes "Lock always wins" uniform. Until then M1 sends are `Unleased(Send)`, as today. |
| Q3 | CoinJoin, PSBT and rebroadcast as `Unleased` (not drained)? | **Yes.** Mixing signs with an unattended signer and stops on lock. PSBT bytes were signed elsewhere. A rebroadcast commits nothing new. |
| Q4 | Does `change_passphrase` revoke flow leases (freeze and drain) although the vault stays unlocked? | **Yes.** The user may be reacting to a leaked passphrase; E0-03 already ends every token, so the leases cannot sign anyway; stopping their dispatch too keeps one rule: a security action revokes. |
| Q5 | Do unlock and scope change (epoch ends) revoke leases, or move them to `NeedsGrant`? | **`NeedsGrant`** (DEC-65). Revoking would make "unlock" cancel a registration started on a locked vault, which no user expects. Dispatch of artifacts already signed continues. `rebind` resumes signing after a fresh prompt when "require authentication for every payment" is on (DEC-67), and silently within the ceiling when it is off. |
| Q6 | H = 10 s? | **Yes.** A Core First is a local enqueue plus one fsync; a state-transition First is clamped by the deadline. The UI then waits at most 10 s on Lock, only when something is in flight. |
| Q7 | `max_credits` bounds an estimate (explicit credits plus `fee_bound`), since Platform charges the actual fee at execution. Accept? | **Accept.** Platform cannot charge more than the identity holds, and each re-signed retry (F10) is charged again. DP1-06 replaces the constants with its cost table. |
| Q8 | Ask key-wallet for a pre-sign hook (`Signer::approve_transaction(tx, inputs)`), so the cap blocks the signature of a library-built asset lock and not only its hand-off? | **Defer.** The hand-off refusal guarantees nothing over the cap leaves the process, and the bytes are an asset lock that credits the user's own identity. File it as a rust-dashcore follow-up. |
| Q9 | During a drain, does `begin_lease` wait or fail? | **Wait** (at most H). The UI is showing "Locking…" then anyway, and waiting keeps "lock_vault returned" a clean boundary. |
| Q10 | Journal GC? | **None in 1.0.** About 120 bytes per asset lock ever built. Revisit if a profile ever shows it. |
| Q11 | Power loss can drop a `Built` row (`wallet.sqlite` at `synchronous=NORMAL`) after its lock was sent. Raise wallet.sqlite to FULL? | **No.** FULL makes every changeset fsync during sync. Rely on DP1-05's reconstruction (`RecoveredFromChain`), and document the case. |
| Q12 | Fail closed for unscoped row-less hand-offs? | **Yes,** as `Deferred` rather than `Refused`, so nothing is sent and no false "definitely not sent" is reported. A debug assertion and a release-build `Notice` fire on every unscoped call, including repeats answered `Resend`. A missed scope breaks a flow visibly, in its tests, instead of silently escaping Lock. |
| Q13 | A registered artifact whose transport returned a definite rejection after `Dispatching` stays committed, and is resent later, even after a lock (L13). Accept? | **Accept.** The alternative, the pin's own `Rejected` arm (untrack and release when no claim exists), is also fund-safe: the entry at 1 is then orphaned. But a kill before that removal lands would resend, at the next load, bytes the user was told were not sent. The readiness check before `admit` (L8) narrows the case to a peer loss in the moment between the check and the enqueue. The UI says "will be sent when the network is back". |
| Q14 | Separate `dispatch.sqlite`, or a table in `app.sqlite`? | **Separate file** (§6.2): stricter durability, kept out of `.dwbackup` export, and kept past wallet removal. |
| Q15 | Shielded redrives (1.1) persist signed state transitions. | **Rule:** they become registered artifacts, keyed by ST hash, before shielded ships. The X-phase task adds `register` on their persist path. |
| Q16 | A tracked row with no journal entry: Resend it (trust the library's `tracked_row` and assume pre-fence) or keep it unsent? | **Keep it unsent**, with a `Notice`. The seeding gives every real pre-fence row an entry, so a row without one is either a bug (a missed `register`) or a file restored by hand. Resending would reopen the r4 shape: a possible dispatch inferred from the call site. Keeping the row loses nothing, since platform-wallet tracks its proofs without a hand-off. |
| Q17 | The catch-up at load (H6) and the re-reservation (L12) are new behaviour. dw had no catch-up at all, and the pin never reserves a loaded row's inputs again (F7). Make them part of E0-04, the re-reservation in the platform PR? | **Yes.** Without the catch-up, a row whose bytes never left (killed between the write and the enqueue) is never resent, and an `Unsent` row is never cleaned up. Without the re-reservation, another build can spend the inputs of a lock that may be on the wire. Both gaps exist at the pin today; E0-04 is where they first matter. |
| Q18 | The row-less set is per process, not persisted. Enough? | **Yes** (§5.5). Row-less bytes are handed off only in the process that signed them. The steps a later process may sign again carry a durable write-ahead step marker instead (rev1, GPT 3, §7.6); the flow's own phase is not evidence. Persisting the whole set would add a journal write to every send for no case that needs it. rev1 also tracks attempts within the process (GPT 1). |
| Q19 | Should a leased flow use the two-call split (`create_funded_asset_lock_proof`, then `FromExistingAssetLock`) everywhere, or only where an own key is held? | **Only for own-key registration and top-up** (§4.4), where the ChainLock fallback must be visible. Vault-key leases may use the library's one-call paths. Either way, never `build_asset_lock_transaction` for a hand-off. |
| Q20 | `proof_wait_started` (L15) is a second, informational fence callback in the PR. Worth it, or accept the `finish + 300 s + A` floor? | **Add it.** It is one call at one site, and without it an own key outlives DASHPAY's window by up to the acceptance bound (65 s or more, F2). |
| Q21 | `QuickUnlock` for `PlatformOp`. | **Accepted by DEC-67**: capped at the spend limit by one combined value over the whole grant set (§3.7). |
| Q22 | Rebinding without a prompt on `Unlocked(Full)` even with "require authentication for every payment" on? | **Rejected by DEC-67.** With the setting on, the rebind prompts again (`Authorize`/`Unlock`); without it, it may be silent within the ceiling (§4.3). |
| Q23 | Auto lock: does a visible flow-progress screen count as activity? | **No** (§4.8). A long ChainLock wait must not keep the wallet unlocked; the flow parks and resumes after unlock. |

## 15. Changes to other documents

**Made in this branch (rev1):**
- **DASHPAY §2.6 "Parking needs no aborted future"** and **ROADMAP DP1-02** now name the two-call split
  (`create_funded_asset_lock_proof`, then `FromExistingAssetLock` once the row holds a proof) instead of the
  build-only `build_asset_lock_transaction` (review Opus 8).
- **UX-SPEC §4.1:**
  - the "key held" lock state on locked and mixing-only vaults, with its Cross short text and the C7 tooltip, and
    the Lock action enabled in that state (rev1, revised in rev2);
  - **§4.13:** auto lock is `lock_vault`, armed only while the vault holds its key, and flow progress is not
    activity (review Opus 3, 14; Opus r2 5d).
- **DASHPAY §2.6 "Lock always wins":** the copy is §16.10's C1, C2 and C3 (rev2).

**After the review closes:**
- **DASHPAY §2.6:**
  - replace the DRAFT bullet (from "Commit points" to the end of "Open issues") with a summary of §0, §2a, §5, §7 and
    §8 and a pointer here;
  - correct the "re-dispatch" row of the commit-point table: dw has no launch catch-up until H6 (F6);
  - the spec check becomes `e0_04_design_model.py`;
  - §3.3 gets `IdentityScan`, `authorize_set` and `QuickUnlock` for `PlatformOp`;
  - §3.4's store table gets `dispatch.sqlite`.
- **ROADMAP E0-04:**
  - drop "(draft)";
  - "within H" becomes "within `max(H, T_gate)` of the call";
  - the acceptance list becomes §2a.4's two lists;
  - the size becomes L–XL;
  - add the phases of §13;
  - DP1-02 "Depends" names P2b or P4.
- **m1-engine §2.1 and §2.2:** as §3.6, with `Vault.lock()` synchronous plus `LockProgress` and `LockReport`.
- **m1-engine and m1-swift for DP3-01:** `send.cancelled` (§4.6).
- **m4** (E0-08), per amendments 1 and 2. Its names and shapes stand, and it matches §16's semantics:
  - §2.11's `dispatch_status` row and §4's retry clause: the host reads every `None` as unknown, only the engine's
    funding gates read an asset lock's `None` as never registered, and live entries read `MaybeSent` (§16.6);
  - §4's refusal list: `discard_registration` unless its funding reads `NotSent` or `None`;
  - `funds_committed`: §16.5's definition, word for word;
  - `dispatch_status`, its `None` and the gates: §16.6, word for word;
  - `discover_identities`: an `IdentityScan` grant id only; a lease id there is `platform.grant_invalid`;
  - `RevokeCause::WalletClosed`;
  - `lease_expired`: `end_flow` and the reaper only, without "its own key passed `key_until`";
  - a lease id's reach and the reaper's 10 minutes, as §16.1 (N-6).
- **DECISIONS-PENDING B5:** name §11's PR content and say that Mode B is the "no" branch.

**Follow-ups from P1** (review DW-E0-04-P1 r1, GPT; the manager's narrowed ruling on interpretation 1):
- **The M1 exception to §3.5.** A direct own-key signer, which keeps its key alive, is allowed only for the M1
  purposes `Spend` and `SignMessage` (send, PSBT, message, CoinJoin), because those flows drop their token as soon as
  they hold the signer. **P5 removes it** when it moves those flows onto `TxDraft` leases and their holds; from then on
  every own-key signer is a held one.
- **`PlatformOp` and `IdentityScan` own-key tokens issue signers only through a hold** (`hold_key`, then
  `platform_signer_held`, and `scan_key_held`, which joins §3.5's list). Directly they are refused
  (`invalid_argument`); so is a held token of any purpose outside its own hold. Vault-key tokens (an unlocked vault)
  need no hold and issue directly.
- **No partial holds.** `hold_key` returns `Result<Option<KeyHold>, VaultError>`: it covers every token it is given
  or fails, changing none. It refuses any token of another vault or of an ended epoch (vault-key tokens included), a
  set mixing vault-key and own-key tokens, a token already held, and a token that has ever issued a signer (a mark
  set at its first issue and never cleared, review r2). `Ok(None)` means a valid set of vault-key tokens.
- **Liveness is the hold's state, not a reference count.** The hold keeps its key in a mutex-guarded slot that its
  `Drop` empties, erasing the key in place. Every use copies the key under that mutex, inside the vault gate, so no use, and
  no signer issue, begins after the drop, while an operation already past its copy finishes. A transient strong
  reference (an upgraded `Weak`) would have let a new use begin after the drop (GPT r1, high).

## 16. The contract surface (single source for E0-08)

Names and shapes are E0-08's implemented contract (369f8f0, Contract-Version 3, which becomes 4 when E0-08 adopts
§16.5 and §16.6), restated here. The semantics are this design's (the manager's correction to rev2 ruling 4). Where
the contract's prose differs, E0-08 matches this section; §15 lists the lines.

### 16.1 Leases

- `NetworkSession.begin_flow(wallet_id, flow: FlowKind, grants: Vec<String>) -> Result<String, PlatformError>`
  returns a lease id. `NetworkSession.end_flow(lease: String) -> Result<(), PlatformError>` is synchronous and
  idempotent. The session owns the lease from `begin_flow` until `end_flow` or the reaper (§4.1, §4.7).
- A lease id is accepted wherever a `grant: String` is, by any call of its wallet whose purpose it carries. A call
  that needs a purpose the lease lacks gets `platform.needs_grant{purpose}`. Another wallet's lease, or an unknown
  id, is `platform.grant_invalid` (N-6).
- **The idle reaper** ends a vault-key lease, or a revoked one, after 10 minutes with no call, no permit and no
  running flow task.
- `FlowKind`: `Registration`, `TopUp`, `Withdraw`, `NameRegistration`, `ProfileEdit`, `ContactRequest`, `Accept`,
  `AcceptAndPay`, `PrivateDetails`, `EnableDashPayKeys`. There is no `Discovery`: `IdentityScan` is a grant, not a
  lease (§3.2). `discover_identities` accepts only an `IdentityScan` grant id; a lease id there is
  `platform.grant_invalid` (review Opus r3 F-3).
- `BudgetPurpose`: `Funding`, `Credits`, `Spend`, `Crypto`.
- `LeaseView` (§4.6) and `NetworkSession.leases()` are owned by m4 §2.11 (`flows.rs`).
- **"Accept and pay"**: one `authorize_set` prompt (one combined Touch ID cap, §3.7), one `begin_flow`, and the id
  passed to `accept_request` and to `TxDraft.prepare`.

### 16.2 Quotes and grants

- `RegistrationQuote`, `TopUpQuote` and `WithdrawQuote` carry `grant: GrantRequest{max_duffs: u64, max_credits: u64}`.
- `DashPay::grant_request(identity: String, action: GrantAction) -> Result<GrantRequest, PlatformError>` covers the
  writes without a quote. `GrantAction` is `SendRequest`, `AcceptRequest`, `RegisterName{label}`, `UpdateProfile`,
  `PublishPrivateDetails` and `EnableDashPayKeys`.
- **The engine charges no more than it quoted.** A stale quote is `platform.grant_exceeded`, and the host quotes
  again. In Mode B a funding quote includes `fee_bound_worst` (§2a.3).

### 16.3 Waiting rows

- `RegistrationWait`: `Unlock`, `Sync`, `InstantSend`, `ChainLock`, `Network`, `Authorize`. A grant is required
  exactly for `Unlock` and `Authorize`.
- A `Parked{ProofWaiting}` row waits in `ChainLock` without a key. When its proof arrives and keys are needed, it
  moves to `Unlock` (vault locked or mixing-only) or `Authorize` (unlocked), so the host never prompts during the
  wait. There is **no** `needs_grant` field on `RegistrationStatus`.
- A rebind with "require authentication for every payment" on moves the row to `Authorize` or `Unlock` (DEC-67,
  §4.3).

### 16.4 Codes

| Engine situation | Code |
|---|---|
| `lease.locked`: a lock landed while `begin_flow` redeemed, or Lock won a flow's permit | `platform.cancelled` |
| `LeaseError::Revoked{cause}`: a call names a lease that a revoking call ended (§4.3's `Revoked{cause}`) | `platform.lease_revoked{cause}`. This call sent nothing; for an uncommitted write it is E0-04's `Cancelled` too |
| `LeaseError::Ended`: a call names a lease ended by `end_flow` or the idle reaper (§4.3's `Ended`) | `platform.lease_expired`; never for a passed `key_until`, which parks (next row) |
| `LeaseError::Parked` or `NeedsGrant`: `key_until` passed, an unlock, a scope or passphrase change, a budget at 0 | `platform.needs_grant{purpose}` |
| a budget refusal | `platform.grant_exceeded{purpose, needed, remaining}` |
| an unknown, used or other-wallet grant or lease id | `platform.grant_invalid` |
| outcome `WillBeSent` | `platform.will_be_sent{artifact}` |
| outcome `MaybeSent` | `platform.broadcast_unknown{artifact}` |
| outcome `Cancelled` | `platform.cancelled` |
| the journal is unavailable, or its schema is newer | the common `storage`, with `Notice{DispatchJournalUnavailable}` |

`RevokeCause`: `Lock`, `Close` (the session's), `PassphraseChange`, `WalletRemoved`, `WalletClosed` (one wallet's
close, §8.6). c3c9bbd lacks `WalletClosed`, and E0-08 adds it (N-4).

### 16.5 Two flags

- **`holds_key`** = `LeaseView.own_key && state ∈ {Active, AwaitingProof}`.
- **`funds_committed`**: true once a funding of the flow has committed and is not definitely unsent. It is the only
  definition, used by `LeaseView`, `RegistrationStatus` and the copy, and E0-08 copies it (N-3):
  - **Mode A:** an asset lock of the flow has its journal entry in `Committing`, `Dispatching`, `Ambiguous` or
    `PreFence`, or has a tracked row with no entry (unknown provenance, §6.5).
  - **Mode B:** a funding marker of the flow exists, or a tracked row matches the draft's own funding key, and the
    funding's status (§2a.5) is not `NotSent`. The marker is taken in the J step that takes the funding call's
    permit, and is durable before the call starts, so the flag turns on with the permit and survives a restart.
  - **Not committed:** no entry and no row (never registered); an `Unsent` entry, whether its lease is live (not
    committed yet) or its origin is dead (never can be); a `Revoked` entry. So a lock that Lock revoked and one
    still being built both read false, and "Lock to cancel" shows exactly while Lock still cancels.

  A contact request's or profile's `MaybeSent` transition does not set it: those are not funding.

### 16.6 `dispatch_status`, retry and discard

- `DashPay::dispatch_status(artifact: String) -> Result<Option<DispatchState>, PlatformError>`, async. `artifact`
  is what `broadcast_unknown{artifact}` and `will_be_sent{artifact}` carry: a txid, a state-transition hash, or a
  funding step id (`registration/<draft>/funding`, `topup/<id>/funding`). Mode B uses the step id whenever the
  engine never saw the funding's txid (§2a.5).
- `DispatchState`: `WillBeSent`, `MaybeSent`, `Sent`, `NotSent`. `None` means the engine has no entry.
- **What it answers**, first matching row wins:

  | Artifact | Evidence | Answer |
  |---|---|---|
  | any | an attempt or a Resend finished `Sent`, or the wallet has seen the transaction (its row is `InstantSendLocked`, `ChainLocked`, `Consumed` or `RecoveredFromChain`) | `Sent` |
  | asset lock, Mode A | entry `Dispatching`, `PreFence` or `Ambiguous` (the catch-up restores a lost row, §6.5) | `WillBeSent` |
  | asset lock, Mode A | entry `Unsent` with a live origin, or `Committing` | `MaybeSent` |
  | asset lock, Mode A | entry `Revoked`, or `Unsent` with a dead origin; Repair's self-spend ChainLocked | `NotSent` |
  | asset lock, Mode A | no entry, but a tracked row of any status | `MaybeSent` (unknown provenance, §6.5) |
  | funding, Mode B | §2a.5's table | as there |
  | state transition, `TxDraft` send | an attempt running or `possibly_out`; a resumable step's marker | `MaybeSent` |
  | state transition, `TxDraft` send | its tombstone (§5.5); a different transition this engine signed is proved executed in its nonce slot (H16) | `NotSent` |
  | any | none of the above | `None` |

- **What `None` means depends on the artifact's kind:**
  - **an asset lock** (a funding txid or step id): never registered. Its entry (in Mode B, its marker) precedes
    tracking and every transport (I1), and an entry outlives its row (§6.2). So with no entry and no row, nothing
    was or can be sent;
  - **a state transition or a `TxDraft` send:** unknown, read exactly as `MaybeSent`. Row-less entries and their
    tombstones live in one process (§5.5), so after a restart `None` is all the engine can say.
- **Who applies the asset-lock reading.** The artifact string does not carry its kind (a txid and a transition hash
  look alike), so:
  - **the host reads every `None` as unknown**, and only `Some(NotSent)` allows a retry it offers (H11). It loses
    nothing by this. It holds only artifacts from `broadcast_unknown` or `will_be_sent`, which the engine reports
    after their record exists, so an asset lock the host holds reads `None` only once its record was erased (a
    wiping removal, or a journal deleted by hand);
  - **the engine's funding gates apply the asset-lock reading.** They ask about a flow's funding step, whose kind
    they know: `discard_registration`, and DP1-02's "Register again", which discards first. With no entry, no
    marker and no tracked row matching the step (in Mode A by its txid, in Mode B by the draft's own funding key or
    the marker's), the step never built a lock, so the gate allows it.
- The `broadcast_unknown` retry rule is "after `DispatchResolved(NotSent)` or a `NotSent` status", never "after a
  re-query shows nothing was sent".
- `discard_registration` is allowed only when the registration's funding reads `NotSent` or `None`; otherwise
  `invalid_argument`. That implies `!funds_committed`, and it also refuses a live `Unsent`, whose flow is still
  building.
- `resume_registration` acts on any state.
- `finish_asset_locks` is not gated by the journal: it resumes committed locks.

### 16.7 Events (engine-side until E0-13)

- `EngineEvent::DispatchResolved{network, resolved: DispatchResolved{wallet_id: String, artifact: String,
  resolution: DispatchResolution}}`, where `DispatchResolution` is `Sent` or `NotSent`. Its sources are listed in
  §4.6.
- `EngineEvent::LeaseChanged{network, lease: LeaseView}`.
- `EngineEvent::LockProgress{network, phase: Draining{in_flight, deadline_in_ms} | Done(LockReport)}`.

dw-ffi maps `EngineEvent` one to one and the Swift bindings are frozen, so these variants stay engine-side. dw-ffi
forwards them from E0-13 (§13).

### 16.8 Notices

`DispatchRecordMissing`, `UnscopedDispatch` and `DispatchJournalUnavailable` join `NoticeCode`, owned by E0-04,
engine-side until E0-13 like the other new notices.

### 16.9 Lock

`Vault.lock()` stays synchronous. It runs its own vault gate (H14), revokes leases even on a vault that is already
`Locked`, and returns `VaultStatus`. `LockReport` arrives engine-side in `LockProgress::Done`.

### 16.10 Copy

`{Flow}` is "Registration", "Top-up", "Withdrawal", "Name registration", "Profile update" or "Contact request".

| Id | When (§4.6) | String |
|---|---|---|
| C1 | no library call running, nothing committed | "{Flow} in progress — Lock to cancel" |
| C2 | a library call of the flow runs (Mode B), or a committed First of it is in flight (Mode A); DEC-67's promise | "Lock stops new signatures; a transaction already signed may still be sent" |
| C3 | no call running, `funds_committed`, a further signature needed | "Funds committed — finishing. Lock to stop; you'll finish after you unlock" |
| C4 | after a manual lock, re-chosen on every change, first match (§4.6) | a call still runs or a committed First is in flight: C2's string; nothing committed: "Cancelled"; an artifact `MaybeSent` (or a transition's `None`): "May have been sent"; an artifact `WillBeSent`: "Will be sent"; either of these on a parked flow adds "; unlock to finish" (locked) or "; confirm to finish" (unlocked); parked with nothing pending: "Unlock to finish" or "Confirm to finish"; otherwise "Sent" ("Sent before the lock" only when the drain saw it finish) |
| C5 | after an auto lock during a flow: "Locked automatically" and C4's line, chosen the same way | "Locked automatically. Lock stops new signatures; a transaction already signed may still be sent" (a call still runs); "Locked automatically — cancelled"; "Locked automatically — may have been sent" or "… — will be sent", each with "; unlock to finish" when parked; "Locked automatically — unlock to finish" (parked, nothing pending); "Locked automatically — sent" |
| C6 | the drain has permits in flight (non-blocking status line) | "Finishing a transaction already handed off" |
| C7 | the "key held" tooltip | "{Flow} of {name} holds a key for {m:ss} — Lock to cancel" (`!funds_committed`) or "… — Lock to stop" (`funds_committed`); Cross short text "Locked · key held" or "Mixing only · key held" |
| C8 | "Accept and pay", accept `Sent`, payment `Cancelled` | "Accepted — payment cancelled. Pay now?" |
| C9 | Tools ▸ Repair | "Unrecorded asset locks", actions "Send it" and "Cancel it"; for a Mode B orphan, "Resolve stuck funding: send all coins to yourself" |

### 16.11 Other contracts

- **DP3-01:** `send.cancelled` joins m1's `SendError` for a contact payment that Lock refused (§4.6). P5 uses the
  same code for M1 sends.
- **DP1-02:** Repair's "Unrecorded asset locks" (§6.5) is engine-internal until DP1-02 adds its facade call under
  Tools ▸ Repair.

## Appendix A. Closure of DASHPAY §2.6's open-issues list

### A.1 Majors and minors

- **M-A.**
  - The single J step (§5.4) re-reads the entry and compare-and-sets it. The model's `split-literal` mutation
    reproduces both traces of `e0_04_split_model.py` as violations, and `split-cas` passes.
  - "Never both" is I3.
  - The lock-order concern is closed differently from the review's suggestion (permit first, then guard): there is
    no permit lock to queue behind, because admission is a non-blocking decision and the drain only waits. The
    review's rule survives as L2, so a journal fsync never stalls the wallet's readers, and the fence takes no
    library lock at all (H1), which model Part 3 shows is what keeps `admit` deadlock-free.
  - The test the review asked for (resume classifies `Unsent`, flow dispatches, lock, resume refused, inputs stay
    reserved) is dispatch-record case 4, and in the model it is the M-A replay.
- **M-B.**
  - (a) The journal write is synchronous `FULL` and returns before the transport (I1); the row is durable before
    admit (L3).
  - (b) The record left the changeset (§6.1); the journal is monotone (§6.2–6.3); a resurrected row is refused again
    by its entry.
  - (c) A failed write is `Ambiguous`, never `Unsent` (§6.4).
  - (d) The journal is seeded with `PreFence` for every row that predates it, and `PreFence` means Resend (§6.5).
    No entry means unknown provenance: kept and not sent. So the decision never rests on the call site.
- **M-C.**
  - H starts at the permit's grant and covers the write and the transport (§5.4).
  - The host enforces it (H3).
  - Revocation is in the synchronous freeze (H4).
  - A dropped future and a second call are §8.3.
  - "Reopen admission only for leases created after the freeze" becomes `lock_gen` (H8): a lease whose creation
    straddles a freeze is never inserted.
  - Model Part 2 covers all of this, including the creation race.
- **m-1.** 128-bit random ids, wallet compared too (H5); the `lease-reuse` mutation.
- **m-2.** Part 2 is a per-lease tick simulation with the freeze as an event; five wrong rules fail it.
- **m-3.** The atomicity is stated in §10.1 and made checkable by the `split-*` variants.

### A.2 Nits

1. **The H wording** in ROADMAP and in DASHPAY's tests bullet is aligned to "within `max(H, T_gate)` of the call,
   plus timer slack" (§8.2, §15).
2. **"Finds the lease revoked"** becomes "is refused (its lease was revoked at the lock's call)". With revocation at
   the freeze, there is no separate admission flag left to refuse on.
3. **A test that nothing enters the host's unconfirmed-outgoing set before its permitted hand-off:** L9 and its
   platform-wallet test (§12). In dw that set is always empty (F8); the test still guards other backends.
4. **An unlock plus a grant during the drain:** both succeed; only `begin_lease` waits, at most H (§8.3, Q9).
5. **The r4 n1 stress barrier:** fixed in E0-03 (`a79b3a9`, the abortable rendezvous). E0-04's stress tests reuse it.
