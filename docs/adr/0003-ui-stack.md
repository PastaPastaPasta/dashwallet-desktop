# ADR 0003 — The desktop UI stack (gate G-UI)

- Status: **draft**. G-02 fills the Tauri column, G-03 the SwiftCrossUI column; the manager decides in
  G-04 (DEC-12, DEC-90) under DASHPAY §2.1's rule.
- Gate definition: [`DASHPAY.md` §2.1, "Gate G-UI"](../design/DASHPAY.md) (criteria U1–U8), with the
  G-01 rulings of DEC-36: U4 is judged on UX-SPEC §2.1a's WCAG AA colours; U5 separates the UI's own
  cost from the engine's.
- Tauri inputs: the G-01 spike ([report](../research/g-01-tauri-spike.md)), measured by G-02 on branch
  `dw/g-02-tauri-measure`. Raw data: [`docs/research/g-02-data/`](../research/g-02-data/).

## Results

<!-- G-02 fills the Tauri column, G-03 replaces only the X_… tokens in the SwiftCrossUI column (agreed
2026-10-11). Each side's detail and raw data stay in its own section below. -->

Verdicts apply DASHPAY §2.1's thresholds and DEC-36's rulings. For U5, DEC-36 separates the UI's own
cost from the engine's, so a U5 cell can read "**pass (UI)** / **fail (engine)**". Machines: agentbox
(32 vCPU, no GPU, shared, load 80–216 during these runs), and the GitHub-hosted `ubuntu-latest`,
`windows-latest` and `macos-latest` runners. Each side's method section lists the runner specs.

| # | Criterion (pass) | Platform | Tauri (G-02) | SwiftCrossUI (G-03) |
|---|---|---|---|---|
| U1 | Real engine: balance and first 200 rows identical to `dwcli` | Linux | 20,601,436,392,896 duffs in both; 200/200 rows identical (txid, record index, amount); 40k-tx regtest wallet. **pass** | X_U1_LINUX |
| U2 | Build, install, launch, open the wallet, screenshot; no crash in 10 min | Windows | The NSIS installer (`/S`, 14.8 MB) installs. The installed app opens the 40k wallet on its lock screen. It is unlocked through UIA (ValuePattern, then Invoke on Unlock), after which its Lock button is enabled and the passphrase field is gone. It runs 608 s without exiting. Private bytes stay at 195–203 MB over 20 samples (7 processes); the working set falls from 432 to 117 MB as Windows trims idle pages. Screenshots: locked, unlocked, after 10 min. **pass** | X_U2_WINDOWS |
| U3a | UX-SPEC §2 checklist per screen (the candidate's column) | Linux | 212/212 computed-style checks in WebKitGTK (the app), Chromium and WebKit (Playwright); 160/160 on the real 40k wallet. **pass** | X_U3A_LINUX |
| U3a | | Windows | 212/212 in WebView2 153 (the app), 160/160 on the 40k wallet. **pass** | X_U3A_WINDOWS |
| U3a | | macOS | 212/212 in WKWebView (Safari 26.6.2, the app), 160/160 on the 40k wallet. **pass** | X_U3A_MACOS |
| U3b | Fidelity gap against the main spec (recorded) | all | **0** items short. Deliberate choices: the sheet radius is 20 (the Cross dialog value), and the modal reuses the floating elevation | X_U3B |
| U3c | Container boxes within 2 px across engines | all | 680 containers (plus 4 text-run boxes) per engine. **Linux:** WebKitGTK on agentbox and on CI, Chromium and WebKit at 1000×620 agree within **1.55 px**. **pass**. **macOS:** WKWebView at 1000×588, with overlay scroll bars, agrees with Chromium and WebKit within **1.0 px** on every box, text runs included. **pass**. **Windows:** WebView2 at 1000×620 differs on 578 boxes, all by its **15 px classic scroll bar**: 520 boxes are 15 px narrower and the rest shift 7.5 or 15 px. Nothing else differs by more than 1 px. The macOS runner's classic scroll bars give the same pattern (17 px). **fail as written (the scroll bar only)**; the manager rules. WebKitGTK sets bold text runs 5–8 px wider than the other engines; this is recorded, and U3c excludes text runs | n/a (Tauri only) |
| U4 | 0 unnamed interactive elements; the walk completes | Linux (AT-SPI) | 0 unnamed (17, 19, 3 and 2 interactive); the whole flow is driven through AT-SPI Actions. axe: 0 serious/critical **including `color-contrast`** on the §2.1a palette (4 screens × light/dark × Chromium/WebKit). **pass** | X_U4_LINUX |
| U4 | UIA walk completes and names every control | Windows (UIA) | 0 unnamed interactive elements on all 4 screens (22, 28, 9 and 2 interactive). The whole flow runs through UIA Invoke and ValuePattern: Transactions, a row, the dialog (a named 'Transaction details' dialog), Close, Lock, the passphrase, Unlock. WebView2's hidden browser views, which sit under parents with no box, are left out, as AT-SPI's `showing` state leaves them out on Linux. **pass** | X_U4_WINDOWS |
| U4 | Every action reachable by keyboard | all | Tab order: toolbar → sidebar → page; Enter opens a row, Escape closes and returns focus; lock and unlock by keyboard. **pass** | X_U4_KEYBOARD |
| U5 | `history_page(200)` p95 ≤ 20 ms | Linux | The UI's share (dispatch, DTO mapping, serialisation, IPC) has p95 **1.9 ms** on CI Ubuntu and **9.4–10.9 ms** on agentbox (load 121–200); 2.9 ms on macOS and 5.2 ms on Windows. The full call's p95 is 61 ms on CI and 421–510 ms on agentbox; of that, the engine takes 60 and 415–503 ms (E-HIST). **pass (UI)** / **fail (engine)** | X_U5_HISTORY |
| U5 | Cold start to first paint ≤ 1.5 s (fixture Overview with balance and history) | Linux | Spawn to first paint on CI Ubuntu: **0.57–1.08 s** cold (page cache dropped) and **0.22–0.72 s** warm, with 0.26–0.33 s of CPU. On agentbox, warm at load 120–122: **1.20–1.78 s** (2 of 5 starts under 1.5 s), with 0.94–1.09 s of CPU. On the 40k wallet: 2.06–2.07 s on CI and 5.9–8.9 s on agentbox; the engine's own wallet open (`dwcli list`) takes 5.2–9.2 s on agentbox. U5 is measured on agentbox, and there it **fails as measured**: 3 of 5 starts are over 1.5 s under shared load. Its CPU time is under 1.1 s, and it **passes** on the quiet runner, cold and warm. A ruling on agentbox's load is needed. **fail (engine)** for wallet open | X_U5_COLD |
| U5 | PSS of all processes, idle, ≤ 400 MB | Linux | The fixture (the UI alone): **194.7 MiB** on agentbox and **257.1 MiB** on CI. The 40k wallet in total: 403.0 MiB on agentbox and 460.2 MiB on CI. The engine's share is 186.3 / 186.1 MiB, which leaves the UI 216.7 / 274.1 MiB. **pass**: the gate judges the UI's own cost. The 40k total exceeds 400 MB because of the engine's share, which the gate assigns to H-06 | X_U5_PSS |
| U6 | Allowlist, CSP, navigation, XSS, unlisted command | all | Probed in the real app on Linux, Windows and macOS. The ACL refuses an unlisted command and a plugin command outside the capability. CSP (`connect-src`) blocks a remote `fetch`. `window.open` returns null. An external link leaves the app on its origin. In Playwright, XSS payloads in a label, a contact name and the wallet name render as text. **pass** | n/a (Tauri only) |
| U7 | Each package installs (or unpacks) and launches | Linux | `.deb` (23.6 MB) installs with apt; the installed app passes its self-test; it removes cleanly. The AppImage (104.9 MB) runs and passes its self-test. **pass** | X_U7_LINUX |
| U7 | | Windows | The NSIS installer (14.8 MB) and the MSI (20.9 MB) install silently (exit 0, 3 s and 2 s). Both installed apps launch with a window; the NSIS app also passes U2 and the UIA walk. **pass** | X_U7_WINDOWS |
| U7 | | macOS | The dmg (21.6 MB) verifies and mounts. The `.app`, copied to `~/Applications`, launches with the fixture (light and dark) and with the 40k wallet (its lock screen). **pass** | X_U7_MACOS |
| U8 | Build, launch, every screen in light and dark; no crash | macOS | The self-test on `macos-latest` captures all 4 screens in light and dark (8 window captures), passes 212/212 checks and does not crash; the dmg-installed app launched 3 times. **pass** | X_U8_MACOS |

- **Tauri:** U1, U2, U3a, U3b, U4, U6, U7 and U8 pass on every platform measured. U3c passes on
  Linux and macOS; on Windows the only difference is WebView2's 15 px classic scroll bar, which
  needs a ruling. U5: the UI's own costs pass (UI share of `history_page(200)` p95 ≤ 10.9 ms;
  fixture PSS 195–257 MiB; cold start ≤ 1.08 s on a quiet runner). The engine-bound costs miss their
  thresholds until E-HIST lands: `history_page` takes 60–503 ms and wallet open 2–9 s. The engine's
  186 MiB of PSS belongs to H-06. Cold start on agentbox, where U5 is measured, is over 1.5 s in 3
  of 5 starts under shared load (1.20–1.78 s at load 121); that needs a ruling.
- **SwiftCrossUI:** X_SUMMARY

## Tauri: method and raw data (G-02)

Measured by G-02 on branch `dw/g-02-tauri-measure` (Tauri 2.12, React 19, dw-app in-process over
dw-engine at desktop main 85ab16e). Raw data: [`docs/research/g-02-data/`](../research/g-02-data/).
G-01's spike report ([`g-01-tauri-spike.md`](../research/g-01-tauri-spike.md)) describes the app.

### Machines

| Where | CPU | Cores / RAM | System | Webview | Load during the runs |
|---|---|---|---|---|---|
| agentbox | Proxmox VM, AVX2, no GPU | 32 vCPU / 160 GB | Ubuntu 26.04, kernel 7.0 | WebKitGTK 2.52.6 | 1-min load 80–216 (shared with other agents) |
| `ubuntu-latest` | AMD EPYC 7763, 9V74 or 9V45 | 4 / 16 GB | Ubuntu 24.04.5 | WebKitGTK 2.52.6 | 1–3 |
| `windows-latest` | AMD EPYC 7763 or 9V74 | 4 / 16 GB | Windows Server 2025 (10.0.26100) | WebView2 153.0.4234.48 | n/a |
| `macos-latest` | Apple M1 (virtual) | 3 / 7 GB | macOS 26.6.2 | WKWebView (Safari 26.6.2) | 5–35 |

Each runner's `runner.json` is next to its data. All builds are release builds (`pnpm tauri build`,
thin LTO).

### Test data

The regtest fixture is `ci/gate/fixtures/regtest40k-dw.tar.gz` (16.4 MB), and
`ci/gate/make-regtest-fixture.sh` rebuilds it:

- **Source:** rust-dashcore's public regtest-40000 chain. Its wallet `default` has 40,153 transactions
  in Core.
- **How it was made:** restored with `dwcli` into an encrypted vault (public test passphrase) and
  scanned over SPV to height 40,000 on agentbox. The scan took 10 min.
- **How the app opens it:** offline, with `--no-platform`, so the runners need no node.

It is four times the gate's 10k minimum; DASHPAY §2.1 allows a larger wallet. G-03 measures on the
same file.

### How each criterion was measured

- **U1.** `apps/desktop/scripts/gate/linux-gate.sh`:
  - The app's `--selftest` reads the hero's full-precision tooltip and the first 200 rows (txid, record
    index, amount) back from the DOM.
  - `u1_compare.py` compares them with `dwcli list` and `dwcli history --limit 200` on the same
    datadir.
  - The rows are identical on agentbox, `ubuntu-latest`, `macos-latest` and `windows-latest`: all four
    `selftest-*-engine.json` reports carry the same `shown` block.
- **U2.** `ci/gate/tauri-windows.ps1`:
  - The NSIS installer (`/S`) installs the app, and the installed app opens the copied wallet.
  - Through UIA, the gate checks that the lock screen is shown, sets the passphrase (ValuePattern),
    invokes Unlock and checks that the wallet opened: the Lock button is enabled and the passphrase
    field is gone (`lockScreenShown`, `typedWith`, `unlocked` in `windows-gate.json`).
  - It is screenshotted after unlock and again after 10 min, with working set and private bytes sampled
    every 30 s. A pass means the process never exited.
- **U3a.**
  - The self-test runs the UX-SPEC §2 checklist as computed-style assertions on every screen in light
    and dark: 212 checks on the fixture, 160 on the engine wallet (no unlock screen).
  - It runs in the real app (WebKitGTK, WKWebView, WebView2) and in Playwright (Chromium, WebKit).
- **U3c.** `compare_boxes.py` compares every `data-testid` container box against WebKitGTK:
  - All engines run at one viewport, **1000×620**, set with `--window-size` or `DW_VIEWPORT`. The
    runners' desktops are 1024×768, too small for the default 1280×820 window.
  - macOS keeps the window clear of the menu bar and Dock, so WKWebView gets 1000×588. It is compared
    with Chromium and WebKit run at 1000×588. WebKitGTK cannot go below its 600 px minimum height, so
    it is not in that comparison.
  - The macOS runner has no trackpad, so by default it draws classic scroll bars 17 px wide. The gate
    sets `AppleShowScrollBars=WhenScrolling`, the trackpad default, and records the setting in
    `macos-scrollbars.txt`.
  - Windows has no overlay setting for WebView2's scroll bars, so it runs as users see it.
  - `compare_boxes.py` refuses two reports taken at different viewports. It reports the largest
    container delta (`maxContainerDelta`) and groups the deltas over tolerance by pattern
    (`overPatterns`), so one cause that moves many boxes shows as one line.
  - The comparisons are in `u3c/`, with the Playwright reports they use.
  - The hero amount and the network capsule are boxes sized by their glyphs. U3c excludes text runs, so
    `textRuns` reports them apart, and `passAllBoxes` (text runs included) is false.
- **U4.**
  - `atspi_walk.py` on Linux and `uia_walk.ps1` on Windows dump each screen's accessibility tree, count
    the interactive elements without a name, and drive the whole flow through the accessibility API:
    Transactions, a row, the dialog, Close, Lock, then Unlock.
  - Only shown elements count. On Linux that is AT-SPI's `showing` state.
  - On Windows an element is shown when it and all its ancestors are on screen and have a box.
    WebView2 also exposes its hidden browser views (an App bar, caption buttons, a tab strip) under
    parents with no box.
  - `uia-summary.json` records how many interactive elements were skipped (`hiddenSkipped`).
  - axe-core runs in Playwright on 4 screens × 2 appearances × 2 engines, with `color-contrast` on.
  - A Playwright keyboard-only test covers the same flow.
- **U5.** U5 is measured on agentbox, as DASHPAY §2.1 asks, and the verdicts are agentbox's. The
  runners' numbers are recorded next to them: on a quiet machine they show what agentbox's shared
  load adds.
  - **Cold start:** `first-paint` is the first Overview frame that shows the balance and history.
    - It is counted from process spawn, over 5 starts of the fixture build.
    - The app logs `before-run`, its process's age when `run` starts. It comes from the kernel's
      start time and `/proc/uptime` (CLOCK_BOOTTIME, 10 ms steps), and covers exec and dynamic
      linking.
    - It also logs `first-paint` from `run`. Spawn to first paint is their sum.
    - The harness's own polled time is kept as `harnessSawFirstPaintMs`. It is up to one poll round
      late (about 100 ms on a quiet runner).
    - The CPU time of the process tree is read from `/proc` when the harness sees the first paint.
    - On the CI runner, 8 starts are taken. Before each of the first 4 the gate drops the page cache
      (`DROP_CACHES=4`), so those 4 are cold starts and the other 4 are warm. agentbox is shared,
      so the gate never drops its caches there, and every agentbox start is warm.
    - `uptime` is recorded with every agentbox run.
    - The 40k-wallet starts (3 per run) are recorded, together with the engine's own wallet open time,
      `dwcli list` on the same datadir.
  - **`history_page(200)`:** `history_page_timed` returns the engine's in-process time with each page.
    The UI's cost is the IPC round trip minus that time: 200 calls after 20 warm-ups.
  - **PSS:**
    - It is summed over the app's process tree (`/proc/*/smaps_rollup`): after 20 s idle on the
      fixture, and after 30 s idle on the wallet, after browsing.
    - The raw data is in KiB; the table gives MiB.
    - The fixture run is the UI's own cost.
    - On the 40k wallet, the engine's share is the `dash-wallet` process's PSS in engine mode minus
      its PSS in fixture mode.
- **U6.** The self-test probes run inside the real app on every OS:
  - an unlisted command;
  - a plugin command outside the capability;
  - `fetch` of a remote URL;
  - `window.open`;
  - a click on an external link.

  Playwright checks XSS payloads in a label, a contact name and the wallet name.
- **U7.**
  - Linux: `linux-packages.sh` installs the `.deb` with apt, self-tests the installed binary and removes
    it, then runs the AppImage (extract-and-run) and self-tests it.
  - Windows: the NSIS installer (`/S`) and the MSI (`msiexec /qn`) install, and the MSI's app launches.
  - macOS: the dmg mounts, the `.app` is copied to `~/Applications`, and it launches with the fixture
    (light and dark) and with the wallet.
- **U8.** On `macos-latest`, the self-test with a window capture of each screen in light and dark,
  plus the dmg-installed app's launch captures.

### Deviations from the gate text, and why

1. **No tauri-driver.**
   - WebDriver for WKWebView does not exist, and tauri-driver on WebKitGTK and WebView2 only drives
     what the app's `--selftest` already does in-process: render each screen and read styles and
     boxes.
   - The self-test runs inside the shipped binary on all three OSes. That is the stronger evidence,
     because it is the real webview with the real fonts.
2. **A 40k-transaction wallet** instead of 10k (allowed: "at least 10,000").
3. **The WCAG AA palette (DEC-36)** is in UX-SPEC §2.1a, in `Roles.swift` (`RoleAA`, `DashOpacityAA`),
   in the generated CSS, and pinned by a Swift test. Both stacks use it. The ratios are in UX-SPEC
   §2.1a.
4. **The renderer choice on Linux without a GPU** (`dw_app::webkit_renderer_without_gpu`).
   - With no `/dev/dri/renderD*` device, the app sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` itself
     before GTK starts. The harnesses never set it.
   - WebKitGTK's DMA-BUF renderer falls back to slow software rendering there. The fallback costs
     about 1 s of first paint and about 170 MB of PSS (interleaved runs in
     `linux/renderer-variants.txt`).
   - Machines with a GPU are unaffected.
5. **U3c at 1000×620**, not the configured 1280×820, for the reason given under U3c.
6. **The U5 split (DEC-36).** E-HIST has not landed. `history_page` and wallet open are therefore
   engine-bound and are reported apart from the UI's cost (see below).

### Findings that bear on the decision

- **Engine-bound costs (both stacks).**
  - On the 40k wallet, `history_page(200)` costs p50 87–330 ms in the engine, depending on the machine.
    `limit: 1` costs the same, so the engine rebuilds every record per call.
  - Wallet open takes 5.2–9.2 s on agentbox in `dwcli` alone.
  - Both belong to E-HIST, which DEC-36 meant to land before G-02 scored U5. It has not, so the
    full-call numbers are not a UI verdict.
- **E-BAL.** The fixture's balance in the engine (206,014.36392896) is 150 DASH below Core's
  (206,164.36392136). U1 compares the app with `dwcli` on the same datadir, so it is unaffected. The
  E-BAL fix is not on the desktop pin yet.
- **Cold start on agentbox is load-bound.**
  - The UI's own start costs 0.94–1.09 s of CPU on agentbox. On a quiet 4-core runner it costs
    0.26–0.33 s of CPU and 0.22–0.72 s of wall clock warm.
  - At load 120–122 on 32 vCPUs, the wall clock on agentbox runs 1.20–1.78 s.
  - Two earlier agentbox runs were timed by the harness's polling, which is up to one poll round late:
    1.40–2.27 s at load 212–216, and 0.95–2.5 s at load 87–91 on the wall clock.
  - The 1.5 s threshold holds, cold and warm, on a quiet machine. It does not hold reliably on agentbox
    under load.
- **First launch on fresh runners is slow.**
  - macOS: first paint at 3.1–6.7 s. Windows: 4.4 s, of which 3.8 s comes before Tauri's setup hook.
  - Both runners launched a freshly built, unsigned binary for the first time, under the OS's first-run
    scanning.
  - These are information, not U5 verdicts.
- **Memory on Windows.** Over the 10-minute U2 run, private bytes stayed flat at 189–203 MB (7
  processes; four runs). The working set fell from about 430 to 116 MB as Windows trimmed idle
  pages. There is no leak trend.
- **Scroll bars (U3c).**
  - WebView2 always draws classic 15 px scroll bars. The macOS runner draws 17 px ones because it
    has no trackpad.
  - Playwright's engines, WebKitGTK's overlay scroll bars and macOS with a trackpad take no layout
    width. So on Windows the scroll pane's content is 15 px narrower, and every box in it narrows or
    shifts.
  - If the manager wants U3c to hold on Windows as written, the app can draw its own fixed-width
    scroll bar (`::-webkit-scrollbar`) in every engine. Then every engine reserves the same gutter.
    That is a design change (UX-SPEC), so G-02 did not make it.
- **Accessibility gaps outside the pass criteria.**
  - AT-SPI reports the application as `dash-wallet`, not "Dash Wallet". GTK 3 takes the program name.
  - On Windows the window title is "Dash Wallet — Testnet"; the network is in the title by design.
  - WebKitGTK refuses `EditableText` on the password field. Keystrokes work, and Orca types normally.
- **Fidelity.** WebKitGTK sets Inter's weight-700 text wider than Chromium, WebKit and WKWebView,
  which agree with each other within 1 px:
  - The self-test's text probe measures `caps700` at 57.9 against 50.0–50.9 px and `amount700` at
    103.2 against 98.0–98.5 px. Weight 400 matches (92.2 against 92.0–92.6 px).
  - All weights load from the app's bundled static Inter files (`interFontLoaded`).
  - The bold text runs (hero amount, network capsule) are therefore 5–8 px wider on Linux.
  - Container boxes agree. The cause, hinting or advance rounding in WebKitGTK's font stack, was not
    investigated.
- **H-01 notes.** These are unchanged from G-01 §4.8: the passphrase goes through a JS string and the
  IPC body, neither of which can be zeroed, and measurement commands are present in release builds.
- **Build cost.**
  - Release builds: 6–25 min on the runners (the Rust cache warm or cold), 9–21 min on agentbox at
    load 110–200.
  - Bundles: `.deb` 23.6 MB, AppImage 104.9 MB, NSIS 14.8 MB, MSI 20.9 MB, dmg 21.6 MB.

## SwiftCrossUI: method and raw data (G-03)

*(G-03)*

## Decision (G-04)

*(the manager)*
