# dashwallet-desktop plan vs the mobile wallets and Dash Core: deficiencies (2026-10-31)

Plan audited: `PastaPastaPasta/dashwallet-desktop` main @ 9e3add03 — `docs/design/ROADMAP.md` (authoritative
2026-10-08), `docs/design/DASHPAY.md`, `docs/parity.md`, CLAUDE.md "Product scope". Compared against
`dashpay/dashwallet-ios` develop @ 41ab11aa2 (10-28) plus active branches, `dashpay/dash-wallet` master @ 472e86090
plus the 12.0.0 branch `feat/kotlin-sdk-phase-1` @ 2e6259194 (440 commits ahead), and `dashpay/dash` develop @
5df218409a46.

Per-platform inventories with file:line evidence: [ios.md](ios.md) (586 live stories), [android.md](android.md) (186),
[core.md](core.md) (112). Claims marked ✔ below were re-checked by hand against source; others rest on the
inventories' citations.

## Verdict

The suspicion is right. The plan was researched around 10-05..10-08 and treats Platform addresses and the shielded
pool as late extras (X1a/X1b/X2, about one table row each). Since then the mobile apps have made them first-class, and
both mobile apps have **dropped CoinJoin mixing in favour of the shielded pool**. The desktop is now the only client
that mixes, and the only one that cannot see, receive or spend shielded funds.

## 1. Platform addresses and the shielded pool

| Fact | Evidence |
|---|---|
| ✔ iOS Home always shows a **Shielded** balance row; the **Platform** row shows in Advanced mode | `HomeBalanceView.swift:233-247`, `HomeView.swift:334` |
| ✔ iOS has full shielded flows: Core→Shielded (restart-safe Max, #1205), Max to an external shielded address (#1251), recovery sheet, sync-info screen, Platform↔Shielded, history rows and filters | `UI/Payments/InternalTransfer/Shielded/*`, `ShieldedActivityHistory.swift`, `ShieldedSyncInfoScreen.swift`, `HomeViewModel.swift:40-41` |
| ✔ iOS has Platform-address sync, activity history and transfers | `PlatformAddressSyncCoordinator.swift`, `PlatformAddressHistory.swift`, `TransactionListDataItem.swift:33` |
| ✔ Android 12.0.0 seeds `USE_KOTLIN_SDK_SHIELDED` ON in **every** build including prodRelease | branch `DashPayConfig.kt:441-469` |
| Android has no user-facing Platform (DIP-17) addresses on master or the branch | android.md |
| ✔ Desktop plan: X1a read-only Platform balance is 1.0 but **cut item 3**; X1b (transfers, receive) and X2 (shielded) are 1.1, scheduled **last** (W13–W16, after the masternode track) | ROADMAP:215-217, :318-320, :382 |
| ✔ X2's rationale is stale: "Android has no shielded pool at all" | DASHPAY:610 |
| ✔ Desktop code: no shielded path beyond a warm-up example; Pay refuses shielded and Platform addresses; "move mixed coins to Shielded" returns `NotImplemented`; no Platform-address sync loop (identity, DashPay, DPNS only) | `pay/model.ts:124-127`, `coinjoin.rs:2234,2274`, `platform/runtime.rs:686-711` |
| ✔ Restoring an iOS seed on desktop today silently omits Platform and shielded funds (X1a's notice is not built) | — |

**Missing beneath X1b/X2** (no task names them): each send route (Core→Shielded incl. Max; shielded→external
shielded incl. Max drain; shielded→Platform; Platform→shielded; Platform→Core withdraw; Core→own Platform; external
Platform send); history rows, memos and filters for Platform and shielded activity; stuck-transfer recovery for
funding asset locks (types 4/5) and bulk "Finish" after restore; identity top-up from Platform or Shielded and credit
withdrawal to a Platform address; the registration funding-source picker (Shielded / Dash / Platform) and the shielded
readiness checklist; vault key scopes for DIP-17 and Orchard keys; a viewing key so shielded sync runs while locked;
shielded sync info; prover warm-up; CoinJoin→Shielded consolidation under the 100-input cap (iOS #1221);
shielded-funded invitations (Android); pay a contact from any balance (iOS PR #1117, branch); Home total definition
(iOS: per-pool rows; Android: Core + Shielded). By the plan's own DashPay sizing this is ~35–50 agent-days, not the
~23–29 budgeted.

## 2. Privacy and CoinJoin — product direction conflict

- ✔ iOS retired mixing (`b89826c80`, `e59855240`); it keeps only "move mixed coins" to Shielded or BIP44.
- ✔ Android's 12.0.0 branch removed mixing (`83ce13ca1`); mixed coins stay spendable, with "Move your mixed coins" to
  Shielded or Dash Wallet. Android master still mixes.
- The desktop has a full CoinJoin client (U-06). That fits the request to bring Dash Core's mixing in, but the plan
  must then also own the bridge to the mobile privacy model: "move mixed coins" UI is 1.2 (MP-08) and its Shielded
  destination is unbuilt.
- Mixing gaps against dash-qt (core.md): ✔ no V24 denomination promotion/demotion (`dw-coinjoin/src/lib.rs:26`);
  mixing stops on lock, auto-lock and quit and never resumes; no App Nap prevention, start-on-login or tray; the level
  sets rounds but not the target amount; no mixed-send confirm warnings (≥10 inputs, no change); no explainer; no
  Tor/proxy; coins mixed by dash-qt (BIP44 chains) probably show as unmixed (unverified).
- Restore risk: Android found key-wallet's default CoinJoin gap limit of 30 loses funds on restore; the desktop's
  1000 lookahead applies only to Core-imported wallets (android.md gap 5).

## 3. Other mobile stories missing, deferred, or decided on stale facts

| Story | Mobile | Desktop plan | Problem |
|---|---|---|---|
| Contested-name cost follows protocol version (0.15 DASH from PV14) | iOS #1199, Android #1583 | ✔ hard-coded 0.25 (`registration_flow.rs:79`); invitation tiers fixed | Bug: over-locks ~0.10 DASH |
| Identity-verify link | iOS #1141 on mainnet + testnet; Android | ✔ excluded "no contract" (DASHPAY:1108, :1131) | Exclusion is stale |
| DashConnect / App Connect (dApp login, grants, v2) | Android master #1510; iOS testnet + 12 v2 branches | post-1.0, no task (DASHPAY:1127) | Unplanned |
| Transaction metadata on Platform | Android master | post-1.0, no task (DASHPAY:1125) | Unplanned |
| Protocol-compatibility advisory banner | iOS #1232 | nothing | Missing |
| Wallet-open failure cards, reset-and-rescan | iOS #1195, #1200 | generic retry | Missing |
| Block sends while a restored wallet is still syncing | iOS #998, #1238 | nothing | Missing |
| OS notifications (contacts, payments) on macOS/Linux | both | Windows toasts only (W-02) | Unowned |
| Fiat everywhere (MP-01), localization (MP-14) | baseline on both | cut items 7 and 8 | Must not be cuttable for parity |
| Username marketplace, extra/second names, instant-name offer | iOS | X4 one line, 1.2 | Thin |
| Invitation claim Home card, invitation QR scan, tiny invites (0.003) | iOS #1156, Android | link intake only | Partial |
| Lock-screen Quick Receive / Scan to Pay; biometrics | both | B4 / none | Unowned |
| In-app network switch, log export / support | iOS | U-04b in no plan row | Unowned |
| Explore, DashSpend, Buy & Sell, Dash DEX (Maya, 20+ chain address parsers), swap rows in history | both | 1.2 or blocked on B1 | Deferred wholesale |
| Tax categories / tax CSV / ZenLedger | both | 1.2 / B1 | Deferred |
| Private-key sweep incl. BIP38; phrase repair | both | 1.2 | Deferred |
| CrowdNode | iOS removing (branch), Android still opens new accounts | "1.2, or drop" | Undecided; iOS-seeded CrowdNode balances have no exit |

## 4. Dash Core stories missing (beyond mixing)

✔ History CSV export: engine call exists (`exportHistoryCsv`) but nothing in the UI calls it, though CLAUDE.md names
CSV export as in scope. Also: Berkeley DB `wallet.dat` cannot be opened (dumpwallet round trip); no multi-recipient
send; no spend-unconfirmed-change option; hardware wallets not before M6; no per-tx notifications; no explorer link.
Operator features stay out by design. Core develop has no DashPay/Platform GUI (dash#7512 unmerged).

## 5. Plan hygiene

- `docs/parity.md` still measures the frozen Swift apps, has no rows IOS-124..140 (cited by DASHPAY F6/F15), and lags
  merged code (CoinJoin, DP5, MG).
- Research docs (`03-ios-features.md`, `dashpay/ios.md`, `dashpay/android.md`, `platform.md`) predate ~45 relevant
  mobile PRs (dated table in ios.md "stale research"); `platform.md:229` says DashConnect is unshipped; `android.md`
  says Android is light-only, single-wordlist and 0.25 DASH contested.
- Ordering: the program moved into 1.1 masternode work (MG-04b) before 1.0 hardening, X1a or any shielded work, so
  owner-only masternode features are landing ahead of balances every iOS user already holds.

## Recommended plan changes

1. Make X1a non-cuttable and add a **read-only shielded balance + Platform/shielded history** to 1.0, so an iOS or
   Android seed shows all its funds. Update the DASHPAY:610 rationale (Android now ships shielded; build cost is
   measured).
2. Split X1b and X2 into per-route engine + UI tasks, each with a design doc, plus explicit tasks for recovery, vault
   key scopes, locked-vault shielded sync, funding-source picker, top-up/withdraw routes, Max drains and CoinJoin
   consolidation. Schedule them ahead of the remaining MG work.
3. Decide the privacy story: keep desktop mixing (Core parity), and pull "move mixed coins" (to Shielded and Core)
   into the same release as X2. Add V24 promotion/demotion, mixing persistence/resume, App Nap / start-on-login,
   mixed-send warnings, dash-qt mixed-coin recognition (with a regtest test), and a CoinJoin restore gap-limit test.
4. Fix stale decisions now: contested cost by protocol version; un-exclude the identity-verify link; add tasks for
   DashConnect v1/v2 and Platform tx metadata (even if post-1.0).
5. Make fiat (MP-01) and localization (MP-14) non-cuttable; add owners for notifications on macOS/Linux, protocol
   advisory, wallet-open failure recovery, restore send guard, network switch, log export, history CSV UI.
6. Re-baseline `parity.md` on the Tauri app, add IOS-124+ and Android 12.0.0 rows, and refresh the research docs
   against current develop/master and the 12.0.0 branch.
