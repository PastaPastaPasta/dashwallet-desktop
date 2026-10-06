# M1 SwiftCrossUI app on Linux (GtkBackend): demo and live run, 2026-10-05

Harness: `scripts/crossui-linux-demo.sh` → image `dwd-linux-crossui` (`ci/linux/Dockerfile.crossui`:
swift:6.3.3-noble + Rust 1.98.1 + protoc 29.3 + GTK 4.14 + Xvfb + D-Bus + AT-SPI) →
`ci/linux/crossui/container-demo.sh` → `ci/linux/crossui/atspi_demo.py` (reuses the G2 probe's
`atspi_smoke.py` helpers) and `close_window.py`. Host: Apple-silicon Mac, OrbStack, so the container is
**linux/aarch64**. x86_64 was not run.

Sources: branch `m1/cross-ui` at f8a0494, rebased on main b3814f2 (all M1 engine, runtime and view-model
work merged). Nothing was copied in by hand.

## What was built

| Step | Result |
|---|---|
| `scripts/build-core.sh --no-bindings` (dw-ffi, dev, aarch64-unknown-linux-gnu) | OK, 127 s (cargo cache volumes reused) |
| `swift build --product dash-wallet` (GtkBackend through DefaultBackend) | OK, 26 s (SwiftPM volume reused); ELF 64-bit aarch64 PIE |
| 7 demo sessions and 1 live session under Xvfb 1280x860 | All demo sessions stayed alive until they were killed (RSS 274–335 MB, debug build, llvmpipe). The live session exited with status 0 when its window was closed. |

## Sessions and screenshots (`xwd -id <window>`)

| Session | What was driven through AT-SPI | Files |
|---|---|---|
| `--demo` | Overview; Send picked in the sidebar (`Selection.selectChild`) | `1-overview.png`, `2-send.png` |
| `--demo --page transactions` | render only | `3-transactions.png` |
| `--demo --page receive` | render only (QR from the engine's `qr_matrix`) | `4-receive.png` |
| `--demo onboarding` | render only | `5-onboarding.png` |
| `--demo onboarding`, **create flow** | Pressed "Create a new wallet". Read the 12 numbered words from the phrase page. Pressed "I wrote it down". Answered the 4 "Select word #N" prompts by pressing the matching word buttons. Typed the passphrase twice into the fields named "New passphrase" and "Repeat new passphrase" (EditableText). Pressed "Encrypt wallet". The new wallet's Overview appeared. | `6-onboarding-phrase.png`, `6-onboarding-passphrase.png`, `6-onboarding-done.png` |
| `--demo --page send`, **send flow** | Typed a testnet address into the field named "Pay To" and 0.25 into "Amount". Pressed "Send". Typed the demo passphrase into "Passphrase" and pressed "Authorize". The review panel ("Confirm send coins") listed 0.25 tDASH to the address, the fee 0.00000374 tDASH and a 0.374 kB size. After the 3 s countdown, pressed "Send". The app opened Transactions with the new row "Sent to, yPgf…vh98, -0.25000374 tDASH". | `7-send-filled.png`, `7-send-authorize.png`, **`7-send-review.png`**, `7-send-done.png` |
| **live**, real engine | `dash-wallet --network regtest --dapi http://127.0.0.1:1 --connect 127.0.0.1:1` with `XDG_DATA_HOME=/tmp/xdg-live` and no node. Same create flow on the real engine and vault: a fresh random phrase (read from the page, masked in `atspi-8-live-phrase.txt` and in the logs, no screenshot), an encrypted vault, a new wallet. The Overview shows "Unknown" balances, "Connecting to peers…", Regtest, 0 peers. | `8-live-passphrase.png`, `8-live-done.png` |

Matching AT-SPI dumps: `atspi-<step>.txt`. Check results: `atspi-checks.json`. Full log: `run.log`.
App stderr: `app-*.log`.

Live data root after the run (`$XDG_DATA_HOME/dashwallet`): `global.json` and `regtest/`. So the
XDG data directory and `WalletRuntimeServices.live` → `AppEnvironment(runtime:)` work on Linux.

## AT-SPI checks

74 checks, 69 hard, **3 hard failures**. The harness exit status was 2 (it also counts the live
shutdown check below).

PASS, among others:
- The app is on the bus. Its window is a `frame` named "Dash Wallet" in all 8 sessions.
- Every list row has its own accessible name (ADR 0002 gap A4 closed for our lists):
  - sidebar rows: "Overview", "Send", …;
  - recent-transaction and transaction rows: "Sent to, Alice, -0.25002260 tDASH, 2026-09-20 17:51".
  - 40 of 40 rows on Transactions.
- Every text and password entry has the caption as its name (gap A3 closed): "Pay To", "Amount",
  "Label", "New passphrase", "Passphrase"…. The harness found each field by that name and typed into it.
- Every flow step listed above.

FAIL:
1. **Switches are unnamed** (`1-overview` and `2-send`, "every entry, switch and picker has an
   accessible name"): the "Discreet mode" and "Subtract fee from amount" switches are still
   `check box ""`.
   - That run set the name only when the view was created.
   - Since the run, `DashToggle` also sets it after every update (`accessibleName(_:afterUpdates:)`).
     **Not re-run.**
2. **The send flow's last check** looked for a "Transaction sent:" toast. The app had instead opened
   the new transaction on Transactions (see `7-send-done.png` and the dump). This was a harness error,
   not an app error: the send succeeded. The check now looks for the new row. **Not re-run.**
3. **Live shutdown on window close** ("FAIL no engine shutdown message"):
   - `close_window.py` sent WM_DELETE_WINDOW, and the app exited with status 0.
   - The app did not print "engine shut down", so the window `destroy` hook did not run the engine's
     orderly shutdown before exit.
   - Since the run, the app also connects to the GtkApplication's `shutdown` signal. **Not re-run, and
     this Linux-only code has not been compiled:** macOS does not build it, and only one Docker run
     was allowed.

Not caught by a hard check in this run:
- **Drop-downs** kept their selected option as their name ("15 minutes"), not their caption. They now
  get the caption after every update as well, and the send check expects "Confirmation time target".
  **Not re-run.**
- **App name (gap A7):** `g_set_application_name("Dash Wallet")` set the AT-SPI application's
  *description* to "Dash Wallet". Its name is still the executable name `dash-wallet`.

Control naming summary (soft checks, `atspi-checks.json`):

| Role | Named | Notes |
|---|---|---|
| push button | all | Every DashButton has a text title. |
| text / password text | all | Caption via `accessibleName` (GTK_ACCESSIBLE_PROPERTY_LABEL on the GtkEntry). |
| list item | all | `accessibleRowNames` labels each GtkListBoxRow. |
| combo box | all, but with the selected value | See above. |
| check box (switch) | none | See FAIL 1. |

## Rendering issues seen in the screenshots

- GTK ellipsizes some captions and buttons that AppKit shows in full: "Cancel" ("Ca…") in the
  review panel, "Custom fee (duffs per …", and the "Type" picker caption. This needs the fork's layout
  patch or wider fixed frames.
- `Gtk-WARNING … GtkLabel reported min width … natural size must be >= min size`: 36 on Transactions
  and 72 in the send flow. They come from the 30x30 direction tile in `TransactionView`. Nothing
  visible breaks.
- `libEGL warning: DRI3 error`: Xvfb has no GPU, so llvmpipe is used.

## Not verified

- The post-run changes: switch and picker names after updates, the GApplication `shutdown` hook, and
  the corrected send-flow check. None has run on Linux, and the shutdown hook has not been compiled
  there.
- Live mode against a regtest node with blocks. No dashd ran: sync, balances, receive and a live send
  were not exercised in the GUI.
- CSV export through the GTK save dialog, receive-request creation, restore flow, lock/unlock,
  settings changes.
- x86_64, Wayland, Orca, keyboard navigation, Windows (WinUIBackend not built).

## macOS (AppKitBackend) for comparison

`../crossui-macos/*.png` are from the earlier m1/cross-ui run, before the rebase. In this pass, on
macOS:
- `swift build --product dash-wallet` passed.
- A live start with `--network regtest --datadir <tmp> --dapi … --connect …` opened the engine and
  created `regtest/` (wallet.sqlite, app.sqlite, spv/), `global.json` and `lastNetwork`.
- `--demo` stayed alive for 6 s.
- Window screenshots failed (`screencapture -l`: "could not create image from window"), and the AX
  tree of the unbundled executable showed no window content. So no macOS screenshots or accessibility
  checks were taken in this pass.
- The stale `live.png` (the old not_implemented page) was removed.
