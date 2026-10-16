# U-06: CoinJoin card, page and settings (Tauri)

ROADMAP U-06 for the Tauri renderer: mixing starts and stops from the UI. The engine's CoinJoin client is
M3 R1 (m3-engine §2.1: `coinjoin_limits`, `coinjoin_settings`, `set_coinjoin_settings`, `coinjoin_status`,
`start_mixing`, `stop_mixing`, the `CoinJoin` event) and dw-app binds every call already (E0-13, generated
`CoinJoin*.ts`). U-06 adds **no engine call** and changes no contract: it is the Home card, the CoinJoin page
and Settings ▸ CoinJoin over those calls, the fixture's CoinJoin engine, and a real-engine check on regtest.

The UX follows the Dash mobile wallets (DEC-01; Android `MixingStatusCard`, `CoinJoinLevelFragment`,
`SettingsScreen`; strings in `wallet/res/values/strings-dashpay.xml`): a mixing level (Intermediate or Advanced),
Start / Stop Mixing, a Home card while mixing with its state, percentage and "mixed of total", and Paused when the
network blocks it. dash-qt's facts that a desktop user expects stay where mobile has nothing: Core's session
status lines (research 02 §9.4, verbatim), the Options ▸ CoinJoin values (§9.3) and the progress breakdown
(QT-042) in a collapsed "Session details".

| Part | Where |
|---|---|
| View model (no React): states, copy, start/stop and settings rules | `apps/desktop/src/coinjoin/model.ts` |
| Its tests (by state and parity ID) | `apps/desktop/src/coinjoin/model.test.ts` |
| Reads and writes (TanStack Query) | `apps/desktop/src/coinjoin/queries.ts`, keys in `api/queries.ts` (`["coinjoin", …]`) |
| Event bridge | `api/events.ts`: `coinJoin` and `balances` re-read the wallet's status |
| Home card | `src/screens/coinjoin/CoinJoinCard.tsx`, mounted by `screens/Overview.tsx` |
| CoinJoin page | `src/screens/coinjoin/CoinJoinPage.tsx` (route `coinJoin`) |
| Settings ▸ CoinJoin | `src/screens/settings/CoinJoin.tsx` (a sixth tab) |
| Fixture engine and scenarios | `src/fixture/coinjoin.ts` (handlers, `coinjoin-*` scenarios), `src/fixture/coinjoin.test.ts` |
| Playwright + axe (light, dark, Chromium, WebKit), baselines | `e2e/coinjoin.spec.ts`, `e2e/coinjoin.ts`, `e2e/visual.spec.ts` |
| Real app on dashd regtest (T1) | `e2e-tauri/regtest-coinjoin.ts`, `scripts/e2e-regtest-coinjoin.sh` |

## States

`coinjoinView(inputs)` derives one state from the wallet's `CoinJoinStatus`, the read's error, the vault's lock
state and the balance; the Home card, the page and the tests share it. Rows are checked in order (S6 before S4); the first that
matches wins.

| # | State | Condition | Title | Line under it | Button | Home card |
|---|---|---|---|---|---|---|
| S1 | `loading` | no status read yet | — | — | none | none |
| S2 | `readFailed` | the status read failed | "Could not read the mixing status" | the failure | Try again | none |
| S3 | `stopping` | `state = stopping` (dw-engine halts synchronously today, so only the fixture reaches it; kept for the engine's `stopping` state) | "Stopping…" | — | Stop (disabled) | yes |
| S6 | `finished` | `state = mixing`, completion ≥ 100 %, whatever the status: it waits for new coins, `notEnoughFunds` below the target and `idle` above it (dw-engine does not rebalance); checked before S4 | "Fully Mixed" | Core's status line | Stop Mixing | yes |
| S4 | `starting` | `state = mixing`, below 100 %, status `idle` (the first step runs 5–15 s after Start) | "Starting…" | — | Stop Mixing | yes |
| S5 | `paused` | `state = mixing`, below 100 %, status one of the **waiting** codes (D4) | "Paused" | Core's status line | Stop Mixing | yes |
| S7 | `mixing` | `state = mixing`, any other status | "Mixing" | Core's status line | Stop Mixing | yes |
| S8 | `disabled` | `unavailable = disabled` | "CoinJoin is turned off" | "Turn it on in Settings ▸ CoinJoin." | Open Settings | none |
| S9 | `watchOnly` | `unavailable = watchOnly` | "Watch-only wallets cannot mix." | — | none | none |
| S10 | `locked` | `state = idle`, `stopReason = vaultLocked` | "Mixing stopped" | "Mixing stopped because the wallet was locked." | Start Mixing | yes, until dismissed |
| S11 | `error` | `state = idle`, `stopReason` `sessionClosed` or `walletUnloaded` (today dw-engine sets neither: a closed wallet's read fails as S2; kept for the engine's reasons) | "Mixing stopped" | "The network session closed." / "The wallet was closed." | Start Mixing | yes, until dismissed |
| S12 | `tooLittle` | `unavailable = insufficientFunds` | "Not Started" | "CoinJoin requires at least 0.00140001 tDASH to use." | Start (disabled) | none |
| S13 | `done` | `state = idle`, completion ≥ 100 % | "Fully Mixed" | — | Start Mixing | none |
| S14 | `off` | otherwise (idle, never started, stopped by the user or by turning CoinJoin off) | "Not Started" | — | Start Mixing | none |

Every state with a status shows the percentage (`progress.overallPercent`, dash-qt's QT-042 formula, floored) and
"Mixed X of Y" (`balances.fullyMixed` of `amountAndRounds.amount`, D3) — masked in discreet mode, as Android
masks them. The stop sheet and a start or stop that fails add their own line (D8, D9).

## Lock and passphrase

The engine has the mixing-only unlock (dw-vault `UnlockedMixingOnly`, `unlock_mixing_only`, QT-112): mixing signs
with the CoinJoin scope, sending stays locked. The renderer already offers it: the lock screen's "For mixing only"
box (U-02) and the status bar's "Unlocked for mixing only", which opens the full unlock (U-02 D12). U-06 adds no
new prompt (D6):

- While the vault is locked the lock screen covers the shell, so Start is never pressed on a locked vault. If the
  vault locks between the press and the call, `start_mixing` answers `coinjoin.vault_locked`: "Unlock the wallet,
  at least for mixing, and try again."
- A vault that locks stops mixing (`stopReason = vaultLocked`, S10). That includes auto lock (U-04 D12): mixing is
  not window activity, so the page says when auto lock is on that it will stop mixing.
- Unlocked for mixing only, the page shows "Unlocked for mixing only: mixing runs, sending stays locked." and
  Start works.

## Settings ▸ CoinJoin, and what persists

| Row | Binds to | Range | Parity |
|---|---|---|---|
| Enable CoinJoin features | `enabled`; off stops every wallet's mixing (`stopReason = disabled`) | — | QT-046 |
| Mixing level: Intermediate (4 rounds) / Advanced (8 rounds) / Custom | `rounds` (D2) | — | AND (`CoinJoinMode`) |
| Mixing rounds | `rounds` | 2–16 | QT-046 |
| Target balance (DASH) | `targetAmountDash` | 2–21,000,000 | QT-046 |
| Enable multi-session, Parallel sessions | `multiSession`, `maxSessions` | 1–10 | QT-046 |
| Inputs per denomination: Target, Maximum | `denomsGoal`, `denomsHardCap`; Target ≤ Maximum | 10–100,000 | QT-046 |
| Restore defaults | `coinjoin_limits.defaults` | — | QT-141 (for this tab) |

The ranges and defaults come from `coinjoin_limits`, never from the renderer. The engine keeps the values in
app.sqlite **per network** for every wallet; they apply live (rounds and target from the next session). Mixing
itself is **not** kept: it stops when Dash Wallet quits, the wallet closes or the vault locks, and starts again
only from Start Mixing (D5). The page and the tab say both.

## Engine events the UI listens to

| Event | Re-reads | Why |
|---|---|---|
| `coinJoin {walletId}` | that wallet's `coinjoin_status` | the engine's own signal: state, sessions, status, balances or progress changed (≤ 1 Hz) |
| `balances {walletId}` | that wallet's `coinjoin_status` | a read registers the wallet with the engine's 1 s refresher for a minute (D7) |
| `sessionOpened`, `sessionClosed`, `eventsOverflowed` | everything (existing rule) | — |
| `set_coinjoin_settings` (the renderer's own write) | `["coinjoin"]`: settings and every status | `enabled` and `rounds` move the status |

A vault lock needs no rule: the mixing loop sees it within a tick, halts, and the engine sends `coinJoin`.

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | The Home card shows only while mixing does something (S3–S7) or stopped without being asked to (S10, S11), as Android's `MixingStatusCard` shows only MIXING, PAUSED and FINISHING. The stopped card has Dismiss, which hides it for that stop until the window reloads. The card's title is a button that opens the CoinJoin page. The dismissal ends when any CoinJoin surface (card or page) sees that wallet mix again, so a later stop shows again. | Mobile keeps Home quiet when mixing is off; a stop the user did not ask for (auto lock) is news. |
| D2 | Mixing levels are Android's: **Intermediate = 4 rounds** (Core's default) and **Advanced = 8 rounds** (`CoinJoinMixingService.DEFAULT_ROUNDS * 2`), with Android's copy and time estimates. A level is `rounds`, a network-wide engine setting; any other value (set in Settings) shows as "Custom: N rounds". Picking a level while mixing applies from the next session; no confirmation (Android asks), because the change is undone by picking again and dash-qt applies options live. | One knob: the engine already has `rounds`, and Settings keeps dash-qt's finer values. |
| D3 | "Mixed X of Y": X is `fullyMixed`, Y is `amountAndRounds.amount` (the engine's `min(anonymizable + fullyMixed, target)`). Android shows mixed of the total balance; ours stops at the target, so Y is what mixing aims for, and the percentage agrees with it. | Showing the total would read as unfinished forever with a balance above the target. |
| D4 | Paused (Android: "blocked by network connectivity") is a running mixer whose status cannot move until something outside changes: `syncInProgress`, `noMasternodes`, `noCompatibleMasternode`, `noRandomMasternode`, `notEnoughFunds`, `noCompatibleInputs`, `unconfirmedDenominated`, `walletLocked`. Every other status (connecting, queue, signing, masternode replies) is Mixing. The status line under the title is Core's string, verbatim (research 02 §9.4; the Swift app's L10n). | The engine retries the transient ones itself; the waiting ones need sync, masternodes, funds or confirmations. |
| D5 | Mixing does not resume on the next launch; Android resumes its chosen level when the app opens. The engine keeps no "was mixing" record (its mixers are memory) and dash-qt's `-coinjoinautostart` is off by default. The page says "Mixing runs while Dash Wallet is open and the wallet is unlocked, at least for mixing." | A resume needs an engine setting and a decision on starting mixing before the user unlocks; follow-up BL row, not a UI rule. |
| D6 | No "lock except mixing" button. The way to mix with sending locked is the lock screen's "For mixing only" (U-02). A button would have to lock (which stops mixing within a tick) and unlock again, racing the mixer. The page explains it. | The vault and the lock screen already do it without a race. |
| D7 | `balances` re-reads the status. The engine refreshes a wallet's status every second only while it mixes or while its status was read in the last minute; an idle wallet's progress (new coins) would otherwise go stale. The read returns the cached status at once and the refresher's next tick sends `coinJoin` if it changed.  Cost: every status read keeps the wallet in that 1 s refresher for a minute, and the Home card reads it for every wallet, mixed or not, so a long sync's `balances` keeps idle wallets refreshed (follow-up: read only once mixing was used). | Keeps an idle page right without polling. |
| D8 | Stop asks first (Android "Are you sure you want to stop mixing?"), with our own line: "Coins mixed so far stay mixed. Open sessions end and their coins are released." Android's "mixed funds will be combined with your unmixed funds" is about its spend-mixed-only mode, which this wallet does not have. | Accurate to the engine's `stop_mixing` (resetPool). |
| D9 | A start or stop that fails shows its reason under the button (`coinjoin.disabled`, `watch_only`, `insufficient_funds{minDuffs}`, `vault_locked`, else the shared failure text) and keeps the state. One call at a time: the button is busy until the engine answers, then the status is re-read. | Same pattern as U-04's dust card (D36 there). |
| D10 | Not built here: dash-qt's "Enable advanced interface" (the page's Session details disclosure is the advanced view), "Show popups for mixing transactions" (desktop notifications are W-02's), "Warn if running out of keys" (HD wallets do not run out; `keysLeft` is always null), the CoinJoin salt (the console has `coinjoinsalt`), the recovery scan and Move mixed coins (MP-08), grouped "Mixing Transactions" rows in Activity, the "Most Common" first-use hint, and a send page fixed to mixed coins (Pay ▸ Advanced already offers "Fully mixed funds only", U-03 D7). | Outside "card, page and settings"; each has its own row or owner. |
| D11 | Settings ▸ CoinJoin saves the numbers with one Save (checked against `coinjoin_limits` first; the engine's `invalid_argument` message is shown if it still refuses: "The settings were not saved: rounds = 1 is outside 2..=16"; Parallel sessions is not saved while multi-session is off). A box shows its new value while its write is out (U-04 D36), and the two boxes at once, as U-04's dust card does. Restore defaults writes `limits.defaults` after a confirmation. | dash-qt applies options live; a form keeps half-typed numbers from reaching the engine. |
| D12 | T1 runs the real app (tauri-driver) on a one-node dashd regtest: it funds the wallet, sets the level, starts mixing from the page and sees the engine report a running mixer (S4, then S5/S7; with no masternode it cannot join a session, and the status line it reports is recorded as evidence), the engine's status carrying the level's 8 rounds, locks the vault and sees mixing stop with `vaultLocked` while the lock screen is up, unlocks for mixing only, starts again and stops from the page (`userRequested`). Real mixing rounds need masternodes: the regtest `coinjoin` suite (D1, `dwd_coinjoin_client.py`, dwcli on the same engine calls) proves those, and was rerun for this task. | The acceptance is "starts and stops from the UI"; a masternode network inside tauri-driver adds minutes and nothing the suite does not check. |
| D13 | Screen readers hear the state and, for a stop or a refusal, why (`announcement`, one hidden `role=status`); the percentage, "X of Y" and Core's status line are shown but not announced. | A running mixer's line moves up to once a second; announcing it would talk over Home's history (review r1 #5). |
| D14 | Load flakes seen at load 130–200, each passing when rerun alone: `fixture/sentinels.test.ts` past its 5 s timeout, and one vitest file reading `rust/crates/dw-app` while a cargo build ran. | Wall-clock limits under load (common brief); no code change. |

## Tests

| Tier | What | Where |
|---|---|---|
| T0 | every S-row, the D4 split, copy, amounts masked, level ↔ rounds, settings validation | `coinjoin/model.test.ts` |
| T0 | the fixture engine: start/stop rules and codes, lock stops mixing, `enabled = false` stops every wallet, settings ranges | `fixture/coinjoin.test.ts` |
| T0 | event invalidations | `api/events.test.ts` |
| T0 (UI) | every state on Home and the page, Start/Stop/confirm, level, Settings ▸ CoinJoin, a lock during mixing, failures; axe, light and dark, Chromium and WebKit | `e2e/coinjoin.spec.ts` |
| T0 (visual) | Home card per state, the page (off, mixing, paused, locked), Settings ▸ CoinJoin | `e2e/visual.spec.ts` |
| T1 | D12 | `e2e-tauri/regtest-coinjoin.ts`; regtest `coinjoin` suite |

## T1 evidence (2026-10-16, head 66f66625)

- **UI on the real engine** (`scripts/e2e-regtest-coinjoin.sh`, tauri-driver on a private X display, dashd
  24.0.0 regtest with no masternodes, a debug app with the production renderer): PASS, 10 steps.
  1. Funded with 10 DASH, the page showed Not Started.
  2. Advanced was picked, and the engine's status then reported "10.00 tDASH / 8 Rounds".
  3. Start Mixing gave Starting…, then Paused with "Can't mix while sync in progress." (with no
     masternodes the engine's masternode sync never completes). The Home card showed Paused.
  4. Lock: with the lock screen still up, the card turned to "Mixing stopped because the wallet was locked.".
  5. Unlocked for mixing only, the page said "Unlocked for mixing only: …". Start Mixing gave Starting….
  6. Stop Mixing, confirmed, gave Not Started.
- **Real mixing rounds** (`regtest/functional/dwd_coinjoin_client.py`, dwcli on the same engine calls, dashd
  24.0.0-rc.2, 4 masternodes): "Tests successful".
  - Stop released the reservations (`after_stop … reserved=0`).
  - Mixing reached 2 rounds with a fully mixed balance.
  - The stop was `UserRequested`.
