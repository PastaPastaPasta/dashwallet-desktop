# M1 SwiftCrossUI app on Linux (GtkBackend): demo and live run, 2026-10-05 (second pass)

Harness: `scripts/crossui-linux-demo.sh` → image `dwd-linux-crossui` (`ci/linux/Dockerfile.crossui`:
swift:6.3.3-noble + Rust 1.98.1 + protoc 29.3 + GTK 4.14 + Xvfb + D-Bus + AT-SPI) →
`ci/linux/crossui/container-demo.sh` → `ci/linux/crossui/atspi_demo.py` (reuses the G2 probe's
`atspi_smoke.py` helpers) and `close_window.py`. Host: Apple-silicon Mac, OrbStack, so the container is
**linux/aarch64**. x86_64 was not run. Run with `DWD_MIN_FREE_GB=12` (the host had 17 GB free).

Sources: `main` at cfe38ba (shared `WalletDemo`, CrossUI sync overlay / peers page / unit selector /
address-book QR, the GApplication shutdown hook). The repo was copied into the container as is.

This pass replaces the first pass of the same day (branch `m1/cross-ui` at f8a0494). Its three open
items are answered below: switch and picker names, the send-flow check and the engine shutdown on
window close.

## What was built

| Step | Result |
|---|---|
| `scripts/build-core.sh --no-bindings` (dw-ffi, dev, aarch64-unknown-linux-gnu) | OK, 113 s (cargo cache volumes reused) |
| `swift build --product dash-wallet` (GtkBackend through DefaultBackend) | OK, 22 s. This compiled the Linux-only code for the first time: `QuitHook.connectApplicationShutdown` (GApplication `shutdown` signal) |
| 9 demo sessions and 1 live session under Xvfb 1280x860 | All demo sessions stayed alive until they were killed (RSS 278–335 MB, debug build, llvmpipe). The live session exited with status 0 when its window was closed, after shutting the engine down. |

## Sessions and screenshots (`xwd -id <window>`)

| Session | What was driven through AT-SPI | Files |
|---|---|---|
| `--demo` | Overview; Send picked in the sidebar (`Selection.selectChild`) | `1-overview.png`, `2-send.png` |
| `--demo --page transactions` | render only | `3-transactions.png` |
| `--demo --page receive` | render only (QR from the engine's `qr_matrix`) | `4-receive.png` |
| `--demo onboarding` | render only | `5-onboarding.png` |
| `--demo onboarding`, **create flow** | Pressed "Create a new wallet", read the 12 numbered words, pressed "I wrote it down", answered the 4 "Select word #N" prompts, typed the passphrase into "New passphrase" and "Repeat new passphrase", pressed "Encrypt wallet". The new wallet's Overview appeared. | `6-onboarding-phrase.png`, `6-onboarding-passphrase.png`, `6-onboarding-done.png` |
| `--demo --page send`, **send flow** | Typed `yPgfYhP6PwdZd8xn1TKDps27nL6kLpvh98` into "Pay To" and 0.25 into "Amount", pressed "Send". The funded demo is unencrypted, so no passphrase was asked. The review panel ("Confirm send coins") listed the address. After the 3 s countdown, pressed "Send". Transactions showed the new row **"Sent to, yPgfYhP6PwdZd8xn1TKDps27nL6kLpvh98, -0.25000226 tDASH, 2026-10-06 03:09"** (0.25 plus a 226-duff fee, spent from the demo's coins). | `7-send-filled.png`, **`7-send-review.png`**, `7-send-done.png` |
| `--demo`, **tools flow** (new) | Pressed the status row's "8 peers" button: the Peers page with "Change Peers" and 8 rows, each named after its peer ("203.0.113.11:19999, User Agent: /Dash Core:23.1.7/  Height: 1234567  Ping: 27 ms  Outbound"). Pressed "Close", then "Address Book", then the first "Show QR": the QR code and `dash:yXPK1R5baHAcD2pw9HRinRs9ck7A2uB1Po?label=Alice` appeared with "Hide QR". | `9-tools-peers.png`, `9-tools-qr.png` |
| `--demo offline`, **overlay flow** (new) | The sync overlay showed by itself (the offline demo has no peers and its tip is three days old): status, 1728 blocks left, last block time, 42.00 %, rates "Unknown" (less than a minute of samples). Pressed "Hide": the Overview returned, and the status row offered "Sync details" and "0 peers". | `10-overlay-shown.png`, `10-overlay-hidden.png` |
| **live**, real engine | `dash-wallet --network regtest --dapi http://127.0.0.1:1 --connect 127.0.0.1:1` with `XDG_DATA_HOME=/tmp/xdg-live` and no node. Same create flow on the real engine and vault (fresh random phrase, masked in `atspi-8-live-phrase.txt`, no screenshot). After "Encrypt wallet" the **sync overlay** covered the new wallet ("Connecting to peers…", 0 peers): see FAIL 2. Then `close_window.py` closed the window. | `8-live-passphrase.png`, `8-live-done.png` |

Matching AT-SPI dumps: `atspi-<step>.txt`. Check results: `atspi-checks.json`. Full log: `run.log`.
App stderr: `app-*.log`.

## Engine shutdown on window close: fixed

`app-live.log` ends with

```
dash-wallet: shutting the engine down
dash-wallet: engine shut down
```

and the harness reports `PASS engine shut down before exit` (exit status 0 after WM_DELETE_WINDOW).

Why the first pass failed: the app ran the shutdown from the GTK window's `destroy` signal. In GTK 4
that signal is emitted only when the window is disposed. SwiftCrossUI 0.10 keeps every window it
creates referenced (`GtkBackend.windows`, plus the wrapper's own `g_object_ref`), so a closed window is
never disposed while the app runs, and the handler never ran. The app now runs the shutdown from the
GApplication `shutdown` signal, which `g_application_run` emits after the main loop ends. The
`destroy` hook is gone.

## AT-SPI checks

94 hard checks, **4 hard failures**; 5 soft checks. The harness exit status was 1.

PASS, among others:
- The window is a `frame` named "Dash Wallet" in all 10 sessions.
- **Switches are named now**: "Discreet mode" on Overview and "Subtract fee from amount" on Send are
  `check box` nodes with those names (first pass: unnamed). Naming after every update fixed them.
- **Send-flow check**: the new transaction row is found by address and amount (first pass: the check
  looked for a toast that the app no longer shows).
- Every list row has its own name: sidebar, recent and all transaction rows, and the new peer rows.
- Every text and password entry is named after its caption ("Pay To", "Amount", "New passphrase"…).
- The tools and overlay flows above, step by step.

FAIL:
1. **Drop-downs are still named after their selected option** (3 checks): "Confirmation time target"
   shows as `combo box "15 minutes"`, the status-row unit selector as `combo box "tDASH"`, the address
   list picker as `combo box "Sending addresses"`.
   - Naming the `Picker` after every update did not help either. GTK computes a combo box's name from
     its LABEL property (`gtkatcontext.c`), and `GtkDropDown` never sets one itself, so the label did
     not reach the `GtkDropDown`. SwiftCrossUI 0.10's GtkBackend says Picker inspection has been broken
     since its PickerStyle refactor; the widget the Picker's own `inspect` gets is presumably not the
     drop-down.
   - **Since this run**, `DashPicker` sets the name from the stack around the picker instead, which
     contains the `GtkDropDown`. **Not re-run.**
   - The generic check "every entry, switch and picker has an accessible name" passes, because the
     selected option is a non-empty name.
2. **Live onboarding: "the new wallet's Overview is shown"**. The app was right: without a node the
   wallet is not synced, so the new sync overlay (QT-027) covered the Overview, as dash-qt's does.
   **Since this run** the harness hides the overlay when it is shown, then expects the Overview.
   **Not re-run.**

Control naming summary (soft checks, `atspi-checks.json`):

| Role | Named | Notes |
|---|---|---|
| push button | all | Every DashButton has a text title. |
| text / password text | all | Caption via `accessibleName` (GTK_ACCESSIBLE_PROPERTY_LABEL on the GtkEntry). |
| check box (switch) | all | Fixed in this pass. |
| list item | all | `accessibleRowNames` labels each GtkListBoxRow. |
| combo box | all, but with the selected value | See FAIL 1. |

## Rendering issues seen in the screenshots

- The status row is crowded: "0 peers" wraps to three lines and the demo notice is ellipsized
  (`10-overlay-shown.png`).
- GTK ellipsizes some captions and buttons that AppKit shows in full: "Cancel" ("Ca…") in the review
  panel and "Custom fee (duffs per …".
- `Gtk-WARNING … GtkLabel reported min width … natural size must be >= min size`, from the 30x30
  direction tile in `TransactionView`. Nothing visible breaks.
- `libEGL warning: DRI3 error`: Xvfb has no GPU, so llvmpipe is used.

## Not verified

- The two post-run changes (drop-down names from the surrounding stack; the live flow hiding the
  overlay). Only one Linux run was allowed for this pass.
- **App name (gap A7)**: `g_set_application_name("Dash Wallet")` sets the AT-SPI application's
  description; its name is still the executable name `dash-wallet`. Not changed.
- Live mode against a regtest node with blocks: no dashd ran, so sync, balances, receive and a live
  send were not exercised in the GUI.
- CSV export through the GTK save dialog (QT-093), receive-request creation, restore flow,
  lock/unlock, settings changes, Change Peers on the live engine.
- x86_64, Wayland, Orca, keyboard navigation, Windows (WinUIBackend not built).

## macOS (AppKitBackend)

Not re-run in this pass. `../crossui-macos/*.png` are from the first m1/cross-ui run. `swift build
--product dash-wallet` passes on macOS at this commit.
