# dashwallet-desktop — reconciled design (authoritative)

Status: **authoritative**, 2026-10-05. Reconciles `DESIGN-opus.md` and `DESIGN-fable.md` (both kept for rationale).
**Base document: `DESIGN-opus.md`.** Everything in it applies unless amended below. Where this file and a
source design disagree, this file wins. Fable's document remains the reference for the detailed
protocol notes it carries (CoinJoin codec/rounds/salt detail, governance object/vote detail, ProTx v24 shared-MN
detail, wallet.dat record list, BIP21 superset parser, NodeLink scope, checklist mapping cross-check).

**Amended 2026-10-08 by [`DASHPAY.md`](DASHPAY.md) §8**: the UI stack, the binding, Platform proof trust, invitation
creation, the masternode and governance scope (DEC-01) and DESIGN-opus's M4 plan (WS-07) are superseded there. The task plan is
[`ROADMAP.md`](ROADMAP.md).

## R1. Decisions where both designs agreed (locked)
- Rust engine owned by this repo, built **directly on the `platform-wallet` Rust library** (generic over its persister),
  with `platform-wallet-storage::SqlitePersister` for wallet state. No SwiftData, no 46-slot vtable, no dependency on
  SwiftDashSDK at runtime. (Fable's U1 HostPersister patch is unnecessary — dropped.)
- App metadata in our own Rust SQLite (`dw-appdb`, `app.sqlite`); UI prefs in Swift `settings.json`.
- Secrets in a Rust vault; secrets never live in Swift beyond a transient `SecretBytes`.
- UI: SwiftUI on macOS; SwiftCrossUI (forked, pinned 0.10.x) on Linux (Gtk) and Windows (WinUI, gated; fallback Gtk on Windows).
  One shared `@MainActor @Observable` view-model layer (Foundation + Observation only, lint-enforced). Rust/egui is not a fallback.
- SE-0482 `staticLibrary` artifact bundles on all three OSes (no XCFramework).
- Own crates for every gap; upstream patches only for closed types, carried as `[patch]` to same-repo branches.
- Full-node-only features handled honestly: SPV-native where possible (governance over `govsync`), optional dashd RPC data
  source, never fabricated values.
- Scope: CoinJoin mixing, governance list/vote/create/resume, full ProTx incl. v24 shared MN, sweep, invitation creation,
  PSBT, dumpwallet + wallet.dat import (SQLite first, BDB later), exports for dash-qt. CrowdNode and HWI behind flags.
  *(Amended by DEC-01, DASHPAY §5a: governance and ProTx are the wallet-holder flows only, in milestone MG after the
  DashPay core. Operator-signed ProUpServTx and ProUpRevTx and server work stay with the node and its CLI.
  `CLAUDE.md` "Product scope" carries the current lists.)*
- Mnemonic passphrase is shown on reveal (behind full unlock). dash-qt bugs listed in research 02 §21 are not copied.

## R2. Decisions taken in reconciliation
| Topic | Opus | Fable | **Chosen** |
|---|---|---|---|
| Binding | UniFFI over coarse façade `dw-ffi` | Hand C ABI + JSON command bus | **UniFFI** (typed, async, callbacks, generated Swift committed). Gate **G1** in M0: UniFFI Swift builds + runs `swift test` on macOS and Linux (Docker); Windows checked when a runner exists. Fallback: hand C façade with the same object model. |
| P2P for CoinJoin/governance | own `dw-p2p` sessions | tap into dash-spv pool (upstream U2) | **own `dw-p2p`** (no upstream patch on the critical path). |
| CoinJoin output chain | DIP9 CoinJoin account m/9'/c'/4' | external BIP44 chain (Core compat) | **DIP9 account** — visible to dash-qt v24 descriptor wallets, dashj/Android and iOS. Restore lookahead also scans BIP44 chains with gap 1000 to find Core-mixed funds. |
| PIN | UI lock only in unencrypted mode | PIN-wrapped KEK quick unlock | **Opus**: passphrase is the root credential; biometric quick-unlock (Touch ID / Windows Hello) wraps the DEK; 4–8 digit PIN only as a labelled UI lock in unencrypted mode. iOS lockout policy kept as throttling. |
| Platform proof trust | trusted context provider | SPV quorum keys by default | **Trusted provider in M4 (iOS parity)**; SPV-derived quorum-key context provider is an M6 hardening item, then default. |
| Windows packaging | MSIX | WiX MSI | **WiX MSI** for 1.0 (works unsigned for testers); MSIX later. |
| Linux packaging | Flatpak + tarball | Flatpak + AppImage | **Flatpak + portable tarball**. |
| Themes | — | Light/Dark/System, drop Traditional | **Light/Dark/System**. |
| macOS min | 14 | 15 | **14**. |
| Announcements | deferred | signed GH Pages feed | deferred past 1.0. |
| Workstream table | 13 WS | 18 WS | **Opus WS-01…WS-13** (§5.2 of DESIGN-opus.md). Fable's §5.4 mapping used as a cross-check. |

## R3. Local environment rules (all agents)
- Rust target dir: on the dev Mac one shared dir, `~/workspace/dashwallet-desktop-deps/target`
  (`$DWD_DEPS_DIR/target` when `DWD_DEPS_DIR` is set); on Linux hosts that run several worktrees (agentbox) one
  `rust/target` per checkout, never shared, with sccache deduplicating compiles. `scripts/build-core.sh` applies
  this. Linux Swift builds run in Docker (`ci/linux/Dockerfile.swift`, `Dockerfile.crossui`) with shared
  `dwd-cargo-registry` / `dwd-cargo-git` volumes and target / SwiftPM volumes per checkout off macOS
  (`scripts/docker-volumes.sh`).
- Never build `release`/`dist` locally except when explicitly assigned; `dev` profile on the host triple.
- Disk guard: abort builds when free space < 15 GB (`scripts/disk-guard.sh`).
- `swift build`/`swift test`/`git` writes must run outside the Claude sandbox.
- Pins: platform `bc321362b9` (v5.0-dev, 2026-10-04), rust-dashcore `e4208c90`, Rust 1.98.1, Swift 6.3.3, SwiftCrossUI 0.10.0.
- A clean platform checkout at the pin exists at `~/workspace/dashwallet-desktop-deps/platform` on the dev Mac for
  reading; elsewhere use cargo's git checkout (`~/.cargo/git/checkouts/platform-*/bc32136`).
- Never touch `/Users/pasta/workspace/platform` (someone else's dirty tree).
- Commits: conventional commits, signed automatically; never `git add -A`.
