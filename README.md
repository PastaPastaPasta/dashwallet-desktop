# Dash Wallet Desktop

A desktop Dash wallet for macOS, Windows and Linux, replacing dash-qt. It is a
[Tauri 2](https://tauri.app) app (React + TypeScript renderer) over a Rust engine built on
dashpay/platform's `platform-wallet`: Core wallet, DashPay (usernames, contacts, invitations) and
Platform on one recovery phrase.

> **This is an early release. No security review has been done. Use small amounts only.**
> The binaries are attested by GitHub but not code-signed by Apple or Microsoft. Keep a written copy of your recovery phrase and do not hold more than you can afford to lose.

## What is in 0.1

| Area | In 0.1 |
|---|---|
| Wallet | create, restore (12/24 words), verify the phrase, passphrase lock, auto-lock, several wallets |
| Network | mainnet or testnet, chosen at onboarding and in Settings (a restart applies it); SPV sync |
| Home | one balance for everything you can spend, with Core, Shielded and Platform rows; sync status |
| Core send and receive | address, QR and `dash:` URI, InstantSend, history, transaction detail, coin control, PSBT, sign and verify messages, CSV export of the history |
| Shielded | shielded balance and receive address, Core → Shielded, send to a shielded address, Shielded → Core. Gated on the network's Platform protocol version: where the pool is not active the app says so |
| Platform addresses | balance, activity, and "Move to Dash wallet" so restored funds are never stranded (no Platform receive address yet) |
| Usernames | register (contested and not), status, main name, profile and avatar, top-up and withdraw credits |
| Contacts | search, send and accept requests, contact list, pay a contact, history with a contact, notifications |
| Invitations | claim an invitation link |
| CoinJoin | mixing start and stop, settings, progress (experimental) |
| Masternodes | governance and username voting screens, hidden until Settings ▸ Show Masternodes is on |

**Not in 0.1:** username marketplace, DEX, Buy & Sell, Explore, DashSpend, CrowdNode, invitation creation,
transaction metadata, private-key sweep, localization (English only), fiat, masternode registration and
shared masternodes, shielded-funded registration, Platform ↔ Shielded transfers, auto-update, notarization,
Windows code signing, hardware wallets. The plan is
[`docs/research/gap-2026-10-31/RELEASE-PLAN.md`](docs/research/gap-2026-10-31/RELEASE-PLAN.md).

## Latest build

Preview builds of `main`, one click each. They are **unsigned or ad-hoc signed previews** (no notarization, no
Windows code signing), made by GitHub Actions on every merge to `main`, and they come with no security review:
small amounts only.

| OS | Latest build |
|---|---|
| **macOS** (Apple silicon and Intel) | [**DashWallet-continuous-macos-universal.dmg**](https://github.com/PastaPastaPasta/dashwallet-desktop/releases/download/continuous/DashWallet-continuous-macos-universal.dmg) |
| Windows (x64) | [DashWallet-continuous-windows-x64-setup.exe](https://github.com/PastaPastaPasta/dashwallet-desktop/releases/download/continuous/DashWallet-continuous-windows-x64-setup.exe) |
| Linux (x86_64) | [DashWallet-continuous-linux-x86_64.AppImage](https://github.com/PastaPastaPasta/dashwallet-desktop/releases/download/continuous/DashWallet-continuous-linux-x86_64.AppImage), [DashWallet-continuous-linux-amd64.deb](https://github.com/PastaPastaPasta/dashwallet-desktop/releases/download/continuous/DashWallet-continuous-linux-amd64.deb) |
| Checksums | [SHA256SUMS](https://github.com/PastaPastaPasta/dashwallet-desktop/releases/download/continuous/SHA256SUMS) |

The [`continuous` release](https://github.com/PastaPastaPasta/dashwallet-desktop/releases/tag/continuous) says which
commit it holds. Each merge also keeps its own build, `v<version>-dev.<run>+<commit>`, on the
[releases page](https://github.com/PastaPastaPasta/dashwallet-desktop/releases), next to the tagged releases
(`v0.1.0`). Check a download with [`docs/release/verifying.md`](docs/release/verifying.md).

## Install

| OS | File (the per-build and tagged names; the latest-build names above replace `dash-wallet-<version>` with `DashWallet-continuous`) |
|---|---|
| macOS | `dash-wallet-<version>-macos-universal.dmg` |
| Windows | `dash-wallet-<version>-windows-x64-setup.exe` |
| Linux | `dash-wallet-<version>-linux-x86_64.AppImage` or `dash-wallet-<version>-linux-amd64.deb` |

The binaries are not notarized (macOS) or signed (Windows), so each OS warns on first launch.

- **macOS.** Open the dmg and drag Dash Wallet to Applications. The app is ad-hoc signed only. Right-click
  the app ▸ **Open** ▸ Open, or clear the quarantine flag once:
  `xattr -dr com.apple.quarantine "/Applications/Dash Wallet.app"`
- **Windows.** Run the installer. When SmartScreen says "Windows protected your PC", choose **More info ▸
  Run anyway**. It installs for the current user. Windows needs the WebView2 runtime, which Windows 11 and
  current Windows 10 already have.
- **Linux AppImage.** `chmod +x *.AppImage && ./*.AppImage`. Needs WebKitGTK 4.1
  (for example Ubuntu 24.04 or Debian 12) and FUSE 2, or run it with `--appimage-extract-and-run`.
- **Linux .deb.** `sudo apt install ./*.deb` (Debian, Ubuntu). It registers the `dash:`, `pay:`,
  `dashwallet:` and `dashpay:` links.

## Verify a download

Every published file and every `SHA256SUMS` carries a GitHub build attestation. With the GitHub CLI (logged in: `gh auth login`):

```sh
gh attestation verify <file> --repo PastaPastaPasta/dashwallet-desktop
sha256sum -c --ignore-missing SHA256SUMS      # Linux; on macOS: grep <file name> SHA256SUMS | shasum -a 256 -c
```

Details, what an attestation does and does not prove, and the release names: [`docs/release/verifying.md`](docs/release/verifying.md).
There are no PGP signatures; a real release key is a 1.0 task.

## Build from source

Requirements: Rust 1.98.1 (`rust/rust-toolchain.toml`; rustup installs it), protoc 29+, libclang, Node 24 and
pnpm (the version in `apps/desktop/package.json`). On Linux the Tauri build needs the WebKitGTK development
packages (`apps/desktop/README.md`, "Setup").

```sh
cd apps/desktop
pnpm install --frozen-lockfile
pnpm tauri dev                  # the app with hot reload, on the real engine
pnpm tauri build                # the release binary and the bundles for this OS
```

The bundles land in `rust/target/release/bundle/`. Release builds come from the `Release` workflow on a `v*`
tag (`.github/workflows/release.yml`, [`docs/ci.md`](docs/ci.md)). For development, read
[`apps/desktop/README.md`](apps/desktop/README.md) (renderer, tests, launch options, the self-test),
[`CLAUDE.md`](CLAUDE.md), [`docs/design/DESIGN.md`](docs/design/DESIGN.md) and
[`docs/design/ROADMAP.md`](docs/design/ROADMAP.md).

## Layout

```
apps/desktop/             Tauri renderer: React 19 + TypeScript + Vite (apps/desktop/README.md)
rust/                     Cargo workspace (members = crates/*, bin/*)
  crates/dw-app           the Tauri 2 app: commands over dw-engine, event bridge, deep links
  crates/dw-engine        Engine: tokio runtime, per-network sessions over
                          PlatformWalletManager<SqlitePersister> + dash_sdk::Sdk
  crates/dw-vault         Passphrase vault (Argon2id) and OS secret store for the seed
  crates/dw-appdb         App database: labels, payment requests, address book
  crates/dw-uri, dw-units, dw-message, dw-compat   dash-qt-compatible pure libraries
                          (dw-compat: wallet.dat / dumpwallet / descriptor readers and writers)
  crates/dw-psbt          Dash Core PSBT subset (BIP174 v0): parse, analyze, sign, finalize
  crates/dw-console       local RPC console: dash-qt grammar, redaction, Core result shapes
  crates/dw-desktop       OS services (autostart, single instance, notifications, QR from images)
  bin/dwcli               CLI over dw-engine (regtest L1/L2 driver)
docs/                     design, contracts, ADRs, research; docs/release holds the release notes
regtest/                  Dash Core regtest harness (docker compose, pytest, functional tests)
ci/, .github/workflows/   CI, the nightly run and the release workflow
Sources/, Package.swift   the earlier Swift apps (see "The earlier Swift apps")
```

## The engine from the command line

`dwcli` drives the engine without a UI. Rust tests for the whole workspace: `cd rust && cargo test --workspace`.

CLI:

```sh
cd rust
DW="cargo run -p dwcli -- --datadir /tmp/dw --network regtest --dapi http://127.0.0.1:2443 --peer 127.0.0.1:19899 --passphrase-file /tmp/dw-pass"
$DW init-vault                      # encrypted with the first line of --passphrase-file
$DW create
echo "<mnemonic>" | $DW import --birth-height 0
$DW sync --min-height 101           # SPV until the wallet has scanned height 101
$DW list
$DW send --to <address>:<duffs>
```

`dwcli --help` lists every command: history, receive, utxos, lock/unlock, book, sign and verify.
M2 added:

- wallet lifecycle and tools: `load-states`, `load`, `unload`, `watch-only` (from a BIP44 account
  xpub), `xpub`, `rescan`, `birth-height`, `abandon`, `resend`, `extras`, `export-csv`, `fees`,
  `summary` (coin-control estimate), `node-info`, `console` (one console line), `watch-events`;
- dash-qt compatibility: `inspect`, `import-dump`, `import-wallet-dat`, `import-key`,
  `export-core`, `core-compat`, `addresses`;
- backups: `backup`, `restore`, `backups`;
- PSBT: `psbt-create`, `psbt-analyze`, `psbt-sign`, `psbt-broadcast`.

### Import formats, backups and PSBT

The engine reads and writes dash-qt's files (contract: `docs/contracts/m2-engine.md` §2.6–2.8):

| What | Status |
|---|---|
| Recovery phrase, 12–24 words, BIP39 passphrase, Core-compatibility check | works |
| `wallet.dat`, SQLite descriptor wallet, encrypted or not | works when the wallet holds its mnemonic; descriptor wallets without one: `not_implemented` |
| `wallet.dat`, legacy Berkeley DB | detected; `not_implemented` (M6), the app says what to do instead |
| `dumpwallet` file | HD wallet rebuilt from the phrase or 64-byte seed, checked against the header xprv; loose keys are counted, not imported (sweep is M5) |
| `listdescriptors true` JSON (with a phrase), raw 64-byte `hdseed` hex | works; xprv-only and shorter seeds: `not_implemented` |
| Watch-only wallet from a BIP44 account xpub | works; it cannot sign or send |
| Export for dash-qt | phrase + passphrase (`upgradetohd`), `dumpwallet` file, `importdescriptors` JSON; written 0600, never over an existing file |
| Backup / restore | `.dwbackup` v1 (`docs/contracts/dwbackup-v1.md`): one wallet, encrypted, network-bound |
| Automatic backups | when a wallet is added or a session opens with the data key; `<wallet id>.YYYY-MM-DD-HH-MM.dwbackup`, keeps 0–10 (Options ▸ Wallet) |
| PSBT | Create Unsigned (Options ▸ Wallet ▸ Enable PSBT controls), load from a file (binary or base64, under 100 MiB) or the clipboard (base64), analyze, sign, broadcast, copy, save |

The phrase, `wallet.dat`, `dumpwallet`, `listdescriptors`, export and PSBT paths are checked
against dashd v24 in the regtest `restore` suite, watch-only in `l2-tools`; backups by
`rust/crates/dw-engine/tests/r2_compat_offline.rs`. In demo mode these calls answer
`not_implemented`, and the screens show that error.

## Rust on a Linux host

The Rust workspace, `dwcli` and the regtest suites build and run on a plain Linux host. On
Ubuntu (checked on 26.04, agentbox):

```sh
sudo apt install build-essential clang libclang-dev libssl-dev pkg-config cmake unzip curl
curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal   # rust-toolchain.toml installs 1.98.1
# protoc 29+: Ubuntu's protobuf-compiler is 3.21, too old. Use the release zip, as the Dockerfiles do
# (or `mise use -g protoc@32`):
curl -fsSLO https://github.com/protocolbuffers/protobuf/releases/download/v29.3/protoc-29.3-linux-x86_64.zip
sudo unzip -o protoc-29.3-linux-x86_64.zip -d /usr/local bin/protoc 'include/*'
cd rust && cargo test --workspace
```

Neither an engine data directory nor any directory above it may be group- or world-writable unless
it has the sticky bit (platform-wallet-storage refuses the database and names the directory; `/tmp`
at 1777 is fine). Under the umask 002 of Ubuntu and Fedora desktops a plain `mkdir` gives 0775, so the
app, dwcli and the tests never rely on the umask:

- The directories on the path to a database or secret are created 0700: the data root and any missing
  parents (a new `~/.local/share` too), the network directories, `backups/` (with storage's
  `backups/auto`), `avatars/`, `spv/` and the vault. The databases, the vault, backups and key exports are 0600
  (`app.sqlite` too), and so are the settings files and the open-session marker. What dash-spv writes
  inside `spv/` takes the umask's mode, behind the 0700 network directory.
- Files written outside the data root are created 0600 too: CSV exports (transactions, address book),
  saved PSBTs and the log-export zip. Saving over an existing file replaces it with a new 0600 one.
- What the app owns loses group and other access when it already has it (an older build created it with
  the umask's mode): the default data root (the Swift app logs it on stderr), and the network
  directories, `backups/`, `backups/auto/`, `avatars/`, `spv/`, the vault directory and `app.sqlite` (the engine
  logs a warning). The settings files are replaced by 0600 ones on their next write. `avatars/` is a
  disposable cache: when it cannot be created (a stray file of that name), the engine logs a warning and
  disables avatars for the session instead of failing to open the network.
- Modes are changed through a descriptor, never by path: each directory is opened with
  `O_NOFOLLOW` relative to its parent (`rust/crates/dw-fs`, `PrivateFileSystem.swift`; `O_PATH` on
  Linux, so a search-only parent such as a 0711 `/home` is fine), so a symlink swapped in after the
  check is not followed. A symlink in place of one of the app's directories is followed but nothing
  behind it changes, and nothing changes below a directory another user can write; both are logged.
  One of the app's directories that another user owns is an error naming it. Root and the owner of
  `/` (root inside a Flatpak or other user namespace) count as the user, as in the storage check.
- An existing `--datadir` or chosen directory, and every existing parent, keep their mode: they are
  the user's. If one is group-writable, opening the network fails and the storage error (on stderr)
  names it and the `chmod go-w` that fixes it. On agentbox `~/workspace/dw-wt` is 0775, so do not
  put a data dir there.


## The earlier Swift apps

The repository still holds the first two UIs over the same engine: a SwiftUI macOS app and a SwiftCrossUI
app (`Sources/`, `Apps/macOS`, `Package.swift`, built with `scripts/build-core.sh`). The Tauri app replaces
them and they are retired after the `swift-final` tag (roadmap U-08). They are not part of a release.
Their build, test and demo instructions are in this file's history before the 0.1 rewrite
(`git log -p -- README.md`), and `scripts/linux-docker-test.sh` still runs their Linux tests in CI.

## Parity

[`docs/parity.md`](docs/parity.md) lists every dash-qt (QT-xxx) and iOS (IOS-xxx) checklist item with its
status, milestone and test reference. It is re-baselined on the Tauri app in 0.2.

## Checks

- Generated bindings are never hand-edited: the Tauri bindings (`apps/desktop/src/api/generated`) come from
  `cd rust && DW_UPDATE_BINDINGS=1 cargo test -p dw-app`, and CI fails when they are stale.
- GitHub Actions runs CI on Linux, macOS and Windows for `main` and `dw/**` pushes, a nightly regtest run
  and the UI-gate measurement runs ([`docs/ci.md`](docs/ci.md)). Run screenshots are downloadable artifacts.
