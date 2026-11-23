# Dash Core (dash-qt) wallet stories vs the desktop plan (2026-10-31)

Core = `dashpay/dash` develop @ 5df218409a46 (read via `refs/remotes/upstream-ro/develop` in ~/workspace/dash).
Desktop = `PastaPastaPasta/dashwallet-desktop` main @ 9e3add03. Desktop status comes from the U-0x design docs and
code, not `parity.md` (which still measures the frozen Swift apps; U-03 D20).

Ordinary? = Y (ordinary holder), Power (in scope as "a Dash Core feature a desktop user would miss", CLAUDE.md:25),
Owner (masternode owner, milestone MG), Operator (out of scope by design).

## Ranked gaps

| # | Gap | Evidence |
|---|---|---|
| 1 | History CSV export has no UI, though CLAUDE.md:25 names CSV export as in scope | engine `exportHistoryCsv` generated (`apps/desktop/src/api/generated/commands.ts:914`), no non-generated caller (verified by grep); Core `qt/transactionview.cpp:379-397` |
| 2 | Coins mixed in dash-qt are probably not recognised as mixed | Core keeps CoinJoin outputs on the BIP44 chains (no `9'/coin'/4'` derivation in `src/wallet/*.cpp`; only a legacy match in `rpc/backup.cpp:2063`); desktop counts only `m/9'/coin'/4'` coins (`dw-coinjoin/src/lib.rs:5`, `dw-engine/src/coinjoin.rs:443,913`). **Unverified by test** |
| 3 | Legacy Berkeley DB `wallet.dat` cannot be opened; user must `dumpwallet` in Core first | `apps/desktop/src/screens/settings/files.ts:26-27`; parity QT-115 not started |
| 4 | Mixing does not persist or resume; stops on quit, lock and auto-lock; no App Nap prevention; no start-on-login/tray | U-06 D5; U-04 D7; Core `qt/bitcoingui.cpp:1610-1615`, `-coinjoinautostart` |
| 5 | No V24 denomination promotion/demotion | `dw-coinjoin/src/lib.rs:26-27` "Not built: post-V24 promotion/demotion" (verified); Core `doc/release-notes-7052.md`, `coinjoin/client.cpp:991-1065` |
| 6 | Mixed-send path thin: no dedicated page, no "no change / higher fee" note, no ≥10-input privacy warning | Core `qt/sendcoinsdialog.cpp:444-469`; desktop uses Pay ▸ Advanced ▸ "Fully mixed funds only" (`CoinControl.tsx:23`) |
| 7 | Mixing level sets rounds only, not the target amount (Android mixed the whole balance) | U-06 D2; Android `CoinJoinService.kt:319` (master) |
| 8 | "Move mixed coins" has no UI (engine done), and its Shielded destination returns `NotImplemented` | MP-08 1.2 (ROADMAP:256); `coinjoin.rs:2234,2274` (verified) |
| 9 | No SOCKS5/Tor proxy | U-04 D7; Core `qt/forms/optionsdialog.ui:749-906` |
| 10 | English only (Core ships 21 locales); MP-14 is infrastructure and cut item 8 | ROADMAP:262, :387 |
| 11 | No per-transaction desktop notifications | U-04 D7 → W-02 (Windows only) |
| 12 | No multi-recipient send (deliberate, U-03 D1) | Core `qt/forms/sendcoinsdialog.ui:1052` |
| 13 | No spend-unconfirmed-change / keep-custom-change options | U-04 D5; QT-065 not started |
| 14 | Hardware wallet / external signer not before M6 | parity QT-080 |
| 15 | My masternodes, registration and shared-masternode UIs not built (engine done through MG-04b 2d) | `screens/masternodes/MasternodesPage.tsx:51-54` |
| 16 | No CoinJoin explainer or first-use hint | UX-SPEC:817 planned it; U-06 D10 |
| 17 | No "show in explorer" opener; no custom date range filter | U-03 D12, D15 |

## CoinJoin depth

| Capability | dash-qt | Android master | Android 12.0.0 branch | iOS develop | Desktop |
|---|---|---|---|---|---|
| Mixing | Full | Full | **Removed** (`83ce13ca1`, "feat!: remove CoinJoin mixing") | **Removed** (`b89826c80` "retire the legacy mixing UI", `e59855240` sweep-only) | Full client (dw-coinjoin) |
| Rounds | 2-16, default 4 | Levels 4 / 8 | — | — | Levels + 2-16 |
| Target amount | Setting (1000) | Whole balance | — | — | Setting (1000) |
| Sessions / multi-session | Setting | Fixed | — | — | Setting |
| Denoms goal / cap | Setting | Fixed | — | — | Setting |
| Resume after restart | `-coinjoinautostart` | Yes | — | — | No |
| Mixed-only send | Separate page | — | — | — | Pay ▸ Advanced source |
| V24 promotion / demotion | Yes | ? | — | — | No |
| Move mixed coins | — | — | To Shielded or Dash Wallet | To Shielded or BIP44 (with consolidation, #1221) | Engine only; Shielded `NotImplemented` |

## Out of scope by design (operator)

Update Service / Unban, Revoke, operator BLS key generation, peer ban/unban, traffic graph, mempool/quorum info, DKG
health, conf-file tools (CLAUDE.md "Leave to the Rust node and its CLI"). Core develop has **no** DashPay/Platform GUI
(dash#7512 unmerged); only Platform key derivation (`src/wallet/platformkeys.cpp`) and RPC-side DIP-18 sends
(`doc/release-notes-7294.md`).

## Research doc 02 staleness

Baseline `3789ec0c719e` (2026-10-04). Post-baseline wallet/Qt changes: #7260 (coin-control shift-click range), #7794
(denomination-gap floor), #7829/#7838 (passphrase and unlock deadline), #7782, #7990 (3-byte input counts), #7820. The
doc treats promotion/demotion only as protocol fields (02:1065) and does not cover RPC-side DIP-18 / Asset Lock v2.
