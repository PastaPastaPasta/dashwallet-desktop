# Design brief — dashwallet-desktop

## Goal (from the user, verbatim intent)
Greenfield project: a replacement for dash-qt built on the stack that dashwallet-iOS uses.
Feature parity and compatibility with Dash Core Qt AND the iOS app (all user-facing features of both).
Finished, tested application for macOS, Windows and Linux. Robust testing on non-macOS will come later
(by agents on those OSes) — but the code must build for and target them now.
"Built like the iOS app" (same architecture/stack), NOT a build/port of the iOS app's code.

## Inputs (read all four before designing)
- docs/research/01-sdk-stack.md — SwiftDashSDK + Rust FFI inventory, portability, gaps, persistence, iOS adapter layer shape
- docs/research/02-dash-qt-features.md — dash-qt feature inventory + QT-001..QT-154 parity checklist, wallet compat rules (§18)
- docs/research/03-ios-features.md — iOS feature inventory + IOS-001..IOS-123 checklist, architecture, design language
- docs/research/04-ui-stack-options.md — UI stack options, hands-on SwiftCrossUI/Option-B probe results

## Environment facts
- Dev machine: macOS 26 (Apple silicon), Xcode 26.6 / Swift 6.3.3, Rust 1.97.1. ~60 GB free disk (tight; one shared CARGO_TARGET_DIR).
- Linux builds/tests possible locally via OrbStack Docker (arm64 native, x86_64 via Rosetta). No Windows machine.
- Platform repo clean worktree pinned to dashpay/platform v5.0-dev bc321362b9: /Users/pasta/workspace/dashwallet-desktop-deps/platform
  (pins rust-dashcore e4208c90). Main /Users/pasta/workspace/platform checkout is someone else's dirty tree — never touch.
- SwiftPM / git writes need to run outside the Claude sandbox (known).
- Project repo: /Users/pasta/workspace/dashwallet-desktop (empty git repo, local only for now).

## Default product decisions already taken (override only with strong reason)
- dash-qt parity wins where iOS dropped something: CoinJoin mixing, governance proposal voting/creation,
  private-key/paper-wallet sweep, masternode tab & tooling are IN scope.
- Invitation creation IN (SDK supports it). CrowdNode behind a feature flag (suspended on iOS).
- Seed is encrypted at rest on desktop with a user passphrase (dash-qt wallet-encryption parity), plus OS secret store.
- DashPay/Platform always compiled in, runtime-gated like iOS (identity present, advanced mode, network).
- Mainnet default; testnet/devnet/regtest selectable.
- SPV (no full node) is the engine; full-node-only dash-qt features (RPC console, mempool stats, PoSe/budget tallies)
  must be handled honestly — e.g. optional connection to a user's dashd RPC, or a debug console over the SDK — design must decide.

## What the design must produce
Write to the path given in your task. Cover:
1. Architecture: layers/modules/packages, language per layer, how Rust engine is linked on each OS, how the
   "SwiftDashSDK on non-Apple" problem is solved (portable core split vs fork vs own thin Swift binding over the C FFI),
   persistence (SwiftData vs platform-wallet-storage SQLite vs own), secrets (per-OS), UI per OS (SwiftUI on macOS +
   SwiftCrossUI elsewhere? one UI? Rust fallback?), view-model sharing, observation model.
2. How SDK feature gaps (CoinJoin mixing, governance, ProRegTx/ProUpServ/ProUpReg/ProUpRev, BIP21, message verify,
   fee estimation, PSBT/multisig?, sweep, dash-qt BIP39 quirks, dumpwallet import/export, wallet.dat import) get built —
   in which repo/crate (own crate in this repo extending platform-wallet? upstream patches?), and how we stay buildable.
3. Repo layout (directories, Package.swift/Cargo workspace), build system per OS, packaging (dmg/notarize, MSI/MSIX, AppImage/Flatpak), CI.
4. Testing strategy: Rust unit tests, Swift unit tests for view models (headless, run on Linux too), UI tests,
   regtest/testnet integration (local dashd regtest in Docker?), compatibility tests vs dash-qt wallets.
5. Delivery plan for an AI-agent swarm: ordered milestones and parallelisable workstreams with crisp interfaces
   so 5-10 agents can build in parallel without conflicts; for each workstream: scope, inputs, outputs, acceptance tests.
   Map every QT-xxx / IOS-xxx checklist item to a workstream (a table or ranges is fine).
6. Risks and go/no-go gates (Windows SwiftCrossUI, MSVC build of Rust deps, disk/build time), with fallbacks.
Be concrete and opinionated. Verify claims against code where cheap. Mark unverified items.
