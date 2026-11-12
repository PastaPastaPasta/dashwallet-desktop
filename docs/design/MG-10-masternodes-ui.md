# MG-10 / MG-10b: My masternodes and shared masternodes (Tauri), design r7

ROADMAP MG-10 (UI, L, deps MG-00, MG-03, MG-04, MG-13, E0-13; acceptance "Register a regtest masternode from the
UI") and MG-10b (UI, M, deps MG-04b, MG-10; acceptance "Create and dissolve a shared masternode on regtest from the
UI"). Both are release 0.3 (ROADMAP §7.1). One document, because they share the list, the detail pane, the keys and
the registration wizard's fields. **Design only**: no code, no commits. Lane MN-UI, for MG desktop.

Read against desktop `origin/main` `95aa54ec`. The MG-13 engine is read on `dw/mg-13-withdrawal` with `git show`: code
at `f9f2b8c6`, branch head `b18152eb` (the docs commit with the MG-00 D42/D44/D47 amendments). Contract-Version is
18 there and 17 on main. D-numbers are this file's own (D1…). Other documents are cited as "MG-00 D42", "MG-04b D9",
"mn §2.4" (`docs/contracts/mn-engine.md`) and "UX §4.25.5" (`docs/design/UX-SPEC.md`). Engine paths shorten
`rust/crates/dw-engine/src/masternodes/` to `mn/`, and `rust/crates/dw-app/src/` to `app/`.

Inputs: ROADMAP MG-10/MG-10b rows and §7; MG-00 (main, plus `b18152eb`'s D42/D44/D47/MG-13 amendments); UX §4.25;
MG-03, MG-04, MG-04b, MG-05, MG-09, MG-11, MG-13; `mn-engine.md` v17 (main) and v18 (MG-13); `m3-engine.md` §2.5
(keychain); the existing renderer (`apps/desktop/src/screens/masternodes/*`, `governance/*`, `fixture/governance.ts`,
`credential.tsx`); dw-app (`api/governance.rs`, `api/files.rs`, `api/vault.rs`) and E0-13 coverage; the gap notes
`docs/research/gap-2026-10-31/{ios,android,core}.md`; the reviews of rounds 1–6 (Opus-h, Sol-h) and MG's rulings
for each round; dash-spv's mempool dispatch at the rust-dashcore revision desktop pins (8e7ad2c).

## Change log (r6 → r7)

All round-6 findings were accepted, as ruled: Sol's two majors (both about sessions that existed before the upgrade)
and Opus's eight nits. Nothing else changed. Each row names the section that answers it.

| Finding | What changed | Section |
|---|---|---|
| Sol M1, Opus n4 | A durable legacy-bound mark, the new column `legacy_bound`. The migration sets it on every session with a legacy `abandon_txid`, in the same statement as the backfill, since no row has a log before it. While the mark is set, the required set R is the **whole original reservation**, even when history holds the latest legacy spend and its inputs: an older version could abandon more than once and overwrite the earlier id. The history-input shortcut is gone. `set_shared_session_abandon_durable` clears the mark in the same transaction once a new spend's inputs cover `reserved`; later spends keep the bound by induction. While a required coin is missing: `coin_elsewhere` (now also for a `BlockedByWalletTx` naming an earlier spend) or the rescanning line, plus the Activity route, with no "only one" promise. The "any two conflict" claim is qualified: it holds for every spend the new engine signs; two legacy spends already signed may be disjoint, a second known limit beside attribution. T4: the two-abandon upgrade (P spends x, A spends y, A abandoned in Activity, P unconfirmed) for a coordinator, a participant and a key-change funder | §2.3.3, §3.8, §4.4 storage, changes 2 and 4, rules 4 and 9, T4, §5.1, S5, D17 |
| Sol M2 | The migration rewrites inherited refusal-class `last_error` codes to `send.broadcast_unknown` on out rows (registration, key change, an abandoned row's inherited abandon outcome). The list is dw-appdb's `INHERITED_REFUSED_CODES`, and a test pins it to `outcome_of_code`'s `refused` class. The backfilled legacy entry gets `maybe_sent`. Contract line on mn §2.4's `send_shared_registration` and `send_maintenance` rows. T4: old refusal → old resend → crash after hand-off → upgrade → restart, and old uncertain → old fence-refused resend → upgrade, both for the coordinator and the key-change funder, reading unknown before any send; a separate new-protocol first refusal that keeps its wording; the inherited abandon outcome | §2.3.3, §3.8, §4.4 storage, change 4, rule 3, T4, derivation tests, §6 EP, D12, D17 |
| Opus n1 | Rule 9 checks `rescanning` before `coin_elsewhere` when every missing required coin is an input of a transaction marked abandoned; abandoned transactions are excluded from the `spender` lookup. T4 row: legacy one-input A abandoned in Activity, the other coin free, rescan running → `blocked{A, rescanning}` | §2.3.3, §4.4 change 2, rule 9, T4 |
| Opus n2 | T4's legacy rows assert against the coin test: "the call refuses whenever `abandon_coins` isn't `Usable`". The partial row's `coin_elsewhere` and the "X removed" row are pinned to after `abandon_spend.overdue`; before it, nothing is listed and nothing blocked | §4.4 T4 |
| Opus n3 | The `send_shared_registration` row (`mn-engine.md:184`) and the `send_maintenance` row's `last_error` sentence join the rewrite list. The marker is written immediately before `deliver_shared`, after `check_resend_inputs`, the lease and the charge, so a guard refusal keeps the first code. **Chosen: a distinct marker code**, `send.resend_in_progress`, after governance's `RESEND_IN_PROGRESS`, plus the never-cleared `resend_marked`: a refused resend writes its own code, keeping its reason, and reads `unknown` with `resend_refused`. New copy for both | §2.3.3, §3.8, §4.4 storage, change 4, rule 3, T4 |
| Opus n5 | `resend_wallet_tx` is listed only with SPV running and connected peers (`tx_actions.rs:466-474`); otherwise it is left out, with `blocked{spv_stopped}` when it would be the only exit. T4 no-peers row | §2.3.3, §4.4 rules 6 and 9, T4, §5.1 |
| Opus n6 | `resend_shared_abandon` returns `stage_refusal(stage)` for a session that isn't out (new step 4), as the session abandon does; added to the Errors cell; T4 row | §4.4 change 4, T4 |
| Opus n7 | [Resend in Activity] sits in the collapsed line until `abandon_spend.overdue` in §2.3.3 and §3.8 too, matching rule 6 and T4 | §2.3.3, §3.8 |
| Opus n8 | QT-127's `spend.input.len() == 1` (`mn/shared/tests/key_change.rs:478`) is named as a behaviour change beside QT-126's, with a multi-input variant | §4.4 change 4, T4, §6 EP |
| Q6, Q10 | Round-6 sentences added | §8 |

## Change log (r5 → r6)

Every round-5 finding was accepted, as ruled. r5's core stands; the changes are in legacy rows, the outcome and replay
boundary, and terminal states. Each row names the section that answers it.

| Finding | What changed | Section |
|---|---|---|
| R1 (Sol M1, Opus m1) | The inclusion bound moves into the shared coin test, as arm `Unbounded`, so the read and the call agree. The required set is A's stored inputs; for a legacy A, its inputs from A's history transaction, else every outpoint of the original reservation. Without the proof: `blocked{coin_elsewhere}`, with the spender's Activity link and no "only one" promise. T4: a two-input legacy reservation with one input held by an unconfirmed non-abandon spender, across a restart and the spender's removal, for a coordinator, a participant and a key-change funder; no disjoint B can be signed. Also a refused A with an input spent elsewhere | §2.3.3, §3.8, §4.4 change 2, rules 4 and 9, T4, §5.1, D17 |
| R2 (Sol m1, Opus m5, Opus n1) | `deliver_abandon` returns `broadcast_inner`'s real result and writes the log outcome; storage failures stay separate. `mark_shared_abandon_replay_durable` moves a null outcome to `maybe_sent` before any replay. The fold records `not_sent` only with `first`, so a refused replay never clears uncertainty. `mark_shared_session_resend_durable` writes `send.broadcast_unknown` before each registration and key-change resend, and a replay's `NotSent` keeps it. T4: hand-off → crash → restart → replay refused by the fence → restart; a first refusal through the real wrapper; a registration resend crash and a refused resend | §4.4 storage, change 4, rules 2–3, T4, §6 EP |
| R3 (Sol m2, Opus m2) | The recorded spend back's Activity exit comes from A's own history entry plus Activity's predicate, whatever the coin test says. A legacy A with no bytes gets Activity's Resend (`resend_wallet_tx`). Collapsed and unbadged until overdue. The session's own transaction relayed back after an Activity abandon gets its exit too. T4: a legacy two-input reservation, A spending one, the other free, before and after overdue and restart | §2.3.3, §3.6, §3.8, §4.4 rule 6, T4 |
| R4 (Sol m3, Opus n2, n3, n4) | Terminal state is settled before any action is built. A spend back in a block is `confirming`/`settled`, never `uncertain`, and `abandon_spend.state` is set only while pending. `resend_shared_abandon`'s check order shares the session abandon's guards, terminal first (`shared.rs:857-864`, `key_change.rs:1115-1120`), then `check_resend_inputs`; its refusal codes and contract row are written out. The mn §2.4 `abandon_shared_session` row ("largest", `mn-engine.md:185`) and MG-04b's facade row (`:613`) are rewritten. Send again leads to `in_history` and its own exit. T4: terminal sessions with A absent from history, after release and restart | §2.3.3, §3.3, §3.8, §4.4 change 4, rules 1–2, 5, T4 |
| Opus m3 | `abandon_shared_session`'s check order is stated. `test_QT_126_abandon_spends_a_reserved_coin_and_releases_once_buried` now expects `abandoning` instead of `funding_signed`: a named behaviour change with a contract line, a control (A refused → passes) and a multi-coin variant. The ownership-path and claimed-coin changes are named too, not called parity | §4.4 changes 2, 4, T4 |
| Opus m4 | Spend backs use `tip_overdue` (the tip-time clause only). The list clauses stay for ProTx operations and `sent` sessions | §4.2, §4.4 rule 11 |
| Opus n5 | Ids that older builds overwrote can't be recovered. A legacy row's other-spend copy is neutral ("A spend of these coins confirmed."), recorded as a known limit | §2.3.3, §3.8, §4.4 storage, D17 |
| Opus n6 | `blocked{no_ownership_path}` is a terminal blocker with its own copy and never shows as rescanning | §2.3.3, §3.8, §4.4 change 2, rule 9 |
| Sol n1 | `blocked{confirmed}` is removed from D16 | D16 |
| Sol n2 | T4's participant row: the same Activity exit and blocked reasons, without registration resend; `shared_stage{funding_signed}` is kept. The table is `masternode_ops`, not `protx_operations` | §4.4 T4, change log r4 → r5 |
| Q6, Q10 | Q6 records round 5's conditions. Q10 carries the legacy "only one" confirmation only with its inclusion condition, plus the new blocker copy | §8 |

## Change log (r4 → r5)

Every round-4 finding was accepted, as ruled. The core is R1, the abandon spend's own recovery: the wallet never
infers "never sent" from what isn't in history. Each row names the section that answers it.

| Finding | What changed | Section |
|---|---|---|
| R1.1 (Sol M1) | The abandon spend is stored before dispatch. No existing store fits: `signed_tx` keeps the session's own bytes, `masternode_ops.signed_tx` is MG-04's, and the dispatch journal settles a row-less `CoreTx` in memory. So EP adds a migration with nullable `shared_mn_sessions.abandon_tx` (A's bytes) and `abandon_log` (every spend back: txid, fee, inputs, `signed_at`, outcome, code). Two synced writes: bytes, id and entry before `deliver_shared`; the outcome after, forward-only. Legacy rows are backfilled with an entry carrying no bytes. dw-appdb tests and gate line | §4 preamble, §4.4 storage, change 4, §6 EP |
| R1 principle (Sol M1, Opus m1) | Rule 3's "not in history" condition is gone. "Didn't go out" appears only for a durably recorded `not_sent`. Uncertain copy is used for unknown outcomes, crashes on either side of the hand-off, cut and deferred hand-offs, and legacy rows | §2.3.3, §3.8, §4.4 principle + rule 1, D12 |
| R1.2 | Same-coin bound: a new spend back takes **every** reserved coin still unspent (n-input fee). It is built only when none is claimed and, with stored inputs, when A's inputs are all included, so any two spends back conflict. Tested with one and several reserved inputs | §2.3.3, §4.4 change 4, T4 |
| R1.3 | `attributed`: every abandon txid the session signed. `settled.by = abandon_tx` for any of them; `conflict_of` prefers them at equal height | §2.3.3, §4.4 rule 9, change 3 |
| R1.4 (Opus M1) | `abandon_spend.state`: `refused` → Abandon again; `uncertain` → **Send again** with A's bytes (new `resend_shared_abandon`, gated on SPV), with the engine refusing a new spend (`shared_stage{abandoning}`); `abandoned_in_activity` → Abandon again, A kept; `in_history` → Send again, then its own two-step exit (`abandon_wallet_tx{A}`) once overdue; `legacy` → uncertain, then a replacement once overdue, behind an explicit "only one can confirm" confirmation | §2.3.3, §3.3, §3.8, §4.4 rules 1, 3–5, D17 |
| R1.5 | T4 and fixtures, each with a restart: the Sol seam (handed off, engine dropped before history), one input and several, then A confirms → `abandon_tx`; refusal → second spend → first still attributed; abandoned in Activity → re-abandon; recorded, overdue → exit; legacy; pre-dispatch crash; cut and deferred hand-offs | §4.4 T4, §5.1, §6 S5 |
| R1.6 (Opus n8) | No reason sentence for the crash window or a legacy row. This deliberately departs from the r3 ruling, which treated the crash window by the registration's older error: that error isn't A's | §2.3.3, §4.4 rule 2 |
| Sol m1 | The blanket `shared_stage{sent}` sentence is gone. `BlockedByWalletTx` / `HeldInFlight` → `stage_refusal(out_stage(role))`; `Unavailable{code}` → its code; `Usable` → the spend. Parity tests pin the coordinator and participant no-coin cases and the fee/dust boundary | §4.4 change 2 |
| Opus m2 | The shared coin check issues no address: a nominal 25-byte P2PKH change script, with the call asserting its real address matches, and a parity test at fee + dust ± 1 | §4.4 change 2 |
| Sol n1 | Owner UI: `reviewed_destination: Some`, no `destination`. Payout UI: `None` and an explicit `destination`. Applies to withdraw and retry | §3.2 |
| Sol n2 | `conflict_of(history, inputs, except, preferred: &[Txid])`, ordered by lowest height, then preferred, then smallest txid; `conflict_height` passes its old exception and no preference | §4.4 change 3 |
| Opus n1 | `blocked{confirmed}` is dropped. `activity_abandon_refusal` names the SPV check (`tx_actions.rs:262-266`) before `Facts::abandon_refusal` | §2.3.3, §3.8, §4.4 rules 5, 8 |
| Opus n2 | An approver's key change lists Discard | §2.3.1, §2.3.3, §4.4 rule 7, T4 |
| Opus n3 | `overdue` = list synced ∧ `list_height + 1 ≥ tip_height` ∧ `tip_block_time() − sent_at ≥ 3 h` (above the 2 h future-time allowance). The signature takes the tip height; a resend doesn't move `out_at` | §4.2 |
| Opus n4 | `blocked{rescanning}` with waiting copy while the rescan runs after an Activity abandon | §2.3.3, §3.8, §4.4 rule 8 |
| Opus n5 | An unknown spend back badges only once overdue; a hand-off error badges only with a listed action | §3.6, §4.4 badge test |
| Opus n6 | `resend` and `resend_abandon` are listed only while SPV runs; otherwise `blocked{spv_stopped}` "Available once the wallet is connected." | §2.3.3, §4.4 rules 4, 6, 8 |
| Opus n7 | 12 `registered()` call sites and 6 `my_shares` call sites listed exactly, with a parity test per site; the `shared_too_early` handling at `together.rs:770` and `key_change.rs:835` is kept | §4.5 |
| Opus Q6, Q10 | Q6 records D17's spend-back policy; Q10's amendment adds the spend-back copy | §8 |

## Change log (r3 → r4)

Every round-3 finding was accepted, as ruled. R2 and R3 are closed; the changes are in R1's recovery contract and in
keeping existing engine behaviour. Each row names the section that answers it.

| Finding | What changed | Section |
|---|---|---|
| Opus m3, Sol M1 | The `last_attempt_tx` column, its migration and the dw-appdb gate line are gone; EP makes no schema change. `handoff_error.tx` is derived: `abandon` if `abandon_txid` is set, else `registration`. The crash window (abandon saved, not handed off) is handled by the code's outcome and the coin test. Derivation tests on existing-shape rows: failed registration, failed abandon, unknown abandon, the crash state | §4 preamble, §4.4 rule 1, §6 EP, D12 |
| Sol m1, Opus m2 | `handoff_error { tx, outcome: refused \| unknown, code }`, where the outcome is `outcome_of_code`, with a parity test against `attempt_outcome` (`send/mod.rs:1842-1853`). Refused abandon → Try again, with the second fee named. Unknown abandon → uncertain copy, no second abandon with one reserved input or several, and `abandon_txid` is never overwritten while the first may confirm. Unknown registration → resend still offered, with uncertain copy. T4 rows for all of these, each across a restart | §2.3.3, §3.8, §4.4 rules 1, 3, 4, T4 |
| Opus m1, Sol m3 | Engine behaviour changes are listed as such. The key-change resend now refuses once `abandon_txid` is set (`shared_stage{abandoned}`, the D72 parity), with a regression test and an mn §2.4 line. `actions` is a **safe subset**: T4 asserts each offered action passes its call, and gives a reason for each one left out. The approver `conflicted` row and fixture are dropped and replaced by the approver's list-derived outcomes; the funder `conflicted` row stays, with its restart coverage | §2.3.1, §2.6, §4.4 change 1 + T4, §5.1, §6 S6 |
| Sol m2, Opus m4, Opus n6 | The coin test has four arms: `Usable`, `BlockedByWalletTx{txid}` (a new history lookup, named), `HeldInFlight` (waiting copy, no action) and `Unavailable{code}` (no ownership path; the value check → `blocked{too_small}`). The selection order, the ownership-path check, the coin fence and the error mapping are preserved, with parity tests | §2.3.3, §3.8, §4.4 change 2, rule 6 |
| Opus M1 | `abandon_wallet_tx` is listed only when Activity's own `Facts::abandon_refusal` is None and SPV is running (one shared function); otherwise `blocked{instant_locked \| spv_stopped \| confirmed}` with copy. [Open in Activity] opens `TxDetailSheet` for the txid. The two-step line is collapsed and unbadged until `overdue`. T4 rows, fixtures and a badge test: successful hand-off, IS-locked, SPV stopped, not overdue | §2.3.1, §2.3.3, §3.6, §4.4 rule 2, §5.1, D16 |
| Sol m4, Opus m5 | `overdue(sent_at, list, tip_time)` = list synced to the header tip ∧ `sent` ∧ `tip_block_time() − sent_at ≥ 3600 s`. One function, shared by EP 2 and EP 4 (sessions get `overdue`). Advisory only, and `HeaderSource` is not extended. Tests: a mocked tip time, a restart, a reorg, the list behind the tip | §1.10, §4.2, §4.4 rule 8 |
| Opus m6 | `registered()` is split into a history half and a list half. The wrapper keeps the refusal order of all 14 callers, with parity tests, and `my_shares` takes `&[CollateralShare]`. `list_unavailable` is an eligibility value: the facts are still shown, with its own pane copy. The precedence is `upgrade_unknown` → `list_unavailable` → `too_early` → `delisted`, and the height is tested before `delisted` | §2.4.2, §4.5 |
| Opus m7 | EP 8's parse is label-agnostic: it keeps the even-length hex lines that decode as a transaction, deduplicated and in order. Core-format fixtures (local date, translated labels) sit next to the wallet-format ones | §2.1, §4.10, §6 S6 |
| Opus m8 | S6 gets two review chains (Opus-h + Sol high). EP's review line names the behaviour changes | §6 EP, §6 S6 |
| Opus n1 | T4 rows and worlds: coordinator successful hand-off (IS-locked split), and a participant whose wallet recorded the registration (same rule) | §4.4 T4, §5.1 |
| Opus n2 | A session leaves In progress on `released: true`, an engine fact that survives a restart | §1.10 |
| Sol n1, Opus n3 | `FREE` entry `("dw_engine::masternodes::standby_file_transactions", "dw-engine/src/masternodes/shared/standby.rs")`, declared `pub fn` in that file; its stub exclusion is removed when S5 claims it | §4.8, §4.10 |
| Opus n4 | S4: dwcli and MG-13 test call sites pass `None`; a malformed or other-network `reviewed_destination` is `invalid_argument` before the P4 wait; the payout-key path passes an explicit `destination` | §3.2 |
| Opus n5 | `conflict_of`: lowest height kept, and at the same height `abandon_txid` first, then the smallest txid; `conflict_height` is its projection, tested against MG-04's `Conflicted` derivation. EP 7's "newest" means the highest `confirmed_height`, then `created_at`. `full_valid_now` is one value per masternode | §2.8, §4.4 change 3, §4.5, §4.7 |
| Opus n7 | `too_early` copy: "Available once the masternode list includes the registration." | §2.4.2, §3.8 |
| Sol n2 | D10 names `latest_confirmed_local` and `latest_attempt` | D10 |
| Opus Q6, Q10 | Q6 records the gated, overdue-badged two-step exit. Q10's amendment adds the IS-locked and the "may have gone out" copy | §8 |

## Change log (r2 → r3)

Every round-2 finding was accepted; the §2.4.2 narrowing stays. Each row names the section that answers it.

| Finding | What changed | Section |
|---|---|---|
| R1 (Opus M1, Sol M2, Sol m6, Opus m1) | EP 4 is now a full recovery contract: `abandon_wallet_tx { txid }` routed to Activity's Abandon; settlement `pending` / `confirming` / `settled { by, released }` from the engine's buried and release predicates; `handoff_error { tx, code }`; abandon retry only after a failed abandon hand-off | §2.3.3, §4.4, D12, D16 |
| R1.4 | §1.10's `sent` operations get [Send again] (`operation.debit`) and, once `overdue` (EP 2), the MG-04 D49 line | §1.10, §4.2 |
| R1.5 | T4 split: refused / unknown / recorded-then-abandoned hand-off, key change after out, failed abandon hand-off, 1–5 / 6 confirmations, ChainLock, reorg, another spend winning, durable release across restart; a fixture world and a Playwright case each | §4.4, §5.1, §6 S5/S6 |
| R1.6 (Sol m5) | Key-change `conflicted` row, with R1.2's treatment and a restart fixture | §2.3.1, §2.6, §5.1 |
| R2 (Sol M3, Opus m2) | EP 7 returns `at_registration` (envelope, else the history registration), `latest_confirmed_local` and `latest_attempt`, addresses from `terms.payout_address`; copy per attempt state; "as far as this wallet knows" | §2.4.1, §2.5, §4.7 |
| R3 (Sol M1) | S4 adds the owner-only `reviewed_destination` check at signing (engine change + contract bump), `withdraw_destination_changed`, the seam test for withdraw and retry, D11's "checked again just before signing" copy; S4 gets two review chains | §1.8, §3.2, §3.7, §3.8, §6 S4, D11 |
| Sol m1, Opus m3 | Validity from the engine's `full_valid` (zero-penalty branch included), exposed in EP 5 as `full_valid_now`; "valid now" | §2.8, §4.5 |
| Sol m7, Opus m4 | EP 5 returns history facts whenever history holds the registration, plus `eligibility: ok \| too_early \| delisted \| upgrade_unknown`; pane copy for each; T5 cases | §2.4.2, §4.5 |
| Opus m5 | Standby-file line extractor in the engine next to `file_text` (EP 8), round-trip test, zero-penalty duplicate imported once | §2.1, §2.8, §3.3, §4.10 |
| Sol m2 | Quote key `["mnPlatform", walletId, hash, "quote"]`; credits stay wallet-independent; snapshot cleared on wallet change; two-wallet test | §1.8, §3.1, §3.5 |
| Sol m3 | "Coinbase income to this wallet"; paid meta checks `last_payout.unattributed` | §1.2 |
| Sol m4 | T2 split: positive debit refuses under the cap; zero debit authorizes at cap 0 with no charge | §4.2 |
| Sol m8 | S5's hand-off-error badge counts registration sessions only; S6 adds maintenance | §3.6 |
| Sol n1 | G3: MG-04's non-shared ProTxs are v2; shared coordinators build v3 with an IPv4 service | §0.1 |
| Opus n1 | Owner-key copy split for evonodes and regular masternodes; "holds" wording widened | §1.4 |
| Opus n2 | EP 6 precedence stated; tracked-only + locked reads `registration_unavailable` | §4.6 |
| Opus n3 | The unlock line is secondary when the quote already holds a usable key | §1.8 |
| Opus n4 | A together send's `dissolve` operation is folded into its session row; S5 lists imported maintenance sessions read-only | §1.10 |
| Opus n5 | EP's `EXCLUDED` rows live in a stub `app/api/masternodes.rs`; the generated E0-13 doc is in EP's gate | §4.8, §6 EP |
| Opus n6 | No credits formatter exists: S1 adds `formatCredits` over `creditsToDuffs`, with a test | §1, §6 S1 |
| Opus n7 | `relay_floor` cited at `dissolve.rs:122-125` | §2.7, §4.9 |
| Opus n8 | Scratch paths dropped from the change log; `handoff_error` restructured (R1) | header, §4.4 |
| Q6, Q10 | Q6 keeps (a) with R1's two-step exit and settlement; Q10's amendment adds R1, the zero-penalty rule and R3's copy | §8 |

## Change log (r1 → r2)

Every finding was accepted. Each row names the section that answers it.

| Finding | What changed | Section |
|---|---|---|
| Rulings: EP slice | New slice EP: seven engine reads, each with its source file:line and a contract test, and the E0-13 rows | §4, §6 |
| Opus M1 | S4 owns `GrantPurposeSpec::MasternodeWithdraw`, its mapping, the generated TS and the E0-13 text; the §3.7 prerequisite is now "MG-13 on main" | §3.2, §3.7, §6 S4 |
| Opus M2 | Caps come from `ProTxOperation.debit` (EP 2) and `Standby.penalty` (EP 3), never from renderer arithmetic | §3.4, §4 EP 2–3 |
| Opus M3 | Platform reads moved to `["mnPlatform", hash, …]`, with their own refresh rule; D4's re-enable rule extended to the withdraw sheet (D11) | §1.8, §3.1, §3.5, D11 |
| Opus m1 | Operator keys match either BLS form, basic form shown first | §1.5, D8 |
| Opus m2 | Funding confirms print the exact `session.debit`, with a dust clause | §2.3.2 |
| Opus m3 | `mn_shared_import_file` runs the dialog and splits standby files in dw-app; paste copy for one standby line | §2.1, §2.8, §3.3 |
| Opus m4 | Sessionless standby recovery, from EP 5 (a read, not a probe; why in §4 EP 5) | §2.4.2, §4 EP 5 |
| Opus m5 | EP adds the E0-13 `FACADES` entry and every exclusion row; v19 numbering | §4.8, §6 EP/S1 |
| Opus m6 | One In progress group: every kind that isn't `confirmed` or `conflicted`; no Discard on `conflicted` | §1.10 |
| Opus m7 | A sentence for every listed error and warning | §3.8 |
| Opus m8 | D5 copies MG-09 D29 whole; §3.4 reworded | D5, §3.4 |
| Opus m9 | Full `kind × role × next` render table | §2.3.1 |
| Opus m10 | Together fee defaults to a size-aware figure, pinned by an engine test | §2.7, §4.9 |
| Opus m11 | Q2's text and §1.2 now agree: flagged records are listed under a qualified heading | §1.2, §1.9, §8 Q2 |
| Opus m12 | Missing withdraw, shared and vault scenarios added; re-read on `vaultLockState` | §5.1, §3.5 |
| Opus m13 | `credits{claimable_credits}`, not `all`; amounts go through the credits formatter | §1.8, §3.2 |
| Opus n1 | The tab is page state today; it becomes `UiSettings::masternodes_tab` | §1.1, D1 |
| Opus n2 | MG-13 head `b18152eb`; Q10 builds on it | header, §8 Q10 |
| Opus n3 | Invalidation uses the bare `["masternodes"]` prefix | §3.1, §3.5 |
| Opus n4 | "Credential (Pay's rule; may need no passphrase)" in place of "inline passphrase" | §3.4, throughout |
| Opus n5 | `mn_shared_export_standby` runs the save dialog itself and returns `Option<String>` | §2.8, §3.3, D6 |
| Opus n6 | `received_code` is import-only: it shows after an import and is gone on reopen | §2.3 |
| Opus n7 | GL waits for the row to read Active once the ProRegTx confirms; no operator node unless the test lasts through DKG | §5.4 |
| Opus n8 | `aria-live` regions, focus on the paste field, revealed keys masked in baselines | §5.3 |
| Opus n9 | S3's success copy names the payout-change barrier on evonodes | §1.7 |
| Opus n10 | G20 and G21 back in order | §0.1 |
| Sol M1 | Participant `funding_signed` offers Copy signed contribution and Save to file, and keeps them after a restart; fixtures and GL run through the hand-off | §2.3.1, §5.1, §5.3, §5.4 |
| Sol M2 | Estimates before selection, exact debit after; dust explained; equal to the grant cap | §2.3.2, §5.2 |
| Sol M3 | Reward destination is "At registration" plus the newest local update and its state (EP 7) | §2.4, §2.5, §4 EP 7 |
| Sol M4 | Recovery view from the engine (EP 4) drives close, abandon, resend and stale; failed hand-off and resend confirms; abandon pending vs settled | §2.3.3, §2.6, §4 EP 4, D12 |
| Sol M5 | Unlock to check added withdrawal keys, separate from "no accepted key" | §1.8 |
| Sol M6 | Registrar capability (EP 6) kept separate from holding an owner key; Add keys and `no_owner_key` copy fixed | §1.2, §1.4, §1.7, §4 EP 6, D13 |
| Sol M7 | History-based standby routes for a wallet without the session; session-only actions gated separately | §2.4.2, §2.8, §4 EP 5 |
| Sol m1 | Standby validity uses the engine's next-block rule and the stored variants | §2.8 |
| Sol m2 | `last_payout` qualified; "coinbase income" wording; a wallet-wide Generated filter | §1.2, §1.9 |
| Sol m3 | "Matched to these known masternodes"; 20 is a page size for operator and platform keys; role-specific exhaustion copy | §1.5, §1.6, D8 |
| Sol m4 | A change in any record's state refetches the quote; a fresh quote before each confirm | §1.8, §3.5 |
| Sol m5 | Slice-aware imports and badges; `create_standby` kept out of S5's badge; Q5's discovery reads land in S1 | §3.6, §6, D14 |
| Sol m6 | Import results shown until dismissed; role-correct matched copy; amount-plus-fee sentence; no-lag fallback | §2.3, §1.4, §1.8, §3.8 |
| Sol m7 | Base64 adapter rules for message saves; opens run in dw-app | §3.1, D15 |
| Sol G3, G5, G19 qualifications | G rows corrected | §0.1 |
| Q1–Q14 | MG's rulings recorded and applied | §8 |

One fact found while revising, reported rather than ruled on: mn §2.4 limits `preview_dissolve`/`dissolve` and
`update_share` to "a confirmed registration of its sessions". Only the standby, together and key-change calls read
the registration from history. So for a wallet without the session, "dissolve alone" is the standby's immediate
variant (§2.4.2). This narrows Opus m4's premise, not its fix.

## 0. Where things stand

- **The engine is done; nothing is bound.** Every `Masternodes` facade call of mn §2.1–§2.4 is implemented on main
  (shared slices 1a–2d, sending switch on since 1c, `can_send: true`). dw-app binds **none** of them:
  `NetworkSession::masternodes`, `masternode_keys` and `reveal_masternode_key` are E0-13 "deferred (MG)". MG-10's
  first job is that binding (§3), after EP (§4).
- **What the renderer has:**
  - the Masternodes route and page (MG-09), with tabs Governance | Username voting (MG-11), and the tab held in page
    state (`MasternodesPage.tsx:57-58`);
  - the `mg-*` styles, `MoreMenu`, `Notice`, `StatusChip`;
  - `useCredential`, which follows Pay's rule and may need no passphrase (`credential.tsx:57`), then
    `authorize(purpose)`;
  - the sidebar item gated on `prefs.showMasternodes` alone (`useMasternodesItem`, `governance/queries.ts:76-83`,
    0.1 release plan).
  - The generated bindings already hold grant purposes `masternodeOp{maxDuffs}` and `revealSecret`, and flow kinds
    `masternodeRegistration`, `keyChange`, `sharedMasternode` and `dissolution`. `AppEvent::masternodes{network}` is
    already forwarded; it carries no wallet id.
- **MG-13** (the credit withdrawal engine) is on its branch, being brought up to date by lane MN-13. Its facade adds
  six `Masternodes` calls and `FlowKind::MasternodeWithdraw`. **The host grant purpose is not MN-13's:** dw-app maps
  an engine-issued `MasternodeWithdraw` to `PlatformOp` "until MG-10 binds it" (`app/api/vault.rs:349-356`, MG-13), and
  excludes `Vault::masternode_withdrawal_signer` as deferred to MG-10 (`vault.rs:197`). S4 owns both (§6). S4 also
  owns one engine change to MG-13 (R3): the owner-key withdrawal's reviewed destination is checked at signing (§1.8).
  MG-13 itself stays as reviewed.
- **No clipboard read** in the renderer (only `writeText`), so every "Paste" in MG-00 is a paste field
  (text area, ⌘V/Ctrl+V), not a button that reads the clipboard.

### 0.1 Where MG-00 / UX §4.25 is silent or out of date against the engine

Each gap is cited as **G*n*** below. The Q1–Q14 calls are ruled (§8).

| # | MG-00 / UX says | The engine now | Consequence for the UI |
|---|---|---|---|
| G1 | D41: shared fee split by share, a per-participant fee maximum in the invitation, "Join: reserve 300.0001 Dash", re-invite when the fee rises | **MG-04b D9 replaces D41**: the coordinator pays the whole fee (default 100,000 duffs = 0.001 Dash). A participant's debit is its share, **plus any change too small to keep** (dust − 1 at most, MG-04b §4 460–463; `debit = inputs − change`, `mn/shared_records.rs:281-286`). The fee can change only in `draft` before any reply (`set_shared_fee`, D33); after that, a `fee_low{needed}` at lock means discard and start again | Rewrite the fee copy: invitation "Alice pays the network fee"; join and confirms per §2.3.2 (estimate before selection, exact `debit` after). No re-invitation flow. §2.2 |
| G2 | D33: v24 "not active" → "Needs the v24 network upgrade, not active on Mainnet yet." | `SharedCapability.v24` is `active` once a version-3 list entry was seen, else **`unknown`**, never `inactive` (MG-04b D3, `mn/shared.rs:189-204`); creation needs `active`. `evonode_shares` is always `inactive` (D4) | A third copy for `unknown` (Q9 ruled). Shared Evonode always disabled. |
| G3 | D39: IPv6 and Tor where the network takes v3 payloads | MG-04's non-shared registrations and registrar updates are **version 2, IPv4 only** (MG-04 D1, `mn/protx/build.rs:441`). A shared coordinator builds a ProRegTx v3 (`mn/shared/build.rs:195-209`) with an IPv4 service input (MG-04b D5). Imported participant messages may carry v3 or routable IPv6 forms, which `mn/shared/build.rs:393-418` handles | Field error in the wallet's own forms only: "Use an IPv4 address and port, like 203.0.113.7:19999. IPv6 and Tor addresses aren't supported yet." An imported message isn't refused by the UI for its form; the engine decides. |
| G4 | D42: payout "Checking…" until the ProTx-hash scan reaches the tip, then "checked up to block N" | The scan doesn't exist: `payout_checked_height` is always `None` (MG-03 D5, `mn/reads.rs:520-527`). The withdraw sheet doesn't depend on it (MG-13 D4) | Q1 ruled (b). |
| G5 | D43: a tracked masternode "finding its collateral", then votable with an attached voting key | `resolve_collateral` always refuses `unsupported{feature: "collateral_lookup"}` (MG-03 D11, `mn/reads.rs:862-870`). `voting_masternodes` lists keychain-derived voting keys only, **including a tracked node whose live voting key this wallet derives** (`governance/voting.rs:39-71`); attached voting keys aren't listed, though `vote` can sign with one | A tracked node with a derived voting key can already vote. One with only an attached voting key can't be shown as votable, and "finding its collateral" never ends. Q3 ruled (a). |
| G6 | D10/D46: per-masternode "Payouts in 30 days", shared "Your payouts 40% …", operator's share "not paid yet / paid first" | Once any version-3 entry was in the list (`any_shared_seen`), every coinbase output is `unattributed` (MG-03 D30): no row's `payouts_30d`; income not matched to a row is in the overview's `unattributed_30d`, whatever the cause (`mn/reads.rs:451-470`). `last_payout` still includes unattributed outputs (`:415-420`). A shared row is `shared_unsupported`: no payout address, no v3 roles, and `payouts()` refuses. Whether an operator payout is set (ProUpServTx) is MG-03b | Q2 ruled (a). Shared payout copy comes from EP 5/7 and the session (§2.4); no claim about the operator payout state (Q13). |
| G7 | D32: one zero-penalty standby, prepared and stored after registration, [Save a copy…] | Core's **pair**: full principal (penalty 0, valid from registered + period) and immediate (penalty paid, valid now); one copy at penalty 0. Validity is judged at `spend_height = max(list height, header tip) + 1` (`mn/shared/standby.rs:292-308`). The immediate variant is refused once a **stored** full one is valid (m-20, `:621-632`). An import can hold either variant (`shared_records.rs:493-499`). `create_standby`, `export_standby` and `send_standby` read the registration from history (`standby.rs:7-10`), and take grants. Stored bytes are plaintext signed transactions (MG-04b D21) | Standby UI per §2.8. Export and file import stay in dw-app (D6, Q11). |
| G8 | Dissolve together: "a session like registration (everyone signs), no penalty" | An actor share pays the fee from its principal. Approving someone else's request that names this wallet's share as actor needs `accept_actor_fee` (`shared_actor_fee{fee}`, m-15, `mn/shared.rs:1191-1215`) | Copy and a checkbox for the actor fee (§2.7). |
| G9 | Change keys together: "a new session where every owner approves; operator-key warning" | Funder-only send; base keys; `stale` when the list moves off the base; the funder's abandon (before `out_at`: release only, charged 0; after: spend a fee input at the relay minimum); warnings `operator_reset`, `usable_by_funder`; `dead{abandoned, inputs_spent, delisted}` (`mn/shared/key_change.rs:296-367`, `:1096-1143`) | States and copy for stale, abandon and dead (§2.6), from EP 4. |
| G10 | Silent | A session continues only on the installation that joined (`shared_message{not_participant}`, D10); sessions and standbys are not in `.dwbackup` (D19). Standby, together and key-change calls read the registration from **history**, so a restored wallet whose history holds the registration can still create, import and send a standby (`standby.rs:7-10, 282-325`) | Session continuation and history-based recovery are gated separately (§2.4.2, EP 5). Backups note in the pane. |
| G11 | Silent | After `out_at` a session can be stuck (`sent` / `funding_signed`) with coins committed; the exit is `abandon_shared_session`, pending until a conflict or the registration confirms (`mn/shared.rs:820-868`); "the UI offers it…; the wallet itself never decides" (MG-04b §9.3, D20) | Abandon per Q6 (ruled (a)) from EP 4's actions (§2.3.3). |
| G12 | One check code | Two codes after a merge: `received_code` on `SharedImport::Session` only (the pasted object's, `shared_records.rs:331-341`) and `message_code` (the merged session's). Import warnings sit beside `received_code`, not in `session.warnings`: `EditedMessage`, `UnvaluedInputs`, `EmptyService`, `SkippedSignatures{shares,count}`, `NoApprovals` (`:373-380`) | The import result is its own panel until dismissed (§2.3). |
| G13 | D44/D47: Sent → Done · Rejected · Couldn't confirm (two nonce branches) → Marked… | MG-13 D6: ten states (`signed`, `sent`, `refused`, `superseded`, `couldnt_confirm`, `marked_received`, `marked_not_received`, `done`, `sibling_paid`, `not_executed`). Couldn't confirm opens after undecided rounds, **not** on nonce errors; `not_executed` is a proved "Nothing was withdrawn"; `refused` **can't** say "Nothing was withdrawn" (MG-13 D14). A background recheck sends no event | Withdraw copy per MG-13 D14 (§1.8). Poll records while shown, and refetch the quote on any record-state change. |
| G14 | D28: "Payouts split 3 ways" for a v24 multi-payee masternode | Payees aren't read (v3 records are `shared_unsupported` until MG-03b) | Not shown. |
| G15 | D1: auto-show the item on roles, a collateral-shaped coin, tracking or work in progress | The 0.1 release plan gates the item on the setting only (`governance/queries.ts:76-83`); MG-09 D3 left roles/tracking to MG-10 | Q5 ruled (b), in S2 (§3.6). |
| G16 | §4.25.6: "Payouts go to (Keep / new address / another)" | `UpdateRegistrarRequest.payout` must always be named (`NewAddress` or `Address`), no Keep (MG-04 D10, `protx_records.rs:169-176`) | Q4 ruled (a). |
| G17 | §4.25.2: Masternode keys rows "Used by Home node · 203.0.113.7:19999" | `masternode_keys(wallet, role, start, count)` returns keys only. The engine's own "used" scan covers 20 owner/voting and 100 operator/platform indices, pending operations, shared sessions and both BLS forms (`mn/protx/keys.rs:23-70`, `mn/shared/flow.rs:221-246`), and isn't exposed | The renderer shows "Matched to these known masternodes", not usage (D8). |
| G18 | §4.25.5 External: one Prove step | `prepare_external_registration` saves `needs_signature`; `inputs_spent` → `refresh_external_registration` (new message); `submit_external_registration` checks the signature (`signature_mismatch`) | Prove step and In progress states (§1.6, §1.10). |
| G19 | §4.25.2 Track: Voting key "checked live", Owner key "checked the first time you use it" | `attach_key` also takes a `payout` role (MG-13's payout-key withdrawal), and needs the vault's full-scope data key (locked → `vault`). `KeyCheck` is `matched` (owner, payout or voting) / `key_mismatch` / `unchecked` (`mn/tracking.rs:110-137`). An attached owner key is **usable for registrar updates only when matched against a registration this wallet's records hold** (`mn/reads.rs:891-915`, `mn/protx/update.rs:222-246`); for a tracked-only node it stays `unchecked` | Track offers Owner and Payout keys; Voting per Q3. Holding the key and being able to update the registrar are separate facts (EP 6, §1.4). A locked vault asks to unlock (§1.4). |
| G20 | Withdraw "0.00002 credits is less than the Platform fee." | `withdrawal_amount_out_of_range{min, max}`, `withdrawal_amount_insufficient{needed, available}` (amount plus fee reserve), `no_identity`, `withdrawal_key_missing`, `withdrawal_no_payout_key`; checking reasons `spv_unsynced`, `unverified`, `payout_change_pending`, `platform_behind{lag?}` (lag optional, `withdrawal_records.rs:114`, MG-13) | One sentence each (§1.8, §3.8). |
| G21 | §4.25.2: after Release, the pane shows "Collateral released…" with [Lock it again] | `release_collateral` writes a `settings_kv` mark (`mn/protx.rs:990-1030`, key at `:1014`; prefix `RELEASED`, `mn/protx/ops.rs:33`) that only the follower reads (`ops.rs:689, 697`); no row or detail read returns it | EP 1 (Q14 ruled (a)). |

Nothing in MG-00 asks for protocol the pin lacks beyond G3–G6/G14 (all honest "not shown" or "not yet"). EP adds
reads of facts the engine already holds; it adds no protocol.

## 1. MG-10 screens and states

Layout, classes and most copy are MG-00's frames (`docs/design/ux/mg-00/*.html`); only additions and changes are
written out. Every screen has §4.25.1's shared states (loading, list failed, locked, watch-only, offline, network
mismatch); a row below lists only its own. Amounts in duffs go through the existing Dash formatter. Credits (raw u64
as a string, 10^11 per Dash) have no formatter today: `dashpay/withdraw.ts` takes a `credits: (c: string) => string`
callback, and callers convert with `creditsToDuffs` (`api/pending.ts:14`, as `coinjoin/move.ts:149`). S1 adds
`formatCredits(c)` to `masternodes/model.ts`, which shows the Dash value of `creditsToDuffs(c)` through the Dash
formatter, with a unit test (10^11 → "1 Dash", below one duff → "0 Dash"). Every credit figure below uses it. No raw
credit integer is ever printed.

### 1.1 Page frame (§4.25.1)

- Tabs **My masternodes | Governance | Username voting** (D3 order). Today the tab is component state
  (`MasternodesPage.tsx:57-58`, "page state, not `ui`"). It becomes the persisted setting
  `UiSettings::masternodes_tab` (dw-app `settings.rs`, serde-defaulted to `myMasternodes`), and MG-11 D2's page-state
  rule ends (D1). MG-09 D1's single-tab body moves under the Governance tab unchanged.
- Header per tab: My masternodes → [Set up a masternode] (filledBlue S) + More (Track a masternode… · Masternode
  keys… · Join a shared masternode…). Items appear only in the slice that builds them (§6).
- A list-height line, "Masternode list at block 1,214,880", from `mn_list_state` (shared by all three tabs).
- **Sidebar** visibility and badge: §3.6.

### 1.2 My masternodes list and pane (§4.25.2, mocks `mn-list`, `mn-empty`, `mn-banned`)

Data:
- `mn_list(wallet)` (`MasternodeOverview`) for the list and summary;
- `mn_masternode(wallet, hash)` (`MasternodeDetail`, with EP 6's `registrar`) for the pane;
- `mn_operations(wallet)` and `mn_shared_sessions(wallet)` for In progress (§1.10).

| Part | Source | Notes |
|---|---|---|
| Summary "Masternodes" | rows with `mine` / `tracked` | "3 yours" / "2 active · 1 tracked" |
| Summary "Payouts, last 30 days" | `payouts_30d`, `unattributed_30d`, `payouts_incomplete` | Q2 (ruled a). Attributed: "7.83 Dash in masternode payouts". With `unattributed_30d > 0`, a second figure "2.10 Dash of coinbase income to this wallet" (it counts every Generated record not matched to a row, mining to any address included, `mn/reads.rs:451-470`) with the note "The wallet can't match these payments to a masternode." When any row reads `payouts_incomplete`, the note adds "This network has shared masternodes, whose payouts it can't tell apart yet." Never "masternode rewards" for the unattributed figure. [See in Activity] → the wallet-wide Generated filter (§1.9) |
| Summary "Evonode credits" | `["mnPlatform", hash, "credits"]` per own evonode (§3.5) | Only with an evonode; "withdrawal being checked" while a record is undecided |
| Group Needs attention | `status == pose_banned && mine && (roles.owner \|\| roles.collateral)` | "noticed Oct 12" from `banned_seen_at` |
| Group In progress | §1.10 | |
| Group My masternodes | `mine`, not removed (shared: "you hold 1 of 3 shares" from `shares`, or EP 5's count without a session) | |
| Group Tracked | `tracked && !mine` | No "finding its collateral" meta (Q3) |
| Group Retired (N) | `status == removed` | collapsed |
| Row | `label ?? "Masternode {type_index}"`, `kind`, `service`, status chip, meta | Meta: "Paid Oct 13" **only when `last_payout.unattributed == false`**, read from the record itself (`MasternodePayout.unattributed`, `mn/records.rs:197-200`), since `payouts_incomplete` covers only the 30-day window and `last_payout` isn't limited to it (`mn/reads.rs:419-420`); otherwise no paid meta. "0 of 1 confirmations" (operation); "0.42 credits to claim" (formatter) |
| Chips | `enabled` → Active, `pose_banned` → PoSe banned, `removed` → Retired, `unknown` → Unknown, `unconfirmed` → Registering, operation `needs_signature` → Needs signature | MG-00 D5, D30 |
| Pane facts | `ListFacts` (service, type, operator key, voting address, platform node id/port, valid) and `RegistrationFacts` (registered height, collateral, owner, payout, operator reward, revoked) | Each only when present (Honest data). Payout per Q1 (§8). PoSe score "— (needs a full node)". Payees row dropped (G14) |
| Pane "In this wallet" | `MasternodeRoles` | `attached: None` (vault locked) → "Keys you added: unlock to see" [Unlock]. Attached keys of every role are listed, voting included (Q3), each with Remove |
| Pane actions | [Vote on proposals]; [Change keys or payout…] by EP 6; More | [Vote] goes to Governance, filter Needs my vote; it's disabled with "A banned masternode can't vote" or "Its voting key is not in this wallet". [Change keys or payout…] shows only for `registrar = available`; the other `registrar` values show the reason line of §1.7, never a disabled button. More: Rename… · Copy ProTx hash · Copy collateral outpoint (`CollateralState::Known`) · Add keys… (tracked) · Service update or revoke… · Release collateral… / Lock it again (EP 1, `roles.collateral`) · Stop tracking (tracked) |
| Payouts, 30 days | `mn_payouts(wallet, hash, since)` | Attributed records: "Payouts, last 30 days" + [See in Activity] (D7). Records flagged `unattributed` (v2 row on a v24 network, `mn/reads.rs:252-262, 756-780`): listed under **"Payments to this payout address (may include shared-masternode payouts)"**, not hidden, with [See in Activity] over those txids. When two of the wallet's rows share one payout address: "Matched by payout address: masternodes that share it show the same payments." `last_payout` in the pane: "Last payment to its payout address Oct 13" when unattributed |
| Banned notice | MG-00 `mn-banned` copy | adds "To move to another operator, use Change keys or payout." only with `registrar = available` |
| Evonode block | `["mnPlatform", hash, "credits" \| "quote"]` (S4) | Platform node ID; Platform status (`quote.status`: identity found, keys, lag); "Credits to claim 0.42" [Withdraw…] (§1.8) |
| Released collateral | `MasternodeRow.collateral_released` (EP 1) | true: "Collateral released: spending it ends this masternode." [Lock it again]; More shows Lock it again. false: More shows Release collateral… |

States:
- **Loading** (`list_state.synced == false`): "Loading the masternode list…"; own masternodes show `Unknown` with
  "list syncing".
- **List failed:** `list_state` has no failed state. A failing `mn_list` call shows MG-00's warning notice with
  [Try again].
- **Empty** (no rows, no operations, no sessions): MG-00 `mn-empty`. Adds "This wallet holds a 1,000 Dash output that
  can be a masternode collateral." when `collateral_coins` is non-empty.
- **Watch-only:** MG-00 notice; roles "(watched)"; every signing action disabled, with its reason.
- **Vault locked → unlocked:** the pane re-reads on `vaultLockState` (§3.5). Unlocking sends no `masternodes` event.

### 1.3 Track a masternode (sheet medium, `mn-track`)

Flow:
1. `mn_find(query)` (ProTx hash or `ip[:port]`) → result cards (`ListedMasternode`: type, service, valid, voting
   address, `tracked`).
2. Label (optional, ≤ `MAX_MASTERNODE_LABEL`).
3. Keys (optional, §1.4).
4. [Track]: `mn_track(hash, label)`, then each `mn_attach_key`.

The "After you track it, the wallet looks up its registration…" paragraph goes (G5, Q3).

Errors:
- `invalid_argument` → "Enter an IP address and port, or a 64-character ProTx hash."
- empty result → "No masternode with this address or ProTx hash on Testnet."
- `already_tracked` → "Already tracked as Friend's evonode." (label from `mn_list`)
- `list_unavailable` → "The masternode list is still loading. Try again in a moment."
- Network mismatch: a mainnet port on testnet gets §4.25.1's copy before the call.

### 1.4 Add keys (sheet small, from Track and from More ▸ Add keys…)

**Roles offered:**
- **Owner key**, evonode: "Lets you withdraw this evonode's credits. Changing its keys from here also needs its
  registration, which this wallet has only when a wallet loaded here holds it."
- **Owner key**, regular masternode: "Lets you change its keys from here once a wallet loaded here holds its
  registration."
- **Payout key** (evonodes only): "Lets you withdraw this evonode's credits to any address."
- **Voting key**: not offered (Q3). Voting keys already attached (dwcli) are listed under the pane's "Keys you
  added", with Remove.

Input is WIF or 64 hex, in a masked field, never echoed in an error (mn §4).

**Results** (`KeyCheck`, by role):

| Result | Copy |
|---|---|
| `matched{owner}` | "Matches this masternode's owner key." |
| `matched{payout}` | "Matches this masternode's payout address." |
| `matched{voting}` | "Matches this masternode's voting key." |
| `unchecked`, owner, evonode | "Saved. No wallet loaded here holds this masternode's registration, so it can't be checked here. Platform checks it when you withdraw." |
| `unchecked`, owner, regular | "Saved. No wallet loaded here holds this masternode's registration, so it can't be checked or used to change keys yet." |
| `unchecked`, payout | "Saved. Platform checks it when you withdraw." |
| `key_mismatch{role}` | "This key doesn't belong to this masternode." |
| uncompressed or other-network WIF | "This key is for Mainnet…" / "Use a compressed key." |

After an owner key is added, the pane reads EP 6's `registrar` again. A tracked-only node then reads
`registration_unavailable` (§1.7), so the user isn't promised a registrar change that can't happen.

The vault needs its full-scope data key. A locked wallet shows the app's unlock sheet (UX §4.4) first ("Unlock to add a
key"). The passphrase is never asked inline, because this is no grant.

Remove a key: the pane's "Keys you added" ▸ Remove (`mn_detach_key`).

### 1.5 Masternode keys (sheet large, `mn-keys`; IOS-083)

- **Segments:** Owner · Voting · Operator · Platform node → `mn_keys(wallet, role, start, 20)`.
- **Paging:** 20 is a page size for every role, with [Show more] (start + 20). The engine's own window is 20 for
  owner/voting and 100 for operator/platform (`mn/protx/keys.rs:23-28`).
- **Header:** "Keys this wallet derives for masternodes it registers. 2 matched to known masternodes."
- **Row:** "Keypair {index+1} · yNq3…8Rz1 · Matched to Home node · 203.0.113.7:19999", or "Not matched to a known
  masternode", with [Show] (D8). The copy never says "used" or "not yet used": usage is the engine's (G17).
- **Shown keypair:**
  - derivation path;
  - address, or the public key: for operator, the **basic** BLS form first (what ProRegTx v2 and `protx register`
    use), legacy as the secondary line; for the platform node, the node id;
  - copy.
- **[Reveal…]:**
  1. credential (Pay's rule; may need no passphrase) → `revealSecret` grant;
  2. `mn_reveal_key(wallet, role, index, grant)`: WIF (secp256k1), hex, Tenderdash form (platform node), monospaced,
     until Hide or close, with the per-role reveal-time note from MG-00.
  3. The secret is dropped from React state on hide, on unmount, on a wallet or network change, and on vault lock.
- **Watch-only:** `masternode.watch_only` → "This wallet can't show private keys: it holds no seed."

### 1.6 Registration wizard (sheet large, `reg-type` → `reg-done`; §4.25.5)

| Step | Calls | Adds to MG-00 |
|---|---|---|
| 1 Type | `mn_shared_capability` for the Shared card | Shared card per G2 (enabled only at `v24 == active`; `unknown` copy Q9); absent before S5 |
| 2 Collateral | `mn_list.collateral_coins` (ExistingUtxo rows: outpoint, address, confirmations, `user_locked`); balance for FundNew | External: outpoint + collateral address fields. When the address is this wallet's, refused before the call: "This output is in this wallet: choose Use a 1,000 Dash output already in this wallet." |
| 3 Operator | `mn_registration_keys(wallet, kind)` for self-run (operator key, basic form, + platform node id, keypair index shown) | IPv4 only (G3). Refusals: `protx_refused{bad-protx-dup-netinfo-entry}` "This server address is already registered."; `bad-protx-dup-key` "The operator key is already used by another masternode."; `protx-dup{operation}` "Another registration of yours is using this operator key. Finish or discard it first." |
| 4 Keys and payout | `mn_registration_keys` (owner and voting keypair index) | `keys_exhausted{role}`: owner/voting "This wallet has no unused {role} key left in its first 20."; operator/platform "…in its first 100." Voting address = owner's → field error "The voting address can't be the owner address." |
| 5 Review | `mn_preview_registration(wallet, request)` → `RegistrationPreview` (terms, fee, `total_debit`, `collateral_amount`) | Fee and total from the preview; `send.amount_exceeds_balance` etc. as Pay's sentences |
| 5′ Register | re-read preview (D5) → `masternodeOp{D5 cap}` → `mn_register(wallet, request, grant)` | If the re-read differs from the review, the new figures show again and Register turns off until they're on screen (D5, D4). Returns the operation: `sent` → step 7; `not_sent{last_error}` → "Not sent: {sentence}. Nothing left this wallet." [Try again] (resend, `masternodeOp{operation.debit}`) |
| 6 Prove (External) | `mn_prepare_external_registration` at Review's [Continue] (no grant; saves `needs_signature`, locks the fee inputs) → `SignRequest{address, message}`; [Register] → `masternodeOp{operation.debit}` → `mn_submit_external_registration(wallet, op, signature, grant)` | The live signature check is the submit's: `signature_mismatch` "This signature isn't from the collateral's address over this message." `inputs_spent` → "The coins for the fee were spent. Sign the new message." [Get a new message] (`mn_refresh_external_registration`) |
| 7 Sent | the operation | `Registering` "waiting for 1 confirmation"; copy per hosted/self-run |

Closing after step 1 asks "Discard this registration?" (nothing was sent). A prepared External registration is
**not** discarded by closing: it is already saved (`needs_signature`, In progress) and closes silently. Its row has
[Continue] (reopens step 6) and More ▸ Discard (§1.10).

### 1.7 Change keys or payout, Release, Operator sheet (§4.25.6, §4.25.7)

**Change keys or payout** (`upd-registrar`):
- **Shown only when** `MasternodeDetail.registrar` is `available{signer}` (EP 6). The other values replace the
  action with a reason line:
  - `registration_unavailable`: "Changing keys needs this masternode's registration, which this wallet doesn't
    hold. You can still withdraw evonode credits with an added owner or payout key."
  - `no_owner_key`: "This wallet doesn't hold this masternode's owner key."
  - `vault_locked`: "Unlock to check the keys you added." [Unlock]
  - `shared`: the row offers §2.4's actions instead.
- **Preview:** `mn_preview_update_registrar(wallet, request)` on every change of the form (debounced): its
  `operator_changes`, `voting_changes`, `payout_changes`, `pose_bans` and fee, and `owner_key` ("Signed with its owner
  key from this wallet" / "…the owner key you added"). The PoSe-ban warning (D29 verbatim) shows when `pose_bans`.
  Payout per Q4 (§8).
- **Send:** grant `masternodeOp{preview.total_debit}` → `mn_update_registrar`. The sent update shows in In progress
  until confirmed (§1.10).
- **Success:** "Change sent. It applies once it's in a block." For an evonode whose payout changes, add: "Credit
  withdrawals wait until Platform sees the new payout address." (MG-13 payout-change barrier, P16; the evonode block
  reads `checking{payout_change_pending}` until it clears.)
- **Refusals** (§3.8): `update_pending{operation}`, `update_superseded{operation}`, `in_flight`, `no_owner_key`,
  `protx_refused{bad-protx-payee-reuse}`, `operation_state`. `bad-protx-shared-mn` is never reached, since shared
  rows don't offer this sheet.

**Release collateral** (`upd-release`):
- A checkbox, then `mn_release_collateral(wallet, hash)`. No grant (it sends nothing).
- [Lock it again] → `mn_lock_collateral`. Which of the two shows comes from EP 1, and survives a restart.
- `collateral_unavailable` → "This wallet doesn't hold this masternode's collateral."

**Service update or revoke…** (`mn-operator`): an explanation sheet, custody-specific (MG-00 D24). No calls beyond
`roles` and `ListFacts`.

### 1.8 Withdraw credits (sheet small, `mn-withdraw`, `mn-withdraw-pending`; engine MG-13)

**Reads and refresh** (§3.5):
- **The quote**, `["mnPlatform", walletId, hash, "quote"]` → `mn_withdrawal_quote(wallet, hash)`. It is keyed by
  wallet because its held keys and default destinations are the wallet's (MG-13 `withdrawals.rs:65-83`,
  `withdrawal/check.rs:332-342`); credits stay `["mnPlatform", hash, "credits"]`. It is re-read:
  - on open;
  - on [Try again];
  - every 60 s while the sheet or the evonode block is shown;
  - on a `masternodes` event only while the quote reads `checking{platform_behind}` (that event is
    `withdrawal_ready`);
  - whenever a polled record changes state (Sol m4);
  - and always once more just before the confirm step.
- **Records**, `["masternodes", "withdrawals", hash]` → `mn_withdrawals(hash)`: polled every 60 s while shown and a
  record is undecided. Background rechecks send no event.

**Key, amount and destination:**
- Key choice: Owner key (destination fixed: `owner_destination`) or Payout key (any address; default
  `payout_destination`). "Signed with: …" comes from `HeldWithdrawalKey.source`.
- Amount + [Max]. [Max] fills `claimable_credits`, and the call sends `WithdrawAmount::credits{claimable_credits}`,
  never `all` (MG-13 `platform/credits.rs:144-151`). Every figure goes through the credits formatter.

**Locked vault and added keys** (Sol M5):
- MG-13 reads attached key ids with the vault's full-scope key, and finds none while it is locked
  (`mn/withdrawal/session.rs:180-204`, MG-13). So a quote taken while locked can't see an attached owner or payout
  key.
- Rule: when the vault is locked **and** the row's `roles.attached` is `None` (custody unknown), the sheet shows
  "Unlock to check added withdrawal keys". [Unlock] opens the app's unlock sheet; after unlock the quote and the
  detail are re-read.
- When the locked quote already holds a usable key (the keychain owner key of an own evonode), that line is a
  secondary note under the key choice, not the sheet's state: the withdrawal with the held key proceeds as usual
  (Opus n3).
- `withdrawal_key_missing` reads "This wallet doesn't hold a key Platform accepts for this evonode." only when
  custody is known: vault unlocked, or `roles.attached` is `Some`.
- Relock while open: back to the unlock line. An attached key that was chosen is cleared, and Withdraw turns off.

**Consent** (D11):
- The confirm shows the amount, the fee (`rules.fee_credits`), the key and the destination.
- If any of them changes after a re-read, [Withdraw] turns off. It turns back on only after the new figures have been
  on screen, with the note "The figures changed. Check them again."
- A wallet change clears the reviewed snapshot and closes the sheet.
- The grant is built from the figures shown: `masternodeWithdraw{maxCredits: amount + fee_credits, destination}`
  (owner key: `None`), then `mn_withdraw_credits(wallet, hash, key, WithdrawAmount::credits{amount}, destination,
  reviewed_destination, grant)`.
- **Owner key: the reviewed destination is checked at signing** (R3). An owner-key transition carries no output
  script by protocol (dw-vault `withdrawal.rs:207-213`, MG-13): Drive pays whatever payout address is registered
  when it executes. MG-13's final look reads the window's script (`withdrawal/sign.rs:184-190`) but doesn't compare
  it with what the user saw. S4 adds `reviewed_destination` (the address shown, owner key only): under the identity
  lock, after the final look and before the grant is used or anything is signed, the engine compares it with the
  final look's window script. A mismatch is `withdraw_destination_changed` and signs nothing; the sheet re-quotes and
  asks for review again (§3.8).
- Owner-key confirm copy: "Platform pays the evonode's payout address, yPay8vK2…Wm4c. The wallet checks it again just
  before signing. A payout change made after signing can still redirect this payment before Platform carries it
  out." (the last sentence is MG-13's caveat).

| Engine | Sheet / pane copy |
|---|---|
| quote loading | "Checking Platform for credits…", Withdraw off |
| `platform_unavailable` | "Couldn't reach Dash Platform. Try again later." [Try again] |
| `no_identity` | "Platform has no identity for this evonode, so it has no credits to withdraw." |
| `checking{spv_unsynced}` | "Waiting for the wallet to finish syncing…" |
| `checking{unverified}` | "Waiting for an answer from Platform the wallet can verify…" |
| `checking{payout_change_pending}` | "A payout change you sent hasn't reached Platform yet. Withdrawing waits for it." |
| `checking{platform_behind{lag}}` | "Waiting for Platform to catch up with Core ({lag} blocks)…"; with no lag: "Waiting for Platform to catch up with Core…" (MG-13 D14) |
| locked, custody unknown | "Unlock to check added withdrawal keys" [Unlock] |
| `withdrawal_key_missing` (custody known) / `withdrawal_no_payout_key` | "This wallet doesn't hold a key Platform accepts for this evonode." / "Platform has no single payout address for this evonode, so the owner key can't withdraw." |
| `withdrawal_amount_out_of_range{min,max}` | "Withdraw between {min} and {max} credits." |
| `withdrawal_amount_insufficient{needed,available}` | "{available} credits isn't enough for this amount plus the Platform fee: it needs {needed}." |
| `signed`, `sent` | MG-00 D44's "Withdrawal sent; checking its result" block, with the warning "Don't send it again from another device." Withdraw off: "Waiting for the result of the withdrawal already sent" |
| `refused` | "Platform refused the withdrawal: {state_reason}." [Try again] (`mn_retry_withdrawal`, a new grant from a fresh quote). Never "Nothing was withdrawn" (MG-13 D14) |
| `superseded` | hidden behind its sibling, in the record's history: "Replaced by your next attempt" |
| `couldnt_confirm` | "Couldn't confirm this withdrawal yet. Platform answered, but the wallet can't tell whether it went through." Then MG-00 D47's "Check whether 0.4199 Dash arrived at yPay8vK2…Wm4c…", with [It arrived] / [It didn't arrive…] (`mn_mark_withdrawal`). D47's two nonce-branch sentences go (G13) |
| `marked_received` / `marked_not_received` | MG-00 D47 copy, plus "You marked this; the network hasn't confirmed it." |
| `done` | "Withdrawal done", paid to `paid_to`, with document status ("Queued", "Pooled", "Broadcast", "Complete", "Failed", "Expired") |
| `sibling_paid` | "Paid by your later attempt" / "Paid by the earlier attempt" |
| `not_executed` | "Nothing was withdrawn." Withdraw on |
| late proof (`late_proof_link`) | MG-00 D47's notice, linking the later record |
| `withdrawal_pending` (another wallet's open record) | "A withdrawal from this evonode is still being checked." |
| `withdraw_destination_changed` (S4) | "The payout address changed just before signing. Nothing was signed. Check the new address." → re-quote, Withdraw off until the new figures are on screen |
| call errors | §3.8 (destination checking, unverified, retry/mark state, not found) |

Watch-only: not offered. Shared evonodes don't exist (G2), so `withdrawal_shared` is mapped but unreachable.

### 1.9 Activity filters (MG-00 D10, Q2)

- **Per masternode:** [See in Activity] passes the txids from `mn_payouts` in route state. Activity shows "Payouts of
  Home node" (attributed) or "Payments to Home node's payout address" (flagged) as a removable filter chip over those
  txids.
- **Wallet-wide** (the summary's [See in Activity]): route state `{type: "generated"}` filters the existing history
  rows whose `TxTypeName` is `generated`. The chip reads "Coinbase income". This is the aggregate source Q2 needs;
  shared-masternode `payouts` refuses, so no per-node txids exist for it.
- No history DTO change (MG-03 D10).

### 1.10 In progress group (Opus m6)

Lists every `ProTxOperation` whose state isn't `confirmed` or `conflicted`, of every kind: `register`,
`update_registrar`, `update_share`, `dissolve`. Also every shared session not settled (`confirmed`, `conflicted`,
discarded).

The row title comes from `kind`: "Registration", "Key change", "Payout change", "Dissolution" (with
"standby", "together" or "alone" from the record). It shows `last_error` when present.

| State | Row | Action |
|---|---|---|
| `needs_signature` (External) | "Needs signature" | [Continue] (reopens step 6); More ▸ Discard |
| `inputs_spent` | "The coins for the fee were spent" | [Get a new message]; More ▸ Discard |
| `signed` / `not_sent` | "Not sent: {sentence}" | [Send again]: confirm "Send it again. It costs at most {debit}." → `masternodeOp{operation.debit}` (EP 2) → `mn_resend_operation`; More ▸ Discard |
| `sent` | "{title} · waiting for 1 confirmation" | [Send again]: confirm "Send the same transaction again. It costs at most {debit}." → `masternodeOp{operation.debit}` → `mn_resend_operation` (it accepts `sent`, `mn/protx.rs:695-700`, MG-04 D7). Not discardable (MG-04 D6) |
| `sent`, `overdue` (EP 2's shared `overdue` rule, §4.2) | adds the MG-04 D49 line, for rows with fee inputs (`register`, `update_registrar`, `update_share`): "If this never confirms: abandon its transaction in Activity, then unlock one of its coins and spend it." [Open in Activity] (filtered to `txid`). A `dissolve` row has no fee inputs and locks nothing (mn §2.4), so it reads instead: "It locks none of your coins. It confirms, or another dissolution replaces it." | [Send again] stays |
| `conflicted` | not in this group: it shows once as a notice "Replaced: a coin it used was spent elsewhere", and clears when the follower settles it | **no action** (`discard_operation` accepts only `needs_signature`, `inputs_spent`, `signed`, `not_sent`) |

Discard confirm: "Discard this {title}? Its coins are released." → `mn_discard_operation`.

**Session rows** render §2.3.1's table, with these rules (Opus n4, R1.2):
- A dissolve-together send also saves a `dissolve` operation (MG-04b D95, `mn/shared.rs:1221-1226`). The operation
  whose `txid` equals a session's `txid` is folded into that session's row, so one masternode step is one row.
- A session whose stage is `conflicted` stays listed while EP 4 reads `confirming`, or `settled` with
  `released: false`, so its recovery pane (§2.3.3) stays reachable until the coins are back. It leaves the group on
  `released: true` (Opus r3 n2). That is an engine fact, so the rule holds across a restart; no per-viewer
  acknowledgement is stored.
- In S5, an imported key-change or together session is listed read-only ("Saved: this version can't open it yet"),
  with no action and no badge (D14). S6 gives it its route.

## 2. MG-10b: shared masternodes

All of §4.25.8's layout stands, with G1/G2/G7–G12 applied. Every call is `mn_shared_*` (§3.3). The session record
(`SharedSession`) drives every session screen: `stage`, `role`, `kind`, `next`, `rows`, `terms`, `warnings`,
`revision`, `consent_hash`, `debit`, `fee`, `message_code`, and EP 4's `recovery`. The UI never infers a stage or a
recovery fact; it renders `next` and `recovery` (D2, D12).

### 2.1 Start, join or continue (sheet large, `sh-start`)

**Entry points:** reg-type's Shared card, More ▸ Join a shared masternode…, and In progress rows. **Start one** →
§2.2.

**Join or continue:**
- a paste field ("Paste the message you received, or one transaction line from a standby file"), focused when the
  sheet opens → `mn_shared_import(wallet, text, target)`;
- [Open file…] → `mn_shared_import_file(wallet, target)`. dw-app runs the open dialog (filters `.json` for messages
  and `.txt` for standby files). A standby file's transactions are extracted by the engine's
  `standby_file_transactions` (EP 8), which reads the transaction lines of this wallet's files and of Dash
  Core-written ones (local dates, translated labels) alike and drops the zero-penalty file's repeated line, and each
  is imported once. It returns only `Option<Vec<SharedImport>>` (`None` on cancel). No file bytes
  reach the renderer (Q11, D6).

**Import result** (§2.3) routes by slice (D14):
- `Session` (registration) → its session sheet;
- `Session` of kind `key_change` / `dissolve_together` → §2.6 / §2.7 from S6. Before S6: "Saved in this wallet.
  This version can't open key changes or dissolutions yet." (read-only, no route).
- `Standby` → §2.8's import result from S6; before S6 the same read-only line.

"Sessions in progress" lists `mn_shared_sessions` not settled, newest first. The Shares fact loses the fee clause
(G1): Your coins "Stay in this wallet, reserved for the session, until the registration is sent. The coordinator
also reserves the network fee."

Import refusals (`shared_message{reason}`): one sentence each, never echoing the text (§3.8). They include
`other_session` "This reply belongs to session 7B21E0, not 3F9A2C." (codes from the stored sessions), and
`not_participant` "Continue this session on the device that joined it." (G10).

### 2.2 Coordinator setup (wizard, `sh-setup`, `sh-terms`)

Steps **Shares · Masternode · Exit terms · Your coins · Invite**, each mapping to one `StartSharedRequest` field;
nothing is sent to the engine before [Create invitation].

| Step | Field | Validation (before the call, mirroring MG-04b §5) |
|---|---|---|
| Shares | `shares[]` (label, amount), row 0 the coordinator | 2–8 rows, each ≥ 100 Dash, summing to exactly 1,000; labels non-empty and unique after trim. Type card Evonode disabled: "Shared evonodes need a later network upgrade, not active on this network yet." |
| Masternode | `terms.service` (IPv4, G3), `terms.operator` (Given, or the keychain via "I run it myself"), `terms.operator_reward` (bp), `terms.voting` (keychain default / Address), `node_run_by` ("Who runs the server", free text, optional) | as §1.6 step 3 |
| Exit terms | `early_period_blocks` presets 0 / 52,560 / 105,120 / 210,240 / 420,480; `early_penalty` < smallest share, 0 without a period | The ZeroPenalty warning text is Core's |
| Your coins | `reward` (Refund / NewAddress / Address); `fee` default 100,000 duffs, shown as "Network fee 0.001 Dash, paid by you" with [Change] | An **estimate** (§2.3.2): "About 400.001 Dash of your 1,204.37 will be reserved: your 400 Dash share and the 0.001 Dash network fee, plus any change too small to keep." |
| Invite | `mn_shared_start(wallet, request)` → `SharedOutcome{session, message}` | No grant (it signs nothing; it reserves coins). [Copy invitation] (`writeText`), [Save to file…] (D15), session code, check code `message.code`. After this the exact `debit` is known (§2.3.2) |

Fee change after Invite and before any reply: session sheet ▸ More ▸ Change network fee… → `mn_shared_set_fee`. After
a reply it is absent (`shared_stage{replied}`). `fee_low{needed}` at lock reads "The registration needs a network fee
of at least {needed}. Discard this session and start again with a higher fee." (Q8).

### 2.3 Session sheet (`sh-session`), stages and consent

- **Stages strip:** **1 Invite · 2 Collect details · 3 Lock terms · 4 Approvals · 5 Signatures · 6 Send** (MG-00
  D31), from `stage` (MG-04b §2.3's table).
- **Participants list** from `rows`: label, share, and chips from `contributed`, `approved` and `funding_signed`;
  "you" on `mine`.
- **Terms** from `terms`, plus `service_read` when it differs ("Core reads this as …").
- **Codes:** the session code and `message_code`.
- **Import result panel** (G12, Sol m6, Opus n6). After an import, a panel above the footer shows the import's own
  facts until the user dismisses it:
  - "You pasted message {received_code}. Ask the sender to read their code: it should be {received_code}."
  - the import's warnings (§3.8), including `skipped_signatures{shares,count}` and `no_approvals`.

  It is held in component state from the `SharedImport` result and isn't rebuilt by a session refetch.
  `received_code` exists only on the import, so it is gone when the sheet is reopened.
- **Session warnings** (`session.warnings`): `edited_message`, `unvalued_inputs{shares}`, `empty_service` and
  `zero_penalty`, as §3.8.
- `aria-live="polite"` on the stage strip and the footer status (§5.3).

#### 2.3.1 Render table: `kind × role × next` (Sol M1, Opus m9)

`next` is the render key (D2). Rows below are the engine's own rules:
- registration: `next_of`, `mn/shared/store.rs:615-647`;
- key change: `mn/shared/key_change.rs:1279-1287`;
- together: `mn/shared/together.rs:1129-1133`.

"Recovery" is EP 4's `recovery.actions`, shown as secondary buttons on any row where the engine lists them (§2.3.3).
Labels: C coordinator (key change: funder), P participant (key change: approver), * either.

| Kind | Role · stage (condition) | `next` | Shows | Primary | Call (grant) |
|---|---|---|---|---|---|
| registration | * `invited` | `join` | MG-00 `sh-join` with G1 copy: share, terms, payout split now / once the operator payout is set, Leaving, "Alice pays the network fee", Your coins as an estimate (§2.3.2); "Nothing is sent to the network now" | [Join: reserve about 300 Dash] (no credential: it reserves coins, signs nothing) | `mn_shared_join(wallet, s, {share_index, reward})`, no grant |
| registration | C `draft`, not every row in | `copy` | "Waiting for replies: 2 of 3 joined" | [Copy invitation] · [Save to file…] | `mn_shared_message` |
| registration | P `draft` (joined) | `copy` | "Send your reply to Alice. Then wait for her to lock the terms." Exact debit (§2.3.2) | [Copy reply] · [Save to file…] | `mn_shared_message` |
| registration | C `draft`, every row and contribution in | `lock` | the full terms table, share table, fee and exit terms; "Locking the terms signs your consent. Your coins move only after everyone approves and you send." | credential → [Lock terms] | `mn_shared_lock(wallet, s, revision, grant)`: `masternodeOp{0}` |
| registration | P `signing`, not yet approved | `approve` | the terms; **diff against `approved_terms`** when present ("Changed since you approved: penalty 20 → 30 Dash"); exact debit (§2.3.2) | credential → [Approve] | `mn_shared_approve(wallet, s, consent_hash, grant)`: `masternodeOp{0}` |
| registration | P `signing`, approved | `copy` | "Send your approval to Alice. Then wait for everyone's approvals." | [Copy approval] · [Save to file…] | `mn_shared_message` |
| registration | C `signing` | `copy` | "2 of 3 approved the locked terms" | [Copy locked terms] · [Save to file…]; More ▸ Unlock terms (`mn_shared_unlock`: "Unlocking drops everyone's approvals.") | `mn_shared_message` |
| registration | P `combined` | `sign` | "Sign your coins into the registration: {debit} leaves this wallet when Alice sends it. After you sign, only Abandon gets them back." (§2.3.2) | credential → [Sign my coins] | `mn_shared_sign(wallet, s, grant)`: `masternodeOp{session.debit}` |
| registration | P `funding_signed` | `copy` | "Send your signed contribution to Alice. She can send the registration once she has everyone's." Kept after close and restart: the row stays in In progress with the same buttons | [Copy signed contribution] · [Save to file…]; recovery | `mn_shared_message` |
| registration | C `combined` / `funding_signed`, some participant not signed | `copy` | "Waiting for signed contributions: 1 of 2" | [Copy signing request] · [Save to file…] | `mn_shared_message` |
| registration | C `combined` / `funding_signed`, every other input signed | `send` | "Everyone signed. Send the registration: {debit} leaves this wallet." (§2.3.2) | credential → [Send registration] | `mn_shared_send(wallet, s, grant)`: `masternodeOp{session.debit}` |
| registration | * `sent` | `wait` | "Sent · waiting for 1 confirmation"; hand-off error, settlement, `blocked` and the two-step exit per §2.3.3 (the exit is a collapsed secondary line until `overdue`) | none; recovery (`resend`, `abandon`, `resend_abandon` or `abandon_wallet_tx`, when listed) | per action |
| registration | * `confirmed` | `create_standby` / `nothing` | moves to the pane (§2.4); `create_standby` prompts there from S6 | — | — |
| registration | * `dead{inputs_spent}` | `nothing` | "One of your reserved coins was spent elsewhere, so this session can't continue." | recovery (discard) | `mn_shared_discard` |
| registration | * `conflicted` | `nothing` | by EP 4's settlement (§2.3.3): `confirming` → waiting copy; `settled { by: abandon_tx }` → "Abandoned"; `settled { by: other_spend }` → "This registration was replaced: a transaction spending one of its coins confirmed." | none | — |
| key_change | * `signing`, own share unsigned | `approve` | §2.6 | credential → [Approve] | `mn_shared_approve_maintenance(…, false)`: `masternodeOp{0}` |
| key_change | * `signing`, own shares signed | `copy` | "Send your approval to {funder}." | [Copy approval] · [Save to file…] | `mn_shared_message` |
| key_change | C `combined` | `send` | "Every owner signed. Send the key change: you pay the {fee} network fee." | credential → [Send] | `mn_shared_send_maintenance`: `masternodeOp{session.fee}` |
| key_change | P `combined` | `copy` | "You hold every signature. Send them to {funder}, who sends the change." | [Copy signatures] · [Save to file…] | `mn_shared_message` |
| key_change | C `sent` | `wait` | "Sent · waiting for 1 confirmation"; hand-off error, settlement, `blocked` and the two-step exit per §2.3.3 (funder only; collapsed until `overdue`) | recovery | per action |
| key_change | C `conflicted` (out, a fee input spent; `key_change.rs:325-328`) | `nothing` | by EP 4's settlement, with the key change's own release rule (`key_change.rs:1198-1215`): `confirming` → waiting copy; `settled { by: abandon_tx }` → "Cancelled. The fee coin was spent back to this wallet."; `settled { by: other_spend }` → "This key change was replaced: a transaction spending one of its fee coins confirmed." Kept after restart | none | — |
| key_change | P, after the funder's send | by the list (`key_change.rs:336-367`) | an approver holds no `funded` record, so it never reads `sent` or `conflicted`: its stage comes from the list and the signatures only (Sol r3 m3). The list takes the requested keys → `confirmed` ("Keys changed."); the list takes other keys → `stale` (the row below); the masternode leaves the list → `dead{delisted}`; otherwise it stays `combined` / `signing` with its copy row, whatever happened to the funder's fee coin. There is no approver `conflicted` row. More ▸ Discard while EP 4 lists `discard` (its row is never out, `key_change.rs:1149-1156`; Opus r4 n2) | per those rows; Discard | `mn_shared_discard` |
| key_change | C `stale` | `abandon` | "The masternode's keys changed since this request was made, so it can't be sent." | [Abandon…] (§2.3.3) | `mn_shared_abandon`: `masternodeOp{action.charge}` |
| key_change | P `stale` | `nothing` | the same sentence, plus "Only {funder} can cancel it." | none | — |
| key_change | * `dead{abandoned \| inputs_spent \| delisted}` | `nothing` | §2.6 | recovery (discard) | `mn_shared_discard` |
| key_change | * `confirmed` | `nothing` | "Keys changed." | — | — |
| dissolve_together | * `signing`, own share unsigned | `approve` | §2.7, with the actor-fee checkbox when it applies | credential → [Approve] | `mn_shared_approve_maintenance(…, accept_actor_fee)`: `masternodeOp{actor_fee ?? 0}` |
| dissolve_together | * `signing`, own shares signed | `copy` | "Send your signature to the others." | [Copy approval] · [Save to file…] | `mn_shared_message` |
| dissolve_together | * `combined` | `send` | "Everyone signed. Anyone holding every signature can send it." | credential → [Send dissolution] | `mn_shared_send_maintenance`: `masternodeOp{0}` |
| dissolve_together | * `sent` | `wait` | "Sent · waiting for 1 confirmation" | none | — |
| dissolve_together | * `confirmed` / `conflicted` / `dead` | `nothing` | "Dissolved." / §2.7's notes | recovery (discard) | — |

Any `(kind, role, stage)` not in the table renders its `next` with the generic button for that `next`, and the
fixture test fails, so a new engine combination can't pass unnoticed (§5.2).

#### 2.3.2 Funding disclosures: estimate, then exact debit (Sol M2, Opus m2)

- **Before contribution selection** (`invited`, and the setup wizard's Your coins), figures are labelled
  **estimates**: "About 300 Dash: your share, plus any change too small to keep." No grant is built from an
  estimate.
- **After selection** (joined, or the invitation created), every confirm and every coins line prints the exact
  `session.debit`, with its parts:
  - participant: "{debit} leaves this wallet: your {share} share";
  - coordinator: "{debit} leaves this wallet: your {share} share and the {fee} network fee";
  - when `debit > share (+ fee)`: "…and {dust} of change too small to keep, which goes to the network fee". Here
    `dust = debit − share − (coordinator ? fee : 0)`.

  These lines appear on Approve (as "if the registration is sent"), Sign, Send and the Close dialog.
- The grant cap is that same `session.debit`. The displayed amount and the cap come from one value, and a unit test
  asserts they're equal for both roles, with and without dust (§5.2).
- A `send.grant_exceeded` means the record moved: re-read the session and show it again (D4).

#### 2.3.3 Recovery: close, hand-off errors, resend, abandon, settlement (Sol M4, R1, Q6)

Every recovery control comes from EP 4's `SharedSession.recovery` (§4.4):
- `out_at`, and `overdue` (advisory, §4.2);
- `settlement: none | pending { txid } | confirming { by, confirmations, chainlocked } | settled { by: abandon_tx |
  other_spend, released }`;
- `handoff_error: Option<{ tx: registration | abandon, outcome: refused | unknown, code, resend_refused }>`;
- `abandon_spend: Option<{ txid, signed_at, legacy, overdue, state? }>`, whose `state` (`refused | uncertain |
  in_history | abandoned_in_activity | legacy`) is set only while the settlement is `pending` and the session isn't
  terminal, and `attributed` (every abandon txid the session signed);
- `blocked: Option<{ txid?, reason: in_flight | instant_locked | spv_stopped | rescanning | coin_elsewhere |
  no_ownership_path | too_small }>`;
- `actions[]`: `resend { charge }`, `abandon { charge, spends, replaces }`, `resend_abandon { charge }`,
  `abandon_wallet_tx { txid }`, `resend_wallet_tx { txid }`, `discard { releases }`. They are a safe subset of what the engine accepts: the pane
  offers nothing else, and a button is shown only for a listed action.

The engine settles terminal state first (the registration or key change won, or a conflicting spend is in a
block): then the pane shows only the settlement copy below, with no recovery button. The engine computes all of it
with the predicates its own calls use. Nothing is inferred from `updated_at` (bumped by
every envelope save, `mn/shared/flow.rs:438-446`), from `stale`, from `SharedStage` alone, from the copy's own
state, or from what history doesn't hold (D12).

- **Close** (`sh-close`):
  - With `discard` among the actions (nothing out): **Keep and close** / **Discard and release {debit}**
    (`mn_shared_discard`) / Cancel.
  - With no `discard` (out): the sheet closes without a question. The row stays in In progress.
- **Hand-off of the {registration | key change} failed** (`handoff_error.tx == registration`, stage `sent`):
  - `refused`: "The wallet couldn't hand the {registration | key change} to the network: {sentence}."
  - `unknown`: "The wallet couldn't confirm the {registration | key change} reached the network: {sentence}. It may
    still confirm."
  - `unknown` with `resend_refused` (the latest resend was refused, an earlier attempt may be out; Opus r6 n3): "The
    wallet couldn't confirm the {registration | key change} reached the network. The last resend didn't go out:
    {sentence}. An earlier attempt may still confirm."
  - `unknown` with the marker code `send.resend_in_progress` (a resend that didn't finish): "The wallet couldn't
    confirm the resend finished. The {registration | key change} may still confirm."
  - A refusal recorded by an earlier version reads `unknown` after the upgrade (§4.4 storage, Sol r6 M2): its code
    can't show whether it came from the first hand-off or was left after a resend that reached the network.
  - Either way [Send again] (the `resend` action) opens a confirm: "Send the same transaction again. It uses the same
    coins and costs at most {charge}." Sending the same bytes again is safe after an unknown outcome.
  - Grant: `masternodeOp{action.charge}`, the engine's figure (the debit for a registration, D71; the fee for a key
    change).
  - Call: `mn_shared_send` for a registration, `mn_shared_send_maintenance` for a key change.
  - The same confirm covers a manual resend while waiting. With SPV stopped there is no button: "Available once the
    wallet is connected." (`blocked{spv_stopped}`).
- **Abandon, one step** (`abandon` among the actions, `replaces` empty; Q6 ruled (a)): an immediate secondary
  [Abandon…], no timer. The engine lists it when its coin test returns usable coins: after a **refused** hand-off,
  after the recorded transaction was abandoned in Activity, and for a participant at `funding_signed`. The spend
  takes **every** reserved coin the wallet still holds back to itself, so any later spend back conflicts with it. The
  full confirmation:
  - "Abandon this registration? The wallet spends your reserved coins back to itself, paying a {charge} network
    fee, so the registration can't confirm."
  - "Your coins come back only after that spend confirms. If the registration confirms first, the masternode stands
    and your share is in it."
  - checkbox "I understand this can't be undone"
  - credential → [Abandon] → `mn_shared_abandon(wallet, s, grant)`, `masternodeOp{action.charge}`.

  A key change before `out_at` (charge 0, `spends: false`) reads instead: "Cancel this key change? Nothing is spent:
  its reserved fee coins are released now."
- **Abandon, two steps** (`abandon_wallet_tx { txid }` for the session's own transaction; R1.1, Opus r3 M1): the
  wallet recorded the sent transaction, so its reserved coins read spent and the session abandon would refuse
  (`shared_stage{sent}`). EP 4 lists it only when Activity's Abandon would accept that transaction (SPV running, then
  `Facts::abandon_refusal`). The pane shows: "To get your coins back: first abandon the {registration | key change}
  transaction in Activity, then abandon here to spend your coins back to this wallet." [Open in Activity] opens
  `TxDetailSheet` for `txid` directly (`apps/desktop/src/app/App.tsx:288`), whose Abandon keeps its own confirmation.
  `abandon_shared_session` never abandons a wallet transaction itself. The same applies to a key change after
  `out_at` (funder).
  - **Until `overdue`** this is a collapsed secondary line under the waiting copy ("Changed your mind? Ways to get
    your coins back ▸"), and it doesn't badge (§3.6). Once `overdue` it is expanded and badges. Q6's "no timer" still
    holds for the button: expanded or not, it works the moment it is listed.
  - **While the rescan runs** after the Activity abandon (`blocked{rescanning}`, Opus r4 n4): "The wallet is finding
    your coins again. Abandon appears here once it has."
- **Nothing to abandon** (`blocked`):
  - `instant_locked`: "The network has locked this {registration | key change | spend back}, so it will confirm.
    There's nothing to abandon."
  - `spv_stopped`: "Available once the wallet is connected."
  - `in_flight`: "The wallet is still waiting for the network's answer. Restarting the wallet clears this."
  - `coin_elsewhere` (R1): "One of the coins the earlier spend back used is spent by another transaction
    ({txid short}), so a new spend back couldn't be sure to replace it. If that transaction never confirms, abandon
    it in Activity; then you can abandon here." [Open in Activity] for that transaction when the engine identified
    it, else: "One of the coins the earlier spend back used isn't in this wallet any more." No "only one can
    confirm" promise is made here. It covers an earlier spend back that an older version made and overwrote, too
    (Sol r6 M1). While every missing coin is an input of a transaction abandoned in Activity, the rescanning line
    shows instead: the coin is on its way back (Opus r6 n1).
  - `no_ownership_path` (Opus r5 n6): "This wallet can't sign for one of your reserved coins, so it can't spend them
    back." It is terminal and doesn't turn into the rescanning line.
  - `too_small`: "Your reserved coins are too small to pay the network fee of spending them back."

  None has a button of its own.
- **The spend back** (`settlement: pending`, by `abandon_spend.state`; R1, Sol r4 M1, Opus r4 M1/m1). Every copy
  names the spend by its short txid. No state says "didn't go out" unless the wallet recorded a definite refusal.
  - **`uncertain`** (an unknown outcome; a crash before or after the hand-off; a cut or deferred hand-off): "The
    spend back to this wallet ({txid short}) may already be on the network. The wallet couldn't confirm it went out."
    No reason sentence for a crash or a legacy row (Opus r4 n8). [Send again] is `resend_abandon`: confirm "Send the
    same spend again. It's the same transaction, so it costs at most {charge}, once." → `mn_shared_resend_abandon`,
    `masternodeOp{action.charge}`. No new spend is offered: the engine refuses one (`shared_stage{abandoning}`) while
    this one may be out. A Send again that reaches the network records the spend in the wallet, so the pane moves to
    `in_history` below, whose own exit is the route if it still never confirms (Opus r5 n4). A refused Send again
    leaves this copy as it is: it never turns into "didn't go out".
  - **`in_history`** (recorded, unconfirmed): "Abandoning: waiting for the spend back ({txid short}) to confirm. Your
    coins are still reserved." [Send again] as above. For a legacy spend whose bytes weren't kept, [Resend in Activity]
    (`resend_wallet_tx`, Activity's existing Resend, listed only while the wallet has peers) sits in the collapsed
    line with the two-step exit, not beside the copy (Opus r6 n7). Its own two-step exit (`abandon_wallet_tx { txid: A
    }`) comes from A's own history entry, whether or not other reserved coins are free (R3). It is the collapsed line
    until `abandon_spend.overdue`, then expanded and badged: "This spend back hasn't confirmed. To replace it:
    abandon it in Activity, then abandon here again." IS-locked: `blocked{instant_locked}`.
  - **`refused`** (a definite refusal, recorded durably): "The spend back to this wallet didn't go out: {sentence}."
    [Try again] is `abandon { replaces: A }`; its confirm adds "This is a new spend of the same coins and pays the
    {charge} network fee again."
  - **`abandoned_in_activity`**: "You cancelled the earlier spend back ({txid short}) in Activity." Once the rescan
    has returned the coins, [Abandon again] is `abandon { replaces: A }`, with the same confirm and the fee named
    again; before that, the rescanning line.
  - **`legacy`** (an abandon saved by an earlier version, whose bytes and inputs weren't kept): "The wallet couldn't
    confirm the spend back ({txid short}) went out. It may still confirm." Once `abandon_spend.overdue`, and only
    when every coin of the original reservation is unspent (§4.4 change 2: an older version may have abandoned more
    than once and overwritten the earlier id, so the latest spend's inputs prove nothing about the others; Sol r6
    M1), [Replace the spend back…] is
    `abandon { replaces: A }`, behind its own confirmation: "The earlier spend back may still confirm. This new one
    spends the same coins, so only one of them can confirm, and you pay the {charge} fee again." with the checkbox
    "I understand the earlier spend may still confirm". Without that proof the pane shows `coin_elsewhere` and its
    Activity route instead, and makes no "only one" promise.
  - No resend of the {registration | key change}: the engine refuses it once `abandon_txid` is set
    (`mn/shared.rs:811`; EP's change 1 for a key change).
- **Confirming** (`settlement: confirming`): a transaction spending a reserved coin is in a block that a reorg could
  still undo. "A transaction spending your reserved coins is in a block ({confirmations} of 6 confirmations). The
  wallet waits until it can't be undone." With `chainlocked` the second sentence reads "…until a ChainLock or 6
  confirmations make it final." A ChainLock over the block, or the sixth confirmation, moves it to `settled`. A reorg
  that takes the block away returns the session to `pending` (or to `sent`), and the copy follows.
- **Settled:**
  - `settled { by: abandon_tx, released: true }`: "Abandoned. Your coins are back in this wallet." This is the only
    state that says so. `by` is `abandon_tx` for **any** spend back the session signed (`attributed`), so an earlier
    spend that confirms after a replacement still reads as the user's own.
  - `settled { by: abandon_tx, released: false }`: "Abandoned. The wallet is releasing your coins." (the follower's
    durable release, `mn/shared/flow.rs:680-699`, hasn't run yet).
  - `settled { by: other_spend }`: "This {registration | key change} was replaced: another transaction spending one
    of its coins confirmed." Nothing is claimed about where the coins went. For a legacy row (an older build may have
    overwritten an earlier spend back's id, which then can't be attributed, Opus r5 n5) it reads instead: "A spend of
    these coins confirmed."
  - The registration won: the session reads `confirmed`, and the pane shows "The registration confirmed. The
    masternode stands."
  - Key change before out, `dead{abandoned}`: "You cancelled this change; its fee coins were released."
- **An approver's key change** has no recovery but Discard (Opus r4 n2): it holds no fee input, and its stage follows
  the list (§2.3.1). Discard reads "Remove this key-change request from this wallet. Nothing is spent."

### 2.4 Shared masternode in My masternodes (`sh-detail`)

Row: "you hold 1 of 3 shares" (`MasternodeRow.shares`, or EP 5's count).

**Sources:**
- EP 5's `mn_shared_shares(wallet, hash)` for shares, early terms and ownership, read from history with or without a
  session;
- the registration session (`kind == registration`, `confirmed`, same `pro_tx_hash`) for labels, terms and the
  session's own addresses when present;
- `mn_masternode` for list facts;
- `mn_shared_standby` for stored standbys.

#### 2.4.1 With the session

- The share bar and legend from EP 5's shares, labelled from the session's `rows`.
- **Your payouts** (Sol M3, R2, D10), from EP 7:
  - Headline: "**As far as this wallet knows**, 40% of each payout goes to {latest_confirmed_local.address ??
    at_registration}."
  - "At registration: yRw4…n8Ka" (EP 7's `at_registration`), always shown.
  - With `latest_confirmed_local`: "Changed to yNw2…k3Pa by this wallet, confirmed at block {h}."
  - With `latest_attempt`, by its state:
    - `signed` / `not_sent`: "A change to yQa7…p2Ld wasn't sent: {sentence}." (its In progress row has [Send
      again]);
    - `sent`: "A change to yQa7…p2Ld was sent {sent_at date}, not confirmed yet.";
    - `conflicted`: "A change to yQa7…p2Ld didn't go through: a coin it used was spent elsewhere."
  - An attempt is never presented as where payouts go; only a confirmed update moves the headline.
  - The caveat stays: "A change made from another wallet wouldn't show here."
  - "About 38% once the operator's payout address is set" uses MG-04b §5's `SplitAmountByShares` rule.
  - Operator's share per Q13.
- Your refund (refund address).
- Early period: "Ends at block {registered height + period}, about Apr 13, 2027", or "None".
- Standby dissolution: §2.8.
- No "Payouts, 30 days" (`payouts()` refuses for `shared_unsupported`, G6).
- Note: "Shared-masternode sessions and standbys aren't in wallet backups. Save a copy of the standby to keep a way
  out." (G10)
- Actions: [Change my payout address] (§2.5), [Change keys together] (§2.6), [Dissolve…] (§2.7). Change keys or
  payout (ProUpRegTx) is not offered.

#### 2.4.2 Without the session (restored wallet, another device; Sol M7, Opus m4)

- **EP 5 answers** (the registration is in this wallet's history, and the keychain derives a share's owner key):
  - The pane shows EP 5's shares ("Share 1 · 400 Dash · yours"), early terms and refund addresses, and "At
    registration" reward addresses (EP 7, read from the history registration's shares, since there is no
    envelope).
  - Note: "This wallet holds a share of this masternode but not the session it was created in, so share names and
    the operator terms aren't shown."
  - **Offered (history-based calls):** Create standby, Import standby, Save a copy, Send standby (§2.8), Dissolve
    together (§2.7) and Change keys together (§2.6).
  - **Dissolve alone** is offered as the standby route: "Leave now: create a standby, then send its immediate
    variant (you pay the {penalty} penalty)", or the full variant once valid. `preview_dissolve`/`dissolve` need the
    session (mn §2.4), so they aren't called.
  - **Not offered** (session-only, gated separately): Change my payout address (`update_share` needs "a confirmed
    registration of its sessions"), and continuing any registration session.
- **EP 5 refuses `shared_registration_unknown`:** "This wallet holds a key that may belong to this shared
  masternode, but its history doesn't hold the registration, so it can't act on it here." No actions. This doesn't
  promise that history discovery on a fresh restore always succeeds.

**Eligibility** (Sol m7, Opus m4). Whenever history holds the registration, EP 5 returns its facts plus
`eligibility`, which mirrors the maintenance calls' guards. These are never shown as missing history. With a
session or without, the pane's maintenance actions follow it:

| `eligibility` | Engine guard | Pane |
|---|---|---|
| `ok` | — | the actions above |
| `upgrade_unknown` | `require_v24` (`mn/shared.rs:131-133`), capability `unknown` (likely early in a restored wallet's sync) | facts shown; actions replaced by "Available once the masternode list has synced." |
| `list_unavailable` | `protx_context` has no synced list (`protx/build.rs:557-566`), as likely as `upgrade_unknown` early in a restore (Opus r3 m6) | facts shown; actions replaced by "The masternode list is still loading. Available once it has synced." |
| `too_early` | `listed_above`: the list's height isn't above the registration's block (`mn/shared/dissolve.rs:510-520`), tested before the entry | facts shown; actions replaced by "Available once the masternode list includes the registration." (Opus r3 n7: the list can trail the tip, MG-04 D43) |
| `delisted` | the list is above the registration's block and has no entry for it | the row is Retired; facts shown; no actions; "This masternode has left the list, so there's nothing to dissolve or change." |

EP 5 reports the first that applies, in the table's order after `ok` (`v24_context`'s order).

### 2.5 Change my payout address (sheet small; ProUpShareTx; session required)

1. Choices: Refund address / New address in this wallet / Another address.
2. `mn_shared_preview_update_share(wallet, {hash, share_index, reward})` (fee, `reward_is_refund`).
3. Credential → grant `masternodeOp{preview.total_debit}` (lease "Shared masternode") → `mn_shared_update_share`.

Copy: "Changes where your share's payouts go. The others' shares don't change."

Success: "Change sent. As far as this wallet knows, your payouts go to {latest_confirmed_local.address, or the
registration's address when there is none} until it confirms." The pane then shows EP 7's `latest_attempt` line,
then, once confirmed, the new `latest_confirmed_local`, all surviving a restart (§2.4.1).

Refusals: `update_pending` "Another change or dissolution of this masternode is still on its way.";
`protx_refused{bad-proupshare-payee-reuse}` "This address is the masternode's voting address or an owner address."
The row stays in In progress until confirmed (§1.10).

### 2.6 Change keys together (session sheet, kind `key_change`)

- **Start (funder, this wallet):**
  - The form is §1.7's operator and voting fields (Keep / set). The operator key is shown in the basic form. The
    PoSe-ban warning shows when the operator key changes.
  - "Every owner signs. You pay the network fee."
  - Credential → `mn_shared_start_key_change`, `masternodeOp{0}` (it reserves and locks the fee inputs, D100).
  - Then copy the request to each owner.
- **Approver:**
  - A pasted request opens the sheet, showing **every key current → requested, against the list now** ("Voting:
    yV1… → yV0…"), Core's effects (`operator_reset`) and `usable_by_funder`.
  - Credential → [Approve] → `mn_shared_approve_maintenance(…, accept_actor_fee: false)`, `masternodeOp{0}`. Then
    copy the reply.
- **Funder, combined:** [Send] → `mn_shared_send_maintenance`, `masternodeOp{session.fee}`.
- **States:** from §2.3.1.
  - `stale` (funder): [Abandon…] per §2.3.3. Before out it is charged 0 and releases; after out it spends a fee input
    at EP 4's charge.
  - `dead{abandoned}`: "You cancelled this change; its fee coins were released."
  - `dead{inputs_spent}`: "A coin reserved for the fee was spent elsewhere, so this change can't be sent."
  - `dead{delisted}`: "The masternode left the list."
  - `conflicted` (funder, out, a fee input spent): §2.3.1's rows, with EP 4's settlement and the key change's own
    release rule (`key_change.rs:1198-1215`). An approver never reads `conflicted`: its row follows the list
    (§2.3.1).
  - Discard where EP 4 lists it.
  - `confirmed`: "Keys changed."
- **Failed hand-off and resend:** §2.3.3.
- **Errors** `shared_message{stale|not_key_change|inputs_hash|other_masternode}`: §3.8.

### 2.7 Dissolve (sheet medium, `sh-dissolve`): Alone, now · Together · Standby

- **Alone, now** (session present; without it, §2.4.2's standby route):
  1. `mn_shared_preview_dissolve(wallet, {hash, share_index, fee: 1,000 duffs})` returns, per share, `returns` (mine
     marked), the penalty, the fee, `early`, `valid_from_height` and the `actor_output_dust` warning. MG-00 copy.
  2. The early-exit checkbox gates [Dissolve now] (plainRed).
  3. Credential → `masternodeOp{preview.total_debit}` (lease "Dissolution") → `mn_shared_dissolve`.

  Dust actor output: Dissolve off, "What you'd get back is too small to send. Use Together or the standby."
- **Together:**
  - **Fee default** (Opus m10): `max(1,000, 300 + 110 × n)` duffs for n shares. This bounds the combined ProDisTx's
    size at the relay minimum (`MIN_FEE_PER_KB` = 1,000 duffs per kB, `send/mod.rs:62`; `relay_floor` at
    `mn/shared/dissolve.rs:122-125`, applied to `combined_size` by `check_relay`, `mn/shared/together.rs:89-101`).
    EP's test T8 pins it (§4.9). The field is editable.
  - Refusal copy: "The network fee is too low for this many shares. Raise it and try again."
  - [Start] → `mn_shared_start_dissolve_together(wallet, {hash, share_index: mine, fee})`, `masternodeOp{fee}` (this
    wallet is the actor) → copy the request.
  - **Approver:** pasted request. "You get back {amount}; nobody pays a penalty." When this wallet holds the actor
    share of a request it didn't prepare (`actor_fee`): "The network fee, {fee}, comes out of your share." A
    checkbox sets `accept_actor_fee: true` (`shared_actor_fee{fee}` without it).
  - **Send:** anyone holding every signature: [Send dissolution] → `mn_shared_send_maintenance`, `masternodeOp{0}`.
- **Standby:** §2.8.

### 2.8 Standby dissolution

- **Prompt** (S6): when a session reads `next: create_standby`, or EP 5 shows an own share with no stored standby, the
  pane shows "Save a standby dissolution. It lets you leave later even if this wallet can't reach the others."
  [Create it] → credential → grant `masternodeOp{Σ (penalty + fee) over this wallet's shares}`, with the penalty from
  EP 5 → `mn_shared_create_standby(wallet, hash, fee: 1,000 duffs, grant)` (Q7). It badges from S6 (§3.6).
- **Shown:** `mn_shared_standby(wallet, hash)`. Each `Standby` row shows only the variants it holds (`full`,
  `immediate`):
  - full: "Full refund: valid from block {full_valid_from} (about Apr 13, 2027)";
  - immediate: "Immediate: valid now, you pay the {Standby.penalty} penalty" (EP 3). A share with no early period
    has only the zero-penalty copy.
  - "Saved on this computer on Oct 15" (`created_at`), or "Imported" for `source: imported`.
- **Validity** (Sol m1, Opus m3, Opus r3 n5): the UI renders EP 5's `full_valid_now`, one value per masternode,
  which the engine computes with its own predicate `standby::full_valid(mn, spend_height) = !is_early(..) ||
  early_penalty(mn) == 0` (dw-protx `shared/standby.rs:56-61`) at `spend_height = max(list height, header tip) + 1`
  (with `list_unavailable`, `tip + 1`). The UI never compares heights
  itself. A share with an early period but a zero penalty has only the full variant (`:46-53`), and it is valid at
  once.
  - **Full held and valid:** [Send the full refund], with "valid now". Immediate isn't offered (the engine refuses it
    once a stored full is valid, `shared_stage{full_valid}`).
  - **Full held, `full_valid_now == false`:** [Send now and pay the penalty] (immediate, if held). Confirm: "You pay the {penalty}
    penalty to the others."
  - **Only the immediate is held** (a partial import), and `full_valid_now`: [Send it] with the warning "This
    standby still pays the {penalty} penalty. Create a full refund first to avoid it." plus [Create a full refund]
    when this wallet holds the share's owner key.
  - Boundaries H−2/H−1/H and a zero-penalty share inside its period are fixture-tested (§5.1).
- **Send grant:** `masternodeOp{fee + Standby.penalty}` (immediate, own share), `masternodeOp{fee}` (full, own share),
  or `masternodeOp{0}` for another share's (`foreign`). Then `mn_shared_send_standby`.
- **Save a copy…:** credential → `masternodeOp{0}` → `mn_shared_export_standby(wallet, hash, share, fee, grant)`.
  dw-app runs the save dialog (the `save_file_as` pattern, `app/api/files.rs:48-62`, purpose `StandbyDissolution`,
  Core's file name), calls `export_standby` and writes the text owner-only and atomically. It returns
  `Option<String>`: the chosen file name, or `None` on cancel, in which case the grant is dropped unused. The signed
  bytes never reach the renderer, logs or errors (G7, Q11, D6).
- **Import:**
  - The pasted hex line or [Open file…] (§2.1) gives `SharedImport::Standby{description, stored}` → "Standby
    dissolution for share {k} of {masternode}: {variant}, returns {returns}."
  - "Saved in this wallet." / "This wallet already holds it."
  - A file reports one result per transaction from EP 8's extractor: two for a pair, one for a zero-penalty file
    (whose immediate line repeats the full one, `mn/shared/standby.rs:240-244`).

### 2.9 Shared states

- **v24:** `unknown` / Evonode `inactive` (G2).
- **Locked:** a credential on each signing stage only.
- **Watch-only:** view only, no Start/Join (`wallet watch-only` before the call).
- **Network mismatch:** per §2.1.
- **A participant's coins spent:** the coordinator's `lock` refuses with `shared_stage{dead}` naming nothing about the
  other party. The wallet can't see their spends: the copy "One of the reserved coins is gone. Ask each participant
  for a new reply." is used only for this wallet's own coins; others' show at abandon time (D26).
- **Coordinator vanished:** participant Discard wherever EP 4 lists it; Abandon once out.

## 3. Engine mapping

### 3.1 Conventions for the binding

- **Domain:** new dw-app domain `masternodes` (`app/api/masternodes.rs`, `masternodes/shared.rs`,
  `masternodes/withdrawals.rs`; DTOs in `dto/masternodes.rs`). Commands are `mn_*`, each
  `#[facade("Masternodes::…")]` through the `commands!` macro. E0-13 rows move from EP's "deferred (MG-10 Sn)" to
  covered in the slice that binds them.
- **Types:** wallet id as `String` (`parse_wallet_id`), duffs as `number` (as `maxDuffs`), credits as `string` (as
  `maxCredits`). Enums are snake-case, tagged by `kind` (mn §1).
- **Secrets:** secrets in (`attach_key`) are moved into `Zeroizing` at once (the `vault.rs` pattern). The revealed key
  out follows `RevealedMnemonic`'s pattern.
- **Query keys** (Opus M3, n3):
  - Engine-local reads go under `["masternodes", …]`. A `masternodes` event invalidates the bare `["masternodes"]`
    prefix (the event carries no wallet id, and `mn_list_state`, `mn_find` and `mn_withdrawals` aren't
    wallet-scoped).
  - Platform reads go under `["mnPlatform", walletId, hash, "quote"]` and `["mnPlatform", hash, "credits"]`, outside
    that prefix, with their own refresh rule (§3.5). The quote is wallet-scoped because its held keys are
    (Sol m2); credits aren't.
- **Message files** (Sol m7, D15):
  - **Save:** the UTF-8 message text → base64 → `save_file_as(sharedMasternodeMessage, message.file_name, data)`
    (`app/api/files.rs:42-52` decodes it and writes owner-only and atomically).
  - **Open:** `mn_shared_import_file` (§3.3), so no base64 → UTF-8 decode in the renderer; standby files go through
    EP 8's extractor. Its size cap (2 MiB) applies to the file's bytes. Cancel → `None`. Read failure, invalid UTF-8 and over-cap each map to a sentence
    (§3.8).
- **Errors:** every `masternodes.*` code, `send.*`, `platform.*`, `vault` and each `reason` value maps to a sentence
  in `masternodes/model.ts` (§3.8). Unknown values fall back to "The wallet couldn't do this: {code}." (MG-09 D13's
  rule).

### 3.2 MG-10 commands (all new)

| Command (dw-app) | Facade | Grant | Refreshes | Slice |
|---|---|---|---|---|
| `mn_list_state() -> MasternodeListState` | `list_state` | — | `masternodes`, `sync` | S1 |
| `mn_list(wallet_id) -> MasternodeOverview` | `list` (+ EP 1 field) | — | `masternodes` | S1 |
| `mn_masternode(wallet_id, pro_tx_hash) -> MasternodeDetail` | `masternode` (+ EP 6 field) | — | `masternodes`, `vaultLockState` | S1 |
| `mn_payouts(wallet_id, pro_tx_hash, since: Option<u64>) -> Vec<MasternodePayout>` | `payouts` | — | `masternodes`, `historyChanged` | S1 |
| `mn_find(query) -> Vec<ListedMasternode>` | `find` | — | — | S1 |
| `mn_evonode_credits(pro_tx_hash) -> EvonodeCredits` | `evonode_credits` | — | `mnPlatform` rule | S1 |
| `mn_track` / `mn_untrack` / `mn_set_label` | `track` / `untrack` / `set_label` | — | event | S1 |
| `mn_attach_key(pro_tx_hash, role, key: String) -> KeyCheck` | `attach_key` | — (vault unlocked) | event | S1 |
| `mn_detach_key(pro_tx_hash, role) -> bool` | `detach_key` | — | event | S1 |
| `mn_keys(wallet_id, role, start, count) -> Vec<MasternodeKeyInfo>` | `NetworkSession::masternode_keys` | — | — | S1 |
| `mn_reveal_key(wallet_id, role, index, grant) -> RevealedMasternodeKey` | `NetworkSession::reveal_masternode_key` | `revealSecret` | — | S1 |
| `mn_operations(wallet_id) -> Vec<ProTxOperation>` | `operations` (+ EP 2 field) | — | `masternodes` | S1 (discovery read, no screen until S2) |
| `mn_registration_keys(wallet_id, kind) -> ProTxKeys` | `registration_keys` | — | — | S2 |
| `mn_preview_registration` / `mn_register` | `preview_registration` / `register` | `masternodeOp{D5 cap}`, lease "Masternode registration" | event | S2 |
| `mn_prepare_external_registration` / `mn_refresh_external_registration` | same | — | event | S2 |
| `mn_submit_external_registration(wallet_id, operation, signature, grant)` | `submit_external_registration` | `masternodeOp{operation.debit}` | event | S2 |
| `mn_resend_operation(wallet_id, operation, grant)` | `resend_operation` | `masternodeOp{operation.debit}` (EP 2) | event | S2 |
| `mn_discard_operation(wallet_id, operation)` | `discard_operation` | — | event | S2 |
| `mn_release_collateral` / `mn_lock_collateral` | `release_collateral` / `lock_collateral` | — | event | S2 |
| `mn_preview_update_registrar` / `mn_update_registrar` | same | `masternodeOp{total_debit}`, lease "Key change" | event | S3 |
| `mn_withdrawal_quote(wallet_id, pro_tx_hash) -> WithdrawalQuote` | `withdrawal_quote` (MG-13) | — | `mnPlatform` rule | S4 |
| `mn_withdraw_credits(wallet_id, pro_tx_hash, key, amount: WithdrawAmount, destination, reviewed_destination: Option<String>, grant)` | `withdraw_credits` (+ S4's `reviewed_destination`) | `masternodeWithdraw{maxCredits, destination}` (S4 adds the purpose) | event | S4 |
| `mn_retry_withdrawal(wallet_id, id, key, amount, destination, reviewed_destination, grant)` | `retry_withdrawal` (same) | same | event | S4 |
| `mn_withdrawals(pro_tx_hash)` / `mn_recheck_withdrawal(id)` / `mn_mark_withdrawal(id, mark)` | same | — | poll 60 s / — / event | S4 |

**S4's engine change** (R3), with its own `mn-engine.md` bump (the next free version after EP's):
- `withdraw_credits` and `retry_withdrawal` take `reviewed_destination: Option<String>`, owner key only (with the
  payout key, `Some` is `invalid_argument`). It is distinct from `destination`, which an owner withdrawal still
  can't supply (`withdrawal/sign.rs:104-110`, MG-13).
- In `plan`'s owner arm (`withdrawal/sign.rs:184-190`), after the final look under the identity lock and before
  `sign` uses the grant, the window script is compared with the reviewed address's script. A mismatch is the new
  `WithdrawalError::DestinationChanged` → `masternodes.withdraw_destination_changed`: nothing is signed, no record is
  written, the grant stays unused.
- `None` skips the check, as today. The signature change touches every caller (Opus r3 n4): dwcli's call sites and
  MG-13's test call sites pass `None`, which is what "dwcli keeps working" means.
- **What the UI passes** (Sol r4 n1), for `mn_withdraw_credits` and `mn_retry_withdrawal` alike:
  - owner key: `reviewed_destination: Some(the reviewed address)` and no `destination` (an owner withdrawal can't
    supply one, `withdrawal/sign.rs:104-108`);
  - payout key: `reviewed_destination: None` and an explicit reviewed `destination`.
- **Argument check:** a malformed or other-network `reviewed_destination` is `invalid_argument`, checked with the
  existing network-address parser before the P4 wait, so a bad argument never waits on Platform or takes the
  identity lock.
- **The payout-key path** always passes an explicit `destination`: with `None`, `plan` picks the final look's own
  script (`withdrawal/sign.rs:172-179`), which would bypass the address the user reviewed.
- **Seam test** (withdraw and retry): the quote's look returns payout A, the final look returns B, both pass the
  height checks; the call returns `withdraw_destination_changed`, the vault's signer is never called, and no record
  exists. The same with A at both looks signs.
- §4 code table row and error sentence (§3.8).

**S4's grant plumbing** (Opus M1):
- `GrantPurposeSpec::MasternodeWithdraw { max_credits: String, destination: Option<String> }`;
- its `TryFrom` into the vault purpose, an address → `WithdrawalDestination` script (dw-vault `types.rs:137-140`),
  refusing a malformed or other-network address as `invalid_argument`;
- the reverse `From` for display, replacing the `PlatformOp` fallback at `app/api/vault.rs:349-356`;
- the generated `GrantPurposeSpec.ts`;
- the E0-13 text: `vault.rs:197`'s "deferred (MG-10)" row for `masternode_withdrawal_signer` is reworded to say
  where it is used.

**Shell fields:**
- `UiSettings::masternodes_tab` (S1, D1);
- `UiSettings::masternodes_item: on | auto | off` (S2, Q5);
- `FilePurpose::SharedMasternodeMessage` (S5, 2 MiB, `.json`);
- `FilePurpose::StandbyDissolution` (S5 for import, S6 for export; small cap, `.txt`).

### 3.3 MG-10b commands (all new)

| Command | Facade | Grant (lease) | Slice |
|---|---|---|---|
| `mn_shared_capability(wallet_id) -> SharedCapability` | `shared_capability` | — | S5 |
| `mn_shared_sessions(wallet_id)`, `mn_shared_session(wallet_id, session)` | `shared_sessions`, `shared_session` (+ EP 4 field) | — | S1 (discovery read) / S5 |
| `mn_shared_start` / `mn_shared_set_fee` / `mn_shared_join` / `mn_shared_message` / `mn_shared_unlock` / `mn_shared_discard` | `start_shared` / `set_shared_fee` / `join_shared` / `shared_message` / `unlock_shared_terms` / `discard_shared_session` | — | S5 |
| `mn_shared_import(wallet_id, text, target: Option<String>) -> SharedImport` | `import_shared_message` | — | S5 |
| `mn_shared_import_file(wallet_id, target: Option<String>) -> Option<Vec<SharedImport>>` | dw-app open dialog (`.json` / `.txt`); a standby file's transactions from EP 8's `standby_file_transactions` (deduplicated); then `import_shared_message` per message or transaction | — | S5 |
| `mn_shared_lock(wallet_id, session, revision, grant)` | `lock_shared_terms` | `masternodeOp{0}` (Shared masternode) | S5 |
| `mn_shared_approve(wallet_id, session, consent_hash, grant)` | `approve_shared` | `masternodeOp{0}` | S5 |
| `mn_shared_sign(wallet_id, session, grant)` | `sign_shared_funding` | `masternodeOp{session.debit}` | S5 |
| `mn_shared_send(wallet_id, session, grant)` | `send_shared_registration` (send or resend) | `masternodeOp{session.debit}` | S5 |
| `mn_shared_abandon(wallet_id, session, grant) -> SharedSession` | `abandon_shared_session` (EP change 4: every reserved coin still unspent, under change 2's inclusion bound, stored before dispatch, `shared_stage{abandoning}` while A may be out) | `masternodeOp{recovery abandon charge}` (EP 4) | S5 |
| `mn_shared_resend_abandon(wallet_id, session, grant) -> SharedSession` | `resend_shared_abandon` (new in EP, change 4): A's stored bytes again, with the session abandon's terminal guards first; refusals `shared_stage{confirmed \| conflicted \| dead \| participant \| sent \| funding_signed \| legacy \| abandon_refused \| abandon_cancelled}`, `invalid_argument` | `masternodeOp{resend_abandon charge}` (A's fee) | S5 |
| `mn_shared_shares(wallet_id, pro_tx_hash) -> SharedShares` | `shared_shares` (EP 5, EP 7) | — | S6 |
| `mn_shared_preview_update_share` / `mn_shared_update_share` | `preview_update_share` / `update_share` | `masternodeOp{total_debit}` (Shared masternode) | S6 |
| `mn_shared_preview_dissolve` / `mn_shared_dissolve` | `preview_dissolve` / `dissolve` | `masternodeOp{total_debit}` (Dissolution) | S6 |
| `mn_shared_create_standby(wallet_id, pro_tx_hash, fee, grant) -> Vec<Standby>` | `create_standby` | `masternodeOp{Σ (penalty + fee)}` (Dissolution) | S6 |
| `mn_shared_standby(wallet_id, pro_tx_hash) -> Vec<Standby>` | `standby` (+ EP 3 field) | — | S6 |
| `mn_shared_export_standby(wallet_id, pro_tx_hash, share, fee, grant) -> Option<String>` | dw-app save dialog, then `export_standby`; dw-app writes `StandbyFile.text` owner-only and atomically | `masternodeOp{0}` (Dissolution) | S6 |
| `mn_shared_send_standby(wallet_id, pro_tx_hash, share, fee, variant, grant)` | `send_standby` | `masternodeOp{fee + penalty / fee / 0}` (Dissolution) | S6 |
| `mn_shared_start_dissolve_together(wallet_id, request, grant)` | `start_dissolve_together` | `masternodeOp{fee}` (Dissolution) | S6 |
| `mn_shared_start_key_change(wallet_id, request, grant)` | `start_key_change` | `masternodeOp{0}` (Shared masternode) | S6 |
| `mn_shared_approve_maintenance(wallet_id, session, grant, accept_actor_fee)` | `approve_maintenance` | dissolution `{actor_fee ?? 0}`; key change `{0}` | S6 |
| `mn_shared_send_maintenance(wallet_id, session, grant) -> MaintenanceSent` | `send_maintenance` (send or resend) | dissolution `{0}`; key change `{session.fee}` / resend `{recovery resend charge}` | S6 |

`mn_shared_import_file` and `mn_shared_export_standby` are the two commands that aren't straight bindings. Both run
the file dialog in dw-app so standby bytes stay in Rust (D6). The rest are one facade call each. Neither returns,
logs or puts into an error message any standby text.

### 3.4 Grants (MG-05, E0-04)

Every money-moving or signing action runs: credential (Pay's rule, which may need no passphrase; `useCredential`,
MG-09 D21) → `authorize(purpose)` → one single-use grant → the command. The lease named by MG-00 D35 then shows in the
status bar's key-held tooltip. Locked wallets browse and are asked only there.

| Action | Purpose | Cap | Lease (D35) |
|---|---|---|---|
| Register (FundNew, ExistingUtxo) | `masternodeOp` | D5 (register only: the re-read preview's `total_debit` + margin) | Masternode registration |
| Register (External, submit) | `masternodeOp` | `operation.debit` | Masternode registration |
| Resend an operation (`signed`, `not_sent` or `sent`) | `masternodeOp` | `operation.debit` (EP 2; 0 for a `foreign` or `together` dissolve row) | the row's kind |
| Change keys or payout | `masternodeOp` | preview `total_debit` | Key change |
| Reveal a masternode key | `revealSecret` | — | — |
| Withdraw / Try again | `masternodeWithdraw` | `credits + rules.fee_credits` as shown (D11), destination bound (payout key) or `reviewed_destination` checked at signing (owner key, R3) | Credit withdrawal |
| Lock terms, approve, start key change, approve key change, export standby | `masternodeOp` | 0 | Shared masternode / Dissolution |
| Sign funding, send or resend registration | `masternodeOp` | `session.debit` (as displayed, §2.3.2) | Shared masternode |
| Abandon (incl. a retry after a failed abandon hand-off) | `masternodeOp` | EP 4's abandon charge (0 for a key change before out) | Shared masternode |
| Abandon the recorded transaction (`abandon_wallet_tx`) | none here: Activity's existing Abandon, with its own confirmation | — | — |
| Change my payout address | `masternodeOp` | preview `total_debit` | Shared masternode |
| Dissolve alone | `masternodeOp` | preview `total_debit` | Dissolution |
| Create standby | `masternodeOp` | Σ (EP 5 penalty + fee) over own shares | Dissolution |
| Send standby | `masternodeOp` | fee + `Standby.penalty` (immediate) / fee (full) / 0 (another share's) | Dissolution |
| Dissolve together: start / approve as actor / send | `masternodeOp` | fee / `actor_fee` / 0 | Dissolution |
| Send key change / resend | `masternodeOp` | `session.fee` / EP 4's resend charge | Shared masternode |

No grant: track, untrack, label, attach/detach (vault unlocked), release/lock collateral, prepare/refresh External,
start/join/import/set fee/unlock/discard a shared session, every preview and read.

Nothing weakens a guard. Each cap is the engine's own figure for what the call is charged: from the preview, the
record, or EP 2–5 (mn §2.3/§2.4, MG-04b §6.3). The one exception is D5's register margin, which MG-09 D29 already
uses. `send.grant_exceeded` means the figure moved: the sheet re-reads and asks again (D4/D5).

### 3.5 Events and refresh

- `AppEvent::masternodes{network}` → invalidate the bare `["masternodes"]` prefix: lists, detail, operations,
  sessions, standbys, shares, capability, withdrawal records.
- `["mnPlatform", walletId, hash, "quote"]` and `["mnPlatform", hash, "credits"]` are re-read on open, on [Try again], every 60 s while shown, on a
  `masternodes` event only while the quote reads `checking{platform_behind}`, and when a withdrawal record changes
  state (§1.8).
- `historyChanged` → payouts and the Generated filter.
- `sync` → list state.
- `vaultLockState` → `mn_masternode` (roles.attached, EP 6) and the open quote (Opus m12, Sol M5).
- `leaseChanged` → status bar (existing).
- Withdrawal records: a 60 s poll while shown and undecided; a state change refetches the quote (Sol m4).
- After any command that returns a record, the sheet uses the returned record at once and lets the event re-read the
  rest.

### 3.6 Sidebar visibility and badge (Q5, Sol m5)

**Visibility** (S2): `UiSettings::masternodes_item` = **On** / **Auto** / **Off**.
- Auto shows the item when `mn_list` reports any `mine` or `tracked` row or a `collateral_coins` entry, or when
  `mn_operations` or `mn_shared_sessions` holds unsettled work.
- Migration: `prefs.showMasternodes == true` → On; otherwise Auto. Off hides it whatever the wallet holds.
- These discovery reads (`mn_list`, `mn_operations`, `mn_shared_sessions`) are bound in S1 and run without the page
  open, so the rule needs no navigation. Settings ▸ Show Masternodes becomes the three-way choice.

**Badge** (MG-00 D8, MG-10's half, added to MG-09's proposals), by slice:

| From | Counts |
|---|---|
| S1 | banned own masternodes with the owner key or the collateral |
| S2 | + operations `needs_signature` / `inputs_spent` / `not_sent` |
| S5 | + registration sessions with `next ∈ {join, lock, approve, sign, send}`; registration sessions whose recovery shows a `handoff_error` **with** a listed `resend` or `abandon` (Opus r4 n5); registration sessions that list `abandon_wallet_tx` for their own transaction **and** read `overdue`; and those whose spend back (`abandon_spend`) lists `abandon` (`refused`, `abandoned_in_activity`, `legacy` once overdue and bounded), or `resend_abandon` / `abandon_wallet_tx{A}` / `resend_wallet_tx{A}` once `abandon_spend.overdue` |
| S6 | + `create_standby`, `abandon`, key-change/together sessions with `next ∈ {approve, send}`; maintenance sessions under the same recovery rules as S5's |

Never `copy`, `wait` or `nothing`. A `sent` session waiting normally for its confirmation is a `wait`, so its
two-step exit counts only once EP 4 reads it `overdue` (Opus r3 M1). An unknown spend back is a wait too, so it
counts only once `abandon_spend.overdue` (Opus r4 n5). A hand-off error with no listed action is a wait. A `blocked`
reason never counts. A slice never
badges a `next` or recovery action whose screen it doesn't render. A per-slice unit test checks the set, including a
CLI-made key-change session with a failed hand-off under the S5 and the S6 configuration (Sol m8), and a `sent`
session listing `abandon_wallet_tx` before and after `overdue`, an `uncertain` spend back before and after
`abandon_spend.overdue`, and `handoff_error{abandon, unknown}` with no listed action.

### 3.7 What the engine still lacks (prerequisites)

| Need | Owner | Blocks | Proposal |
|---|---|---|---|
| EP reads 1–7, the key-change resend guard, the stored and bounded abandon spend (`abandon_tx` / `abandon_log` migration, `resend_shared_abandon`), the coin-test / `conflict_of` / `registered()` refactors + E0-13 rows | MG-10 EP (this) | S1… | §4 |
| dw-app `Masternodes` binding | MG-10 S1–S6 (this) | everything | §3.2–3.3 |
| MG-13 on main | lane MN-13 | S4 | — (S4 owns the host grant purpose, §3.2) |
| Owner-key `reviewed_destination` check at signing (R3) | MG-10 S4 (engine part) | S4's withdraw sheet | §3.2 |
| Standby-file transaction extractor | MG-10 EP (EP 8) | S5's file import | §4.10 |
| MG-00 amendment (Q10) | MG-10 docs PR on `b18152eb`'s edits | S1 merge | §8 Q10 |
| Payout reconciliation (MG-00 D42) | MG-03b | honest payout display (Q1) | none in 0.3 |
| Collateral lookup + attached voters in `voting_masternodes` | MG-03 D11 upstream API; MG-02 | tracked voting (Q3) | none in 0.3 |
| Share-table payouts, operator-payout state | MG-03b | G6, Q2, Q13 | none in 0.3 |
| Engine key-usage read | — | "Used by" (G17) | not proposed; D8's "Matched" copy instead |

### 3.8 Error, warning and result copy (Opus m7, Sol m6)

| Code / warning | Copy |
|---|---|
| `update_pending{operation}` | "An earlier change to this masternode hasn't reached the masternode list yet. Wait for it, or discard it in In progress if it was never sent." |
| `update_superseded{operation}` | "A newer change to this masternode replaced this one." (re-read operations) |
| `in_flight` | "Another step on this masternode is still running." |
| `operation_state` | "This item changed since you opened it." (re-read) |
| `no_owner_key` | from EP 6: "This wallet doesn't hold this masternode's owner key." / "…doesn't hold this masternode's registration." |
| `protx_refused{bad-protx-payee-reuse}` | "The payout address can't be the owner or voting address." |
| `shared_voting` (voting = a share's owner key, `mn/protx/build.rs:701`) | "The voting address can't be a share's owner address." |
| `share_not_mine` | "This share isn't held by this wallet." |
| `shared_registration_unknown` | "This wallet's history doesn't hold this masternode's registration." |
| `shared_too_early` | "Available once the masternode list includes the registration." |
| `shared_funding_mismatch{fee_low{needed}}` | §2.2's restart sentence |
| `shared_funding_mismatch{other reasons}` | "The coins in this message don't match the session. Ask for the message again." |
| `penalty_paid_after_period` (warning) | "This standby pays a penalty, though the early period is over. Create a full refund instead." |
| `actor_output_dust` (outside the dissolve sheet) | "What this share would get back is too small to send." |
| import `skipped_signatures{shares,count}` | "{count} signatures for {labels} didn't apply to this session and were skipped." |
| import `no_approvals` | "This message carries no approvals." |
| `edited_message` | "This message was edited after it was copied. Ask for it again before you approve." |
| `unvalued_inputs{shares}` | "This wallet can't check the amounts of {labels}'s coins. If one is short, the registration never confirms; your coins come back only through Abandon." (D26) |
| `empty_service` | "The coordinator gave no server address: the masternode starts banned until the operator sends a service update." |
| `zero_penalty` | Core's text |
| `shared_message{not_shared}` | "This isn't a shared-masternode message." |
| `shared_message{too_large}` | "This file is too large to be a shared-masternode message." |
| `shared_message{parse}` | "This message is damaged. Ask for it again." |
| `shared_message{network}` | "This message is for another network. This wallet is on Testnet." |
| `shared_message{other_session}` | "This reply belongs to session 7B21E0, not 3F9A2C." (codes from the stored sessions) |
| `shared_message{terms_changed}` | "The terms in this message differ from the ones you joined. Ask Alice to send the current locked terms." |
| `shared_message{older}` | "This is an older copy of the session. Ask for the latest one." |
| `shared_message{conflict}` | "This copy conflicts with the session you hold. Ask the coordinator for the current one." |
| `shared_message{bad_signature{share}}` | "{label}'s signature doesn't verify. Ask {label} to approve again." |
| `shared_message{not_participant}` | "Continue this session on the device that joined it." (G10) |
| `shared_message{labels}` | "Two participants have the same name in this message." |
| `shared_message{stale}` | "The masternode's keys changed since this request was made, so it can't be signed or sent." |
| `shared_message{not_key_change \| inputs_hash \| other_masternode}` | "This isn't a key change." / "This request's fee coins don't match. Ask for it again." / "This request is for another masternode." |
| file open: cancel / read error / over cap / invalid UTF-8 | (none) / "Couldn't read this file." / "This file is too large to be a shared-masternode message." / "This file isn't a shared-masternode message." |
| `withdrawal_destination_checking{reason}` | "Platform's answer changed while you were confirming. Nothing was sent." (re-read the quote, show the checking sentence) |
| `withdrawal_unverified` | "The wallet couldn't verify Platform's answer. Nothing was sent. Try again." |
| `withdrawal_retry_state` / `withdrawal_mark_state` / `withdrawal_not_found` | "This withdrawal changed since the sheet opened." (re-read records and quote) |
| `send.grant_exceeded` | "The amount changed. Check the figures again." (re-read, D4) |
| `withdraw_destination_changed` (S4) | "The payout address changed just before signing. Nothing was signed. Check the new address." (re-quote) |
| recovery `handoff_error{registration, refused}` | "The wallet couldn't hand the {registration \| key change} to the network: {sentence}." [Send again] |
| recovery `handoff_error{registration, unknown}` | "The wallet couldn't confirm the {registration \| key change} reached the network: {sentence}. It may still confirm." [Send again]. Also an inherited refusal after the upgrade (Sol r6 M2) |
| recovery `handoff_error{registration, unknown, resend_refused}` | "The wallet couldn't confirm the {registration \| key change} reached the network. The last resend didn't go out: {sentence}. An earlier attempt may still confirm." [Send again] |
| recovery `handoff_error{registration, unknown}`, code `send.resend_in_progress` | "The wallet couldn't confirm the resend finished. The {registration \| key change} may still confirm." [Send again] |
| recovery spend back `refused` | "The spend back to this wallet didn't go out: {sentence}." [Try again], confirm "This is a new spend of the same coins and pays the {charge} network fee again." |
| recovery spend back `uncertain` | "The spend back to this wallet ({txid short}) may already be on the network. The wallet couldn't confirm it went out." (with `{sentence}` only when A's own entry has a code) [Send again] |
| recovery spend back `in_history` | "Abandoning: waiting for the spend back ({txid short}) to confirm. Your coins are still reserved." [Send again] (legacy: [Resend in Activity] in the collapsed line, while the wallet has peers); once overdue "This spend back hasn't confirmed. To replace it: abandon it in Activity, then abandon here again." |
| recovery spend back `abandoned_in_activity` | "You cancelled the earlier spend back ({txid short}) in Activity." [Abandon again] |
| recovery spend back `legacy` | "The wallet couldn't confirm the spend back ({txid short}) went out. It may still confirm." Once overdue, and only with every coin of the original reservation unspent (§4.4 change 2), [Replace the spend back…], confirm "The earlier spend back may still confirm. This new one spends the same coins, so only one of them can confirm, and you pay the {charge} fee again." |
| recovery `blocked{instant_locked}` | "The network has locked this {registration \| key change \| spend back}, so it will confirm. There's nothing to abandon." |
| recovery `blocked{spv_stopped}` | "Available once the wallet is connected." |
| recovery `blocked{in_flight}` | "The wallet is still waiting for the network's answer. Restarting the wallet clears this." |
| recovery `blocked{rescanning}` | "The wallet is finding your coins again. Abandon appears here once it has." |
| recovery `blocked{too_small}` | "Your reserved coins are too small to pay the network fee of spending them back." |
| `shared_stage{abandoning}` on abandon (a stale pane) | "The earlier spend back may already be on the network, so the wallet won't sign another one now." (re-read recovery) |
| `shared_stage{legacy}` on Send again (a stale pane) | "This spend back was saved by an earlier version, so it can't be sent again from here." (re-read recovery) |
| `shared_stage{abandon_refused \| abandon_cancelled}` on Send again (a stale pane) | "This changed since you opened it." (re-read recovery) |
| `shared_stage{confirmed \| conflicted \| dead}` on Send again or abandon (a stale pane) | "This session has settled." (re-read recovery, which shows the settlement) |
| recovery `blocked{coin_elsewhere}` | "One of the coins the earlier spend back used is spent by another transaction ({txid short}), so a new spend back couldn't be sure to replace it. If that transaction never confirms, abandon it in Activity; then you can abandon here." [Open in Activity] / without a txid: "One of the coins the earlier spend back used isn't in this wallet any more." |
| recovery `blocked{no_ownership_path}` | "This wallet can't sign for one of your reserved coins, so it can't spend them back." |
| recovery `settled{other_spend}` on a legacy row | "A spend of these coins confirmed." |
| `shared_stage{sent}` on abandon (a stale pane) | "This changed since you opened it." (re-read recovery, which shows the current exit) |
| `shared_stage{abandoned}` on resend (a stale pane; registration, and key change after EP's change 1) | "You've abandoned this {registration \| key change}, so it isn't sent again." (re-read recovery) |

## 4. EP: engine read prerequisites (before S1)

**What EP is:**
- One engine PR in `dw-engine` plus one dw-appdb migration (`shared_mn_sessions.abandon_tx`, `abandon_log`, `legacy_bound`,
  `resend_marked` and the rewrite of inherited refusal codes, §4.4),
  with one `mn-engine.md` bump: **Contract-Version 19** if MN-13 (v18) lands first, the expected order; otherwise
  renumber at merge.
- It binds nothing in the UI. Each read's facts already live in the engine; EP exposes them. Each read gets a contract
  row (§2 Kind/Errors cells, §3 generated listing re-blessed) and a behaviour test.
- **It also changes engine behaviour in named places**, each with a contract line and a regression test: the
  key-change resend guard (§4.4 change 1); the abandon spend's storage, same-coin bound, `shared_stage{abandoning}`
  refusal and the new `resend_shared_abandon` call (change 4); and three behaviour-preserving refactors with parity
  tests, the coin test (change 2, which also stops issuing an address in the shared check), `conflict_of` (change 3)
  and the split of `registered()` (§4.5). Nothing else changes what a call accepts or refuses.
- EP also adds one pure helper (EP 8) that dw-app calls from S5.
- The listing test is `rust/crates/dw-engine/tests/mn_masternodes_contract.rs`, blessed at v19. Behaviour tests go
  in the module test blocks named below.

### 4.1 EP 1: `MasternodeRow.collateral_released: bool` (Q14)

- **Fact lives at:** `mn/protx.rs:990-1030` `set_collateral_lock` writes the mark (key at `:1014`), with prefix
  `RELEASED = "masternodes.collateral_released:"` (`mn/protx/ops.rs:33`). Today only the follower reads it
  (`ops.rs:689, 697`).
- **Read:** the row builder reads the same key. The field is false when absent.
- **Test T1** (`protx.rs` tests): release → row true; lock → false; restart (new engine on the same db) → still true;
  a coin locked by hand through coin control alone → false.

### 4.2 EP 2: `ProTxOperation.debit: u64` and `ProTxOperation.overdue: bool` (Opus M2, R1.4, Sol r3 m4, Opus r3 m5, Opus r4 n3, Opus r5 m4)

- **Fact lives at:**
  - `mn/shared/dissolve.rs:66` `row_debit`;
  - `DissolveRecord::debit` (`dissolve.rs:77-84`): 0 when `foreign` or `together`, else penalty + fee;
  - charged on resend at `mn/protx.rs:722`; `resend_operation` accepts `sent` (`protx.rs:695-700`);
  - existing tests `dissolve.rs:734-760`;
  - record type `protx_records.rs:272-286`.
- **Read `debit`:** `row_debit(row)`, the same function `resend_operation` charges. For non-dissolve rows it equals
  `fee`.
- **Read `overdue`**, from facts the engine already has. Two named functions, shared by this read and EP 4 (§4.4
  rule 11):
  ```
  fn tip_overdue(sent_at: u64, tip_time: Option<u64>) -> bool
      = tip_time.is_some_and(|t| t.saturating_sub(sent_at) >= OVERDUE_SECS)          // OVERDUE_SECS = 3 h = 10800
  fn overdue(sent_at: u64, list_synced: bool, list_height: Option<u32>,
             tip_height: Option<u32>, tip_time: Option<u64>) -> bool
      = list_synced
        && matches!((list_height, tip_height), (Some(l), Some(t)) if l + 1 >= t)
        && tip_overdue(sent_at, tip_time)
  ```
  - `overdue` is for what waits on the masternode list: a ProTx operation whose state is `sent` (its `sent_at`) and a
    shared session at `sent` (its `out_at`; a resend doesn't move it).
  - `tip_overdue` alone is for an abandon spend (its `signed_at`, §4.4): a spend back is never a list event, so an
    unavailable or stuck list must not hold back its exit (Opus r5 m4).
  - **Three hours** because a block's header time may run up to two hours ahead of real time (the future-time rule):
    a block stamped early can't make a just-sent row overdue. `sent_at` is the local clock; the rule stays advisory.
  - **`list_height + 1 ≥ tip_height`**: dash-spv requests the list diff after each block
    (`dash-spv/src/sync/masternodes/sync_manager.rs:270-300`), so a list one block behind is current, and the value
    doesn't flap for the seconds after each block.
  - `tip_time` is the tracker's tip time, which the pump fills from `spv.tip_block_time()`
    (`rust/crates/dw-engine/src/events.rs:1200-1205`, `set_tip_time`), the one header time the engine reads; `None`
    (no tip yet) is not overdue.
  - `list_synced`, the list's height, the header tip height and tip time come from the tracker
    (`rust/crates/dw-engine/src/sync.rs:313-390`). A list further behind isn't overdue: it may not show the
    transaction yet.
  - `HeaderSource` (`events.rs:595-615`) is **not** extended: no per-block header times are read.
  - Nothing is persisted, so a restart recomputes the same value from the same facts. Header times aren't monotonic
    and `sent_at` is the local clock, which is why the rule is advisory: three hours of tip time are far past the
    expected 2.5-minute block.
  - **Advisory:** it adds §1.10's D49 line and drives the S5/S6 badge and the collapsed exits. It gates no call, never
    expires an operation and never releases a coin. The one listing it holds back is a legacy abandon row's
    replacement (§4.4 rule 4), as ruled.
- **Test T2:** for each kind and each dissolve flavour (alone early, alone after the period, standby immediate,
  standby full, foreign, together), `operation.debit` equals the charge `resend_operation` takes, from `signed`,
  `not_sent` and `sent`.
  - Rows with `debit > 0`: a grant capped at `debit − 1` is refused `send.grant_exceeded`; `debit` passes.
  - Rows with `debit == 0` (`foreign`, `together`): a grant capped at 0 authorizes the resend, and nothing is
    charged.
  - `overdue` (unit tests on the function, plus one through the read): a mocked tip time 10 799 s / 10 800 s after
    `sent_at`; a tip time two hours ahead of `sent_at` right after sending (false); no tip time; the list two blocks
    behind (false), one behind (true), at the tip (true); `tip_overdue` with the list not synced and the tip time
    past 3 h (true: an abandon spend's exit isn't held back by the list); a restart (a new
    engine on the same db, the same mocked tip: the same value); a reorg that lowers the tip and its time (follows
    the new tip); false once the row is confirmed; the same function's result for a shared session (EP 4).

### 4.3 EP 3: `Standby.penalty: u64` (Opus M2)

- **Fact lives at:**
  - `mn/shared/standby.rs:144-158` `standby_of` builds `Standby` without it;
  - the penalty is `standby::early_penalty(&mn)` (`:409`, `:526`) and `variant_penalty` (`:582`, `:660`);
  - struct `shared_records.rs:489-502`.
- **Read:** the immediate variant's penalty for that share; 0 when there is no early period or no penalty.
- **Test T3:** created and imported standbys report the penalty `send_standby` charges for `immediate`. A full-only
  import reads the share's penalty too, for display; its send charges 0 penalty.

### 4.4 EP 4: `SharedSession.recovery: SharedRecovery` (Sol M4, R1, r3–r5 rulings)

**Shape:**
```
SharedRecovery {
  out_at: Option<u64>,
  overdue: bool,                           // §4.2's overdue on the session's out_at (list clauses + tip time); advisory
  settlement: none
            | pending    { txid }                                    // abandon_txid saved, no conflict in a block
            | confirming { by, confirmations: u32, chainlocked: bool }  // a conflict in a block, not yet buried
            | settled    { by: abandon_tx | other_spend, released: bool },
  handoff_error: Option<HandoffError { tx: registration | abandon, outcome: refused | unknown, code: Option<String>,
                                       resend_refused: bool }>,  // the latest resend's own refusal; outcome unknown
  abandon_spend: Option<AbandonSpend {     // A, the latest abandon spend; present exactly when abandon_txid is set
    txid, signed_at: u64, legacy: bool,
    overdue: bool,                         //   §4.2's tip_overdue(signed_at, tip_time): the tip-time clause only
    state: Option<refused | uncertain | in_history | abandoned_in_activity | legacy>,  // only while pending, not terminal
  }>,
  attributed: Vec<Txid>,                   // every abandon txid the session has signed, oldest first
  blocked: Option<Blocked { txid: Option<Txid>, reason: in_flight | instant_locked | spv_stopped | rescanning
                                                       | coin_elsewhere | no_ownership_path | too_small }>,
  actions: Vec<RecoveryAction>,            // a safe subset of the calls the engine accepts now
}
RecoveryAction = resend { charge }                       // the session's own transaction, same bytes
               | abandon { charge, spends: bool, replaces: Option<Txid> }   // a new abandon spend
               | resend_abandon { charge }               // A's stored bytes again
               | abandon_wallet_tx { txid }              // Activity's Abandon of a recorded transaction
               | resend_wallet_tx { txid }               // Activity's Resend of a recorded transaction (no stored bytes)
               | discard { releases: u64 }
```
`registration` in `HandoffError.tx` means the session's own transaction: the registration, or the key change.

**Principle (R1, r4 and r5 rulings):** the wallet never infers "never sent" from what isn't in history, and never
from a refused **replay**. dash-spv hands a transaction to peers before the wallet processes it
(`dash-spv/src/sync/mempool/manager.rs:373-395`, `start_broadcast` at `:487-493`, at the rust-dashcore revision
pinned in desktop's `rust/Cargo.lock`, 8e7ad2c), and a deadline-cut (`HandOff::Cut`, `send/mod.rs:1752-1757`),
deferred (`Verdict::Deferred`, `:1653`) or `TransactionBroadcastUnconfirmed` hand-off can leave no wallet record
either. `refresh_wallet_state` (`engine/session.rs:689-707`) reads balances and heights, not history, and
`PendingSpends` goes at restart. A definite refusal does mean the bytes never left: platform-wallet allows
`BroadcastError::Rejected` "only when the transaction definitively did not enter the network"
(`rs-platform-wallet/src/broadcaster.rs:73-77`, platform 03200c8), and the p2p path has no negative signal. But a
replay's refusal says nothing about an earlier attempt that may be out. So the only evidence that an abandon spend
didn't go out is a **durably recorded definite refusal of its first dispatch**, and the only safe second spend is one
that **provably conflicts with every earlier one**.

**Chosen: an engine-derived contract, not raw facts.** The rules that decide which recovery call the engine accepts,
what it charges and when coins are back are spread across:
- `send_shared_registration`: resend at `sent`, refused once `abandon_txid` is set (`mn/shared.rs:775-818`, D72 at
  `:809-815`); its participant role refused (`:797-800`);
- `send_key_change` at `Sent` → `resend_key_change` (`mn/shared/key_change.rs:988-994, 1052-1081`): its pre-checks
  `check_no_pending_ops`, `check_no_key_change_out`, `check_resend_inputs` (`mn/protx.rs:948-963`), and today **no**
  `abandon_txid` guard (change 1);
- `abandon_shared_session`: `require_sending`, `protx_op`, `require_wallet`, `claim_shared_session`; refused for
  together; key changes delegated (`abandon_key_change`, `key_change.rs:1096-1143`: an approver is
  `shared_stage{participant}`, `Confirmed` / `Conflicted` / `Dead` are `shared_stage{stage}` at `:1115-1120`, before
  `out_at` it releases at 0, after it spends); a registration session refuses `Confirmed` and `Conflicted`
  (`shared.rs:857-864`) and needs `out_at` (`shared.rs:820-868`); `key_change_discard` accepts any row whose `out_at`
  is unset (`key_change.rs:1149-1156`), an approver's included;
- `abandon_session` (`mn/shared/send.rs:200-298`): today the **largest** reserved coin the wallet holds unspent and
  no pending spend holds, by `(value, outpoint)`, then that coin's ownership path; with none it refuses
  `stage_refusal(out_stage(role))`, i.e. `shared_stage{sent}` for a coordinator and `shared_stage{funding_signed}`
  for a participant (`:237-238`); it issues a fresh change address (`:240-245`), charges the relay minimum of a
  one-input spend (`:248`), refuses a coin worth no more than fee + dust as `send.amount_with_fee_exceeds_balance`
  (`:249-257`), saves `abandon_txid` durably and then hands the spend off (`:274-294`). Tests: QT-091,
  `test_QT_126_*` (`mn/shared/tests.rs:2312-2367, 2413-2448, 2543-2554`);
- `deliver_shared` (`send.rs:47-73`) awaits `broadcast_inner`, stores the error code as `last_error` through the
  unsynced `record_shared_session_attempt`, and returns `Ok(())` whatever the network said, so its caller can't see
  the outcome;
- the stores: `shared_mn_sessions.signed_tx` holds the session's **own** signed transaction, written with `txid` and
  `out_at` before it can leave (`mark_shared_session_out_durable`, dw-appdb `shared_sessions.rs:300-321`), which is
  what the registration and key-change resends read (`send.rs:176-182`, `key_change.rs:1060-1066`); MG-04's
  `resend_operation` reads `masternode_ops.signed_tx` (`V2026102101__mg_protx_ops.sql:7, 23`; `mn/protx.rs:714-715`).
  The abandon spend is stored nowhere: `set_shared_session_abandon_durable` writes only its id and overwrites it
  (`shared_sessions.rs:341-356`). The dispatch journal (`dw-appdb/src/dispatch.rs`) keeps payloads only for
  registered asset-lock artifacts, and a row-less `CoreTx` with no step marker settles in memory
  (`platform/lease/fence.rs:1580-1606`);
- the send path's split of a hand-off's outcome: `attempt_outcome` (`send/mod.rs:1842-1853`) is `NotSent` for
  `NoPeers`, `BroadcastRejected`, `PreparedTxSpent`, `Cancelled`, `NetworkNotOpen` and `WalletNotFound`, else
  `MaybeSent`; in the fenced path a `MaybeOut` settlement is stored as `send.broadcast_unknown` (`:1653-1673`), but
  a `Verdict::Refused` returns `send.cancelled` (`:1650-1652`) even for a replay (`maybe_out: true` doesn't
  normalize that early return); `settle` (`:1825-1838`) releases a never-sent attempt's coins and keeps an unknown
  one's claimed (`PendingSpends`, in memory, `:383-399`); a repeat while SPV is stopped is always
  `send.broadcast_unknown` (`:1710-1718`);
- `derive_stage` reads `conflicted` as soon as a conflict is in a block, and a registration's own confirmation takes
  precedence (`mn/shared/store.rs:504-517`), while `releasable` waits for a **buried** one (`store.rs:444-474`),
  buried being ChainLocked or six confirmations deep (`mn/protx/ops.rs:489-502`); the follower's durable release is
  later still (`mn/shared/flow.rs:675-699`). An unconfirmed partial spend of the reservation conflicts nothing.
  Key changes derive their conflict at `key_change.rs:325-328` (funder only) and release at `:1198-1215`;
- Activity: `abandon_transaction` (`tx_actions.rs:252-271`, bound at `app/api/history.rs:159-164`) refuses
  `SpvNotRunning` first (`:262-266`), then `Facts::abandon_refusal` (`:153-165`): `AlreadyAbandoned`, `Confirmed`,
  `InstantLocked`, `Coinbase`. `resend_transaction` (`:458-484`, `Facts::resend_refusal` at `:168-180`) is
  Activity's Resend, from the history transaction. `abandon_inner` releases the spent coins, which come back through
  a rescan, and the entry stays, marked abandoned, across a restart (`tx_actions.rs:277-340`,
  `history_ops.rs:188-239`). Neither Activity action checks the session's reservation locks, and a second wallet with
  the same keys can spend a reserved coin too: a reservation is not a network prohibition.

Raw fields would make the renderer re-derive all of that, which is D2's drift in a money path. EP 4 computes each
field with the predicate the matching call uses: each call's guard and the field share one function.

**Storage (R1.1): a new migration.** No existing store can hold the abandon spend: `signed_tx` must keep the
session's own bytes for its resend and settlement, and `mark_shared_session_out_durable` refuses other bytes once
out. EP adds `V2026111x__mg_shared_abandon.sql` (the next free version) to `shared_mn_sessions`:
- `abandon_tx BLOB`: the consensus bytes of A, the latest abandon spend;
- `abandon_log TEXT`: a JSON array, one entry per abandon spend the session has signed, oldest first:
  `{ txid, fee, inputs: ["txid:vout", …], signed_at, outcome: null | "sent" | "not_sent" | "maybe_sent", code,
  legacy }`;
- `legacy_bound INTEGER NOT NULL DEFAULT 0 CHECK (legacy_bound IN (0, 1))` (Sol r6 M1, Opus r6 n4): the durable
  legacy-bound mark. While it is set, a new spend back must take the whole original reservation (change 2);
- `resend_marked INTEGER NOT NULL DEFAULT 0 CHECK (resend_marked IN (0, 1))` (Opus r6 n3): set before the first
  resend of the session's own bytes and never cleared, so no later refusal code reads as a first refusal;
- `CHECK (abandon_tx IS NULL OR abandon_txid IS NOT NULL)`, `CHECK (abandon_log IS NULL OR json_valid(abandon_log))`;
- **backfill**, one `UPDATE … WHERE abandon_txid IS NOT NULL` (no row has a log before this migration, so these are
  exactly the sessions with a legacy `abandon_txid`): `abandon_log = [{ txid: abandon_txid, signed_at: <migration
  time, strftime('%s','now')>, outcome: "maybe_sent", legacy: true }]`, with no bytes, inputs or code, and
  `legacy_bound = 1`. Its `signed_at` is the upgrade time, so a legacy spend can only read overdue later than it truly
  is, never sooner. Its outcome is `maybe_sent` because an old-format record can't show a first refusal (Sol r6 M2);
- **inherited codes** (Sol r6 M2), a second statement: `UPDATE shared_mn_sessions SET last_error =
  'send.broadcast_unknown' WHERE out_at IS NOT NULL AND last_error IN (…)`, the list being every code
  `outcome_of_code` classes `refused`. It covers an out registration, an out key change (funder) and an abandoned
  row's inherited abandon outcome. The old engine wrote `last_error` only after `broadcast_inner` returned and wrote
  no marker before a resend (`mn/shared/send.rs:62-70, 168-196`, `key_change.rs:1052-1078`), so an old code can't
  show whether it came from a first dispatch or was left behind by a resend that reached a peer before a crash, or
  that the fence refused (`send/mod.rs:1650-1652`). Every inherited refusal on an out row therefore reads unknown,
  and its reason sentence goes with it. Rows that aren't out keep their code: nothing of theirs was handed off.
  dw-appdb exports the list as `INHERITED_REFUSED_CODES`; a dw-engine test asserts it equals the `refused` class of
  `outcome_of_code`, and the migration test rewrites one row per listed code. Refusals the new engine records keep
  their "couldn't hand it to the network" wording;
- **known limits** (D17): ids an older build overwrote (a second abandon under the old code) are gone and can't be
  backfilled, so they aren't in `attributed` (Opus r5 n5); and two legacy spends that build already signed may be
  disjoint, so both can confirm. No migration can make two signed transactions conflict. Every spend the new engine
  signs conflicts with all of them (change 4).

dw-appdb writes, each in one synced transaction (`AppDb::durably`):
- `set_shared_session_abandon_durable(wallet, id, txid, tx, fee, inputs, now)` replaces today's: sets
  `abandon_txid` and `abandon_tx` and **appends** the entry (outcome null), before the bytes can leave; `false` when
  the session is gone or isn't out. It never removes an entry. In the same transaction it clears `legacy_bound` when
  the entry's `inputs` include every outpoint of the row's stored `reserved`; the write checks that itself, so no
  caller can clear the mark early (Sol r6 M1);
- `mark_shared_abandon_replay_durable(wallet, id, txid)`: before any replay of a stored spend, moves that entry's
  null outcome to `maybe_sent` (Sol r5 m1). A `not_sent`, `maybe_sent` or `sent` entry is left as it is;
- `record_shared_abandon_outcome_durable(wallet, id, txid, outcome, code, first: bool)`: folds the attempt into that
  entry, and writes `last_error`, synced. Forward-only: null → `not_sent` **only when `first`**, null →
  `maybe_sent` | `sent`; `not_sent` → `maybe_sent` | `sent` (a replay of a refused spend isn't offered, but the fold
  allows it); `maybe_sent` → `sent`. A replay's `not_sent` result is folded as nothing: it never clears an earlier
  uncertainty (as the dispatch journal's `Sent`/`MaybeSent` rows are final, DEC-154);
- `mark_shared_session_resend_durable(wallet, id)` (R2, Opus r5 m5, Opus r6 n3): immediately before each resend's
  hand-off of the session's own bytes, sets `last_error = 'send.resend_in_progress'` and `resend_marked = 1`, synced,
  so a crash mid-hand-off reads unknown, never the earlier attempt's refusal;
- `COLUMNS` and `read_row` gain the four columns; `SharedSessionRow` gains `abandon_tx`, `abandon_log`,
  `legacy_bound` and `resend_marked`.

dw-appdb tests (`migration_tests.rs`, `shared_sessions.rs` tests). Old-format rows are written at the previous
schema version, then migrated:
- an abandoned row (legacy entry backfilled with `maybe_sent`, `abandon_tx` NULL, `legacy_bound = 1`) and one without
  (both NULL, `legacy_bound = 0`);
- the inherited-code rewrite: one out row per `INHERITED_REFUSED_CODES` code → `send.broadcast_unknown`, an abandoned
  row's code likewise; an unknown-class code, a NULL and a row not yet out unchanged;
- the append keeps earlier entries; it clears `legacy_bound` when its inputs cover `reserved` and keeps it when they
  don't;
- the fold (`first` true and false, every transition, a replay's `not_sent` ignored); the replay mark; the resend
  mark (both fields); the CHECKs; a `delete_*_durable` still refuses any out row.

**Engine behaviour changes in EP** (named as such; each has a contract line and a regression test):
1. **Key-change resend guard** (Opus r3 m1, Sol r3 m3). `send_key_change`'s `Sent` arm refuses once `abandon_txid` is
   set, with registration's D72 error, `shared_stage{abandoned}` (`shared.rs:809-815`). Test
   `test_an_abandoned_key_change_is_never_resent`, plus a control that a key change with no abandon still resends.
   mn §2.4's `send_maintenance` row gains the refusal.
2. **The coin test returns its outcome, with the inclusion bound inside it** (Sol r3 m2, Opus r3 m4, n6, Sol r4 m1,
   Opus r4 m2, Opus r5 m1, Sol r5 M1, R1). The selection moves into `abandon_coins(..) -> AbandonCoins`, which both
   the call and the read run, so they can't disagree. It **issues no address**: its value check uses a nominal P2PKH
   change script of 25 bytes, the length of every change address the call issues; the call issues the address only
   after `Usable` and asserts its script is 25 bytes long. With `U` = the reserved coins the wallet holds unspent,
   first match:
   1. `HeldInFlight`: a `PendingSpends` claim holds a coin of `U`. It ends at restart.
   2. `NoOwnershipPath { outpoint }`: a coin of `U` has no ownership path. A new spend takes all of `U`, so this
      blocks it (Opus r5 n6). Terminal: it doesn't clear by waiting.
   3. `BlockedByWalletTx { txid }` / `Unavailable`: `U` is empty. `BlockedByWalletTx` when an identified,
      unconfirmed, not-abandoned wallet transaction spends a reserved input (**a new history lookup**: the session's
      `row.txid`, then A, then the first such history transaction by outpoint order); else `Unavailable`.
   4. `Unbounded { missing, spender: Option<Txid> }` (R1): `abandon_txid` is set and `U` doesn't contain the
      **required set** `R`, the outpoints a new spend must take so it conflicts with every earlier one:
      - while `legacy_bound` is set (Sol r6 M1, Opus r6 n4): **every outpoint in `reserved`**, the session's
        original reservation, fixed once out (`appdb/shared_sessions.rs:269-283`). That holds even when history holds
        the latest legacy A and its inputs: the old call could abandon more than once, each time spending one free
        reserved coin and overwriting the earlier id (`mn/shared/send.rs:216-238`, `appdb/shared_sessions.rs:341-354`),
        so any reserved coin may sit in an earlier spend the wallet no longer ties to the session. Every legacy A has
        the mark: the backfill sets it on every legacy row, and only a spend taking the whole reservation clears it;
      - otherwise A's stored `inputs`. Each modern spend took every coin of its `U`, which contained its
        predecessor's required set, and the first one after a legacy spend took the whole reservation, so by
        induction A's inputs cover every earlier spend's, legacy ones included.

      `missing = R − U`; `spender` is an identified, unconfirmed wallet transaction spending one of them and not
      marked abandoned (abandoned transactions are excluded, Opus r6 n1), if any. Kept for a `refused` A too: A
      never left, but the earlier spends whose inputs A's cover may be out.
   5. `TooSmall`: `U`'s total is worth no more than fee + dust (the fee of an n-input spend).
   6. `Usable { coins: U }`.

   **The call's error mapping:** `HeldInFlight`, `BlockedByWalletTx`, `Unavailable` and `Unbounded` return the
   caller's existing out-stage refusal, `stage_refusal(out_stage(role))`; `NoOwnershipPath` the same; `TooSmall`
   `send.amount_with_fee_exceeds_balance`; `Usable` continues to the spend. Parity tests: coordinator no-coin
   (`shared_stage{sent}`), participant no-coin (`shared_stage{funding_signed}`), the value check at fee + dust and
   fee + dust + 1 (the read's `too_small` boundary equals the call's refusal, with the call's real address), QT-091.
   **Not parity, named behaviour changes:** a reserved coin without an ownership path now blocks the spend even
   when it isn't the largest (today only the largest's path is checked); a claimed reserved coin now blocks it
   (today a claimed coin is skipped and the largest free one spent); and `Unbounded` is new.
3. **`conflict_of`** (Opus r3 m8, n5, Sol r4 n2). `conflict_of(history, inputs, except: Option<&Txid>, preferred:
   &[Txid]) -> Option<(Txid, u32)>` over today's scan, ordered by lowest height, then any id in `preferred`, then the
   smallest txid. `conflict_height(..) = conflict_of(history, inputs, except, &[]).map(|(_, h)| h)` passes its old
   exception (`protx/ops.rs:175-196`) and no preference, so MG-04's operation states, the shared stage
   (`store.rs:504-517`), `releasable` (`:444-474`) and the key-change follower (`key_change.rs:1198-1215`) are
   unchanged. Registration's `session_hash` / malleated-copy exception (`store.rs:491-508`) and the key change's own
   inputs stay. Recovery passes `attributed` as `preferred`. Test: MG-04's `Conflicted` derivation and the follower's
   release decisions are unchanged on the existing fixtures.
4. **The abandon spend is stored, bounded and re-sendable** (R1, Sol r4 M1, Opus r4 M1/m1, r5 R1–R4):
   - **Same-coin bound.** A new abandon spend spends **every** reserved coin the wallet holds unspent, all to one
     fresh change address, charged the relay minimum of that n-input spend, and only on `Usable`: so it includes the
     required set `R` (change 2). So every spend back this engine signs conflicts with every earlier spend back of
     the session, legacy ones included. Two legacy spends an older version already signed may be disjoint, and no
     migration can change that (storage's known limits, D17). A legacy row (`legacy_bound`) gets a replacement only
     when the whole original reservation is unspent; otherwise it gets the blocker and the Activity route, with no
     "only one can confirm" promise.
   - **Stored before dispatch, outcome from the real result** (R2). The call signs, writes
     `set_shared_session_abandon_durable` (bytes, id, entry), and only then hands off through `deliver_abandon`, a
     wrapper over `deliver_shared`'s body that **returns** `broadcast_inner`'s result to its caller and writes
     `record_shared_abandon_outcome_durable(.., first: true)` in place of `record_shared_session_attempt`. Storage
     failures stay `MasternodesError`s, separate from the network outcome; an error before any hand-off
     (`new_draft`'s `NetworkNotOpen` / `WalletNotFound`) is a first-dispatch `NotSent`.
   - **New spends refused while A may be out.** `abandon_shared_session`'s order (Opus r5 m3): its existing checks
     (`require_sending`, `protx_op`, `require_wallet`, `claim_shared_session`, together → `invalid_argument`, key
     change delegated, approver → `shared_stage{participant}`, `Confirmed` / `Conflicted` (key change: also `Dead`) →
     `shared_stage{stage}`, `out_at` unset → its existing arm); then **`abandoning`**: `abandon_txid` set and A's
     state `uncertain` or `in_history` → `shared_stage{abandoning}`, a new stage value; then `coin_fence` and
     `abandon_coins`' arms; then the spend. This changes the expected error in
     `test_QT_126_abandon_spends_a_reserved_coin_and_releases_once_buried` (`mn/shared/tests.rs:2413-2448`): its
     second abandon, after a first whose spend the test network records nothing of, now reads `abandoning`, not
     `funding_signed` (`:2438`). EP updates that assertion, adds a control (A refused → the second abandon passes),
     and a multi-coin variant asserting `spend.input.len() == n` (the existing `== 1`, `:2430`, holds because that
     participant reserves one coin). This is a behaviour change with a contract line, not parity. The key-change
     funder has the same dependency (Opus r6 n8): `test_QT_127_a_sent_key_change_is_abandoned_by_a_spend_to_the_wallet`
     asserts `spend.input.len() == 1` (`mn/shared/tests/key_change.rs:478`), true only while the funder reserves one
     fee input. EP names it as a behaviour change beside QT-126's `:2430` and adds a multi-input variant asserting
     `== n`.
   - **New call `resend_shared_abandon(wallet, session, grant)`** (R4, Opus r5 n2, Sol r5 m3) hands A's stored bytes
     off again, charged A's fee. Its order, sharing `abandon_shared_session`'s applicability guards:
     1. `require_sending` (`SHARED_SENDING`), `protx_op`, `require_wallet`, `claim_shared_session`;
     2. together → `invalid_argument`; a key change's approver → `shared_stage{participant}`;
     3. the session's terminal stages, as the session abandon: `Confirmed` / `Conflicted` → `shared_stage{stage}`
        (`shared.rs:857-864`), for a key change also `Dead` (`key_change.rs:1115-1120`). This runs before anything
        about A, so a won or conflicted session is refused even when A is absent from history;
     4. `out_at` unset → `stage_refusal(stage)`, as the session abandon returns there (`shared.rs:856-865`; Opus r6
        n6);
     5. `abandon_txid` unset → `stage_refusal(out_stage(role))` (`sent` / `funding_signed`);
     6. A legacy (no bytes) → `shared_stage{legacy}`; A `refused` → `shared_stage{abandon_refused}`; A
        `abandoned_in_activity` → `shared_stage{abandon_cancelled}` (new stage values);
     7. `coin_fence`, then `check_resend_inputs` (MG-04 D7, as `resend_registration` runs it, `send.rs:184`);
     8. `mark_shared_abandon_replay_durable`, then `deliver_abandon` with `maybe_out: true`, folding with
        `first: false`.

     A Send again that reaches dash-spv is processed into the wallet (`handle_tx` → `process_mempool_transaction`,
     `dash-spv/src/sync/mempool/manager.rs:373-395`), so A then reads `in_history`, whose own two-step exit is the
     route if it never confirms (Opus r5 n4). dw-app binds it as `mn_shared_resend_abandon` (§3.3).
   - **Registration and key-change resends** (R2, Opus r5 m5, Opus r6 n3): `resend_registration` and
     `resend_key_change` write `mark_shared_session_resend_durable` immediately before `deliver_shared`, after
     `check_resend_inputs`, the lease and the charge (`send.rs:184-190`, `key_change.rs:1066-1072`), so a guard's
     refusal leaves the earlier code as it was. The replay's result is then written as today, its own code included,
     so a refused resend keeps its reason (no peers, say), and `resend_marked` makes rule 3 read it `unknown` with
     `resend_refused`, since the earlier attempt may be out. **Chosen: a distinct marker code**,
     `send.resend_in_progress`, classed unknown, after governance's `RESEND_IN_PROGRESS`
     (`governance/proposals.rs:68-70, 771`), so an interrupted resend reads "couldn't confirm the resend finished"
     rather than the generic unknown sentence. With the migration's rewrite of inherited codes (storage, Sol r6 M2),
     a code reads `refused` only for a first dispatch this engine recorded.
   - **Contract rows EP rewrites** (R4): mn §2.4's `abandon_shared_session` row (`docs/contracts/mn-engine.md:185`,
     "spends its largest unspent reserved coin") becomes "spends every reserved coin the wallet holds unspent, when
     they include every input an earlier abandon spend took (change 2), the spend and its record stored durably
     before dispatch; `shared_stage{abandoning}` while the latest spend back may be out"; a new
     `resend_shared_abandon` row with its Kind and Errors cells (`shared_stage{confirmed | conflicted | dead |
     participant | sent | funding_signed | legacy | abandon_refused | abandon_cancelled}`, `shared_stage{stage}` for a
     session not yet out, `invalid_argument`); the `send_shared_registration` row (`mn-engine.md:184`, "A failed
     hand-off keeps `out_at` and records its code in `last_error`") and the `send_maintenance` row's `last_error`
     sentence, which gain the resend marker and `resend_marked` (Opus r6 n3) and the upgrade line "an inherited
     refusal code on an out session reads `send.broadcast_unknown`" (Sol r6 M2); the `send_maintenance` row's
     refusal (change 1); and MG-04b's facade row (`docs/design/MG-04b-shared-masternodes.md:613`,
     "spending one of the wallet's reserved inputs") the same way. The generated listing is re-blessed.
5. **`registered()` is split** (§4.5).

Nothing else in EP changes what a call accepts or refuses.

**Rules** (the read; A = the latest abandon spend, `attributed` = every `abandon_log` txid). **Terminal state comes
first** (R4): the settlement and stage are computed before anything else, and a terminal session lists no recovery
action but `discard` where the engine accepts it.
1. **Terminal precedence:**
   - the registration or key change won (stage `confirmed`): no recovery actions, `abandon_spend.state` none;
   - a conflicting spend is in a block: `confirming` / `settled` (rule 10), no recovery actions,
     `abandon_spend.state` none. That includes A itself in a block: a spend back in a block is never `uncertain`
     (Opus r5 n3);
   - a key change `dead{…}`: `discard` per its rules.
2. **A's state**, only while `settlement == pending` and not terminal, first match:
   1. `abandoned_in_activity`: history holds A, marked abandoned (by the user in Activity);
   2. `in_history`: history holds A, unconfirmed, not abandoned;
   3. `refused`: A's entry records `not_sent` (a first-dispatch refusal, durably);
   4. `uncertain`: A's bytes are stored and none of the above: `maybe_sent`, `sent` but not in history, or no outcome
      at all (a crash before or after the hand-off, a cut or deferred hand-off, a replay of any of these);
   5. `legacy`: A's entry is a backfilled legacy one and history doesn't hold A.
3. **`handoff_error { tx, outcome, code }`:**
   - `abandon_txid` unset: from `last_error` (the session's own transaction): `tx = registration`; `outcome =
     unknown` when `resend_marked` is set, else `outcome_of_code(code)`; `resend_refused` when `resend_marked` is set
     and `outcome_of_code(code)` is `refused` (the latest resend's own refusal, its reason kept; Opus r6 n3). With
     change 4's marker and the migration's rewrite (Sol r6 M2), `refused` means a first dispatch's refusal that this
     engine recorded. `outcome_of_code` sits beside `attempt_outcome`, with a parity test over every `SendFailure`
     and engine error the hand-off can return: `outcome_of_code(error_code(e)) == attempt_outcome(Err(e))`; the
     marker `send.resend_in_progress` isn't an engine error and is pinned `unknown` separately;
   - `abandon_txid` set: from A's entry, `tx = abandon`, only when the entry has a code. The crash window and a legacy
     row have none, so no reason sentence is shown for them (Opus r4 n8).
4. **`abandon { charge, spends, replaces }`** (a new spend; `charge` = the relay minimum of change 4's n-input
   spend): `out_at` set, not terminal, kind not together, `abandon_coins` returns `Usable`, and either
   `abandon_txid` unset, or A is `refused` or `abandoned_in_activity`, or A is `legacy` and `abandon_spend.overdue`.
   `Usable` already includes R1's bound (for a `legacy_bound` row, the whole original reservation), so a listed
   replacement provably conflicts with every earlier spend. Its
   `replaces` is A's txid when `abandon_txid` is set. A key change before `out_at`: `abandon { charge: 0, spends:
   false }`. The legacy listing is the one place `overdue` holds an action back from the list (the call itself
   accepts it); the pane puts it behind its own confirmation (§2.3.3).
5. **`resend_abandon { charge }`:** A is `uncertain`, or `in_history` with its bytes stored; not terminal; SPV
   running (Opus r4 n6); and `check_resend_inputs` passes, as the call runs it. `charge` = A's fee.
6. **Activity exits** (R1.1, Opus r3 M1, R3):
   - `abandon_wallet_tx { txid }` when `activity_abandon_refusal(spv_running, facts(txid))` is `None` (the function
     `abandon_transaction` calls, in its order: `SpvNotRunning` first, then `Facts::abandon_refusal`), for:
     - **the session's own transaction**: `abandon_coins` returns `BlockedByWalletTx { row.txid }`, with
       `abandon_txid` unset, or with A `refused` or `abandoned_in_activity` (the registration relayed back after
       an Activity abandon, Opus r5 m2);
     - **A**, whenever A is `in_history`, **whatever the coin test says** (R3): derived from A's own history entry,
       so a legacy one-input A keeps its exit while another reserved coin is usable;
   - `resend_wallet_tx { txid: A }`: A is `in_history` with no stored bytes (legacy), `Facts::resend_refusal(A)` is
     `None`, SPV is running and the wallet has connected peers, in Activity's own order (`tx_actions.rs:466-474`;
     Opus r6 n5). With no peers it is left out: Activity would refuse `NoPeers` before any hand-off. It is
     Activity's existing Resend (`tx_actions.rs:458-484`) on the history transaction;
   - **for A**, both are collapsed and unbadged until `abandon_spend.overdue`; for the session's own transaction,
     until `overdue`. With A IS-locked: `blocked{A, instant_locked}`.
7. **`resend { charge }`:** stage `sent`, `abandon_txid` unset, not stale, SPV running (Opus r4 n6), and the resend's
   own read pre-checks pass (for a key change `check_no_pending_ops`, `check_no_key_change_out`,
   `check_resend_inputs`); charge = the debit (registration, D71) or the fee (key change). Listed whatever
   `handoff_error.outcome` says. Never for a participant (`send_shared_registration` refuses its role).
8. **`discard { releases }`:** exactly when `discard_shared_session` accepts the row (mn §2.4), an approver's key
   change included (Opus r4 n2); `releases` = the reserved total (0 for an approver).
9. **`blocked`**, when no action covers the exit, first match:
   - `spv_stopped`: SPV isn't running and a `resend`, `resend_abandon`, `abandon_wallet_tx` or `resend_wallet_tx`
     would otherwise be listed; also when SPV runs with no peers and `resend_wallet_tx` would otherwise be the only
     exit (Opus r6 n5; the copy, "Available once the wallet is connected.", fits both);
   - `instant_locked` (txid): the blocker, or A, is InstantSend-locked;
   - `in_flight`: `HeldInFlight`;
   - `no_ownership_path` (Opus r5 n6): `NoOwnershipPath`. Terminal; never shown as `rescanning`;
   - `rescanning` (txid), checked before `coin_elsewhere` (Opus r6 n1): no reserved coin claimed, and either
     `Unavailable` while history holds the session's own transaction or A marked abandoned, or `Unbounded` /
     `BlockedByWalletTx` whose every missing required coin (`R − U`; `reserved` when `abandon_txid` is unset) is an
     input of a history transaction marked abandoned (the session's own, A, or any id in `attributed`): the rescan
     hasn't returned the coin yet. The txid is that abandoned transaction;
   - `coin_elsewhere` (txid = `spender`) (R1, Opus r5 m1, Sol r6 M1): `abandon_txid` set, a new spend would
     otherwise be listed, and `Unbounded`, or `BlockedByWalletTx` naming a transaction that is neither the session's
     own nor A (such as an earlier spend back an older version overwrote). With an identified spender the pane links
     it in Activity;
   - `too_small`: `TooSmall`.

   `AlreadyAbandoned`, `Coinbase` and a confirmed spender can't arise from the lookup; a test pins all three.
10. **Settlement**, using `releasable`'s own scan through `conflict_of` with `preferred = attributed`:
    - `pending { txid }`: `abandon_txid` set, no conflict in a block;
    - `confirming { by, confirmations, chainlocked }`: a conflict in a block that `buried` rejects; `by` is
      `abandon_tx` when the conflicting txid is **any** id in `attributed`, else `other_spend`; `chainlocked` says
      whether the tracker has a ChainLock height at all;
    - `settled { by, released }`: the conflict is buried (`releasable` true); `released` is true once
      `release_shared_session_locks_durable` has cleared `reserved`;
    - a reorg that removes the conflict's block takes the session back to `pending` or its out stage;
    - key changes use `key_change_stage` and the release rule at `key_change.rs:1198-1215` in the same way, funder
      only. An approver's key change has no recovery but `discard`; its stage follows the list (§2.3.1).
11. **`overdue`:** §4.2's `overdue` (list clauses and tip time) on `out_at` for the session; §4.2's `tip_overdue`
    (the tip-time clause only, Opus r5 m4) on A's `signed_at` for `abandon_spend.overdue`, since a spend back is
    never a list event. Advisory, except for the legacy listing in rule 4.

**Facts used:** `abandon_txid`, `abandon_tx`, `abandon_log`, `out_at`, `txid`, `reserved` (dw-appdb
`shared_sessions.rs:14-87` plus EP's migration; `mn/shared/store.rs:329`); stage (`store.rs:615-650`); `last_error`
(`shared_records.rs:304`); history and `PendingSpends::snapshot` (`send/mod.rs:411-417`); `Facts`
(`tx_actions.rs:153-180`) and the SPV check (`:262-266`). Today's `SharedSession` (`shared_records.rs:257-305`) has
none of the recovery fields.

**Test T4** (`shared.rs` and `key_change.rs` tests). `actions` is a **safe subset** of what the engine accepts. Each
case asserts:
- every offered action passes its call: the call, made with the listed charge as its grant cap, gets past its guard;
- every call left out is refused by the engine with the code in the row, or deliberately suppressed, with the reason;
- the settlement, `handoff_error`, `abandon_spend`, `attributed` and `blocked` values.

Each case runs again after a **restart** (a new engine on the same db, which also drops `PendingSpends`) and has a
fixture world (§5.1) and a Playwright case (§6). Network outcomes come from the engine's seams (`protx_seam`'s
broadcaster, `send.rs:58-61`, `send/mod.rs:1712-1716`), not from fixtures.

| Case | Expect | Left out, and why |
|---|---|---|
| registration before out | `discard` | `abandon`, `resend`: refused |
| coordinator `sent`, successful hand-off, not IS-locked | `resend`; `abandon_wallet_tx{row.txid}` (collapsed until `overdue`) | `abandon`: refused `shared_stage{sent}` |
| coordinator `sent`, successful hand-off, IS-locked | `resend`; `blocked{row.txid, instant_locked}` | `abandon_wallet_tx`: Activity refuses `InstantLocked` |
| coordinator `sent`, SPV stopped | `blocked{spv_stopped}` | `resend`: suppressed (a repeat would store `send.broadcast_unknown`); `abandon_wallet_tx`: Activity refuses `SpvNotRunning` |
| coordinator `sent`, refused first hand-off | `resend` + `abandon`; `handoff_error{registration, refused}` | — |
| coordinator `sent`, unknown hand-off | `resend`; `handoff_error{registration, unknown}`; `blocked{in_flight}` while claimed and unrecorded, else `abandon_wallet_tx{txid}` | `abandon`: refused `shared_stage{sent}` |
| registration refused, then a resend, crash in its hand-off (R2) | after restart `handoff_error{registration, unknown}`, code `send.resend_in_progress` (the marker's own copy) | — |
| registration refused, then a resend refused by the fence (`send.cancelled`) (Opus r6 n3) | `handoff_error{registration, unknown, resend_refused}` with `send.cancelled`'s sentence: `resend_marked` keeps it unknown, the reason stays | — |
| registration refused, then a resend refused by its guard (`check_resend_inputs`) | `handoff_error{registration, refused}`: the marker sits after the guards | — |
| **upgrade** (Sol r6 M2): old refusal (`send.no_peers`) → old resend → crash after the hand-off, before history and outcome → upgrade → restart; coordinator and key-change funder | immediately, before any send: `handoff_error{registration, unknown}` (`send.broadcast_unknown`); `resend` with its same-bytes confirm | — |
| **upgrade**: old uncertain attempt → old resend refused by the fence (`send.cancelled`) → upgrade; coordinator and key-change funder | `handoff_error{registration, unknown}` before any send | — |
| **upgrade**: inherited refused abandon outcome (legacy A, refused-class `last_error`) | `last_error` reads `send.broadcast_unknown`; the legacy entry `maybe_sent`; A `legacy`, no reason sentence | — |
| first refusal on the new protocol, on an upgraded db | `handoff_error{registration, refused}`: "couldn't hand … to the network" | — |
| registration abandoned in Activity, rescan running | `blocked{row.txid, rescanning}` | `abandon`: refused (no coin yet) |
| registration abandoned in Activity, rescan done | `abandon` + `resend` | — |
| participant whose wallet recorded the registration (Sol r5 n2) | the same Activity exit and blocked reasons as the coordinator, **without** registration resend | `abandon`: refused `shared_stage{funding_signed}`; `resend`: refused (participant role) |
| participant `funding_signed` | `abandon` | `discard`: refused (out) |
| key change after out: successful / IS-locked / refused / unknown / abandoned in Activity | as the coordinator rows, funder, the key change's charges | as those rows |
| key change, approver, funder sent | `discard{0}`; stage from the list (§2.3.1) | `abandon`: refused `shared_stage{participant}` |
| key change stale before out | `abandon { charge: 0, spends: false }` | `resend`: refused (`stale`) |
| abandon A **accepted**, recorded | `pending`; A `in_history`; `resend_abandon`; after `abandon_spend.overdue`, `abandon_wallet_tx{A}` too | `abandon`: refused `shared_stage{abandoning}`; `resend`: refused `abandoned` |
| abandon A **refused** at first dispatch, through the real `deliver_abandon` (no-peers seam), then a second abandon B | before B: A `refused` (`not_sent`); `handoff_error{abandon, refused}`; `abandon{replaces: A}`. After B: `attributed = [A, B]`, B ⊇ A's inputs | `resend_abandon`: refused `shared_stage{abandon_refused}` |
| … then A confirms after all | `confirming{by: abandon_tx}` | — |
| refused A, one of its inputs spent by another transaction X | `blocked{X, coin_elsewhere}` | `abandon`: refused `stage_refusal(out_stage(role))` (`Unbounded`) |
| abandon A **unknown**, one reserved input and several | A `uncertain` (or `in_history` once recorded); `handoff_error{abandon, unknown}`; `resend_abandon` | `abandon`: refused `shared_stage{abandoning}` |
| **Sol r4 seam**: A handed to the network, engine dropped before the wallet processes it, one input and several | after restart: A `uncertain`, bytes and id intact, `attributed = [A]`; `resend_abandon`; no "didn't go out" anywhere | `abandon`: refused `shared_stage{abandoning}` |
| … then A's confirmation is delivered | `confirming{by: abandon_tx}`, then `settled{abandon_tx}` | — |
| **replay refused** (Sol r5 m1): A handed off → crash before history and outcome → restart → Send again refused by the fence (`Verdict::Refused`, `send.cancelled`) → restart | A `uncertain` throughout (outcome `maybe_sent` after the replay mark); bytes and id kept; never "didn't go out" | `abandon`: refused `abandoning` |
| uncertain → Send again reaches dash-spv → recorded → overdue (Opus r5 n4) | A `uncertain` → `in_history` → `abandon_wallet_tx{A}` expanded | `abandon`: refused `abandoning` |
| pre-dispatch crash (A stored, no outcome, never handed off) | A `uncertain`; no reason sentence; `resend_abandon` | `abandon`: refused `abandoning` |
| deadline-cut hand-off (`HandOff::Cut`) and deferred hand-off (`Verdict::Deferred`) | A `uncertain` (`maybe_sent`); `resend_abandon` | `abandon`: refused `abandoning` |
| A abandoned in Activity, rescan running → done | `blocked{A, rescanning}` → `abandon{replaces: A}`; `attributed` keeps A | — |
| A recorded, unconfirmed, overdue | `abandon_wallet_tx{A}` (expanded, badged) and `resend_abandon`; IS-locked: `blocked{A, instant_locked}` | `abandon`: refused `abandoning` |
| **legacy row** not in history, every reserved coin unspent, before and after `abandon_spend.overdue` | before: A `legacy`, nothing listed; after: `abandon{replaces: A}` (B takes the whole reservation) | `resend_abandon`: refused `shared_stage{legacy}`; before overdue `abandon` suppressed (rule 4) |
| **legacy, two-input reservation, one input held by an unconfirmed non-abandon spender X** (R1), for a registration coordinator, a participant and a key-change funder | before `abandon_spend.overdue`: A `legacy`, nothing listed, no `blocked`; after (Opus r6 n2): `blocked{X, coin_elsewhere}`, no replacement listed, no "only one" copy | `abandon`: refused (`Unbounded`) throughout |
| … after `abandon_spend.overdue`: restart, then X removed (abandoned in Activity, rescan done) | the whole reservation unspent → `abandon{replaces: A}`, B takes both inputs, `legacy_bound` cleared | — |
| … at every point | no disjoint B can be signed: the call refuses whenever `abandon_coins` isn't `Usable`. Before overdue the read lists nothing although the call would accept a bounded B (rule 4) | — |
| **legacy two-abandon upgrade** (Sol r6 M1, Opus r6 n4): an old build's P spends x (recorded, unconfirmed), then its A spends y and overwrites P's id; upgrade; A abandoned in Activity; P stays unconfirmed; for a registration coordinator, a participant and a key-change funder | while the rescan runs and once it's done, before and after restart: `blocked{P, coin_elsewhere}`; `attributed = [A]` | `abandon`: refused (`BlockedByWalletTx{P}`, then `Unbounded{missing: [x], spender: P}`); no B spending only y can be signed |
| … then P abandoned in Activity, rescan done | both inputs back → `abandon{replaces: A}`; B takes x and y; `legacy_bound` cleared; `attributed = [A, B]` | — |
| **legacy one-input A in history, two reserved coins, the other free** (R3), before and after overdue and restart | `abandon_wallet_tx{A}` and `resend_wallet_tx{A}`, collapsed until `abandon_spend.overdue`, then expanded and badged | `abandon`: refused `abandoning` (A `in_history`); `resend_abandon`: refused `legacy` |
| legacy one-input A abandoned in Activity, the other reserved coin free, rescan running (Opus r6 n1) | `blocked{A, rescanning}`, not `coin_elsewhere` | `abandon`: refused (`Unbounded`; the missing coin is A's) |
| legacy A abandoned in Activity, rescan done, every reserved coin back | `abandon{replaces: A}` with `R` = the whole reservation (`legacy_bound`); B takes every reserved coin; the mark cleared, so a later spend's `R` is B's inputs | — |
| legacy one-input A in history, SPV running, no peers (Opus r6 n5) | `abandon_wallet_tx{A}` (collapsed until overdue) | `resend_wallet_tx`: suppressed (Activity refuses `NoPeers`) |
| legacy row whose winning conflict isn't in `attributed` (an id an old build overwrote) | `settled{other_spend}`, with the legacy neutral copy (§2.3.3) | — |
| same-coin bound, one and several reserved inputs | B's inputs ⊇ A's; A and B conflict; at most one confirms; the n-input fee | — |
| SPV stopped with A `uncertain` | `blocked{spv_stopped}` | `resend_abandon`: suppressed |
| **terminal with A absent from history** (R4): registration confirms; another spend in a block; key change `dead`; each also after the durable release and a restart | no recovery actions (`discard` for `dead`); `abandon_spend.state` none | `resend_shared_abandon`: refused `shared_stage{confirmed \| conflicted \| dead}`; `abandon`: refused the same |
| A itself in a block | `confirming{by: abandon_tx}` / `settled`, never `uncertain` | all refused (`conflicted`) |
| abandon spend at 1–5 confirmations | `confirming{by: abandon_tx, confirmations: n}` | `abandon`, `resend`: refused (`conflicted`) |
| 6 confirmations | `settled{abandon_tx, released: false}`, then `released: true` after the follower | all refused (settled) |
| ChainLock at 1 confirmation | `settled` | as above |
| reorg back out of `confirming` | `pending` | as the `pending` rows |
| another conflicting spend wins | `confirming{other_spend}`, then `settled{other_spend}` | as above |
| durable release across a restart | `released` true after restart, never back to false | — |
| registration wins against the abandon | stage `confirmed`, no recovery actions | all refused (`confirmed`) |
| `dead{abandoned}` | `discard` per its rules | — |
| key change `conflicted` (funder) | as the registration rows, with the key change's release rule | as those rows |
| reserved coins worth ≤ fee + dust in total | `blocked{too_small}` | `abandon`: refused `send.amount_with_fee_exceeds_balance` |
| a reserved coin without an ownership path (not the largest) | `blocked{no_ownership_path}`, also after an Activity abandon | `abandon`: refused `stage_refusal(out_stage(role))` (behaviour change, change 2) |
| `test_QT_126_abandon_spends_a_reserved_coin_and_releases_once_buried` | its second abandon refused `abandoning` (was `funding_signed`); control: A refused → passes; multi-coin `input.len() == n` | — |
| `test_QT_127_a_sent_key_change_is_abandoned_by_a_spend_to_the_wallet` (Opus r6 n8) | `input.len() == 1` (`tests/key_change.rs:478`) named as a behaviour change, kept for one fee input; multi-input variant `input.len() == n` | — |
| `resend_shared_abandon` on a session not yet out (a stale pane; Opus r6 n6) | — | refused `stage_refusal(stage)` |

Derivation tests on existing-shape rows, each before and after a restart: an inherited failed registration
(`last_error = send.broadcast_rejected`, out, no `abandon_txid`) reads `unknown` after the migration (Sol r6 M2),
while the same refusal recorded by the new engine reads `refused`; a migrated legacy abandon with `last_error` set
(no reason sentence, `legacy`, `legacy_bound` set).

**Badge test** (vitest, §3.6): a `sent` session listing `abandon_wallet_tx` doesn't badge until `overdue`; A
`uncertain` or `in_history` (listing `resend_abandon`, `abandon_wallet_tx{A}` or `resend_wallet_tx{A}`) doesn't
badge until `abandon_spend.overdue`; `handoff_error` badges only with a listed action; `blocked` never badges.

### 4.5 EP 5: `Masternodes::shared_shares(wallet, pro_tx_hash) -> SharedShares` (Sol M7, Opus m4, Sol m1/m7, Opus m3/m4, Opus r3 m6/n5)

**Shape:**
```
SharedShares {
  registered_height, spend_height, early_period_blocks, early_penalty, full_valid_from,
  eligibility: ok | upgrade_unknown | list_unavailable | too_early | delisted,
  full_valid_now: bool,                    // per masternode: standby::full_valid(mn, spend_height)
  session: Option<String>,                 // this wallet's confirmed registration session, if any
  shares: Vec<ShareFact { index, amount, refund_address, mine: bool,
                          reward: RewardDestination }>,   // EP 7
}
```

- **Fact lives at:**
  - the module note that standby work reads history, not a session (`mn/shared/standby.rs:1-10`);
  - `in_history` (the registration in a block of the wallet's history, `standby.rs:262`), `registered()`
    (`:282-311`, `spend_height = max(list height, header tip) + 1`), `my_shares()` (`:314-325`, keychain owner
    keys);
  - used by `create_standbys` (`:388-409`), and by the together/key-change starts (`together.rs:669`,
    `key_change.rs:669`);
  - `full_valid` (dw-protx `shared/standby.rs:56-61`).
- **Engine refactor (named, Opus r3 m6):** `registered()` (`standby.rs:282-311`) runs `in_history` and then
  `listed_above` (`mn/shared/dissolve.rs:510-520`), and takes `voting_key_id` and `service` from the listed entry, so
  no history fact is reachable without the list guard; `my_shares` takes `&Registered` (`:313-325`). EP splits it:
  - **history half** `registration_of(chain, hash) -> Result<HistoryReg, _>`: `in_history`, then collateral, shares,
    `registered_height`, early period and penalty, operator key and reward. Refuses `shared_registration_unknown`;
  - **list half** `listed_of(ctx, &HistoryReg) -> Result<Listed, _>`: `listed_above`, then `voting_key_id`,
    `service`, the list height, `tip` and `spend_height = max(list height, header tip) + 1`. Refuses
    `shared_too_early`;
  - `registered()` becomes the wrapper `registration_of` → `listed_of`, returning the same `Registered`, so its **12**
    call sites (`standby.rs:396, 499, 546, 619`, `together.rs:669, 770, 879, 942`, `key_change.rs:669, 835, 939,
    999`; Opus r4 n7) keep their refusal order. `together.rs:770` and `key_change.rs:835` `match` on the result and
    handle `shared_too_early` themselves; the wrapper returns the same error value, so that handling is unchanged.
    The order: `v24_context` (`require_v24`, then
    `protx_context`'s `list_unavailable`, `mn/shared.rs:131-133`, `protx/build.rs:557-566`), then
    `shared_registration_unknown`, then `shared_too_early`;
  - `my_shares(wallet, &[CollateralShare])`, called with `&reg.shares` at its **6** call sites (`together.rs:581`,
    `key_change.rs:671, 856, 942`, `standby.rs:398, 572`).

  Parity tests, one per call site: each existing refusal test of those calls (unknown registration, too early, v24 unknown, list
  unavailable) returns the same code after the split, and `registered()` returns an identical `Registered` on the
  existing fixtures.
- **History facts vs eligibility.** EP 5 runs `registration_of` and `my_shares` whenever history holds the
  registration and returns the history facts, then reports the guards as `eligibility` instead of refusing, in
  `v24_context`'s order:
  1. `upgrade_unknown`: `require_v24` reads `unknown`;
  2. `list_unavailable`: `protx_context` has no synced list. The history facts still show; `spend_height` is then
     from the header tip alone (`tip + 1`);
  3. `too_early`: the list's height isn't above the registration's block. **The height is tested first**: a list at
     or below that height is `too_early` even when the entry is missing, since `listed_above` folds both into one
     refusal;
  4. `delisted`: the list is above the registration's block and has no entry for the masternode;
  5. `ok`: `listed_of` passes.
- **`full_valid_now`** (Opus r3 n5) = `standby::full_valid(mn, spend_height)` once per masternode, so the
  zero-penalty branch is the engine's.
- **Refusals:** `shared_registration_unknown` only when history lacks the registration; `invalid_argument` for a
  malformed hash. Issues, locks and signs nothing.
- **Chosen: a read, not Opus m4's probe:** `preview_dissolve` needs a session and a fee, `standby` is empty on a
  restore, and neither returns the share table or the penalty when it refuses.
- **Test T5:**
  - wallet A registers a shared masternode; wallet B, restored from A's seed **with the registration in its scanned
    history** (birthday before the registration, scan past it) and no session rows: B's `shared_shares` equals A's
    except `session` and `reward`'s local updates;
  - B can `create_standby` with a cap from it, and `send_standby` the immediate variant;
  - eligibility at the registration's own height (`too_early`), at that height with the entry missing (still
    `too_early`), after delisting above that height (`delisted`, facts still returned), with v24 unknown
    (`upgrade_unknown`), with no synced list (`list_unavailable`, facts returned, `spend_height` from the tip), and
    v24 unknown with no list (`upgrade_unknown`: the precedence);
  - a zero penalty with a nonzero period, before its end: `full_valid_now` true;
  - the parity tests above;
  - an unrelated wallet: `shared_registration_unknown`.

### 4.6 EP 6: `MasternodeDetail.registrar: RegistrarCapability` (Sol M6, Opus n2)

**Shape:**
```
RegistrarCapability = available { signer: keychain | attached }
                    | registration_unavailable      // no loaded wallet holds the registration
                    | no_owner_key
                    | vault_locked                  // an attached owner key may exist; custody unknown
                    | shared
```

- **Fact lives at:**
  - `mn/reads.rs:360-366` (`roles.owner` includes attached keys);
  - `owner_facts` (`reads.rs:891-915`): `owner_key_id` comes from a non-shared provider record, and an attached owner
    key counts only when `Matched`;
  - the `NoOwnerKey` refusals in `preview_update_registrar` (`mn/protx/update.rs:222-246`);
  - `mn/tracking.rs:110-129` (an owner key stays unchecked without the registration).
- **Precedence** (first match): `shared` → `registration_unavailable` (no record; unlocking can't help) →
  `available{keychain}` → `vault_locked` → `available{attached}` (matched) → `no_owner_key`.
- **Read:** one predicate shared by `owner_facts`' caller and the read, so the read and the preview's owner guard
  can't disagree.
- **Test T6** ("preview passes" means its owner/capability guard passes with otherwise valid inputs):

  | Case | Expect |
  |---|---|
  | own registration, keychain owner | `available{keychain}`, preview passes |
  | tracked-only node + attached owner key | `registration_unavailable`, preview `no_owner_key` |
  | tracked-only node + attached owner key, vault locked | `registration_unavailable` |
  | tracked node whose registration another loaded wallet holds + matched attached owner | `available{attached}`, preview passes |
  | the same, vault locked | `vault_locked` |
  | shared row | `shared` |
  | registration held, no key | `no_owner_key` |

### 4.7 EP 7: reward destination, in `ShareFact.reward` (Sol M3, R2)

**Shape:**
```
RewardDestination {
  at_registration: String,
  latest_confirmed_local: Option<RewardUpdate { operation, address, confirmed_height }>,
  latest_attempt: Option<RewardAttempt { operation, address,
                                         state: signed | not_sent | sent | conflicted,
                                         sent_at: Option<u64> }>,
}
```

- **Fact lives at:**
  - `at_registration`: the session envelope's reward address, else its refund (`mn/shared/store.rs:655-675`); with
    no session, the history registration's share, `CollateralShare::effective_reward_script()` (dw-protx
    `shared/mod.rs:138-150`), from the registration `in_history` decodes;
  - updates: the independent `update_share` operation rows (`mn/shared/update.rs:256-274`), whose states are
    `signed | not_sent | sent | confirmed | conflicted` (`protx_records.rs:245-259`, derived at
    `protx/ops.rs:149-168`). The resolved address is the row's **`terms.payout_address`** (`update.rs:163-177`),
    never `request`, which holds the unresolved choice for `RewardChoice::NewAddress`; the share index comes from
    `request`.
- **Rules:**
  - `latest_confirmed_local`: the newest `confirmed` update row for (hash, share index). "Newest" (Opus r3 n5) is the
    highest `confirmed_height`, then the latest `created_at`.
  - `latest_attempt`: the row with the latest `created_at` after `latest_confirmed_local`'s that isn't confirmed or
    discarded, with its state; `sent_at` when it has one.
  - Only a confirmed update sets `latest_confirmed_local`. The envelope is never rewritten.
- EP 7 is a field of EP 5's read, so the pane makes one call. It has its own test.
- **Test T7:**
  - A at registration → B confirmed → C pending: `latest_confirmed_local` B, `latest_attempt` C `sent`;
  - C `not_sent` (refused hand-off), with no `sent_at`;
  - C `conflicted`: `latest_attempt` C conflicted, `latest_confirmed_local` still B;
  - a `NewAddress` update: the address is the issued one from `terms.payout_address`;
  - sessionless (wallet B of T5): `at_registration` from the history registration;
  - restart: everything unchanged;
  - a discarded update: absent;
  - two confirmed updates at the same height: the later `created_at` wins.

### 4.8 E0-13 rows (Opus m5, n5, Sol r3 n1, Opus r3 n3)

- EP adds `("Masternodes", "dw-engine")` to `FACADES` (`app/coverage.rs:16-25`). The `EXCLUDED` rows live in a domain
  module (`coverage.rs:1-4`): EP adds a stub `app/api/masternodes.rs` holding only `EXCLUDED`, registered where
  coverage collects the domains' lists, with a row for every `Masternodes` method and EP 8's free function, so
  `facades_are_covered` passes with nothing bound:
  - "deferred (MG-10 S1)" … "deferred (MG-10 S6)", per §3.2–3.3's Slice column;
  - `resolve_collateral` (`reads.rs:862`): "not bound: always refuses collateral_lookup (MG-03 D11)";
  - MG-13's six methods: "deferred (MG-10 S4)". If EP lands before MN-13, MN-13 adds these six rows; if after, EP
    does.
- **EP 8's free function** is covered through `FREE` (`coverage.rs:27-41`), not `FACADES`: EP adds
  `("dw_engine::masternodes::standby_file_transactions", "dw-engine/src/masternodes/shared/standby.rs")`.
  `facades_are_covered` checks that the named file contains `pub fn standby_file_transactions(` (`:404-412`), so the
  function is declared `pub fn` in that file, not only re-exported. Its stub `EXCLUDED` row is "deferred (MG-10
  S5)"; an exclusion of a function missing from `FREE` would be rejected as unknown (`:299-302`). S5 removes the
  exclusion when `mn_shared_import_file` claims it.
- The generated `docs/design/E0-13-facade-coverage.md` is regenerated in EP and checked in its gate.
- Each slice deletes its rows as it binds them, in the same stub module that becomes the domain.

### 4.9 Test T8: together-fee default (Opus m10, n7)

`together.rs` tests: for n = 2…8 shares, with every output present and the largest standard refund scripts (P2PKH
and P2SH), `relay_floor(combined_size)` ≤ `300 + 110 × n` (`relay_floor`, `mn/shared/dissolve.rs:122-125`;
`combined_size`, `together.rs:89-96`). The renderer's default (§2.7) can't fall below the floor unnoticed.

### 4.10 EP 8: `standby_file_transactions(text: &str) -> Result<Vec<String>, MasternodesError>` (Opus m5, Opus r3 m7)

- **Where:** a pure function next to `file_text` in `mn/shared/standby.rs` (`:203-260`), re-exported from
  `masternodes` (a free function hosts call, so it has a contract row and a `FREE` row, §4.8).
- **Rule, label-agnostic:** `file_text` is a port of Dash Core's `standbyFileText` (`:203-209`), and Core writes the
  local date, the locale's short date-time and Qt-translated header and labels where this wallet writes UTC ISO and
  English (D84). Files users import will often come from Core. So the parse reads no header or label:
  - it keeps each line that, trimmed, is non-empty, even-length hex and decodes as a transaction
    (`consensus::deserialize::<Transaction>`), in file order;
  - it drops a line equal to an earlier kept one: at penalty 0 the immediate line repeats the full transaction
    (`:240-244`);
  - every other line (header, labels, dates, blank lines, any language) is ignored;
  - a file with no transaction line is `invalid_argument` ("not a standby file"), never echoing the text.

  Validation stays where it is: each extracted line goes through `import_shared_message` (S5), which decides whether
  it is a standby of a known masternode.
- **Test:**
  - round trip `file_text` → `standby_file_transactions` → each line passes `import_shared_message`, for a pair, a
    full-only file, an immediate-only file, and a zero-penalty file (one line);
  - a **Core-written fixture** checked in next to them (`mn/shared/testdata/standby-core-*.txt`): a file in Core's
    layout with a local date and short date-time, and a copy with translated header and labels (a non-English
    locale's strings), each extracting the same transactions as the wallet-format file of the same standby. If no
    Core-written file is at hand, the fixture is produced from Core's `standbyFileText` source layout and says so in
    its first line;
  - a text with hex that isn't a transaction (a txid line, an odd-length line) ignores it; a text with none is
    `invalid_argument`.
- dw-app's `mn_shared_import_file` (S5) calls it.

## 5. Fixture and tests

### 5.1 Fixture (`apps/desktop/src/fixture/masternodes.ts`, `fixture/masternodes.test.ts`)

A `MasternodesWorld` in `World` (as `governance`), testnet-shaped like MG-00's boards (19999, `y…`, tDASH).
- Handlers for every `mn_*` command refuse as the facade does (codes and order of mn §2), carry EP's fields, and
  `emit("masternodes")` on change.
- Grants are `FixtureGrant`s, checked for purpose and cap as governance's are.
- Platform reads have their own handlers and **don't** re-run on `masternodes` events, so a spec can assert the
  §3.5 rule.

| Scenario | World | Slice |
|---|---|---|
| `mn-list` | Home node (regular, enabled, all roles, attributed payouts), Office node (pose_banned, owner + collateral, `banned_seen_at`), Club evonode (tracked, `CollateralState::Unresolved`, attached owner key, `registrar: registration_unavailable`), Old node (removed); `unattributed_30d = 0` | S1 |
| `mn-v24-payouts` | as `mn-list`, with `unattributed_30d > 0`, rows `payouts_incomplete`, flagged records on Home node, two rows sharing a payout address, mining income to another wallet address (counted, copy says "to this wallet"), and an unattributed `last_payout` older than 30 days (no paid meta) | S1 |
| `mn-empty` / `mn-empty-coin` | no rows / plus a 1,000 Dash `CollateralCoin` | S1 |
| `mn-syncing`, `mn-list-failed` | `list_state.synced: false`; `mn_list` failing | S1 |
| `mn-watchonly`, `mn-locked` | watch-only wallet; locked vault (`roles.attached: None`, `registrar: vault_locked` on Club) | S1 |
| `mn-track-*` | find results; `matched{owner\|payout\|voting}`, `unchecked` (evonode and regular copy), `key_mismatch`; tracked-only + owner (`registration_unavailable`, also when locked) vs tracked-with-registration (`available{attached}`, `vault_locked` when locked) | S1 |
| `mn-keys` | keys per role, 25 operator keys (paging), operator matched in basic form and legacy form, platform node id matched | S1 |
| `mn-released` | Home node `collateral_released: true`, then restart | S2 |
| `reg-*` | registration keys, collateral coins (one `user_locked`), preview figures, preview that changes on the re-read (D5); refusals keyed by input (dup service, dup key, `keys_exhausted` per role, protx-dup) | S2 |
| `mn-ops` | operations in every `ProTxOperationState` and every kind, `debit` ≠ `fee` on a dissolve row, `debit` 0 on a foreign row, `sent` before and after `overdue` (D49 line for a fee-input row, the no-lock line for a dissolve row), a `conflicted` row (no action), a together send's `dissolve` row folded into its session | S2 (kinds S3/S6 added there) |
| `mn-evonode` | own evonode, credits, quote `fixed`; one variant per checking reason, `platform_behind` with and without lag | S4 |
| `mn-withdraw-*` | `-sent`, `-refused`, `-superseded`, `-couldnt`, `-marked-received`, `-marked-not`, `-sibling-paid`, `-late-proof`, `-done`, `-not-executed`; `-bg-resolve` (record turns `done` on a poll with no event; the quote's `open` clears on refetch); `-figures-change` (quote changes while the confirm is open); `-locked-attached-owner` and `-locked-attached-payout` (locked → unlock → relock); `-locked-keychain-owner` (unlock line secondary, withdrawal proceeds); `-insufficient` (balance covers the fee, not amount + fee); `-destination-changed` (the call returns `withdraw_destination_changed`, for withdraw and for Try again); `-two-wallets` (two wallets, the same evonode hash, different held keys) | S4 |
| `sh-capability-unknown`, `sh-capability-active` | `v24` unknown / active | S5 |
| `sh-coordinator-*` | draft (0/2, all in), signing (2/3 approved), combined (1/2 signed), ready to send, sent, registration won, confirmed + `create_standby` | S5 |
| `sh-recovery-*` | one world per T4 row (§4.4): `-sent-ok` (two-step exit, collapsed) and `-sent-ok-overdue` (expanded, badged), `-instant-locked`, `-spv-stopped`, `-refused`, `-unknown` (`-in-flight` before restart, two-step exit after), `-abandoned-in-activity` (with `-rescanning`), `-participant-recorded`, `-participant-signed`; the spend back: `-abandon-recorded` (Send again) and `-abandon-overdue` (its two-step exit), `-abandon-refused` then `-abandon-replaced` (`attributed` A, B) and `-abandon-first-confirms`, `-abandon-unknown-1` / `-abandon-unknown-n`, `-abandon-seam-1` / `-abandon-seam-n` (handed off, engine dropped before history), `-abandon-predispatch-crash`, `-abandon-cut`, `-abandon-deferred`, `-abandon-abandoned-in-activity` (rescanning, then Abandon again), `-abandon-legacy` and `-abandon-legacy-overdue` (Replace, whole reservation unspent), `-abandon-legacy-partial` (one input held by another spender: `coin_elsewhere`, then Replace once it's gone) for a coordinator, a participant and a key-change funder, `-abandon-legacy-recorded` (one-input A in history, the other coin free: its Activity exit and Resend) and `-abandon-legacy-recorded-no-peers` (no Resend), `-abandon-legacy-rescanning` (A abandoned in Activity, the rescanning line), `-abandon-legacy-two` (an old build's two spends: `coin_elsewhere` on the earlier one, then Replace taking both) for a coordinator, a participant and a key-change funder, `-abandon-legacy-other-spend` (neutral copy), `-abandon-replay-refused`, `-abandon-send-again-recorded`, `-abandon-refused-coin-elsewhere`, `-abandon-spv-stopped`, `-same-coin-n`; `-terminal-won` / `-terminal-conflicted` / `-terminal-dead` with A absent from history, each after release; `-no-ownership-path`; `-registration-resend-crash`, `-registration-resend-refused` (unknown, with the resend's reason); `-upgrade-refused-crash` and `-upgrade-fence-refused` (inherited codes read unknown) for a coordinator and a key-change funder, `-new-first-refusal`; `-too-small`, `-confirming-1`…`-confirming-5`, `-settled-6`, `-chainlock`, `-reorg`, `-other-spend`, `-released-restart`; each with its restart variant | S5 |
| `sh-participant-*` | invited, joined, signing to approve (with an `approved_terms` diff), signing approved, combined, **funding_signed (copy signed contribution, also after restart)**, sent, dead{inputs_spent}, conflicted, confirmed | S5 |
| `sh-dust-*` | coordinator and participant with no change (`debit = share (+ fee) + dust`), asserting displayed = cap | S5 |
| `sh-close-*` | before out (keep / discard), out (no question) | S5 |
| `sh-import-*` | every `shared_message{reason}`, every warning incl. `skipped_signatures`, `no_approvals`; a key-change and a standby import before S6 (read-only result) | S5 |
| `sh-detail` | confirmed shared masternode, `shares {1, 3}`, standbys (full + immediate), early period | S6 |
| `sh-detail-nosession` | EP 5 answers, no session: standby, together and key-change routes; payout change absent; `at_registration` from history; a world where EP 5 refuses `shared_registration_unknown`; and one world per `eligibility` (`too_early`, `delisted`, `upgrade_unknown`) | S6 |
| `sh-reward-*` | A at registration → B pending → B confirmed → C sent; C `not_sent`; C `conflicted`; a `NewAddress` update; sessionless; each after restart | S6 |
| `sh-standby-*` | H−2 / H−1 / H around `full_valid_from` (`full_valid_now` from the fixture's engine rule), full-only and immediate-only imports, zero-penalty share with no period, `-zero-penalty-period` (nonzero period, zero penalty, before its end: valid now), import stored / already held, a pair file (two results), a zero-penalty file (one result) | S6 |
| `sh-dissolve`, `sh-together-*`, `sh-keychange-*` | previews; together as preparer, approver, actor-fee approver, combined, 8-share fee default; key change signing, signed (copy), combined funder / non-funder, stale before out / after out, `dead{abandoned\|inputs_spent\|delisted}`, sent with each `handoff_error` outcome, `-unknown` (two-step exit), `-instant-locked`, `-abandoned-resend-refused` (EP's change 1), `conflicted` (funder) with each settlement, after restart; the approver after the funder's send, by the list: keys changed (`confirmed`), other keys (`stale`), delisted, list unchanged (stays `combined`, Discard offered); a key-change spend back `uncertain` and `refused` | S6 |

The fixture's message texts are fixed JSON blobs (fake sessions; public-shaped data only, no keys). Standby file
texts exist only inside the fixture's `mn_shared_import_file` / `export` handlers, never in a renderer value. No
seed, key or real address appears in any fixture or image (DEC-1484).

### 5.2 Unit tests (vitest)

`masternodes/model.test.ts`:
- chips, groups and In progress membership (§1.10);
- badge set per slice (§3.6);
- a sentence for every code and reason (§3.8);
- the payout split (`SplitAmountByShares`, remainder to the last);
- Q1/Q2 copy and `last_payout` qualification;
- the withdraw state table, D11's figure snapshot and re-enable;
- `next` → button for every row of §2.3.1, and an unknown combination failing;
- consent diff (`approved_terms` vs `terms`);
- §2.3.2's displayed debit = grant cap for both roles, with and without dust;
- grant caps per action (§3.4), from EP fields only;
- the together-fee formula;
- `formatCredits`;
- recovery copy for every settlement × `handoff_error` (tx × outcome) × `blocked` combination, that only
  `settled { abandon_tx, released: true }` produces "Your coins are back", that "didn't go out" appears only for a
  `refused` spend back, that each `abandon_spend.state` renders its own copy and buttons, and that no button renders
  for an action EP 4 didn't list;
- EP 7 copy per attempt state, and that the headline uses only `latest_confirmed_local` or `at_registration`.

`fixture/masternodes.test.ts`: handler refusal order against mn §2 for each command.

### 5.3 Playwright and accessibility

Flows run on the fixture in Chromium and WebKit, with axe (0 serious) per sheet. Specs per slice: §6.

**Accessibility** (Opus n8):
- `aria-live="polite"` regions for withdraw state and session stage changes;
- focus moves to the paste field when Join opens, and is trapped and restored per sheet;
- key and code fields are labelled;
- disabled reasons are readable without hover;
- keyboard-only runs of the registration and the session sheet.

**Visual baselines** (`e2e/visual.spec.ts`, regenerated in the image, light and dark): one per MG-00 frame a slice
builds, under the same names. The new states are `mn-v24-payouts`, `mn-withdraw-couldnt`, `mn-withdraw-locked`,
`sh-capability-unknown`, `sh-participant-signed`, `sh-recovery-pending`, `sh-recovery-two-step`,
`sh-recovery-confirming`, `sh-keychange-stale`, `sh-standby`, `sh-detail-nosession`. Revealed keys are **masked** in every baseline (a `mask` locator on the secret field), as
well as dropped from React state.

### 5.4 Regtest acceptance (GL)

Modelled on MG-09's `dwd_governance_ui.py` + `e2e-tauri/regtest-governance.ts` + `scripts/e2e-regtest-*.sh`.
No testnet or mainnet broadcasts: every money path stays on regtest (COMMON safety). dwcli never runs against the
app's own data directory (Q12).

- **`regtest/functional/dwd_masternodes_ui.py`** (MG-10), driven by `e2e-tauri/regtest-masternodes.ts`, in parts:
  - `--part track` (S1): dashd registers a masternode from its own wallet. The app tracks it by service, and the row
    reads Active with list facts. The app attaches its owner key: `unchecked`, and the pane reads
    `registration_unavailable`, with no Change keys action.
  - `--part register` (S2):
    1. funds the app's wallet from dashd;
    2. the app registers a regular masternode (FundNew, hosted: the operator BLS public key from a dashd-side keypair,
       an IPv4 service);
    3. the judge checks `protx info` (owner, voting, payout, operator key, service);
    4. the row reads Active once the ProRegTx confirms (no operator node is run unless the test lasts through DKG and
       PoSe, Opus n7);
    5. Release collateral → restart the app → the pane offers Lock it again (EP 1);
    6. a second run does ExistingUtxo and External (dashd's wallet signs the message).
  - `--part update` (S3): Change payout (ProUpRegTx); `protx info` shows it.
  - `--part withdraw-smoke` (S4): a regtest evonode registered by the app, with Platform unreachable. The sheet reads
    "Couldn't reach Dash Platform", and no grant is requested. The live withdrawal proof stays MG-13's dashmate T2.
- **`regtest/functional/dwd_shared_ui.py`** (MG-10b):
  - **Setup:** dashd with `-vbparams=v24:…` (MG-04b §12); dashd registers one version-3 masternode first, so `v24`
    reads `active` (MG-04b D3; precedent `dwd_shared.py:209-292`).
  - `--part create` (S5), run A: the app is the coordinator; two dwcli wallets with separate data roots are
    participants (`dwcli mn shared import|join|approve|sign`). Messages pass as files, **including each participant's
    signed contribution back to the app**. The app sends, and `protx info` shows the share table. An extra step
    restarts the app between import and send. The refused- and unknown-hand-off recovery arms are fixture-level (§5.1)
    unless the harness can make dashd refuse the transaction or keep it out of blocks. GL does run one real abandon:
    in run B, a separate session where the app signs as participant and the dwcli coordinator never sends; the app
    abandons at `funding_signed`, and after six blocks the judge checks the spend back to the app's wallet and the
    pane reads "Your coins are back" only then.
  - `--part create` (S5), run B: the roles reversed. The app is a participant, through join → approve → sign →
    **Copy signed contribution** (saved to a file, after an app restart) → the dwcli coordinator imports it and
    sends → the app reads Confirmed.
  - `--part maintain` (S6):
    1. the app changes its reward address A → B; the pane reads pending, then confirmed after a block, then the same
       after restart (precedent `:295-306`);
    2. the app creates a standby and saves a copy to a file;
    3. a **fresh app data directory restored from the same seed** (no session), with its birthday before the
       registration and a scan past it so history holds the registration (the precedent `:440-489` shows standby
       operations, not a seed-only restore, so this step arranges the scan itself), shows EP 5's shares, imports the
       saved standby file and sends its immediate variant; the judge checks every refund output and the penalty;
    4. a second masternode dissolves alone in the early period (penalty paid; precedent `:369-395`);
    5. a third dissolves together with dwcli.

## 6. Slices

Each UI slice is one PR with DEC-1484 artifacts (before = base revision, after = head, light + dark, fixture data),
mergeable on its own. Everything not yet built is **absent**, not disabled: a header button, More item, wizard card,
pane action, badge entry or import route appears only in the slice that makes it work (D14). The page stays behind
Settings ▸ Show Masternodes until S2 changes the setting (Q5).

Order: **EP → S1 → S2 (+ Q5) → S3 → S4 (MG-13 on main) → S5 → S6**. S3 and S4 are independent of each other.

Every merge gate is COMMON's gate, plus the rows below.

### EP: engine reads

- **Content:** §4: reads 1–7, EP 8's extractor, the key-change resend guard, the abandon spend's storage, same-coin
  bound, `shared_stage{abandoning}` refusal and `resend_shared_abandon` (change 4), the three behaviour-preserving
  refactors (coin test, `conflict_of`, `registered()` split; §4.4, §4.5), the dw-appdb migration adding
  `shared_mn_sessions.abandon_tx`, `abandon_log`, `legacy_bound` and `resend_marked`, with its backfill, its rewrite of
  inherited refusal codes and its durable writes, `mn-engine.md` v19 (or the next
  free number), E0-13 `FACADES`, the `FREE` row and the stub module's `EXCLUDED` rows (with `resend_shared_abandon`
  "deferred (MG-10 S5)").
- **Merge gate:**
  - `cargo test -p dw-engine` with T1–T8, EP 8's round trip and Core-format fixture, the parity tests (coin test with
    both roles and the fee/dust boundary, `conflict_of` with MG-04's `Conflicted` derivation, every `registered()`
    and `my_shares` call site, `outcome_of_code`), the derivation tests on existing-shape rows, the key-change
    regression test and its control, and T4's seam, crash, replay-refused, cut, deferred, legacy (whole, partial,
    recorded, no peers, rescanning, two-abandon upgrade), inherited-code upgrade, new first refusal,
    terminal-with-A-absent, same-coin and resend-marker (crash, fence-refused, guard-refused) cases, through the real `deliver_abandon`;
  - the updated `test_QT_126_abandon_spends_a_reserved_coin_and_releases_once_buried` (`abandoning`) with its
    control and multi-coin variant, and QT-127's named `input.len()` change with its multi-input variant;
  - the rewritten mn §2.4 rows (`abandon_shared_session`, the new `resend_shared_abandon`, `send_shared_registration`,
    `send_maintenance`) and
    MG-04b's facade row, with the listing re-blessed;
  - `cargo test -p dw-appdb`: the migration on an existing db (legacy backfill with `maybe_sent` and `legacy_bound`,
    the inherited-code rewrite per `INHERITED_REFUSED_CODES`, NULLs), the bound's clearing, the append-only log, the
    forward-only fold (`first` true and false, a replay's `not_sent` ignored), the replay and resend marks and the
    CHECKs (`migration_tests.rs`, `shared_sessions.rs` tests);
  - the contract listing test re-blessed and passing without `DW_BLESS`;
  - `cargo test -p dw-app` (`facades_are_covered`) and the regenerated `docs/design/E0-13-facade-coverage.md`;
  - no renderer change.
- **Review:** EP 4 is a recovery path for funds, and EP changes the abandon spend, the key-change resend and the
  funds-recovery predicates, with a migration in a shared table: two review chains (Opus-h + Sol high).
- **Fixtures, Playwright, GL, DEC-1484:** none (no UI).

### S1: read + track + keys

- **Content:**
  - binding of `list_state`, `list`, `masternode`, `payouts`, `find`, `evonode_credits`, tracking, `masternode_keys`,
    `reveal_masternode_key`;
  - the discovery reads `operations` and `shared_sessions` (no screen);
  - `UiSettings::masternodes_tab`; the tab bar;
  - My masternodes list, summary, pane (read only), empty, loading, failed, watch-only, locked;
  - Track, Add keys (owner, payout), Rename, Stop tracking, Keys you added with Remove;
  - Masternode keys with reveal; both Activity filters;
  - `formatCredits` (§1) with its test;
  - the ban badge; MG-09 F3 (labels in vote rows).
- **Hidden:** Set up a masternode, Join…, Change keys (the `registrar` reason line shows, no action), Release, the
  Withdraw button, In progress.
- **Needs:** EP; the MG-00 amendment (Q10) merged before S1 merges.
- **Merge gate:** E0-13 rows for S1 removed and the commands covered; vitest; Playwright below.
- **Fixtures:** `mn-list`, `mn-v24-payouts`, `mn-empty*`, `mn-syncing`, `mn-list-failed`, `mn-watchonly`, `mn-locked`,
  `mn-track-*`, `mn-keys`.
- **Playwright:** `e2e/masternodes.spec.ts`: list → pane → rename → copy; flagged payouts heading → See in Activity
  (per node and Generated); locked → unlock re-read. `e2e/mn-track.spec.ts`: track → add key (each `matched` role,
  mismatch, unchecked) → registration_unavailable line → remove → stop tracking. `e2e/mn-keys.spec.ts`: segments,
  paging, basic/legacy match, reveal → hide/lock clears.
- **Baselines:** `mn-list`, `mn-empty`, `mn-banned`, `mn-track`, `mn-keys`, `mn-watchonly`, `mn-locked`,
  `mn-v24-payouts`.
- **GL:** `dwd_masternodes_ui.py --part track`.

### S2: registration + sidebar (Q5)

- **Content:**
  - `mn_registration_keys`, preview, register (D5), External (prepare/submit/refresh), resend/discard,
    release/lock collateral (EP 1);
  - the wizard (regular + evonode; hosted + self-run; FundNew + ExistingUtxo + External);
  - the In progress group (§1.10);
  - the operations badge;
  - `UiSettings::masternodes_item` On/Auto/Off with the migration (§3.6).
- **Hidden:** the Shared card, Change keys, Withdraw.
- **Needs:** S1.
- **Merge gate:** vitest (D5 re-read, caps from `debit`); Playwright below; the GL part.
- **Fixtures:** `reg-*`, `mn-ops`, `mn-released`.
- **Playwright:** `e2e/mn-registration.spec.ts`: hosted FundNew, self-run evonode, ExistingUtxo, External prove →
  restart → continue → submit, preview change at final step, refusals per field, In progress states (no action on
  conflicted; `sent` → Send again at `debit`; `overdue` → the D49 line and Open in Activity). `e2e/mn-sidebar.spec.ts`: Auto shows on roles/tracking/coin/work, Off hides, migration from the old
  bool.
- **Baselines:** `reg-type`, `reg-collateral`, `reg-operator`, `reg-operator-evo`, `reg-keys`, `reg-review`,
  `reg-prove`, `reg-done`, `upd-release`, `mn-inprogress`.
- **GL:** `--part register`.

### S3: change keys

- **Content:** `mn_preview_update_registrar`, `mn_update_registrar`; the Change keys or payout sheet gated by EP 6;
  the operator explanation sheet; the payout-barrier success line.
- **Hidden:** Withdraw, shared.
- **Needs:** S2.
- **Merge gate:** vitest (Q4 preselection); Playwright below; the GL part.
- **Fixtures:** `mn-list` registrar variants, an `update_registrar` row in `mn-ops`, `update_pending` /
  `update_superseded` / `in_flight`.
- **Playwright:** `e2e/mn-update.spec.ts`: change keys with the PoSe warning, the Q4 payout default and its warning,
  each refusal, tracked-only vs tracked-with-registration, operator sheet custody variants.
- **Baselines:** `upd-registrar`, `mn-operator`.
- **GL:** `--part update`.

### S4: credits

- **Content:**
  - **the engine change** (R3): `reviewed_destination` on `withdraw_credits` and `retry_withdrawal`, the
    `withdraw_destination_changed` code, its contract bump and the seam test (§3.2);
  - MG-13 binding (§3.2);
  - **the `masternodeWithdraw` host grant**: `GrantPurposeSpec::MasternodeWithdraw`, its mapping, the generated TS,
    and the E0-13 text (Opus M1);
  - the withdraw sheet and the pane's Platform block with every state, the `mnPlatform` keys, D11, and the unlock
    path.
- **Hidden:** shared.
- **Needs:** S1 + MG-13 on main.
- **Merge gate:**
  - `cargo test -p dw-engine` with the seam test (withdraw and retry) and the contract re-blessed;
  - dw-app unit tests for the grant mapping (valid address → script, other-network refused, owner key → `None`);
  - the generated TS diff checked in;
  - vitest (D11, quote refetch on a record change);
  - Playwright below;
  - the GL smoke.
- **Fixtures:** `mn-evonode`, `mn-withdraw-*`.
- **Playwright:** `e2e/mn-withdraw.spec.ts`: each state; background resolve with no event; figures change while
  confirming (Withdraw off until shown); locked attached owner and payout → unlock → withdraw → relock; locked
  keychain owner (unlock line secondary); Max sends `credits{claimable}`; the owner call carries
  `reviewed_destination` equal to the shown address; `withdraw_destination_changed` → re-quote → review again (and
  for Try again); two wallets on one hash don't share a quote, and a wallet change clears the snapshot; no Platform
  read on an ordinary `masternodes` event.
- **Review:** S4 touches consent: two review chains (Opus-h + Sol high).
- **Baselines:** `mn-withdraw`, `mn-withdraw-pending`, `mn-withdraw-couldnt`, `mn-withdraw-locked`.
- **GL:** `--part withdraw-smoke`. The live proof is MG-13's T2.

### S5: shared creation (MG-10b-1)

- **Content:**
  - shared capability, sessions (EP 4), start/set fee/import/import file/join/message/lock/unlock/approve/sign/send/
    abandon/discard;
  - the Shared card in reg-type, Join… in More, the Start/Join sheet, the coordinator wizard, participant join, the
    session sheet with §2.3.1's registration rows, §2.3.2's disclosures and §2.3.3's recovery, and close;
  - In progress sessions;
  - the session badge (S5 set);
  - `FilePurpose::SharedMasternodeMessage` and `StandbyDissolution` (import only);
  - the base64 save adapter;
  - shared rows read "you hold k of n" with list facts;
  - maintenance-kind and standby imports return the read-only result.
- **Hidden:** the shared pane's maintenance actions, standby screens, key change, together.
- **Needs:** S2.
- **Merge gate:** vitest (render table, debit = cap, badge set under the S5 configuration, recovery copy); Playwright
  below; the GL part.
- **Review:** recovery and consent: two review chains (Opus-h + Sol high).
- **Fixtures:** `sh-capability-*`, `sh-coordinator-*`, `sh-participant-*`, `sh-dust-*`, `sh-close-*`, `sh-import-*`.
- **Playwright:** `e2e/mn-shared.spec.ts`:
  - coordinator start → paste 2 replies → lock → paste approvals → paste signed contributions → send;
  - participant join → approve (with diff) → sign → copy signed contribution → restart → still offered;
  - dust both roles;
  - one case per `sh-recovery-*` world: successful hand-off → collapsed two-step line, then expanded and badged
    once overdue (Open in Activity opens that transaction's sheet → Activity's Abandon → session Abandon);
    IS-locked and SPV stopped → `blocked` copy, no button; refused hand-off → send again or abandon; unknown
    hand-off → in-flight copy, then the two-step exit after restart; participant recorded; registration abandoned
    in Activity → rescanning line → Abandon; the spend back: recorded → Send again, then its two-step exit once
    overdue; refused → Try again (second fee named) → both ids attributed, the first confirming reads "Abandoned";
    unknown, seam, pre-dispatch crash, cut and deferred, each with one and several reserved coins → uncertain copy,
    Send again, no new spend, no "didn't go out", before and after restart; abandoned in Activity → Abandon again;
    legacy → uncertain, then Replace behind its confirmation once overdue; legacy with one input held elsewhere →
    `coin_elsewhere` with its Activity link and no "only one" copy, then Replace once the spender is gone; legacy
    recorded with another coin free → its collapsed Activity exit and Resend, expanded once overdue (no Resend without peers); legacy
    rescanning → the rescanning line, not `coin_elsewhere`; the legacy two-abandon upgrade → `coin_elsewhere` on the
    earlier spend, then Replace taking both; an inherited refusal after the upgrade → the unknown copy before any
    send; a refused resend → unknown with its reason; an interrupted resend → "couldn't confirm the resend
    finished"; a refused
    Send again → still uncertain; Send again recorded → its own exit; terminal sessions with A absent → settlement
    copy only; no ownership path; SPV stopped → no Send again; too small; confirming
    1–5; settled at 6 and at a ChainLock; reorg back to pending; another spend wins (neutral copy); released after
    restart; a conflicted session stays reachable until released and leaves In progress on `released: true`;
  - close keep/discard;
  - import refusals and warnings panel (survives refetch, gone on reopen);
  - read-only maintenance/standby import;
  - file open/save cancel and errors.
- **Baselines:** `sh-start`, `sh-setup`, `sh-terms`, `sh-join`, `sh-session`, `sh-close`, `sh-capability-unknown`,
  `sh-participant-signed`, `sh-recovery-pending`, `sh-recovery-sent-ok` (collapsed) and `-sent-ok-overdue`,
  `sh-recovery-instant-locked`, `sh-recovery-abandon-unknown-1`, `sh-recovery-abandon-refused`,
  `sh-recovery-abandon-legacy-overdue` (with its confirmation open), `sh-recovery-confirming`.
- **GL:** `dwd_shared_ui.py --part create` (runs A and B).

### S6: shared maintenance (MG-10b-2)

- **Content:**
  - `mn_shared_shares` (EP 5/7) and the shared pane (§2.4.1 and §2.4.2);
  - update share;
  - dissolve alone;
  - standby (prompt, create, export, import, send, with the §2.8 validity rules);
  - dissolve together (size-aware fee);
  - key change with §2.3.1's rows and §2.3.3's recovery;
  - the S6 badge entries;
  - `FilePurpose::StandbyDissolution` export;
  - `mn_shared_export_standby`.
- **Needs:** S5.
- **Merge gate:** vitest (standby validity from `full_valid_now`, sessionless and eligibility gating, badge set under
  the S6 configuration, EP 7 copy); Playwright below; the GL part.
- **Review:** S6 renders the key-change half of R1's recovery contract (the two-step exit after out, the new resend
  refusal) and builds grants for sends that pay penalties (standby immediate at fee + penalty, dissolve alone
  early) and the actor-fee consent: two review chains (Opus-h + Sol high; Opus r3 m8).
- **Fixtures:** `sh-detail`, `sh-detail-nosession`, `sh-reward-*`, `sh-standby-*`, `sh-dissolve`, `sh-together-*`,
  `sh-keychange-*`.
- **Playwright:** `e2e/mn-shared-maint.spec.ts`:
  - pane with and without session;
  - reward A→B pending → confirmed → restart;
  - standby create → save copy (no bytes in the renderer: assert the IPC result is only a file name) → send at each
    boundary;
  - partial import;
  - dissolve alone (early, checkbox);
  - together (actor fee, 8-share default);
  - key change approve, copy, stale before/after out, abandon, the two-step exit after out (collapsed until
    overdue; IS-locked → `blocked`), resend refused after abandon, dead states, conflicted (funder, each settlement,
    after restart), the approver after the funder's send (each list-derived outcome);
  - eligibility worlds (upgrade unknown, list unavailable, too early, delisted) with and without a session;
  - zero-penalty share inside its period sends the full variant ("valid now");
  - standby files: a pair imports two, a zero-penalty file one, a Core-written file (local date, translated labels)
    imports its transactions.
- **Baselines:** `sh-detail`, `sh-detail-nosession`, `sh-dissolve`, `sh-standby`, `sh-keychange-stale`.
- **GL:** `dwd_shared_ui.py --part maintain`.

## 7. D-rows (this design)

| # | Decision | Why |
|---|---|---|
| D1 | The page-state tab (`MasternodesPage.tsx:57-58`) becomes `UiSettings::masternodes_tab` (default My masternodes); MG-11 D2's page-state rule ends with S1. | MG-00 D3; MG-11 D2 said so until MG-10 lands. |
| D2 | The UI renders `SharedSession.next`, `stage` and `recovery`; it derives no session state itself. Its render table is §2.3.1. | The engine derives state on read from history and the list (MG-04b D27); a second derivation would drift. |
| D3 | "Paste" is a paste field plus [Open file…]; no button reads the clipboard. | The renderer has no clipboard read, and adding one is a shell permission change. |
| D4 | Consent screens show exactly the `revision` / `consent_hash` the user reviewed and pass them back. An import absorbed while the sheet is open re-renders it, and the button re-enables only after the new copy is on screen. | MG-04b D56: consent binds to the copy reviewed. |
| D5 | **Register cap (MG-09 D29 whole):** [Register] reads `preview_registration` again. If the figures match the review, it asks a grant capped at the confirmed `total_debit` + max(fee, 10,000 duffs) (MG-09 D29's margin, `feeGrantCap`). If they differ, the new figures show again (MG-09 D33), and Register stays off until they are on screen (D4). **Abandon** uses EP 4's charge. **Every other cap** is the engine's own figure from the preview, the record or EP 2–5. | The engine picks coins again at register time, and a cap of exactly the previewed fee would refuse. The cap is never sized from a figure the user didn't see; above the engine's figure it would weaken the guard. |
| D6 | Standby bytes stay in Rust: `mn_shared_export_standby` runs the save dialog and writes the file, and `mn_shared_import_file` runs the open dialog and passes the text to the engine's extractor (EP 8). Neither returns, logs or includes in an error any standby text. | MG-04b D21: a renderer not trusted to sign mustn't read a standby, which dissolves the masternode for every party. |
| D7 | Activity's masternode filters are route state: per node over `mn_payouts`' txids; wallet-wide over `TxTypeName::generated`. | MG-03 D10; no history DTO change; Q2's aggregate source. |
| D8 | Masternode keys show "Matched to {masternode}", joined in the renderer. Owner/voting match by address (`RegistrationFacts` / `ListFacts`), operator by **either BLS form** against `ListFacts.operator_public_key` (as `dw-protx/src/rules.rs:205-209` `other_scheme`), platform by node id. Basic form shown first. It never claims usage. | No engine read reports usage, and the engine's own scan also counts pending operations and sessions (G17); list entries v2 carry the basic form. |
| D9 | Owner and payout key secrets entered in Add keys are never echoed, logged or kept in React state after the call. | mn §4 (no code repeats a pasted key); U-02's secret-input rules. |
| D10 | A shared masternode's shares come from EP 5 (history); labels and terms from the session when present; the reward destination is "At registration" plus EP 7's `latest_confirmed_local` (the newest confirmed local update) and `latest_attempt` (a later unconfirmed one, with its state), each shown with its own wording; never one "newest update" standing in for both. | The SML carries no shares (MG-04b D29); the envelope isn't rewritten by `update_share` (Sol M3). |
| D11 | The withdraw confirm snapshots amount, fee, key and destination; any change after a re-read turns [Withdraw] off until the new figures are on screen; the grant is built from the shown figures. For the owner key, the shown destination is passed as `reviewed_destination` and checked again just before signing (R3); the copy keeps MG-13's caveat that a change after signing can still redirect the payout. Platform reads use `["mnPlatform", …]` keys with §3.5's refresh rule; the quote's key includes the wallet. | D4's rule for a money path whose figures come from Platform (Opus M3); MG-11 D3's reason for not re-reading Platform per event. |
| D12 | Close, resend, abandon, Send again, the two-step exits and discard come from EP 4's `recovery.actions` and their charges, a safe subset of what the engine accepts; the waiting copy from `settlement`, `handoff_error`, `abandon_spend.state` and `blocked`, all computed with the engine's own coin test (its arms and inclusion bound shared by the read and the call), buried, release and Activity-abandon predicates and the stored abandon record (D17); nothing is inferred from `updated_at`, `stale`, `SharedStage` alone or from what history doesn't hold; "didn't go out" only for a durably recorded definite refusal of a first dispatch that this engine recorded (inherited codes read unknown); terminal state settled before any action is built; "Your coins are back" only for `settled { by: abandon_tx, released: true }`. | Sol M4, r3 and r4 rulings: the rules are spread over several engine files, and a wrong guess shows a resend the engine refuses, a retry that signs a second spend, or a release that hasn't happened. |
| D13 | Holding an owner key (`roles.owner`) and being able to update the registrar (`registrar`, EP 6) are separate; the UI gates Change keys on the latter and names the missing fact. | Sol M6: a tracked-only node with an attached owner key can't update the registrar. |
| D14 | A slice shows no import route, badge entry or action whose screen it doesn't build; unsupported imports get a read-only "saved" result. | Sol m5. |
| D15 | Message saves go UTF-8 → base64 → `save_file_as`; opens go through `mn_shared_import_file`. | Sol m7: the existing file API is base64 both ways. |
| D16 | When the wallet recorded a session's transaction, its exit takes two steps: Activity's existing Abandon (its own confirmation and QT-091 conditions), then the session's Abandon. EP 4 lists the first step only when Activity's Abandon would accept it (SPV running, then `Facts::abandon_refusal` is None), else shows `blocked` (InstantSend-locked, SPV stopped); it is a collapsed secondary line, and unbadged, until `overdue`. A recorded spend back gets the same exit from its own history entry, whatever the other reserved coins read, with Activity's Resend when its bytes weren't kept. `abandon_shared_session` never abandons a wallet transaction implicitly; the pane names both steps and opens the transaction's sheet. | R1.1, Opus r3 M1: the engine refuses the session abandon while the recorded transaction spends the reserved coins (`send.rs:200-238`); folding the transaction abandon in would bypass its confirmation; offering it for an IS-locked transaction sends the user to a disabled button, and badging it flags every normal wait. |
| D17 | The abandon spend is a stored transaction like the session's own: its bytes, id, inputs and fee are written durably before it can leave, its outcome from the real broadcast result durably after, and every id it ever had is kept (`abandon_tx`, `abandon_log`). `not_sent` is recorded only for a definite refusal of its first dispatch; a replay first marks it `maybe_sent`, and a refused replay changes nothing. A new spend back takes every reserved coin still unspent and only when that set includes every input an earlier spend took (the shared coin test), so every spend back this engine signs conflicts with every earlier one and at most one of them confirms; for a legacy row, which an older version may have abandoned more than once, that proof is the whole original reservation unspent (a durable `legacy_bound` mark until the first spend that takes it all), and without it the pane shows the blocker and its Activity route and promises nothing. While the latest spend may be out the wallet only sends those same bytes again; it offers a new spend after a recorded definite refusal, after the user abandoned it in Activity, or, for a provably bounded legacy row, once overdue behind an explicit confirmation. Terminal state comes first. Any of the session's spends back that confirms reads as the user's own; ids an older build overwrote can't be recovered, so a legacy row's other-spend copy stays neutral ("A spend of these coins confirmed.", a known limit), and two legacy spends that build already signed may be disjoint and both confirm (a second known limit: no migration can make them conflict). Refusal codes inherited from an older version read unknown, since they can't show a first dispatch. | Sol r4 M1, Opus r4 M1/m1, r5 R1–R4: SPV hands a transaction to peers before the wallet records it, so missing history proves nothing, and a replay's refusal says nothing about the first attempt; the abandon spend needs the same durable-record discipline as the registration (MG-04 D6) and its own exit when it doesn't confirm. Sol r6 M1/M2: an older version's overwritten spends and unmarked resends leave nothing that proves a bound or a first refusal. |

## 8. Q1–Q14: MG's rulings, applied

| Q | Ruling | Where applied |
|---|---|---|
| Q1 | (b): the recorded payout with provenance and the caveat ("yPay8…Wm4c, from this wallet's registration records; a change made elsewhere wouldn't show here"). Own rows only; tracked rows have none. | §1.2 |
| Q2 | (a), plus Opus m11 and Sol m2. "Payments to this payout address (may include shared-masternode payouts)" lists the flagged records; `last_payout` is qualified; "coinbase income", not "masternode rewards", for unattributed; the wallet-wide Generated filter is the aggregate Activity source. | §1.2, §1.9 |
| Q3 | (a). Track offers Owner and Payout keys. Voting keys already attached are listed under "Keys you added" with Remove. G5 corrected: a tracked node with a derived voting key can already vote. | §0.1 G5, §1.2, §1.4 |
| Q4 | (a). Preselect "{recorded payout} (the address this wallet registered)" only when `payout_changes == Some(false)`; with `None`, no default. The final review shows the recorded destination and "If the payout was changed elsewhere, this sets it back." | §1.7 |
| Q5 | (b), with an On/Auto/Off setting, shipped with **S2**. The discovery reads come in S1, without navigation. | §3.6, §6 S1/S2 |
| Q6 | (a) stands: Abandon is an immediate secondary action behind the full confirmation, which says the coins come back only after a conflicting spend confirms. No timer. With R1: the two-step exit when the transaction is recorded (D16), and the `pending` / `confirming` / `settled` states. EP 4 supplies its facts and charges. Round 3: the two-step exit is offered only when Activity's Abandon would accept the transaction, and badges only once `overdue`; the buttons stay timer-free; no second abandon while the first may still confirm. Round 4 (D17): the spend back is stored before dispatch and only sent again while it may be out; a new spend back takes every reserved coin, after a recorded refusal, an Activity abandon, or (legacy) once overdue with its own confirmation; the spend back has its own two-step exit once overdue. Round 5: the legacy replacement and its "only one can confirm" promise need the shared coin test's inclusion proof; a refused replay never reads "didn't go out"; terminal state comes first. Round 6: a legacy replacement needs the whole original reservation (a durable mark until the first spend that takes it all); inherited refusal codes read unknown; the rescanning line wins over `coin_elsewhere` while the coins are on their way back. | §2.3.3, §4.4 |
| Q7 | (a): a fixed 1,000 duffs for the unilateral standby, with the refusal path (§3.8). Together uses m10's size-aware default. | §2.7, §2.8 |
| Q8 | (a): fee shown with [Change] in draft before replies only; after that, the restart sentence. | §2.2 |
| Q9 | (a), worded: "This wallet hasn't yet seen evidence that v24 is active on {network}. Shared masternodes become available once it has." | §1.6, §2.9 |
| Q10 | (b): one docs-only MG-00 amendment, based on `b18152eb`'s MG-00 edits. It lands on main after MN-13 merges and before S1 merges; S1's code can start in parallel. It covers G1–G21 and the review's additions: debit and dust (G1), participant v3 forms (G3), derived voting keys (G5), standby variants and next-block validity (G7), history-based recovery (G10), registrar capability (G19), "At registration" (D10), Q1/Q2 copy, Q5's setting, Q6's abandon. Round 2 adds R1's two-step exit and settlement copy, the zero-penalty validity rule, and R3's "checked again just before signing" copy. Round 3 adds the InstantSend-locked "nothing to abandon" copy and the "may have gone out" / "couldn't confirm" copy for unknown hand-offs (Opus r3 Q10). Round 4 adds the spend-back copy by state: "may already be on the network" with Send again, "You cancelled the earlier spend back", the legacy replacement's "only one of them can confirm" confirmation, and the rescanning line (Opus r4 Q10). Round 5 adds the `coin_elsewhere` and `no_ownership_path` lines and the legacy neutral settlement copy, and carries the legacy "only one" confirmation only with its inclusion condition (Sol r5 Q10, Opus r5 Q10). Round 6 adds the "resend didn't finish" and "the last resend didn't go out" unknown copy, the rescanning precedence over `coin_elsewhere` and the collapsed legacy Resend in Activity, and drops any "didn't hand it to the network" claim for an inherited refusal (Opus r6 Q10, Sol r6 Q10). | §3.7 |
| Q11 | (a), with n5 and m3: both the save dialog and the file import run in dw-app. | §2.8, §3.3, D6 |
| Q12 | (a): the app as coordinator with separate-root dwcli participants, then the roles reversed. Never dwcli against the app's own data directory. | §5.4 |
| Q13 | (a): "5%, paid first once the operator sets a payout address. This wallet can't see yet whether that has happened.", and the reward destination as in Sol M3. | §2.4.1 |
| Q14 | (a), in EP 1. | §4.1 |

---

Lane MN-UI, 2026-11-10, r7. Design only: no code, commits or pushes were made.
