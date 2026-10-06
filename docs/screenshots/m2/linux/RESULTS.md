# M2 SwiftCrossUI app on Linux (GtkBackend): integration run, 2026-10-06

Harness: `DWD_CROSSUI_SUITE=m2 DWD_MIN_FREE_GB=10 scripts/crossui-linux-demo.sh docs/screenshots/m2/linux`
→ image `dwd-linux-crossui` (swift:6.3.3-noble + Rust 1.98.1 + GTK 4.14 + Xvfb + D-Bus + AT-SPI) →
`ci/linux/crossui/container-demo.sh` (`m2` suite) → `ci/linux/crossui/atspi_demo.py`. Host: Apple-silicon
Mac, so the container is **linux/aarch64**; x86_64 was not run.

Sources: `main` at **6fb2e7e** (m2/mac-ui and m2/cross-ui merged, plus the CrossUI Create Unsigned wiring
c1513c4). This is the one Linux GUI run of the M2 integration. It replaces the m2/cross-ui branch run at
6c91999, whose failures A (harness depth limit) and C (check text) no longer occur.

## What was built

| Step | Result |
|---|---|
| `scripts/build-core.sh --no-bindings` (dw-ffi, aarch64, dev) | OK, 5 s (the Linux variant of the same Rust sources was already in the `/target` volume from `scripts/linux-docker-test.sh`) |
| `swift build --product dash-wallet` | OK, 25 s (incremental) |
| 8 M1 demo sessions + the M1 live session (regression), 5 M2 demo sessions, 1 M2 live session | Every demo session stayed alive until killed (RSS 285–346 MB, debug, llvmpipe). Both live sessions exited after `WM_DELETE_WINDOW` (the M1 one with status 0) and printed `engine shut down`. |

## Results

| Suite | Hard checks | Failed | Failures |
|---|---|---|---|
| M2 (`atspi-checks.json`) | 74 | 1 | security: the Auto Lock drop-down is named after its selection (B) |
| M1 regression (`m1-regression-checks.json`) | 95 (100 with soft) | 3 | send: "Confirmation time target" combo box; tools: unit selector "Unit to show amounts in"; tools: address-list picker "Address list" (all B) |

Every failure is **B: drop-downs are named after their selected option** (`combo box "Never"`,
`"tDASH"`, `"Sending addresses"`). This is the M1 FAIL 1 / M1 L8 item. It is not fixed: the package still
pins upstream `stackotter/swift-cross-ui` 0.10.0 (Package.swift `TODO(fork)`), and the name needs fork patch
P1 (accessibility modifiers on `GtkDropDown`).

The M1 overlay harness item (L8) passes: the offline demo showed the sync overlay by itself (rows "Number of
blocks left", "Last block time", "Progress increase per hour"), Hide returned to the wallet, and the status
row offered "Sync details".

## M2 sessions

| Session | Driven through AT-SPI | Result | Files |
|---|---|---|---|
| `--demo`, menus | — | PASS 9/9. The window is a `frame` named "Dash Wallet - Demo wallet - [testnet]" (QT-011) with a `menu bar` File, Settings, Window, Help (`ShellModel.menus` through `.commands`). | `m2-1-menus-overview.png` |
| `--demo --page options` | Wallet tab → "Enable coin control features" on → OK ("Options saved."); Network tab (proxy fields say they need an engine update); Display tab ("Third-party transaction URLs" field); Send in the sidebar showed "Coin Control Features"; pressed "Inputs…": Coin Selection, "automatically selected"; toggled the coin switch "Select 4.87999774 at yesAWt…"; the summary showed Quantity. | PASS 20/20 | `m2-2-options-{main,wallet,network,display,coin-selection,coin-selected}.png` |
| `--demo --page tools-console` | Welcome text and anti-scam warning; typed `getblockcount`, pressed Run: echoed and answered `1234567`. Information tab ("Client version"; full-node rows say "Requires full-node data source"), Peers tab ("Change Peers"), Repair tab ("Rescan Chain (full)", "Reset chain data and resync"). | PASS 14/14 | `m2-3-tools-{console,information,peers,repair}.png` |
| `--demo --page psbt` | "Load PSBT from clipboard…": "Unable to decode PSBT from clipboard (invalid base64)" (no `xclip` in the container); typed a base64 PSBT, Load: "This feature is not available yet." (the demo has no PSBT parser and shows no invented analysis). | PASS 8/8 | `m2-4-psbt-{empty,load}.png` |
| `--demo`, pages | Wallets ("Import File…", "Account xpub"), Security (no biometric unlock on this system), About ("Export Logs"), Command-line options (dash-qt options listed), Transactions → first row selected: details with "Abandon transaction", "Resend transaction" and "Net amount". | 16/17: FAIL B on the Auto Lock picker | `m2-5-pages-{wallets,security,about,command-line,transaction-details}.png` |
| **live** `-choosedatadir --network regtest` (no node) | dash-qt's Intro page: "Welcome to Dash Wallet.", the default directory status line; OK opened the engine on it and onboarding appeared; then the window was closed. | PASS 6/6; data under the chosen directory; `engine shut down` | `m2-6-live-{chooser,opened}.png` |

Matching AT-SPI dumps: `atspi-<step>.txt`. Full log: `run.log`. App stderr: `app-*.log`. M1 regression
screenshots were not kept (they stay in the container).

## Seen in the screenshots, not caught by a check

- **Coin Selection summary layout.** After a pick (`m2-2-options-coin-selected.png`) the summary grid
  gives the Fee and Change values a column one character wide, so "≈ 0.00000192 tDASH" wraps one character
  per line. The values are right; the layout is not. Not fixed (no second Linux run in this pass).
- The coin switches show their accessible name ("Select 4.87999774 at …") as visible text next to the
  amount and address, which repeats the row.

## Not verified on Linux

- Menu items were not activated (GTK 4 popover menus); enablement, the Open Wallet submenu and the
  Discreet-mode check item were not inspected. SwiftCrossUI 0.10 menu items have no shortcuts or tooltips.
- The shutdown page (File ▸ Exit), the splash, the unreadable-settings question (QT-007),
  `-resetguisettings`.
- Create Unsigned (wired in c1513c4; the demo answers `not_implemented`), imports, backup/restore,
  export for Dash Core, rename/remove, forgot passphrase, wipe, Abandon/Resend presses.
- Coin Selection tree mode.
- x86_64, Wayland, Orca, Windows.
