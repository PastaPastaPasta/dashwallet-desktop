# Dash Wallet on Tauri 2 (gate G-UI spike)

The Tauri candidate of gate G-UI ([`DASHPAY.md` §2.1](../../docs/design/DASHPAY.md)), task G-01. It shows
Overview, Transactions, the transaction detail sheet and the lock screen in light and dark, drawn to
[`UX-SPEC.md`](../../docs/design/UX-SPEC.md) with tokens generated from the same source as the Swift
toolkits. Findings and the Linux pre-measurements for G-02 are in
[`docs/research/g-01-tauri-spike.md`](../../docs/research/g-01-tauri-spike.md).

```
apps/desktop/                 renderer: React 19 + TypeScript + Vite, TanStack Query
  src/api/generated/          IPC types, generated from rust/crates/dw-app/src/dto.rs (ts-rs)
  src/api/                    Backend interface, TauriBackend (invoke), FixtureBackend, queries, event bridge
  src/styles/tokens.generated.*  CSS custom properties + typed tokens (scripts/gen-tokens.mjs)
  src/app/selftest.ts         --selftest: element boxes, UX-SPEC §2 style checks, IPC timings, U6 probes
  e2e/                        Playwright in Chromium and WebKit (screens, style checks, axe, keyboard, XSS)
  scripts/gate/               Linux gate harness (Xvfb + AT-SPI), AT-SPI walker, U1 comparison
rust/crates/dw-app/           Tauri 2 app: commands over dw-engine (in-process), event bridge, ACL, CSP
```

`dw-app` lives in the Rust workspace, not in `src-tauri/`, so it shares `Cargo.lock` and the pinned
platform graph with `dw-engine`. `pnpm tauri …` runs the Tauri CLI against it (`scripts/tauri.mjs`).

## Setup (Ubuntu 26.04, as on agentbox)

```sh
# Tauri's Linux build and runtime dependencies (WebKitGTK 4.1 is the webview)
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev
# Headless GUI runs, screenshots and the AT-SPI walk (gate harness only)
sudo apt install xvfb dbus-x11 at-spi2-core gir1.2-atspi-2.0 python3-pyatspi python3-gi \
  xdotool imagemagick x11-apps netpbm webkitgtk-webdriver
# Node 24 + pnpm 12 (agentbox has them through mise)
pnpm install
pnpm exec playwright install --with-deps chromium webkit
```

The Rust toolchain is the workspace's (`rust/rust-toolchain.toml`, 1.98.1). Building `dw-app` needs the
WebKitGTK development packages above, so on Linux `cargo build --workspace` now needs them too.

## Commands (from `apps/desktop`)

| Command | What it does |
|---|---|
| `pnpm dev` | Vite dev server on :1420 with the fixture backend (open in any browser; `?theme=dark`, `?variant=xss\|empty\|large`, `?locked=1`) |
| `pnpm tauri dev` | the app with hot reload, on the real engine |
| `pnpm tauri build` | release binary plus `.deb` and AppImage (`--no-bundle` for the binary only) |
| `pnpm test` | unit tests: amount formatting against `testdata/amount_format.json`, presentation, fixture, event bridge, token hygiene |
| `pnpm e2e` | Playwright on the built renderer (`pnpm build` first) in Chromium and WebKit; `DW_WRITE_SCREENSHOTS=1` writes into `docs/screenshots/gate/` |
| `pnpm tokens` | regenerates `src/styles/tokens.generated.*` (`pnpm build` fails when they are stale) |
| `cd rust && DW_UPDATE_BINDINGS=1 cargo test -p dw-app bindings` | regenerates `src/api/generated/` (`cargo test` fails when it is stale) |

The app (`rust/target/release/dash-wallet`):

```
dash-wallet [--datadir DIR] [--network mainnet|testnet|regtest|devnet-NAME] [--sync]
            [--spv-peer IP:PORT]… [--dapi URL]… [--theme light|dark]
            [--fixture] [--selftest FILE [--selftest-pause MS]]
```

`--fixture` uses the TypeScript sample wallet (passphrase `demo`) instead of the engine. `--selftest FILE`
renders the four screens in light and dark, writes every `data-testid` box, the style-check results, the
IPC timings and the security probes to `FILE`, and exits.

Linux gate harness (Xvfb + private D-Bus + AT-SPI; nothing shows on a display):

```sh
apps/desktop/scripts/gate/linux-gate.sh OUT_DIR [--datadir DIR --network regtest --spv-peer … --dapi …]
apps/desktop/scripts/gate/linux-packages.sh OUT_DIR   # U7: install the .deb, run the AppImage (sudo)
```
