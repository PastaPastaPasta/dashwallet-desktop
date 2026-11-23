# G-01: the Tauri spike — what was built, and Linux pre-measurements for G-02

Status: G-01 done on branch `dw/g-01-tauri-spike`, 2026-10-08. This document is the input to G-02 (the
full U1–U8 measurement on agentbox and the CI runners) and to ADR 0003. It records problems as plainly
as successes: a competing SwiftCrossUI measurement (G-03) runs on the same criteria afterwards.

Everything below was measured on **agentbox** (Ubuntu 26.04, 32 vCPU, no GPU, WebKitGTK 2.52.6) under a
private Xvfb display. agentbox is shared with other agents. During these runs the **1-minute load
average was 45–130 on 32 vCPUs**, so every timing below is an upper bound, and the load is recorded
beside each sample. G-02 should repeat the timings on a quiet box and on the CI runners.

Raw data: [`g-01-data/`](g-01-data/). The final run (all criteria, release build of the branch
head) is `linux-gate.json`, `linux-packages.json`, `u1.json`, `boxes-u3c.json`, `contrast.json`,
`atspi-summary.json` and the trimmed self-test reports. Some numbers in §2 are ranges over earlier runs
of the same day at different loads. Screenshots: [`docs/screenshots/gate/`](../screenshots/gate/).
How to rerun everything: [`apps/desktop/README.md`](../../apps/desktop/README.md).

## 1. Scope built (DASHPAY §2.1, "What the Tauri side builds")

| Item | Where | State |
|---|---|---|
| `apps/desktop`: Tauri 2, React, TypeScript, Vite | `apps/desktop` | done. React 19, TanStack Query 5, Vite 8, TS 7, Tauri 2.12.1 |
| `rust/crates/dw-app`: Tauri commands over dw-engine, in-process | `rust/crates/dw-app/src/commands.rs` | done |
| TypeScript types generated from the Rust records | `dw-app/src/dto.rs` → `apps/desktop/src/api/generated` (ts-rs 12) | done; `cargo test` fails when they are stale |
| Commands: open a network, list wallets, unlock, balance, sync status, history page, transaction detail | `open_network`, `list_wallets`, `unlock` (+ `lock`, `lock_status`), `balance`, `sync_status`, `history_page`, `tx_detail` | done |
| Screens, light and dark, per UX-SPEC §4.5 and §4.9: Overview, Transactions, transaction detail, unlock | `apps/desktop/src/screens` | done. Transactions has list mode only (table mode, export and the date/amount filters are not in the spike) |
| CSS custom properties generated from the token source `gen-tokens.swift` reads | `apps/desktop/scripts/gen-tokens.mjs` | done. Reads `Resources/Tokens/tokens.json` and the hand-written `Sources/DesignTokens/*.swift` |
| Event bridge: engine signals → query invalidation | `dw-app/src/bridge.rs` → `apps/desktop/src/api/events.ts` | done |
| TypeScript fixture backend for screenshot tests | `apps/desktop/src/api/fixture.ts` | done (variants `default`, `xss`, `empty`, `large` = 10,003 rows) |
| `--selftest`: render the four screens and write every `data-testid` box to JSON | `apps/desktop/src/app/selftest.ts` + `selftest_report` | done; it also writes the U3(a) style checks, a text-metrics probe, start-up marks, IPC timings and the U6 probes |

Beyond the scope (to give G-02 a head start): Playwright specs in Chromium and WebKit (screens, U3a style
checks, axe, keyboard, XSS), and a Linux gate harness (`apps/desktop/scripts/gate/`) that runs the real
app under Xvfb with the AT-SPI bus, and compares the screens with `dwcli` on a real datadir.

### Design choices worth reviewing

- **`dw-app` lives in the Rust workspace**, not in `apps/desktop/src-tauri`. That way it shares `Cargo.lock`
  and the pinned platform graph with dw-engine. A second workspace would need its own lock and a second
  copy of the platform build. `apps/desktop/scripts/tauri.mjs` points the Tauri CLI at it.
  Consequence: on Linux, `cargo build --workspace` now needs the WebKitGTK development packages
  (see §4.9).
- **ts-rs, not tauri-specta.** tauri-specta is still `2.0.0-rc.25`; ts-rs 12 is stable. The command
  wrappers in `src/api/tauri.ts` are written by hand (one line each, typed with the generated records).
- **Signals carry no rows.** An engine event becomes `engine://signal {kind, network, walletId}` and the
  renderer re-queries (the m1 rule). The engine already debounces to 4 Hz.
- **Duffs cross IPC as JSON numbers.** 21e14 is far below 2^53.
- **Formatting ported, not called.** `src/format/amount.ts` is a port of `dw-units`. It passes all
  3,600 dash-qt vectors in `testdata/amount_format.json`, so the fixture backend formats exactly like
  the engine would. Titles, chips and day grouping are ported from `Presentation.swift`.
- **Static Inter faces** (400/500/600/700, `@fontsource/inter`) are bundled for every platform, as UX-SPEC
  §2.2 asks for Linux and Windows. macOS gets Inter too, not SF Pro.

## 2. Pre-measurements against U1–U8 (Linux only)

"Not measurable here" means it needs the Windows or macOS runners (T-03).

| # | Criterion | Linux result | Verdict so far |
|---|---|---|---|
| U1 | Same balance and first 200 rows as `dwcli` on the same datadir | **Match**: the hero's full-precision tooltip equals the `dwcli` total to the duff (20,601,436,392,896); the floored hero reads `206 014.36`; 200/200 rows are identical (txid, record index, amount read back from the DOM). Wallet: the rust-dashcore regtest fixture `default`, 40,000 blocks, 40,051 transactions in Core, engine-mode app with an encrypted vault | **pass** (on a 40k-tx wallet; G-02 should repeat on its own 10k wallet) |
| U2 | Windows build, installer, launch, 10 min | not measurable here. `tauri.conf.json` targets MSI and NSIS | open |
| U3a | UX-SPEC §2 checklist per screen, computed-style assertions | **212/212 pass** in Chromium, WebKit (Playwright) and WebKitGTK (the real app), in light and dark (colour roles, radii, elevation, type ramp, spacing and layout) | **pass** on Linux engines; macOS/Windows webviews still to run the same self-test |
| U3b | Fidelity gap against the main spec | 0 checklist items short. Deliberate deviations: sheets use the Cross dialog radius 20 (macOS draws 16 itself); the modal sheet reuses the floating elevation (UX-SPEC has no web value for level 3) | recorded |
| U3c | Container boxes agree within 2 px across engines | **680 of 684** boxes agree; no container is missing in any engine. The 4 outliers are the two content-sized **bold** text containers on Overview (network capsule +7.9 px, hero amount +5.0 px wide) in WebKitGTK only. Cause: §4.5 | **fail** as measured (4 boxes) |
| U4 | AT-SPI: 0 unnamed interactive elements | **0 unnamed** on all four screens (17, 19, 3, 2 interactive). Driven entirely through AT-SPI `Action`: sidebar, a row, the named dialog "Transaction details", Close, Lock, Unlock | **pass** |
| U4 | axe: 0 serious/critical | **0 apart from `color-contrast`**. Contrast fails on 14 colour pairs, every one from the UX-SPEC palette (§4.1) | **fail as specified**; needs a decision (§4.1) |
| U4 | Keyboard only | Playwright: Tab order toolbar → sidebar → page, Enter opens a row's dialog, focus moves in, Escape closes and returns focus to the row, lock and unlock by keyboard (wrong passphrase announced) | **pass** |
| U4 | UIA walk on Windows | not measurable here | open |
| U5 | `history_page(200)` p95 ≤ 20 ms | IPC + serialisation of a 200-row page (synthetic `ipc_bench_rows`, an async command like the engine calls): **p50 3–8 ms, p95 5–18 ms**; one run at load 107 gave p95 62 ms. Real `history_page(200)` on the 40k wallet: **p50 379–968 ms, p95 595–2,141 ms**; `limit: 1` costs about the same (p50 735–764 ms), so the time is the engine rebuilding every record per call (§4.2) | IPC **pass**; real call **fail, engine-bound** |
| U5 | Cold start to first paint ≤ 1.5 s | Fixture, warm: **1.47–1.89 s** at moderate load; 1.53–2.68 s at load 50–60; 2.17–2.58 s at load 103–107. Cold after dropping caches: 3.1–3.8 s. Engine mode, 40k wallet: the window (splash) at 1.2–2.0 s; balance and history at 8.4–9.7 s at load 47–118, and 23.8–31.5 s at load 134. Breakdown in §4.3 | **fail on agentbox** (no GPU, loaded); recheck on a quiet box and the runners |
| U5 | PSS of all processes, idle, ≤ 400 MB | Fixture (no engine): **362–385 MB** = UI process 83–93 + WebKitWebProcess 259–272 + network process 20–21. Engine mode, 40k wallet, after browsing: 548–606 MB, of which the dw-app process (engine included) is 236–252 MB and WebKit 296–370 MB | UI cost **pass, with little margin**; engine share belongs to H-06 |
| U6 | Allowlist, CSP, navigation, XSS, unlisted command | From the real app's self-test:<br>• unlisted command `probe_unlisted` → "not allowed by ACL";<br>• `plugin:shell\|execute` → "not allowed by ACL";<br>• `fetch("https://example.com")` → refused, with a `connect-src` CSP violation event;<br>• `window.open` → `null`;<br>• a click on an external link → refused by the navigation guard ("still on the app origin"). Playwright: XSS payloads in a label, a contact name and the wallet name render as text, nothing executes | **pass**; secrets handling in §4.8 for H-01 |
| U7 | Linux `.deb` + AppImage install and launch | The `.deb` (19.3 MB) installs with apt; the installed `/usr/bin/dash-wallet` passes its fixture self-test; it uninstalls cleanly. The AppImage (101.1 MB) runs (extract-and-run) and passes the same self-test. Details in §3 | **pass** on Linux |
| U8 | macOS build, launch, screenshots | not measurable here | open |

## 3. Packaging (U7)

`apps/desktop/scripts/gate/linux-packages.sh` checks both bundles on agentbox. Result:
[`g-01-data/linux-packages.json`](g-01-data/linux-packages.json).

| Bundle | Size | Contents | Result |
|---|---|---|---|
| `Dash Wallet_0.1.0_amd64.deb` | 19,294,470 B | `/usr/bin/dash-wallet` (57 MB, release, thin LTO), icons, `Dash Wallet.desktop`; `Depends: libwebkit2gtk-4.1-0, libgtk-3-0` | `apt install` OK. The installed binary launches and passes its self-test (212/212 style checks, final report written). `apt remove` OK |
| `Dash Wallet_0.1.0_amd64.AppImage` | 101,054,968 B | bundles WebKitGTK, GTK and their libraries | runs with `APPIMAGE_EXTRACT_AND_RUN=1` (no FUSE needed). Self-test passes |

Not measured here: a Flatpak (DESIGN R2 picks Flatpak + tarball for Linux; Tauri has no Flatpak target, so a
manifest has to be written), Windows MSI/NSIS (U2) and the macOS `.app`/dmg (U8). The bundles are
unsigned, and the app icon is an upscaled 90 px logo: real icon artwork is a follow-up.

## 4. Problems found

### 4.1 The UX-SPEC palette fails WCAG AA contrast, so U4's axe check cannot pass on a faithful rendering

axe reports `color-contrast` (impact: serious) on every screen. Each failing pair is a palette pair the
spec prescribes, not a styling mistake (raw list: `g-01-data/contrast.json`):

| Pair | Ratio (AA needs 4.5:1 below 18.7 px bold / 24 px) | Used for |
|---|---|---|
| white on Dash blue `#008DE4` | 3.54:1 | selected sidebar row, selected filter chip, filled buttons |
| blue `#008DE4` on `blueAlpha10` | 3.15:1 light, 4.13:1 dark | status chips ("Pending", "InstantSend") |
| blue `#008DE4` on canvas `#F7F7F7` | 3.30:1 | plain blue buttons ("Filter") |
| `textTertiary` `#75808A` on white | 4.02:1 | weekday in day headers, captions |
| white on the hero strip (white 12 % over blue) | 2.21–3.04:1 | breakdown titles and subtitles |
| white 80 % on hero blue | 2.78:1 | lock-screen prompt |
| white on the orange capsule (90 %) | 2.46:1 | TESTNET capsule |
| orange `#FA9169` on white | 2.25:1 | "Locked" status on immature rewards |

The iOS app uses the same colours. SwiftCrossUI would draw the same pairs, but U4 runs axe only on
Tauri, so this criterion penalises Tauri alone, for a property of the spec. **Decision needed (manager
or UX-SPEC owner):** either judge axe without `color-contrast` for spec-mandated pairs (the Playwright
spec already writes the pairs to `contrast-*.json` and fails on every other rule), or add AA-compliant
variants to UX-SPEC (for example a darker blue for small text on light surfaces). The second option
would apply to both stacks.

### 4.2 `history_page` costs 0.4–1.2 s on a 40k-transaction wallet, all in the engine

`NetworkSession::wallet_records` rebuilds every dash-qt record of the wallet on each call: it
decomposes all transactions, joins labels and sorts. Only then does it page. A `limit: 1` call costs
as much as `limit: 200`, and the IPC share is 3–18 ms. On the gate's 10k wallet this should be roughly
a quarter (still well over 20 ms). The cost is the same under SwiftCrossUI (its FFI call reaches the
same function), so it does not separate the candidates. But **U5 cannot pass for either stack until the
engine keeps an incremental, sorted record index**. This belongs to H-06 or an engine task before G-02.
Related: opening the session on this wallet takes about 7–8 s (loading the history from
`wallet.sqlite`); the app now shows a splash meanwhile instead of a blank window.

Also seen, outside the UI's scope: the engine's balance for this wallet (206,014.36392896) is 150 DASH
below Core's `getbalance` for the same mnemonic (206,164.36392136). The SPV sync stopped in the
Masternodes phase (regtest), with the filter scan complete. Worth an engine issue.

### 4.3 Cold start is GTK/WebKit start-up, not the renderer

The Rust-side marks (`dw-app: startup …` on stderr) split a warm fixture start at load ≈ 60 like this:
`main` → Tauri `setup` 0.4–0.9 s; → web page load started 0.9–1.5 s; → load finished +40 ms; → React shell
frame +0.3 s; → Overview with data +0–0.5 s. The renderer's own share (script start to first frame) is
about 0.3–0.7 s. The rest is GTK and WebKit process start, with software rendering, because agentbox has
no GPU (`libEGL warning: DRI3 error`).

Two harness traps G-02 must avoid:
- Under `dbus-run-session`, the xdg-desktop-portal backends fail without `DISPLAY` in the D-Bus
  activation environment. The first launch then took 5.0 s.
  `dbus-update-activation-environment DISPLAY` fixes it (the harness does this).
- The load average distorts the numbers by ±1 s.

### 4.4 WebKitGTK's text metrics drift from Chromium and WebKit (U3c outliers)

The self-test's `textProbe` measures reference strings in each engine:

| Probe | Chromium | Playwright WebKit | WebKitGTK 2.52.6 |
|---|---|---|---|
| "30.39", Inter 400, 34 px | 92 | 92.55 | 92.22 |
| "30.39", Inter 700, 34 px | 98 | 98.48 | **103.22** |
| "TESTNET", Inter 700, 11 px | 50 | 50.9 | **57.92** |
| a 39-character sentence, Inter 400, 15 px | 274 | 272.18 | **261.65** |

In the distro's WebKitGTK, bold text is about 1 px per glyph wider and long regular text about 4 %
narrower. Playwright's WebKit is also a Linux WebKit port (a newer trunk) and matches Chromium, so this
looks specific to the WebKitGTK 2.52.6 build (font back-end or hinting settings).

Tried without effect:
- the variable Inter font replaced by static faces (kept: one face per weight is the cleaner choice
  anyway);
- `text-rendering: geometricPrecision`;
- a fontconfig override with hinting off.

Only content-sized containers move. Every box whose width comes from layout matches within 2 px. G-02
options: re-measure on a desktop session (GNOME's font settings, GPU); judge U3c on layout containers
only; or give these two containers a layout width.

### 4.5 Memory is dominated by the web process

In fixture mode, WebKitWebProcess alone is about 260 MB of the 362 MB total PSS. With software
rendering, after scrolling 200 rows and opening the detail, it grows to 330–350 MB. The budget is
400 MB, so the margin is thin. With a GPU, the web process normally moves rendering memory elsewhere.
G-02 should measure on the runners too. Virtualising the Transactions list (it currently keeps every
loaded page in the DOM) would cap the growth on large wallets; it is a follow-up.

### 4.6 Build cost

- Cold release build of the whole app: 15 min 41 s (agentbox, loaded; it includes the platform graph).
- Incremental release relink after a change in `dw-app` or the renderer: 3–4 min (thin LTO).
- The dev loop is fast: `pnpm dev` (Vite hot reload, fixture backend, any browser) and incremental
  `cargo build -p dw-app` (dev profile) in seconds.

### 4.7 Accessibility gaps outside the pass criteria

- The AT-SPI application name is the binary name, `dash-wallet`, not "Dash Wallet".
  `glib::set_application_name` did not change it; GTK 3 uses the program name, which also sets
  `WM_CLASS`, so it needs care with the `.desktop` file.
- WebKitGTK refuses `EditableText` on the password field. Typing works, and Orca types, so assistive
  technology is unaffected. A test driver has to use keystrokes.
- Every list row is a button named by its full content ("Received 1:40 AM Pending +0.20 tDASH"). That
  reads well. Large lists will want `aria-setsize`/`posinset` once virtualised.

### 4.8 For H-01: how the renderer handles secrets

- The passphrase goes DOM `<input type=password>` → JS string → IPC JSON → Rust `String`, which is
  wrapped in `Zeroizing` at once and zeroed after `Vault::unlock`. The DOM value is cleared after each
  attempt. **The JS string, the IPC request body and serde's intermediate buffers cannot be zeroed**;
  they live until garbage collection or reuse.
- The lock screen follows the vault's throttle (`retryAfterSecs` on `vault.wrong_passphrase` and
  `vault.throttled`) and disables Unlock until the wait ends. The iOS "N attempts remaining" copy was
  dropped, because the vault enforces no attempt limit; whether it should is a question for H-01.
  This is inherent to a webview. The recovery-phrase reveal (not in this spike) should be rendered by
  the engine into an image or a native view, not passed as text.
- While locked, the shell stays mounted under the lock screen. It is `inert`, so no focus and no AT,
  but the balance and history remain in the DOM. If that matters for privacy, unmount the shell when
  locked.
- Measurement commands are granted in production builds: `startup_mark`, `selftest_report` (it writes
  only when launched with `--selftest FILE`, to that path), `selftest_checkpoint` and `ipc_bench_rows`
  (synthetic data). None reaches wallet data. H-01 may still want them behind a `measure` feature.
- `probe_unlisted` exists only to prove the ACL refuses it. It must never be added to a capability.

### 4.9 Repository and CI impact

- Building `dw-app` needs `libwebkit2gtk-4.1-dev` and friends (list in `apps/desktop/README.md`), so the
  Linux CI image (`ci/linux/Dockerfile`) needs them before `cargo test --workspace` runs there. That is
  T-03's job; this branch does not change the image.
- `Cargo.lock` gained 159 packages. Of the existing ones only build-time `toml` crates moved
  (`serde_spanned`, `toml_datetime`, `toml_parser`, `toml_writer`). The dashcore and platform graph is
  unchanged.
- Node 24 + pnpm 12 are new toolchain requirements. `pnpm-workspace.yaml` carries pnpm 12's
  minimum-release-age exclusion for Playwright 1.64.0 (it was released within the age window).

### 4.10 Small spec questions met on the way

- UX-SPEC §5.2 says the hero is floored to the Decimal digits setting (default 2, so `30.39`). The
  reference `home-light.png` shows `30.39698287`. The spike follows the text.
- The gap between day-group cards is `DashSpacing.stack` (15), which matches the reference image.
  `sectionGap` (24) is used between sections.

## 5. Review

The branch went through `code-review-validator` and `code-simplifier` before handover. Fixed from the review:
- `ipc_bench_rows` overflowed `u64` in debug builds (a panic on the main thread);
- a failed next page could loop fetching forever (it now stops and offers a retry);
- every history signal refetched every loaded page (lists are now cut back to the first page first);
- `--selftest` hung when Overview never painted (it now times out);
- the lock screen ignored the vault's throttle;
- U1 compared the balance only to 2 decimals;
- SIGTERM skipped the engine shutdown (signals now exit through `RunEvent::Exit`);
- an SPV start failure under `--sync` stranded the user on the splash (the session now stays open, and
  the splash has a retry);
- a one-frame unlocked flash before the lock state was known;
- the status-bar live region announced every percent;
- the detail sheet did not make the shell inert;
- day groups were region landmarks;
- internal moves showed a minus sign;
- the received-address fallback could pick the sender's change;
- the U6 fetch probe could not tell CSP from no network;
- `window.open` was not probed;
- sync commands distorted the IPC timing;
- missing boxes did not fail U3c.

The simplifications applied: the `mirror_enum!` macro, ts-rs `with_large_int`, one theme enum, shared
logo and capsule components, and dead variants removed.

Known and not fixed:
- undated (mempool) rows group at the bottom, as in Swift `HistoryDay.group`;
- internal rows lack the circular-arrows accessory;
- the measurement commands ship in production builds (see §4.8);
- the Transactions list is not virtualised (see §4.5).

## 6. What G-02 still has to do

- U2, U8, U3 on macOS and Windows: build the same branch on the runners. Run
  `dash-wallet --fixture --network testnet --selftest out.json` there and compare its boxes with
  `apps/desktop/scripts/gate/compare_boxes.py`. Take screenshots, and run the UIA walk on Windows.
- Repeat U5 timings on a quiet agentbox and on the runners (the harness records the load).
- U1 on G-02's own 10k regtest wallet: `linux-gate.sh OUT --datadir … --network regtest …`, then
  `u1_compare.py`.
- Get decisions on §4.1 (contrast) and §4.2 (engine history index) before scoring U4 and U5.
