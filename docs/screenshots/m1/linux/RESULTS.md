# M1 SwiftCrossUI app on Linux (GtkBackend): demo run, 2026-10-05

Harness: `scripts/crossui-linux-demo.sh` → image `dwd-linux-crossui` (`ci/linux/Dockerfile.crossui`:
swift:6.3.3-noble + Rust 1.98.1 + protoc 29.3 + GTK 4.14 + Xvfb + D-Bus + AT-SPI) →
`ci/linux/crossui/container-demo.sh` → `ci/linux/crossui/atspi_demo.py` (reuses the G2 probe's
`atspi_smoke.py` helpers). Host: Apple-silicon Mac, OrbStack, so the container is **linux/aarch64**.
x86_64 was not run.

## What was built

The view models came from a **local copy of the m1/d-view-models work in progress**. They are not on
main, and the branch does not build without them. The Swift sources in the run were this branch
(`m1/cross-ui`, commit e7cfdee) plus that uncommitted `Sources/WalletFeatures` copy and
`Sources/PlatformServices/ScreenCaptureGuard.swift`.

| Step | Result |
|---|---|
| `scripts/build-core.sh --no-bindings` (dw-ffi, dev, aarch64-unknown-linux-gnu) | OK, 256 s (cargo cache volumes reused; dw-ffi and build scripts recompiled) |
| `swift build --product dash-wallet` (GtkBackend through DefaultBackend) | OK, 101 s; ELF 64-bit aarch64 PIE, dynamically linked |
| Launch `dash-wallet --demo …` under Xvfb 1280x860, 4 sessions | All 4 stayed alive until they were killed (RSS 275–332 MB, debug build, llvmpipe) |

Two earlier attempts failed at build time, before anything ran. These were image problems, now fixed in `Dockerfile.crossui`:
1. Ubuntu's `clang` package repoints `/usr/bin/clang` from the Swift toolchain's clang-21 to clang-18.
   SwiftPM then passes `-index-store-path` to it for the C targets of SwiftCrossUI's
   dependencies (`clang: error: unknown argument: '-index-store-path'`).
2. Without that package, Rust has no `cc` (`linker 'cc' not found`). The fix keeps the package and
   points `/usr/bin/clang{,++}` back at the toolchain's clang after the install.

## Screenshots (Xvfb, `xwd -id <window>`)

| File | How it was reached |
|---|---|
| `1-overview.png` | `--demo`, first page |
| `2-send.png` | same process: **Send selected in the sidebar through the AT-SPI `Selection` interface** |
| `3-transactions.png` | `--demo --page transactions` |
| `4-receive.png` | `--demo --page receive` (QR from the engine's `qr_matrix`) |
| `5-onboarding.png` | `--demo onboarding` |

Matching AT-SPI dumps: `atspi-<step>.txt`. Check results: `atspi-checks.json`. Full log: `run.log`.
App stderr: `app-*.log`.

## AT-SPI checks

26 checks, 21 hard, **1 hard failure**:

- PASS: the app is on the bus. The window is a `frame` named "Dash Wallet" (3 of 4 sessions). Each page's
  marker text is exposed. The sidebar `list` holds Overview/Send/Receive/Transactions.
  `Selection.selectChild` switches the page to Send. The Send page has push buttons named "Send", "Add
  Recipient", "Clear All" and "Use available balance", and its address entry exposes
  `placeholder-text:Pay to: Dash address`.
- FAIL (harness race, not the app): in the `transactions` session the frame check ran right after the app
  appeared on the bus and found no frame yet (`[]`). The same session's page marker was found moments
  later, and the screenshot shows the window. `atspi_demo.py` now waits up to 30 s for the frame. That
  fix was **not re-run**.

Control naming (soft checks; gaps already recorded in ADR 0002):

| Role | Named | Notes |
|---|---|---|
| push button | all (4–10 per page) | Every DashButton uses a string title (no icon-only controls) |
| combo box (Picker), toggle button | all | |
| text (TextField/SecureField) | 0 | No accessible name. Each entry carries a descriptive `placeholder-text` (gap A3). |
| check box (`Toggle` `.switch`) | 0 | The label is a sibling (gap A2): "Discreet mode", "Subtract fee from amount" |
| list item | 0 | The row text is on child labels (gap A4). The harness reads it with `descendant_text`. |

ADR 0002 rules followed: every `List` (sidebar, Overview recent transactions, Transactions) sits in a
`ScrollView`, with fixed heights and single-line rows. The Transactions page with 36 records (1371 AT-SPI
nodes) did not grow the window or hit the X11 size limit.

## Rendering issues seen in the screenshots

- GTK ellipsizes some captions and buttons that AppKit shows in full: the "Type" picker caption
  ("Ty…"), "Clear All" ("Clear …") and "Custom fee (duffs per …". Label measurement differs between
  backends; this needs the fork's layout patch or wider fixed frames.
- `Gtk-WARNING … GtkLabel reported min width 17 and natural width 14 … natural size must be >= min size`,
  about 36 times on the Transactions page. They come from the 30x30 direction tile ("+"/"−") in
  `TransactionView`. Nothing visible breaks.
- `libEGL warning: DRI3 error`: Xvfb has no GPU; llvmpipe is used.

## Not verified

- No typing, button presses or end-to-end flows on Linux (send review/confirm, request creation,
  CSV export via the GTK save dialog, onboarding create/restore). Only the sidebar selection was driven.
- x86_64, Wayland, Orca, keyboard navigation.
- Windows (WinUIBackend): not built.
- Live mode (`dash-wallet` without `--demo`) on Linux. On macOS it opens the engine on the data
  directory and then shows `not_implemented`, because the WalletRuntime adapters are not on main yet.

## macOS (AppKitBackend) for comparison

`../crossui-macos/*.png`: the same binary target built with `swift build` on macOS. Each screenshot is a
capture of the app window only (`screencapture -l <window id>`), taken from 6-second launches of
`--demo [--page …]`, `--demo onboarding|locked`, and live mode with a temporary `--datadir`.
