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
  - SwiftUI `ImageRenderer`, or the `ScreenTests.capture` helper, which runs as an accessory app with no Dock icon or activation and draws into a window placed off screen;
  - write PNGs with `DWD_WRITE_SCREENSHOTS=1` and look at them with the Read tool.
- New test helpers that need AppKit must set `NSApplication.shared.setActivationPolicy(.accessory)` first and must never call `activate`, `makeKeyAndOrderFront` or `runModal`.
- Do Linux and CrossUI visual QA in Docker (Xvfb/GTK) with `scripts/crossui-linux-demo.sh`; nothing appears on the host.
