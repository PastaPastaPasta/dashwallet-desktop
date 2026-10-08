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

This app is the DashPay wallet on the desktop: the iOS and Android wallets, adapted to macOS, Windows and Linux. It is not a full replacement for Dash Core. Dash Core (dash-qt) stays the power-user tool. The authoritative scope is `docs/design/DASHPAY.md` (DEC-01, §5a); the task plan is `docs/design/ROADMAP.md`.

- Build first: what the mobile apps do. That means the DashPay identity, username, contacts and payments flows; send and receive; history; CoinJoin; backup and restore; security (PIN or password, biometrics, auto-lock); the iOS tools and settings.
- Add a Dash Core feature only when an ordinary wallet user on a desktop would miss it. Examples: coin control, PSBT, sign and verify messages, CSV export, and opening a dash-qt wallet to move funds.
- Add the wallet-holder masternode and governance flows (milestone MG, release 1.1), after the DashPay core. They are owner-side only (DASHPAY §5a). In scope:
  - governance: proposal list and detail; voting yes/no/abstain with the wallet's masternode voting keys, one or many masternodes at once; creating a proposal (collateral transaction, confirmations, submit, resume); the governance clock, superblock dates and budget;
  - my masternodes: masternodes whose collateral, owner, voting or payout key is in this wallet; tracking any masternode and attaching its keys; status from the SPV masternode list; payouts attributed in history; evonode status and claimable credits; evonode Platform credit withdrawal to the payout address;
  - the registration wizard: ProRegTx for regular masternodes and evonodes, with collateral funded new, from an existing UTXO, or held externally and proven with a signed message. The operator's public key, service address and Platform node details are entered as the operator gives them; a self-operator may fill the operator key from this wallet's masternode keychain;
  - ProUpRegTx: the owner changes the voting, payout or operator key;
  - v24 shared masternodes: creation (2-8 shares), reward-address change, key rotation, dissolution, standby;
  - optional: voting on contested DPNS names with the wallet's masternode voting keys.
- Leave to the Rust node and its CLI: operator and server tasks. Do not build UI for these. If a task asks for one, say so in your report rather than building it:
  - running a masternode or evonode, node administration, and debug and peer tools beyond the existing console;
  - operator-key-signed transactions: ProUpServTx (service updates, including "Unban" after PoSe) and ProUpRevTx (revocation);
  - generating or holding an operator BLS secret for a node the wallet does not derive, and Platform node operation.
- The wallet never signs with an operator key, and it generates no operator secret outside the existing keychain reveal flow.
