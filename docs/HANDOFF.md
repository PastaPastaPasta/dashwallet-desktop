# Handoff: dashwallet-desktop (2026-10-08)

This project moves from the laptop to agentbox. The laptop session that built it has been stopped.
Finishing this project is part of your plan. You will need to start new agents to do the work below.

## Everything here is a suggestion

The code, the design documents and the roadmap below are what the previous agent produced in three days. **None
of it is binding.** If you judge that a decision was wrong, change it, even if that means starting over. That
includes SwiftUI on macOS with SwiftCrossUI elsewhere (two sets of screens), UniFFI, the Rust engine on
`platform-wallet`, the milestone plan and the vendored SwiftCrossUI fork. Starting from zero is acceptable if
that gives a better result. The user's goal is what matters, not what already exists.

The user's goal, latest version (2026-10-07): **DashPay on the desktop.** It should have full feature parity
with the mobile apps (iOS and Android), plus tasteful compatibility with Dash Core. It targets macOS, Windows
and Linux. Dash Core (dash-qt) remains the power-user tool, so masternode, ProTx and governance UI, the RPC
console and node operation are out of scope. See "Product scope" in `CLAUDE.md`.

## Where it is

- Repo: https://github.com/PastaPastaPasta/dashwallet-desktop (public). Work happens on `main`.
- Parked branches: `m3/r2-governance` and `m3/r3-protx` hold governance and ProTx work that was cut by the
  scope change. They are kept for reference only.
- The laptop copy is `/Users/pasta/workspace/dashwallet-desktop`. It has about 50 old local workstream
  branches, all merged or obsolete, and none of them were pushed.

## What exists

The stack, described in `docs/design/DESIGN.md`, which takes precedence over `DESIGN-opus.md`:

- A Rust engine, `rust/crates/dw-*`, built directly on the `platform-wallet` and
  `platform-wallet-storage` libraries from dashpay/platform. It pins platform v5.0-dev `bc321362b9` and
  rust-dashcore `e4208c90` as git dependencies.
- `dw-ffi` exposes the engine through UniFFI as the library `dashwallet_core`.
- On top of that sit the Swift layers `DashKit` → `WalletRuntime` → `WalletFeatures`. They are shared
  `@Observable` view models that use only Foundation and run on every OS.
- UI: `MacUI` + `DashUIMac` use SwiftUI and are macOS only. `CrossUI` + `DashUICross` use SwiftCrossUI 0.10,
  vendored and patched in `Vendor/`. They run on Linux with GTK; Windows is planned but has never been built.
- `bin/dwcli` is a command-line driver. `regtest/` holds a dashd v24 harness with sync, send, tools, restore
  and CoinJoin suites.

Milestones:

- M0 to M2 are merged. That covers the engine, vault, both UIs, and the dash-qt layer-1 tools: PSBT, coin
  control, sign/verify, import/export and backups.
- M3 is merged: the CoinJoin client and mixing, plus an iOS-style restyle of both UIs. Governance and ProTx were
  removed from `main`.
- Parity status is in `docs/parity.md`:
  - dash-qt checklist: 51 done, 63 partial, 22 not started, 18 parked.
  - iOS checklist: 12 done, 48 partial, 60 not started.
  - DashPay itself (identities, usernames, contacts, payments) is **not started**.

Known gaps:

- The macOS XCUITests have never run.
- No GUI has been tested against a syncing node or a real send; only `dwcli` has, through the regtest suites.
- Windows has never been built.

Reading order: `CLAUDE.md`, then `README.md`, `docs/design/DESIGN.md`, `docs/design/UX-SPEC.md`,
`docs/parity.md`, the `docs/contracts/` files and `docs/research/`.

## Work that was in flight when it stopped

A design workflow was running on the laptop:

1. Finish M3: committed, but the final check never finished. The CoinJoin regtest suite was cut off
   mid-mixing when the workflow stopped, so run it again.
2. Research (completed).
3. Two independent designs for DashPay on the desktop, one by Fable and one by Opus (never started).
4. Reconcile them into `docs/design/DASHPAY.md` and `docs/design/ROADMAP.md` (never started).

The research output is in `docs/research/dashpay/`:

- `ios.md` and `android.md` cover how the mobile apps implement DashPay.
- `platform.md` covers what the pinned Platform revision supports and what is missing.

The research left these questions open. Answer them yourself where you can, and ask the user about the rest:

1. **Invitations.** Android can create invitations, iOS removed the feature, and DESIGN D11 includes it.
   Building it needs an upstream platform-wallet-storage patch (estimated 3–5 days).
2. **Pin timing.** Should the platform pin move to v5.0-dev head now or at the start of the DashPay work? Should
   the desktop follow iOS to v5.1-dev, which brings contact pay without Core funding (#4623) and claim status
   (#4997)?
3. **Shielded transactions.** When should the `shielded` feature be enabled, given the halo2 build and the
   larger binary?
4. **Desktop credentials for integrations.** These are Coinbase and Uphold redirect URIs, a CTX client ID, and
   Topper, ZenLedger and Imgur keys. They need DCG decisions.
5. **Explore database.** Does it need Firebase anonymous auth, as Android uses?
6. **Local devnet.** Where should dashmate run, and are v5 Docker images published for the pin?
7. **Trust model.** Keep the trusted quorum service for 1.0, as iOS does, and move to SPV-derived quorum keys
   later?
8. **DashPay QR.** Should it stay a plain `dashpay://user` link, as on iOS, or use DIP-15 auto-accept?

## Suggested next steps

Each step needs its own agents. Run waves of at most four agents, and have one agent stabilize `main`
before you fan out.

1. **Set up agentbox.**
   - Clone the repo.
   - Make the shared dependency and target directories configurable instead of hard-coded.
   - Get `cargo test --workspace`, the Linux `swift test` targets, the regtest suites and the CrossUI
     Xvfb demo (`scripts/crossui-linux-demo.sh`) passing on agentbox.
2. **Design DashPay on the desktop** (the unfinished phases 3 and 4 above) and write a roadmap. Base both on
   the research.
3. **Implement DashPay** (identities, usernames, contacts, payments) across the engine, the view models and
   the UI.
4. **Remaining mobile parity.** These are the iOS checklist items still not started. They include
   services/integrations, localization and packaging (Flatpak/tarball, WiX MSI, a macOS app bundle).
5. **Windows.** Get a first build and decide between WinUI and GTK.
6. **Hardening and QA.** Run real-node GUI flows on Linux and macOS, run the XCUITests, and do security
   review of the vault and signing.

## The macOS part

agentbox runs Linux, so it cannot build or test the SwiftUI app (`MacUI`, `DashUIMac`, `Apps/macOS`) or run
XCUITests. You have three options:

- keep the macOS screens and ask the user for a Mac to work on;
- use SwiftCrossUI's AppKit backend on macOS too, so there is a single UI codebase;
- replace the UI approach entirely.

Your choice; see "Everything here is a suggestion".
