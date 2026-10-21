# DP6-03: the DashPay card in Tools ▸ Information

ROADMAP DP6-03 (size S, UI, tier T0, engine review N). DASHPAY F23 (iOS Sync Info, IOS-114/139) puts a DashPay card
in Tools ▸ Information: the loops, the last pass, pending contact crypto, the scan verdict, the quorum source and
"Sync now". It replaces U-05's "not yet (DP6-03)" row. Its sources are E0-05 (the bring-up outcome and the loops),
E0-10b (the quorum source and the degraded mode, DEC-168) and BL-153 (`DashPay.status`). Every value comes from
generated IPC types in `apps/desktop/src/api/generated/`. The task adds no engine call, no app command and no binding.

| Part | Where |
|---|---|
| View model (no React) | `apps/desktop/src/tools/dashpayInfo.ts` (`dashpayInfo`, the row formatters) |
| Its tests | `apps/desktop/src/tools/dashpayInfo.test.ts` |
| Screen | `apps/desktop/src/screens/tools/DashPayInfo.tsx`, placed in `Information.tsx` where the "not yet" row was |
| Backend methods (generated commands, already bound) | `api/backend.ts`, `api/tauri.ts`: `dashpaySyncStatus`, `quorumSource`, `trustedQuorumsOnly` |
| Fixture | `src/fixture/handlers.ts` (`dashpay_sync_status`, `quorum_source`, `trusted_quorums_only`), `dashpay.ts` (`fixtureLoops`, `lastPass`), `scenarios.ts` (`trust`, `dashpay-info-*`) |
| Fixture tests | `src/fixture/tools.test.ts` "fixture DashPay card (DP6-03)" |
| Playwright + axe | `e2e/dashpay-info.spec.ts`; the Information test in `e2e/tools.spec.ts` |
| Visual baselines | `e2e/visual.spec.ts` `visual tools-dashpay-card …` (the card alone) |

## 1. Fields

| Row | Source (facade → command → generated type) | Format |
|---|---|---|
| State chip and headline | `dashpayInfo` over all the reads below (§2) | a chip (`chip-success` / `-neutral` / `-warning` / `-danger`) and one line; a second line explains, where needed |
| Identity scan (the scan verdict) | `DashPay::sync_status().startup` → `dashpay_sync_status` → `DashPaySyncStatus.startup` (`StartupStatus`, the bring-up outcome, DASHPAY §3.2) | one text per outcome (`STARTUP_TEXT`), e.g. "Complete: identity found", "Failed: discovery did not finish", with a note where the user can act or wait |
| Last pass | `DashPaySyncStatus.lastPass` (`SyncPassReport \| null`) | "—" with "No pass has reported yet." while `null` (always today, D4); else its end as the tab's dates ("Tue Oct 6 11:59:55 2026") and "2 new requests, 1 new contact", "Nothing new" or "Failed: <code>" |
| Contacts waiting for an unlock (pending crypto) | `DashPaySyncStatus.pendingContactCrypto` (the bring-up's `contact_accounts_pending`) | "None", else the count |
| One row per loop | `DashPaySyncStatus.loops` (`SyncLoopStatus[]`, `NetworkSession::platform_loops`, engine order) | "Running · last run 11:59:55 · next 12:00:10", "Running · no run yet", "Stopped · last run …", "Stopped"; today's times as HH:MM:SS, older ones as the tab's dates (D9) |
| Quorum keys (the quorum source) | `DashPaySyncStatus.quorumSource` (`QuorumSource`); without a wallet `NetworkSession::quorum_source` → `quorum_source`; the toggle `NetworkSession::trusted_quorums_only` → `trusted_quorums_only` | "SPV (verified masternode list)"; "Trusted service until masternode sync finishes" (data unverified); "Trusted service (degraded mode)" (compared with SPV where it holds the quorum); "Trusted service only (developer setting)" (D5) |
| Whether the wallet has an identity | BL-153 `DashPay::status` → `dashpay_status` → `DashPayStatus`, the shell's query (`useDashPayShell`, key `["dashpay", "status", walletId]`) | not a row: it decides Watch-only, Not set up and Starting (§2) |
| Sync now | `NetworkSession::dashpay_sync_soon` → `dashpay_sync_soon` (D3) | a small tinted button in the card's header; held, with the reason under the headline, while it would do nothing |

## 2. States

`dashpayInfo` takes the first that applies (D2):

| State | When | Chip / headline | Sync now |
|---|---|---|---|
| `noWallet` | no wallet on the network | neutral "No wallet" / "No wallet on this network yet"; the quorum row alone, from `quorum_source` | none |
| `error` | `dashpay_sync_status` failed and nothing was read | danger "Unread" / "Could not read DashPay's sync state", the reason, Try again; no rows (D7) | none |
| `loading` | not read yet | neutral "Reading" with a spinner | none |
| watch-only (`watchOnly`) | the status is `noIdentity{watchOnly}` | neutral "Watch-only" / "DashPay needs this wallet's keys" (DP1-08's copy) | held: "A watch-only wallet can't use DashPay because it has no keys." |
| failed (`failed`) | the bring-up ended `discoveryFailed`, `identityScanIncomplete`, `partialNoIdentity`, `partialAccountsPending` or `seedBindingUnverified` (the Home card's failure set, status.ts), or the last pass reported a `failureCode` | danger "Failed" / the Home card's "Could not finish looking for your DashPay identity" or "Could not confirm your DashPay identity", or "The last DashPay pass failed" with "It stopped with <code>." | as the loop allows |
| not started (`notStarted`) | the bring-up did not run (`notRun`) | neutral "Not started" / "DashPay has not started" | held while the loops stop (D3) |
| starting (`starting`) | the bring-up runs (`starting`, with a spinner) or waits for an unlock (`identityUnsettled`), or the status is `startupIncomplete` (no identity known yet, BL-153 D16) | neutral "Starting" / "Looking for your DashPay identity…" or "Unlock to look for your DashPay identity" | as the loop allows |
| not set up (`notSetUp`) | the status is `noIdentity` (`null` or `waitingForSync`) | neutral "Not set up" / "No DashPay identity on this wallet", "Join DashPay to create a username." (or "…once the wallet has synced.") | as the loop allows |
| degraded or fallback (`degraded`) | an identity (or the status unread), and quorum keys not from SPV | warning "Degraded" / "Running in degraded mode", "Running on trusted quorum keys until masternode sync finishes" or "Running with SPV quorum checks off", with the quorum row's note | as the loop allows |
| running (`running`) | an identity (or the status unread), quorum keys from SPV | success "Running" / "Running on SPV-verified quorum keys" | as the loop allows |

All states but `noWallet`, `error` and `loading` show every row. Each has a fixture scenario (`src/fixture/tools.test.ts`
checks it end to end): `dashpay-info-spv` (running, a pass report, two contacts waiting), `dashpay-registered` (degraded,
the default quorum source), `dashpay-info-fallback`, `dashpay-info-trusted-only`, `dashpay-info-pass-failed`,
`dashpay-look-failed`, `dashpay-starting`, `dashpay-identity-unsettled`, `dashpay-not-started`, `funded` /
`dashpay-no-identity`, `dashpay-waiting-for-sync`, `dashpay-watch-only`, `no-wallet`, and `dashpay-info-stopped` (SPV
stopped: the loops stop and Sync now is held). A failed read is `fail=dashpay_sync_status:<class>`.

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | The card reads `dashpay_sync_status` and the shell's BL-153 status query (same key, same cache), plus `trusted_quorums_only`; `quorum_source` only without a wallet (with one, the sync status carries the source). The wallet is the toolbar's (`ui.walletId`, else the first), as Settings' DashPay card picks it. No engine call, command or binding is added: all four commands were bound by E0-13/E0-10b and only lacked a `Backend` method. | The brief: reuse BL-153's status rather than a second status path; generated IPC types only; adding engine API is out of scope for a UI row. |
| D2 | The precedence of §2. Watch-only first (nothing else can change for a wallet without keys); then a failure, which the user may need to act on even with an identity on file; then the bring-up's own progress; then no identity, which beats degraded (without an identity nothing of the user's is verified by those keys, and the quorum row still says where they come from); then degraded, then running. The failure set is the Home card's (`startupCard` in status.ts), so the two surfaces never disagree. With the status unread the sync status decides alone. | The brief's six states, each with one place in the order; consistency with DP1-08. |
| D3 | "Sync now" calls `dashpay_sync_soon` (E0-05's pass-now entry: a DashPay pass and the balance pass, in the background; nothing while the Platform loops are stopped; a pass in flight is not repeated). `DashPay::sync_now`, which would return the pass's report, is still a `platform.not_implemented` stub (startup.rs), so the card does not call it and no API was added. The button is held, with the reason as its `aria-describedby`, while the DashPay loop does not run (the kick would do nothing) and for a watch-only wallet. Since the kick reports nothing, the card says the pass was asked for and reads itself again once, 5 s later (`SYNC_NOW_REREAD_MS`); a pass that finds news also sends its Platform signals (D6). | The brief: use the existing cadence entry, add none. A disabled button with its reason (DASHPAY §3.2's watch-only wording) is more honest than a click that does nothing. |
| D4 | `last_pass` is `None` until sync passes report (E0-10b §7, m4 §2.1), so today the row always shows "—" and "No pass has reported yet.". The formatter and the failed-pass state are built and tested on fixture reports anyway, so the card needs no change when the engine fills the field. | The contract's field exists; the card shows what the engine has. |
| D5 | `QuorumSource::Trusted` is both the default degraded mode (DEC-168) and the developer toggle, so the card also reads `trusted_quorums_only` to tell them apart. While the toggle is unread the card assumes the default, the degraded mode. Every source other than `spv` is the degraded state. Until H-09 turns on enforcement, every wallet with an identity therefore shows "Degraded" (E0-10a failed, DEC-168). | The truth, as m4 §2.2 describes it; the toggle changes what "trusted" means for the user's data. |
| D6 | The sync status's query key is `["dashpay", "syncInfo", walletId]`, under `["dashpay"]`, so every `platform` and `dashPayNews` event re-reads it (events.ts). Sync events re-read it too (`useReloadOnSync`, as the tab's other cards), because the loops start and stop with SPV and send no Platform signal. Nothing polls (DASHPAY §3.5). The loops' times are those of the last read. | Hosts re-query on signals and never poll; sync events come at most 4 Hz. |
| D7 | A failed `dashpay_sync_status` read shows the reason and Try again, and no rows: nothing is guessed. A failed status read alone (`dashpay_status`) is not an error: the sync status decides (D2). | BL-153: a failed read is never a guessed state. |
| D8 | The scan verdict is the bring-up's `StartupStatus`, one text per value (DASHPAY §3.2 lists the library's seven plus `NotRun` and `Starting`, and `identityUnsettled`). The notes reuse DP1-08's copy where it already says the same thing. | F23's "scan verdict" is the bring-up's identity scan outcome; one source of copy. |
| D9 | Today's times show as HH:MM:SS, older ones in the tab's dash-qt date format (`dateText`). The pass's end always shows the full date. | Loops run every 15 s to 10 min, so a time is enough, unless the machine slept across midnight. |
| D10 | The loop rows follow the engine's list and order (identity, DashPay, DPNS contests). `platformAddressSync` has a title for when X1a adds it. | The engine decides which loops exist; the card shows each. |
| D11 | Fixture: `World.trust` (`{source, trustedOnly}`, default `trusted`/`false`: DEC-168's default), `DashPayWorld.lastPass` (a pass report, for scenarios), `fixtureLoops` (running while SPV runs and a bring-up ran, as `kick`/`platform_loops` behave). `withStatus` now sets the bring-up outcome to match a `startupIncomplete` status and `readOnly` for a watch-only one; before this, it was always `noIdentity`. Nothing else in the renderer reads `dashpay_startup`. | The card reads both the status and the outcome, so a scenario must not contradict itself. |
| D12 | Visual baselines are of the card alone (an element shot), running and degraded, light and dark (four files). The tab's own viewport baseline ends above the card and is unchanged (all 260 visual tests pass with only the four new files). | The brief: baselines for the new card only. |
| D13 | `docs/parity.md` IOS-114 moves to partial (the DashPay part of iOS's Sync Info screens; Platform address and Shielded sync info are not built), with the summary counts. The iOS summary row was already one off (it said 58 partial and 50 not started; the rows had 59 and 49), so it now gives the rows' own counts, 60 and 48. | DashPay rows record their Tauri state (DP4-03, DP1-10). |
| D14 | The axe pass is one test per state and theme (24 per browser), not one test looping over every state: looping over 12 page loads ran past the 30 s test timeout on WebKit. | Each state's result stands alone, inside the default timeout. |

## Mutation check

`/work/scratch/dw-dp6-03/mutants.py`: 10 hand-written mutants of `tools/dashpayInfo.ts`, each run against
`dashpayInfo.test.ts` and `fixture/tools.test.ts`. All 10 are killed, among them degraded (`trusted`) shown as
healthy (M1, killed by "D5: quorum keys not from SPV are degraded, never Running"), every source healthy (M2), a
scan failure not a failure (M3), Sync now gated on any loop instead of the DashPay loop (M4), watch-only missed (M5),
no identity shown as degraded (M6), the toggle's headline swapped (M7), a failed pass ignored (M8) and a failure's
tone (M9). M10, the row's text with the toggle unread, survived the first round. A test now pins it.

## Follow-ups

| # | What | Owner |
|---|---|---|
| F1 | When `DashPay::sync_now` gets its body, Sync now can show the pass's own report instead of D3's re-read. | the engine row that implements `sync_now` |
| F2 | When passes report (`last_pass`), the row fills with no UI change (D4). | E0-05 / the sync-report owner |
| F3 | Once H-09 makes enforcement the default, the card shows Running on SPV keys with no change (D5). | H-09 |
