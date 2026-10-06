# M2 SwiftCrossUI app on Linux (GtkBackend): one demo + live run, 2026-10-06

Harness: `DWD_CROSSUI_SUITE=m2 scripts/crossui-linux-demo.sh docs/screenshots/m2/linux` → image
`dwd-linux-crossui` (swift:6.3.3-noble + Rust 1.98.1 + GTK 4.14 + Xvfb + D-Bus + AT-SPI) →
`ci/linux/crossui/container-demo.sh` (new `m2` suite) → `ci/linux/crossui/atspi_demo.py` (new `m2-*`
flows). Host: Apple-silicon Mac, so the container is **linux/aarch64**; x86_64 was not run.

Sources: branch `m2/cross-ui` at **6c91999**. The run started with `DWD_MIN_FREE_GB=8`: the host had
11 GB free, but the container's `/target` volume reported 10 GB and the default guard is 15 GB. An
earlier start the same hour stopped at that guard (9 GB on `/target`) before anything was built or
run.

The commits after the run (fd9b40b) fix what this run found. They are **not re-run on Linux**; see
"Fixed after the run".

## What was built

| Step | Result |
|---|---|
| `scripts/build-core.sh --no-bindings` (dw-ffi with the merged M2 crates, aarch64) | OK, 53 s (cargo volumes reused; `/target` 4.9 GB) |
| `swift build --product dash-wallet` | OK, 53 s. First Linux compile of the M2 CrossUI screens, `ShellMenus` (GtkBackend application menus), `AppOSServices` (`CommandLineClipboard`, dw-desktop services) and the M2 composition |
| 8 M1 demo sessions + the M1 live session (regression), 5 M2 demo sessions, 1 M2 live session | Every demo session stayed alive until killed (RSS 285–345 MB, debug, llvmpipe). Both live sessions exited after `WM_DELETE_WINDOW` (the M1 one with status 0; the M2 one's status is not recorded) and printed `engine shut down`. |

## M2 sessions

| Session | Driven through AT-SPI | Result | Files |
|---|---|---|---|
| `--demo`, menus | — | PASS 7/7. The window is a `frame` named **"Dash Wallet - Demo wallet - [testnet]"** (QT-011, from `ShellModel.windowTitle`). It has a `menu bar` with **File, Settings, Window, Help** (`ShellModel.menus` through SwiftCrossUI `.commands`). | `m2-1-menus-overview.png` |
| `--demo --page options` | Pressed the Wallet tab, flipped "Enable coin control features" (switch `Action`), pressed OK ("Options saved."), opened the Network tab (the proxy fields say "Proxy support requires an engine update and is not available yet.") and the Display tab ("Third-party transaction URLs" field). Picked Send in the sidebar: "Coin Control Features" appeared on the Send page. | 13 PASS, then **FAIL: press "Inputs…"**. The button is there (the panel was found), but deeper than the harness walked (see FAIL A). The coin selection page was therefore not reached. | `m2-2-options-main.png`, `m2-2-options-wallet.png`, `m2-2-options-network.png`, `m2-2-options-display.png` |
| `--demo --page tools-console` | The welcome text and the anti-scam warning were found; typed `getblockcount` into the field named "Console command". | **FAIL: press "Run"** (FAIL A). No reply and no screenshot: the flow stopped there, so the Information, Peers and Repair tabs were not visited. | — |
| `--demo --page psbt` | Pressed "Load PSBT from clipboard…": **"Unable to decode PSBT from clipboard (invalid base64)"** (the container has no `xclip`, and the page says so). Typed a base64 PSBT into "PSBT (base64)" and pressed Load: **"This feature is not available yet."** The demo has no PSBT parser, and the page does not make up an analysis. | PASS 6/6 | `m2-4-psbt-empty.png`, `m2-4-psbt-load.png` |
| `--demo`, pages | Pressed Wallets, Security, About, then About's "Command-line options"; picked Transactions in the sidebar and selected the first row (`Selection.selectChild`). | 11 PASS, 5 FAIL. Security says **"This computer has no biometric unlock that Dash Wallet supports; unlock with your passphrase."** (Linux has no biometric store). The transaction details list "Net amount". FAILs: "Import File…" / "Account xpub" controls, the "Auto Lock" picker name, "Export Logs", `-choosedatadir`, the Abandon/Resend buttons; see FAIL A, B and C. Each control is visible in its screenshot. | `m2-5-pages-wallets.png`, `m2-5-pages-security.png`, `m2-5-pages-about.png`, `m2-5-pages-command-line.png`, `m2-5-pages-transaction-details.png` |
| **live** `-choosedatadir --network regtest` (no node) | dash-qt's Intro page (QT-004): "Welcome to Dash Wallet.", the default `/tmp/xdg-chooser/dashwallet`, **"A new data directory will be created."**, "9 GB of space available". Pressed OK: the engine opened the chosen directory and onboarding appeared. Then the window was closed. | PASS 6/6. `PASS regtest data under the chosen (default) directory`, `PASS engine shut down`. | `m2-6-live-chooser.png`, `m2-6-live-opened.png` |

Matching AT-SPI dumps: `atspi-<step>.txt`. M2 checks: `atspi-checks.json` (61 hard, 7 failed). M1
regression checks: `m1-regression-checks.json`. Full log: `run.log`. App stderr: `app-*.log`. M1
regression screenshots were not kept.

## M1 regression (same binary, M1 flows)

91 hard checks, 6 failed. The onboarding, overlay and live flows passed in full, including the live
create flow on the real engine and the engine shutdown on window close.

| Failed check | Cause |
|---|---|
| send: combo box named "Confirmation time target"; tools: unit selector named "Unit to show amounts in" | FAIL B (already failing in the M1 run) |
| 4-receive: marker "Request payment"; send flow: the review panel's second "Send"; tools: "Change Peers"; tools: "Show QR" | FAIL A (inferred: those trees were not kept, but every M2 dump of a comparable page reached the walk's depth limit) |

## Failures

**A. Harness depth limit (fixed in the harness after the run).** `atspi_smoke.walk` stopped at depth
80. Each SwiftCrossUI modifier is its own `GtkFixed` (ADR 0002 gap A6), and the M2 pages are a few
levels deeper than M1's: the page sat in an extra stack under the shell banners. So buttons inside
cards and rows lay below depth 80. Evidence: the dumps that missed a control have nodes exactly at
depth 80 whose children were not walked (wallets 18, options-wallet 15, transaction details 124,
security 6, about 1). The screenshots show the controls. Fix: the walk now goes to depth 200, and
the banners moved above the window content, so pages are no longer wrapped in another stack.

**B. Drop-downs are named after their selected option** (`combo box "Never"`, `"tDASH"`,
`"(Default)"`). This is the M1 FAIL 1. The "name from the stack around the picker" change made after
that run does not reach the `GtkDropDown` either. It needs fork patch P1 (accessibility modifiers);
the generic "every picker has a name" check passes only because the selected option is a non-empty
name.

**C. Check text.** The demo's stand-in parser lists `--choosedatadir`; the check expected
`-choosedatadir` (the real `LaunchArgumentsParser` spelling). The page lists all 11 options (see the
screenshot). The check now accepts both.

## Fixed after the run (not re-run on Linux)

- **Truncated button titles.** SwiftCrossUI truncates `Text` to the size it is offered. Rows of
  buttons on a page showed "OK" as "…", "Wallet" as "Wa…", "Documentation" as "Documentat…" and
  "Import File…" as "Import Fil…". `DashButton` now applies `fixedSize()`.
- **"Wallets found on this device" / Delete All on every visit to Wallets.** IOS-009 is a first-run
  question. On the Wallets page the open network always has data, so the page no longer asks.
- The depth limit and the check text, as above.

## Not verified

- The coin selection page (list and tree mode, selecting a coin, the summary), the console reply,
  and the Information, Peers and Repair tabs on Linux: the flows stopped at FAIL A before reaching
  them. They compile, and they launch on macOS (AppKitBackend; each page stayed up for 6 s under
  `--page`).
- Menu items were not activated through AT-SPI (GTK 4 popover menus). Menu enablement, the Open
  Wallet submenu and the Discreet-mode check item were not inspected. SwiftCrossUI 0.10 menu items
  have no keyboard shortcuts and no tooltips.
- The shutdown page (File ▸ Exit), the splash, and the unreadable-settings question (QT-007).
- The `fixedSize()` button change and the moved banners: macOS build only.
- x86_64, Wayland, Orca.
