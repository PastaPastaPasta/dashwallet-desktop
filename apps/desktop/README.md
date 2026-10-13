# Dash Wallet on Tauri 2

The desktop app. G-04 chose Tauri ([ADR 0003](../../docs/adr/0003-ui-stack.md)); this tree starts from
the gate G-UI candidate (tasks G-01 and G-02, branch `dw/g-02-tauri-measure`). It shows
Overview, Transactions, the transaction detail sheet and the lock screen in light and dark, drawn to
[`UX-SPEC.md`](../../docs/design/UX-SPEC.md) with tokens generated from the same source as the Swift
toolkits. Findings and the Linux pre-measurements for G-02 are in
[`docs/research/g-01-tauri-spike.md`](../../docs/research/g-01-tauri-spike.md).

```
apps/desktop/                 renderer: React 19 + TypeScript + Vite, TanStack Query
  src/api/generated/          IPC types, generated from rust/crates/dw-app/src/dto.rs (ts-rs)
  src/api/                    Backend interface, IpcBackend (generated commands), queries, event bridge,
                              lock-report summary and reconciliation (lockReport.ts)
  src/fixture/                fixture backend (U-07): Tauri mock IPC answering from scenario data; never in
                              a production build
  src/styles/tokens.generated.*  CSS custom properties + typed tokens (scripts/gen-tokens.mjs)
  src/app/selftest.ts         --selftest: element boxes, UX-SPEC §2 style checks, IPC timings, U6 probes
  e2e/                        Playwright on the fixture backend, Chromium and WebKit (scenarios, screens,
                              style checks, axe, keyboard, errors, lock report, XSS, visual baselines)
  e2e-tauri/                  tauri-driver smoke: the real app on the real engine (WebKitGTK, Linux)
rust/crates/dw-app/           Tauri 2 app: commands over dw-engine (in-process), event bridge, ACL, CSP
```

`dw-app` lives in the Rust workspace, not in `src-tauri/`, so it shares `Cargo.lock` and the pinned
platform graph with `dw-engine`. `pnpm tauri …` runs the Tauri CLI against it (`scripts/tauri.mjs`).

## Setup (Ubuntu 26.04, as on agentbox)

```sh
# Tauri's Linux build and runtime dependencies (WebKitGTK 4.1 is the webview)
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev
# Node 24 + pnpm 12 (agentbox has them through mise)
pnpm install
pnpm exec playwright install --with-deps chromium webkit
```

The Rust toolchain is the workspace's (`rust/rust-toolchain.toml`, 1.98.1). Building `dw-app` needs the
WebKitGTK development packages above, so on Linux `cargo build --workspace` now needs them too.

## Commands (from `apps/desktop`)

| Command | What it does |
|---|---|
| `pnpm dev` | Vite dev server on :1420 with the fixture backend (open in any browser; URL parameters below) |
| `pnpm tauri dev` | the app with hot reload, on the real engine |
| `pnpm build` | production renderer in `dist/`; fails if it holds fixture code (`scripts/check-bundle.mjs`) |
| `pnpm build:fixture` | renderer with the fixture backend in `dist-fixture/` (`pnpm preview:fixture` serves it on :4173) |
| `pnpm tauri build` | release binary plus `.deb` and AppImage (`--no-bundle` for the binary only); runs `pnpm build` |
| `pnpm test` | unit tests: amount formatting against `testdata/amount_format.json`, presentation, fixture, event bridge, token hygiene |
| `pnpm e2e` | `pnpm build:fixture`, then Playwright in Chromium and WebKit; `DW_WRITE_SCREENSHOTS=1` writes into `docs/screenshots/gate/` |
| `scripts/e2e-docker.sh [args]` | the same in the pinned Playwright image, as CI runs it; `DW_VISUAL=1 … --project=chromium e2e/visual.spec.ts` compares the visual baselines |
| `pnpm e2e:tauri` | tauri-driver smoke on Linux: builds the debug app and dwcli, then drives the app on a new regtest wallet; needs a private `DISPLAY` (`xvfb-run -a`, or `agent-gui start N`), `WebKitWebDriver` and `tauri-driver` (`cargo install tauri-driver --version 2.1.0 --locked`, or `$TAURI_DRIVER`) |
| `pnpm tokens` | regenerates `src/styles/tokens.generated.*` (`pnpm build` fails when they are stale) |
| `cd rust && DW_UPDATE_BINDINGS=1 cargo test -p dw-app bindings` | regenerates `src/api/generated/` (`cargo test` fails when it is stale) |

The app (`rust/target/release/dash-wallet`):

```
dash-wallet [--datadir DIR] [--network mainnet|testnet|regtest|devnet-NAME] [--sync]
            [--spv-peer IP:PORT]… [--dapi URL]… [--no-platform] [--theme light|dark]
            [--window-size WxH] [--selftest FILE [--selftest-pause MS]]
```

The app always runs on the engine; there is no fixture mode in it (the fixture backend is for browsers,
below). `--selftest FILE` renders the four screens in light and dark, writes every `data-testid` box, the style-check results, the
IPC timings and the security probes to `FILE`, and exits. `--no-platform` opens a chain without Platform
(a plain dashd regtest; regtest still needs a `--dapi` address, which nothing connects to).
`--window-size` sets the window's inner size; the gate's self-tests use 1000x620 on every OS.

On Linux without a GPU (no `/dev/dri/renderD*`), the app sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` before
GTK starts: WebKitGTK's DMA-BUF renderer falls back to slow software rendering there (about 1 s of first
paint and 170 MB of memory on agentbox). Set the variable yourself to override the choice.

The gate harness that measured G-UI (Xvfb + AT-SPI, the UIA walk, `ci/gate/`) stays on branch
`dw/g-02-tauri-measure`. Results and method: [ADR 0003](../../docs/adr/0003-ui-stack.md).

## Fixture backend (U-07)

`src/fixture/` stands in for dw-app at the IPC boundary. It installs Tauri's mock IPC
(`@tauri-apps/api/mocks`), so the renderer runs its real code: the generated command wrappers and the
`engine://event` listener. The handlers are typed by the generated `CommandArgs` and `CommandResults`, so
a changed command fails `pnpm typecheck` until the fixture follows. A command without a handler rejects
with `platform.not_implemented`, as the engine's stubs do.

It is reachable only where the build-time constant `__DW_FIXTURES__` is true: the dev server and
`vite build --mode fixture`. `pnpm build` (and so every `pnpm tauri build`) leaves it out and
`scripts/check-bundle.mjs` proves it: `dist/` must not contain the fixture's sentinel or the mock IPC,
`dist-fixture/` must. In a fixture build `window.__dwFixture` lets tests emit events, fail commands and read
the calls made.

URL parameters (`pnpm dev`, `pnpm preview:fixture`, Playwright):

| Parameter | |
|---|---|
| `scenario=` | `funded` (default; passphrase `demo`), `empty`, `no-wallet`, `locked`, `large` (10,000 rows), `xss`, `dashpay-no-identity`, `dashpay-registering`, `dashpay-registered` (contacts in every relation and channel state, pending requests both ways), `lock-report-omitted` (the last lock omitted flows, artifacts and wallets: unresolved until its leases end), `lock-report-pending`, `lock-report-resolved`, or `error-<class>`: `funded` with `list_wallets` failing with a sample of that `ErrorData` class (`none`, `duffs`, `rpc`, `retryAfter`, …; `src/fixture/errors.ts`) |
| `theme=light\|dark` | the launch theme |
| `fail=<command>:<class>` | that command rejects with a sample of the class (repeatable), e.g. `fail=history_page:rpc` |
| `delay=<ms>` | every command but `launch_info` answers after this long (loading states) |

Unknown names throw, so a typo shows an error rather than the default scenario.

The renderer has no DashPay screen yet (the DP tasks add them), so the `dashpay-*` scenarios are checked
at the command level for now (`src/fixture/fixture.test.ts`): every relation and channel state, requests in
both directions, registration in flight.
