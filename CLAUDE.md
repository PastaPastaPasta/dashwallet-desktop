# Agent rules for dashwallet-desktop

## No visible UI on the developer's Mac

The developer works on this machine while agents build and test. These rules override any task prompt.

- Do not open windows, apps, panels or prompts on the host. That rules out:
  - `open`, `osascript` and `swift run` of a GUI target;
  - running `DashWallet.app` or `DashWalletCross` with the AppKit backend;
  - running XCUITests (`xcodebuild test` of `DashWalletUITests`), Simulator, or Codex computer use;
  - anything that triggers Touch ID, Keychain, notification-permission or file-panel dialogs.
- You may write XCUITests and build them (`xcodebuild build-for-testing`), but do not run them. Report the exact command for a later run in an isolated GUI session.
- Do macOS visual QA offscreen only:
  - SwiftUI `ImageRenderer`, or the `ScreenTests.capture` helper, which runs as an accessory app with no Dock icon or activation and draws into a borderless window placed off screen;
  - write PNGs with `DWD_WRITE_SCREENSHOTS=1` and look at them with the Read tool.
- New test helpers that need AppKit must set `NSApplication.shared.setActivationPolicy(.accessory)` first and must never call `activate`, `makeKeyAndOrderFront` or `runModal`. Any window they create must be `.borderless`; AppKit moves a titled window back onto the screen.
- Tests that touch the real Keychain, Touch ID or notifications must be opt-in behind an environment variable (for example `DWD_BIOMETRIC_TEST=1`) and must not run by default.
- Do Linux and CrossUI visual QA in Docker (Xvfb/GTK) with `scripts/crossui-linux-demo.sh`; nothing appears on the host.

## Product scope

This app is the DashPay wallet on the desktop: the iOS and Android wallets, adapted to macOS, Windows and Linux. It is not a full replacement for Dash Core. Dash Core (dash-qt) stays the power-user tool.

- Build first: what the mobile apps do. That means the DashPay identity, username, contacts and payments flows; send and receive; history; CoinJoin; backup and restore; security (PIN or password, biometrics, auto-lock); the iOS tools and settings.
- Add a Dash Core feature only when an ordinary wallet user on a desktop would miss it. Examples: coin control, PSBT, sign and verify messages, CSV export, and opening a dash-qt wallet to move funds.
- Leave to Dash Core: masternode and ProTx management, governance proposals and voting, the RPC console, debug and peer tools, and node operation. Do not build UI for these. If a task asks for one, say so in your report rather than building it.
