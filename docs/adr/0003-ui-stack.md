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
| U1 | Real engine: balance and first 200 rows identical to `dwcli` | Linux | 20,601,436,392,896 duffs in both; 200/200 rows identical (txid, record index, amount); 40k-tx regtest wallet. **pass** | 20,601,436,392,896 duffs in both; 200/200 rows identical (txid, record index, type, signed amount, status, confirmations, address); unlocked through AT-SPI; agentbox and CI Ubuntu. **pass** |
| U2 | Build, install, launch, open the wallet, screenshot; no crash in 10 min | Windows | The NSIS installer (`/S`, 14.8 MB) installs. The installed app opens the 40k wallet on its lock screen. It is unlocked through UIA (ValuePattern, then Invoke on Unlock), after which its Lock button is enabled and the passphrase field is gone. It runs 608 s without exiting. Private bytes stay at 195–203 MB over 20 samples (7 processes); the working set falls from 432 to 117 MB as Windows trims idle pages. Screenshots: locked, unlocked, after 10 min. **pass** | WinUIBackend, release, on windows-latest (4 vCPU, 16 GB). The zip unpacks and launches with the Swift toolchain off PATH. GateProbe's hook unlocks the 40k wallet; Overview paints at 8.8 s, and the app is still up at 612 s (peak working set 245 MB). This needs the 8 MB main-thread stack: with Windows' default 1 MB, every screen after the unlock died with a stack overflow (0xC00000FD). GtkBackend (the U4 fallback) also passes: Overview at 11.8 s, still up at 601 s, peak working set 212 MB. **pass** (with the stack fix) |
| U3a | UX-SPEC §2 checklist per screen (the candidate's column) | Linux | 212/212 computed-style checks in WebKitGTK (the app), Chromium and WebKit (Playwright); 160/160 on the real 40k wallet. **pass** | 56/120 checks (lock 6/11, Overview 8/18, Transactions 7/15, detail 7/16, in each appearance). Colours, Inter and the type scale are exact; layout and components miss (no header row, detail inline not a sheet, toolkit text fields, bracketed amounts, clipped labels). **fail** |
| U3a | | Windows | 212/212 in WebView2 153 (the app), 160/160 on the 40k wallet. **pass** | WinUIBackend: 48/104 checks (lock 4/9, Overview 8/17, Transactions 7/14, detail 6/12 light and 4/12 dark; 16 not counted because the un-maximized window runs off the 1024×768 screen). Colours and sizes are exact, but the face is Segoe UI, not Inter; plus Linux's layout misses, WinUI's default sidebar selection (white on #EEEEEE) and text fields, and a dark detail shot that shows no detail. **fail** |
| U3a | | macOS | 212/212 in WKWebView (Safari 26.6.2, the app), 160/160 on the 40k wallet. **pass** | 51/120 checks: Linux's misses, plus colours shifted from the sRGB tokens (hero #0079C3 for #0063B7; links and "Locked" under 4.5:1) and a grey sidebar selection with white text. **fail** |
| U3b | Fidelity gap against the main spec (recorded) | all | **0** items short. Deliberate choices: the sheet radius is 20 (the Cross dialog value), and the modal reuses the floating elevation | **18** items short and not permitted by the Cross column (header row, detail sheet, text fields, lock-screen controls, amount anatomy, breakdown strip, dates, macOS colours, the WinUI sidebar selection…); 10 permitted ones (no shadows, circular corners, no motion, toolkit pickers…) |
| U3c | Container boxes within 2 px across engines | all | 680 containers (plus 4 text-run boxes) per engine. **Linux:** WebKitGTK on agentbox and on CI, Chromium and WebKit at 1000×620 agree within **1.55 px**. **pass**. **macOS:** WKWebView at 1000×588, with overlay scroll bars, agrees with Chromium and WebKit within **1.0 px** on every box, text runs included. **pass**. **Windows:** WebView2 at 1000×620 differs on 578 boxes, all by its **15 px classic scroll bar**: 520 boxes are 15 px narrower and the rest shift 7.5 or 15 px. Nothing else differs by more than 1 px. The macOS runner's classic scroll bars give the same pattern (17 px). **fail as written (the scroll bar only)**; the manager rules. WebKitGTK sets bold text runs 5–8 px wider than the other engines; this is recorded, and U3c excludes text runs | n/a (Tauri only) |
| U4 | 0 unnamed interactive elements; the walk completes | Linux (AT-SPI) | 0 unnamed (17, 19, 3 and 2 interactive); the whole flow is driven through AT-SPI Actions. axe: 0 serious/critical **including `color-contrast`** on the §2.1a palette (4 screens × light/dark × Chromium/WebKit). **pass** | 0 unnamed in all 8 walks (lock 7, Overview 32, Transactions 133, detail 139 interactive), on agentbox and CI Ubuntu; unlock driven through AT-SPI. Transactions rows turn defunct in the tree once focused. **pass** |
| U4 | UIA walk completes and names every control | Windows (UIA) | 0 unnamed interactive elements on all 4 screens (22, 28, 9 and 2 interactive). The whole flow runs through UIA Invoke and ValuePattern: Transactions, a row, the dialog (a named 'Transaction details' dialog), Close, Lock, the passphrase, Unlock. WebView2's hidden browser views, which sit under parents with no box, are left out, as AT-SPI's `showing` state leaves them out on Linux. **pass** | WinUIBackend: `FindAll` over the window fails with E_UNEXPECTED on the lock screen, Overview and Transactions alike, and the app then exits with 0xC0000005 (swift-cross-ui#787). The backend logs that it does not support accessibility labels. **fail**; GtkBackend fallback: GtkBackend (run 38135678506): `FindAll` returns **0 elements** on the lock screen, Overview and Transactions, and the app stays up. gvsbuild's GTK 4.22.4 is built without AccessKit, GTK's only accessibility backend on Windows, so the window has no UIA provider. **fail** on both backends |
| U4 | Every action reachable by keyboard | all | Tab order: toolbar → sidebar → page; Enter opens a row, Escape closes and returns focus; lock and unlock by keyboard. **pass** | Linux: unlock (Return) and the overlay's Hide (Tab, Space) work, but Tab into the sidebar selects each row it passes, so Overview's 9 controls (shortcuts, Hide balance, Filter, rows, See all) are never reached; Load more and opening a row's details are not reachable. macOS and Windows not driven. CI's Xvfb has no window manager, so the window never takes keyboard focus there; agentbox's run is the result. **fail** |
| U5 | `history_page(200)` p95 ≤ 20 ms | Linux | The UI's share (dispatch, DTO mapping, serialisation, IPC) has p95 **1.9 ms** on CI Ubuntu and **9.4–10.9 ms** on agentbox (load 121–200); 2.9 ms on macOS and 5.2 ms on Windows. The full call's p95 is 61 ms on CI and 421–510 ms on agentbox; of that, the engine takes 60 and 415–503 ms (E-HIST). **pass (UI)** / **fail (engine)** | Render of the 200 rows, first round of each launch: **3.2–4.0 s** on CI Ubuntu (two runs; CPU 4.1 s), **14.0–17.2 s** on agentbox. Later rounds of the same launch grow to 21–23 s on CI and 31–37 s on agentbox. The FFI call is 100 ms / 400–660 ms (engine, E-HIST). **fail (UI)** / **fail (engine)** |
| U5 | Cold start to first paint ≤ 1.5 s (fixture Overview with balance and history) | Linux | Spawn to first paint on CI Ubuntu: **0.57–1.08 s** cold (page cache dropped) and **0.22–0.72 s** warm, with 0.26–0.33 s of CPU. On agentbox, warm at load 120–122: **1.20–1.78 s** (2 of 5 starts under 1.5 s), with 0.94–1.09 s of CPU. On the 40k wallet: 2.06–2.07 s on CI and 5.9–8.9 s on agentbox; the engine's own wallet open (`dwcli list`) takes 5.2–9.2 s on agentbox. U5 is measured on agentbox, and there it **fails as measured**: 3 of 5 starts are over 1.5 s under shared load. Its CPU time is under 1.1 s, and it **passes** on the quiet runner, cold and warm. A ruling on agentbox's load is needed. **fail (engine)** for wallet open | Demo Overview (balance and history, no engine), two CI Ubuntu runs of 10 starts: the first start, the only one with a cold page cache, **3.20 s** and **2.56 s**; warm median **1.53 s** (CPU 1.54 s) and **1.27 s** (CPU 1.27 s); first frame 0.35 / 0.28 s. agentbox at load 148: **5.0 s** (CPU **4.4 s**; first frame 1.2 s). 40k wallet: lock screen 3.95 / 3.56 s (CI) and 16.3 s (agentbox), then unlock to Overview 1.46 / 1.26 s and 3.57 s. Under DEC-189 both legs fail: the cold CI starts (2.6–3.2 s) and agentbox's CPU time to that paint (4.4 s) are both over 1.5 s. **fail (UI)** / **fail (engine)** for wallet open |
| U5 | PSS of all processes, idle, ≤ 400 MB | Linux | The fixture (the UI alone): **194.7 MiB** on agentbox and **257.1 MiB** on CI. The 40k wallet in total: 403.0 MiB on agentbox and 460.2 MiB on CI. The engine's share is 186.3 / 186.1 MiB, which leaves the UI 216.7 / 274.1 MiB. **pass**: the gate judges the UI's own cost. The 40k total exceeds 400 MB because of the engine's share, which the gate assigns to H-06 | Demo Overview (UI only): **231 MiB** on agentbox, **266 MiB** on CI. 40k wallet, one process with the engine: Overview 480–530 / 508–544 MiB; Transactions (100 rows) 623–656 / 846–882 MiB. Judged on the UI's share (DEC-189), the engine-free demo is the UI baseline: **pass (UI)** on Overview. Measured over that baseline, Transactions' 100 rows add 127–148 MiB (agentbox) and 338–346 MiB (CI) of UI, an estimated UI share of 340–380 and 600–610 MiB. On CI that is over 400 MiB |
| U6 | Allowlist, CSP, navigation, XSS, unlisted command | all | Probed in the real app on Linux, Windows and macOS. The ACL refuses an unlisted command and a plugin command outside the capability. CSP (`connect-src`) blocks a remote `fetch`. `window.open` returns null. An external link leaves the app on its origin. In Playwright, XSS payloads in a label, a contact name and the wallet name render as text. **pass** | n/a (Tauri only) |
| U7 | Each package installs (or unpacks) and launches | Linux | `.deb` (23.6 MB) installs with apt; the installed app passes its self-test; it removes cleanly. The AppImage (104.9 MB) runs and passes its self-test. **pass** | Portable tarball (52.4 MB; 136 MB binary, Swift runtime static, needs GTK ≥ 4.14) unpacks and runs every gate phase, agentbox and CI. Flatpak (GNOME 47 runtime, 34.6 MB bundle) builds in 159 s, installs and reaches Overview. No `.deb`/AppImage built. **pass** |
| U7 | | Windows | The NSIS installer (14.8 MB) and the MSI (20.9 MB) install silently (exit 0, 3 s and 2 s). Both installed apps launch with a window; the NSIS app also passes U2 and the UIA walk. **pass** | Zip (95.6 MB, 35 entries: the exe, its resource bundles, and 32 DLLs from the Swift 6.3.3 runtime and the build). It unpacks with `Expand-Archive` and launches with the toolchain off PATH. It needs the Windows App Runtime 1.5, installed separately. The GtkBackend zip (108.7 MB, 102 entries, 99 DLLs with GTK's) needs nothing installed and launches the same way. **pass** |
| U7 | | macOS | The dmg (21.6 MB) verifies and mounts. The `.app`, copied to `~/Applications`, launches with the fixture (light and dark) and with the 40k wallet (its lock screen). **pass** | Unsigned `.dmg` (33.1 MB; app 89.4 MB) installs by copy and launches through LaunchServices. **pass** (unsigned) |
| U8 | Build, launch, every screen in light and dark; no crash | macOS | The self-test on `macos-latest` captures all 4 screens in light and dark (8 window captures), passes 212/212 checks and does not crash; the dmg-installed app launched 3 times. **pass** | All 4 screens in light and dark reached on the 40k wallet, no crash (AppKitBackend, macos-latest M1 virtual); styling per U3a. **pass** |

- **Tauri:** U1, U2, U3a, U3b, U4, U6, U7 and U8 pass on every platform measured. U3c passes on
  Linux and macOS; on Windows the only difference is WebView2's 15 px classic scroll bar, which
  needs a ruling. U5: the UI's own costs pass (UI share of `history_page(200)` p95 ≤ 10.9 ms;
  fixture PSS 195–257 MiB; cold start ≤ 1.08 s on a quiet runner). The engine-bound costs miss their
  thresholds until E-HIST lands: `history_page` takes 60–503 ms and wallet open 2–9 s. The engine's
  186 MiB of PSS belongs to H-06. Cold start on agentbox, where U5 is measured, is over 1.5 s in 3
  of 5 starts under shared load (1.20–1.78 s at load 121); that needs a ruling.
- **SwiftCrossUI:** U1, U2 (Windows, with the 8 MB stack; WinUIBackend and GtkBackend alike), U4 on
  Linux (AT-SPI, 0 unnamed), U7 on all three platforms and U8 pass. U5 memory passes on the UI's
  share for Overview (DEC-189; demo 231/266 MiB), but Transactions' 100 rows put the estimated UI
  share at 600–610 MiB on CI. U3a fails everywhere (56/120 Linux, 51/120 macOS, 48/104 WinUI), with
  18 U3b items short of the Cross column. U4 fails on Windows on both backends (WinUIBackend crashes
  in the walk, swift-cross-ui#787; gvsbuild's GTK has no UIA provider) and by keyboard (the sidebar
  takes Tab; row details unreachable). U5 fails the 200-row render (3.2–4.0 s on CI, about 200× the
  budget, growing each round) and, under DEC-189, cold start on both measures: the cold CI starts
  take 2.56 and 3.20 s, and agentbox spends 4.4 s of CPU to first paint. U3c and U6 do not apply.
  The engine-bound costs (E-HIST, wallet open, E-BAL) are the same as Tauri's.

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

Measured by G-03 on branch `dw/g-03-crossui-measure`, from main 85ab16e. The app is the existing
`dash-wallet` (SwiftCrossUI 0.10.0, vendored at `Vendor/swift-cross-ui`): GtkBackend on Linux,
AppKitBackend on macOS and WinUIBackend on Windows. Raw data is in
[`docs/research/g-03-data/`](../research/g-03-data/).

### Machines and builds

| Where | Machine | Build |
|---|---|---|
| agentbox | 32 × Xeon E5-2687W v4, 157 GiB, no GPU (GTK draws with GL on llvmpipe), Ubuntu 26.04, GTK 4.14.5, Xvfb 1440×900; load 98–245 during the run, logged per phase | Swift 6.3.3 release build in the `dwd-linux-crossui` Docker image (core 21 s warm, Swift 273 s); the tarball runs on the host |
| CI Ubuntu | GitHub `ubuntu-latest`: 4 × EPYC 7763, 16 GB, Ubuntu 24.04, GTK 4.14.5, Xvfb 1440×900, no GPU; load 0.2–3.7 | native release build, core 886 s, Swift 507 s (run 38115456933) |
| macOS | GitHub `macos-latest`: Apple M1 (virtual), 3 CPUs, 7.5 GB, macOS 26.6.2 | core 187 s, Swift 207 s (run 38116062636) |
| Windows | GitHub `windows-latest`: AMD EPYC 9V45 or 7763 (it varies by run), 4 vCPUs, 16 GB, Windows Server 2025, 1024×768 | release, Swift 6.3.3 toolchain and runtime: core 1016–1630 s, Swift 766–1376 s (WinUI runs 38114456775 and 38116419564 with the 1 MB stack, 38119740971 with 8 MB; GtkBackend 38130603972 and 38135678506: core 1703–1800 s, Swift 583–598 s) |

Every run uses the 40,153-transaction regtest fixture (`ci/gate/fixtures/regtest40k-dw.tar.gz`)
with no peer (`--connect 127.0.0.1:1`). The cold-start and PSS criteria are judged on the app's
`--demo` mode, which plays the fixture's role in the Tauri column: the built-in demo wallet's
Overview, with a balance and a history, and no engine. The harness sets no renderer, theme or
accessibility variables. GTK picks GL, which is llvmpipe on both Linux machines.

### Harness

- **Probe:** `Sources/DashWalletCross/GateProbe.swift` is off unless `DWD_GATE=1`. It writes
  `gate-mark NAME SECONDS … cpu=S` lines to stderr: `first-paint`, then `<screen>-paint` each time
  the screen changes (`lock`, `sync-overlay`, `overview` once it lists transactions,
  `transactions`, `detail`), each with the process CPU time.
  - On Linux the marks come from the window frame clock's `after-paint`, timed on CLOCK_MONOTONIC
    like the harness's own clock.
  - On macOS and Windows there is no frame hook, so a 250 ms poll of the view models produces them,
    and they only time the screenshots.
  - The probe also gives runners without AT-SPI the same steps a user takes: unlock through the lock
    screen's view model, the overlay's Hide, and selecting the first row. It also runs
    `DWD_GATE_HISTORY_BENCH`.
- **Linux:** `ci/gate/crossui/linux-gate.sh` and `linux_gate.py`. Each phase launches a fresh
  process from the unpacked release tarball, on a private Xvfb display with its own D-Bus and
  AT-SPI bus. U1, U4 and the keyboard checks drive the app only through AT-SPI (pyatspi) and xdotool
  keys. The keyboard checks follow focus with AT-SPI `focused` events.
- **macOS:** `ci/gate/crossui/macos.sh` installs the app from the `.dmg`. It waits for the screen's
  mark after the unlock, then captures the window.
- **Windows:** `ci/gate/crossui/windows.sh` and `windows-run.ps1` unpack the zip, launch the app,
  wait for the mark and capture the screen. They also walk UI Automation. A stack overflow is
  re-run under `cdb`.

### What G-03 changed so that the gate could run

Each change is a commit on this branch. The measured builds include all of them.

- **SwiftCrossUI patch P12 (`Vendor/PATCHES.md`).** Swift Bundler metadata is now read from the
  executable's 16-byte trailer. Upstream read the whole 136 MB binary and compared bytes one at a
  time, which cost about 1 s of CPU before `App.init`. Without P12, every cold start would carry
  that second.
- **P13.** GtkBackend's root widget had no `dispose`, so any window closed after a Transactions
  page with rows segfaulted (exit -11).
- **The lock-state gap.** The app showed the shell, with a stale Overview, before the vault had
  reported whether it was locked. `WalletRootView` now shows the loading message until the vault
  reports. (dw-app does not have this gap; see G-02.)
- **Gate tooling:** the probe, the harnesses, the Linux tarball (`linux-pack.sh`, with the
  `org.dash.wallet.desktop.CrossUI` app ID in the metadata trailer) and a Flatpak manifest.
- **The theme test.** The accent test now uses the WCAG AA value that main already ships
  (DEC-36).

### Per criterion

- **U1.** `phase_u1` unlocks through AT-SPI and reads the Overview balance and the first 200
  Transactions rows from the accessibility tree. It compares them with
  `dwcli balance` and `dwcli history --limit 200` on a copy of the same datadir.
  - Fields compared: txid, record index, type, signed amount in duffs, status, confirmations,
    address.
  - Result: 0 mismatches on agentbox and on CI.
  - The balance is the engine's 20,601,436,392,896 duffs, which is 150 DASH below Core (E-BAL);
    the app matches the engine.
- **U3.** Four screens (lock, Overview, Transactions, detail) were captured in light and dark on
  each platform. An independent reviewer judged them against UX-SPEC's Cross column alone; the
  checklist, counts and every finding are in
  [`g-03-data/u3-review.md`](../research/g-03-data/u3-review.md).
  - The Linux font check (U3) passed: the bundled Inter is the face in use.
  - On Linux, every sampled colour and type size is exact.
  - The misses are structural:
    - no header row (C2);
    - the detail is inline, not a sheet (C23);
    - toolkit default text fields (r≈4);
    - the lock screen lacks the mixing-only box, Quick Receive and Scan to Send;
    - amounts print as `[+0.00000008]`;
    - the breakdown strip's cells are unequal;
    - the capsule sits outside the hero;
    - labels clip, and dates differ between screens.
  - macOS adds a colour shift: every role is lighter and bluer (hero #0079C3), probably because
    AppKitBackend builds colours outside sRGB. It also draws the sidebar selection grey.
- **U4 (Linux).**
  - The AT-SPI walks count interactive nodes (push button, toggle, check box, text, combo,
    list item, menu item, link, tab) with an empty name.
  - Result: 0 in all eight walks, on both machines.
  - After focus moves into the Transactions list, AT-SPI reports the rows as defunct.
- **U4 (keyboard, Linux).** Driven with xdotool; focus was read from AT-SPI events
  (`results.json`, `u4-keys`).
  - Unlock (Return in the field) and Hide on the sync overlay (Tab, Space) pass.
  - Tab from Overview goes into the sidebar. A GtkListBox row is selected when it gets focus, and
    selection changes the page, so the first Tab through the sidebar lands on Transactions. None
    of Overview's controls are reached: the shortcuts, Hide balance, Filter, the rows, See all.
  - On Transactions, Tab never reaches Load more, and Return on a row does not open it.
- **U5 cold start.** Ten launches each.
  - Demo: spawn to `overview-paint`.
  - Live: spawn to `lock-paint` on the 40k wallet, then unlock to `overview-paint`.
  - There is no polling error. The harness reads CLOCK_MONOTONIC just before `Popen`, and the app
    stamps each mark in its frame clock's `after-paint` on the same clock. The CPU time and the
    load are recorded with each start.
  - DEC-189 rules that it passes only if the median of the cold CI starts is ≤ 1.5 s *and*
    agentbox's process CPU time to that paint is ≤ 1.5 s.
  - This harness does not drop the page cache (G-02's does), so only the first start of each run is
    cold: 3.198 s (run 38115456933) and 2.562 s (run 38124495440,
    [`ci-ubuntu-r2/`](../research/g-03-data/ci-ubuntu-r2/results.json)).
  - The warm medians are 1.529 and 1.270 s, and agentbox's CPU time is 4.41 s. Both legs fail.
    agentbox's wall-clock time is recorded but not judged.

  | | CI Ubuntu (load ≈2) | agentbox (load 98–148) |
  |---|---|---|
  | demo Overview, median (min–max) | **1.529 s** (1.504–3.198) | **5.006 s** (max 5.629) |
  | CPU at that mark, median | 1.542 s | 4.41 s |
  | demo first frame, median | 0.346 s | 1.227 s |
  | 40k: lock screen, median | 3.95 s (CPU 3.68 s) | 16.3 s (CPU 10.4 s) |
  | 40k: unlock to Overview, median | 1.457 s | 3.569 s |

  - Between the first frame and the Overview, the process is CPU-bound: wall time equals CPU time
    on CI. The demo services add no delay on this path.
  - On agentbox, about 57 % of that CPU goes to llvmpipe shader compiles. With
    `GSK_RENDERER=cairo`, the CPU at Overview falls from 4.3–5.0 s to 3.2–3.7 s (5 launches each,
    [`agentbox/renderer-ab.txt`](../research/g-03-data/agentbox/renderer-ab.txt)). This is
    information only: the app does not choose a renderer.
  - On the live wallet, the engine opens the wallet before the lock screen. That is the same E-HIST
    and wallet-open cost the Tauri column reports (`dwcli list` 5.2–9.2 s on agentbox).
- **U5 `history_page(200)` round trip.**
  - `DWD_GATE_HISTORY_BENCH` on the live 40k wallet. Each round empties the list and waits for that
    frame, calls `history.page(limit: 200)` through the FFI, puts the 200 records on the page, and
    waits for the frame that draws them. It then does the same with `limit: 1`.
  - Three launches on agentbox with 3 rounds each, and three on CI with 5 rounds each.

  | | CI Ubuntu | agentbox |
  |---|---|---|
  | FFI call, limit 200, median (p95) | 100 ms (151) | 505 ms (660) |
  | FFI call, limit 1, median (p95) | 100 ms (151) | 551 ms (1,711) |
  | render of 200 rows, round 0 (3 launches) | 3.98, 4.01, 3.96 s | 14.0, 14.2, 17.2 s |
  | render, later rounds of one launch | 5.3, 7.4, 11.8, 21.5 s | 31–37 s |
  | CPU during the render | ≈ wall | ≈ wall |

  - The engine's share equals Tauri's E-HIST: the call costs the same at limit 1.
  - The UI's share is 200–850 times the 20 ms budget even in the best round.
  - A perf profile of one round on agentbox: 62 % libgtk-4, 22 % libgobject, about 7 % the app.
    Turning accessibility off (`GTK_A11Y=none`) left the CPU per round unchanged: 51 and 64 s
    without it, 50 and 67 s with it (one launch each,
    [`agentbox/a11y-ab.txt`](../research/g-03-data/agentbox/a11y-ab.txt)).
  - SwiftCrossUI 0.10's `List`/`ForEach` builds a full widget subtree per row, with no
    virtualization.
  - The probable cause of most of the cost: every widget SwiftCrossUI styles gets its own
    display-wide `GtkCssProvider` (`Gtk/Widgets/Widget.swift` `cssProvider`,
    `Gtk/Utility/CSS/CSSProvider.swift`).
    - Each one is added with `gtk_style_context_add_provider_for_display` under a unique class.
    - Adding a provider invalidates style for the whole display, and GTK matches every widget
      against every provider.
    - A provider is removed only when its Swift widget object deinits.
    - An experiment confirmed it as the larger part. A provider scoped to each widget's style
      context, not the display, was patched in temporarily
      ([`widget-scoped-css.diff`](../research/g-03-data/agentbox-css-experiment/widget-scoped-css.diff),
      not committed). The same agentbox history phase (3 launches × 3 rounds) then gave a median of
      5.90 s (p95 8.24 s, CPU median 5.50 s), against 33.60 s (CPU 31.38 s) unpatched. The rounds
      no longer grew much: 5.6, 7.8 and 8.2 s in one launch, against 17.7, 34.0 and 37.2 s
      ([`agentbox-css-experiment/results.json`](../research/g-03-data/agentbox-css-experiment/results.json),
      load 128–145). The eight screens drew the same pixels as unpatched, apart from a hover
      highlight on Send (the pointer's position), and AT-SPI naming was unchanged.
    - What is left, about 5.3 s of CPU per 200 rows on agentbox, is still about 270 times the
      budget: the per-row widget subtrees.
  - Unpatched, each later round costs more than the one before: the emptied rows' providers are
    not released. The bench sets the list's rows the way Load more does (`rows = …`), so this cost
    would also grow during a session.
- **U5 PSS.**
  - `smaps_rollup` of the app process, after 30 s idle. On Linux, the SwiftCrossUI app is one
    process with the engine linked in.
  - Demo Overview (3 launches): 210–231 MiB on agentbox and 259–266 MiB on CI.
  - 40k wallet:
    - Overview: 480–530 MiB on agentbox, 508–544 MiB on CI.
    - Transactions with its first 100 rows: 623–656 MiB on agentbox, 846–882 MiB on CI.
  - The engine's share cannot be split out of the single process; G-02 estimates 154–193 MiB for
    the same wallet. DEC-189 judges memory on the UI's share, measured over an engine-free
    baseline. The demo Overview is that baseline, and it passes.
  - The 100 rows add UI over that baseline: 127–148 MiB on agentbox and 338–346 MiB on CI
    (Transactions minus Overview in the same launch). The UI share of Transactions is therefore
    about 340–380 MiB on agentbox and 600–610 MiB on CI.
- **U7.**
  - Linux:
    - The tarball (`linux-pack.sh`) runs every gate phase from its unpacked copy. `ldd` lists no
      Swift libraries.
    - The Flatpak (`ci/gate/crossui/flatpak/`, GNOME 47) was built and installed on agentbox and
      reached Overview ([`agentbox/flatpak.txt`](../research/g-03-data/agentbox/flatpak.txt)).
  - macOS: an unsigned `.dmg`, copied to /Applications and launched with `open`.
  - Windows: see below.
- **U8.**
  - All eight shots (4 screens × light/dark) reached their marks after the unlock: 4.1–11.0 s from
    launch on the 40k wallet.
  - RSS 313–740 MB.
  - Each launch stayed up until it was captured, and the LaunchServices launch stayed up as well.

### Windows

- **Harness.** `ci/gate/crossui/windows.sh` and `windows-run.ps1` on `windows-latest`:
  - They build the release `dash-wallet`, pack a zip (the exe, its resource bundles, the Swift
    runtime's DLLs; with GtkBackend also GTK's DLLs and GSettings schemas), unpack it with
    `Expand-Archive`, and launch it with the toolchain's runtime and GTK's `bin` dropped from PATH.
  - GateProbe's hooks unlock the vault and open a row's details; there is no AT-SPI here.
  - Each launch waits for its screen mark, captures the whole screen and appends a line to
    `results.txt`. The U4 runs then walk the window's UI Automation tree (`FindAll` over its
    descendants, the swift-cross-ui#787 walk).
  - A crash leaves a WER minidump. A stack overflow re-runs once under `cdb`, which prints the stack.
- **The 1 MB main-thread stack (fixed).**
  - With Windows' default 1 MB main-thread stack, every launch that unlocked died with 0xC00000FD
    before it painted. Only the lock screen rendered (runs 38114456775 and 38116419564,
    [`windows-1mb-stack/`](../research/g-03-data/windows-1mb-stack/)).
  - The `cdb` stack ([`overview-cdb.txt`](../research/g-03-data/windows-1mb-stack/overview-cdb.txt))
    is SwiftCrossUI's nested view updates through `withObservationTracking`: 172 `swiftObservation`
    frames. Linux and macOS give the main thread 8 MB.
  - `Package.swift` now links `dash-wallet` with `/STACK:8388608` on Windows. The harness reads the
    reserve back from the PE header (`stack reserve: 8388608 bytes`).
- **WinUIBackend with the 8 MB stack** (run 38119740971,
  [`windows/`](../research/g-03-data/windows/)):
  - All eight shots reach their screen mark: lock 6.1 s, Overview 7.1–8.8 s, Transactions
    7.6–10.2 s, detail 11.7 s from launch, on the 40k wallet. Working set 222–336 MB.
  - U2: the unlocked Overview is still up at 612 s; peak working set 245 MB.
  - U4: the UIA walk fails on all three screens. `FindAll` throws E_UNEXPECTED, and within 5 s the
    app exits with 0xC0000005. WinUIBackend logs that it does not support accessibility labels.
    This is swift-cross-ui#787, and it is why the brief's GtkBackend fallback applies.
  - U3: 48/104 ([`u3-review.md`](../research/g-03-data/u3-review.md)). The window opens
    un-maximized and larger than the 1024×768 screen.
  - The dark detail shot shows the list with a row selected and no detail, although the probe
    logged its detail mark.
- **GtkBackend fallback** (`swiftcrossui.sh --backend gtk`, Vendor/PATCHES.md P14):
  - The same app on gvsbuild's GTK 4 (2026.8.0, MSVC), with GTK's DLLs in the zip.
  - SwiftPM's pkg-config parser reads a line containing ':' as a "Key: value" field, so gvsbuild's
    `prefix=C:/…` left the include paths empty. The harness rewrites the prefix relative to
    `${pcfiledir}` and checks that gtk4's 24-file Requires closure has no drive letter left.
  - The app asks Fontconfig for Inter but does not register its bundled copy on Windows (that is
    `os(Linux)` code), so GTK falls back to a system face.
  - Run 38135678506 ([`windows-gtk/`](../research/g-03-data/windows-gtk/)), after two build fixes:
    P15 (GLib's `gulong` is 32-bit and `gsize` imports as `UInt64` on Windows) and
    `GtkAccessibleRole`'s raw type (MSVC gives C enums a signed `Int32`; `DashUICross` uses
    `RawValue`). Its first run (38130603972) missed every screen mark because Pango's
    "couldn't load font" warning ends without a newline, so the next mark shared its line; the
    harness now finds a mark anywhere in a line.
  - The zip is 108.7 MB (102 entries, 99 DLLs: the Swift runtime and GTK's). It unpacks and
    launches with the toolchain and GTK off PATH.
  - U2: Overview paints at 11.8 s and the app is still up at 601 s (peak working set 212 MB).
    **pass**.
  - U4: `FindAll` returns 0 elements on all three screens. gvsbuild's `gtk-4-1.dll` reports
    "accesskit - Disabled during GTK build", and AccessKit is GTK's only accessibility backend on
    Windows. **fail**.
  - Screens: the lock screen and Overview match their marks (Overview with the 40k balance). The
    Transactions and detail shots still show the sync overlay (4) or the lock screen with an
    unlock error (1), although the probe logged their marks: on Windows the marks come from the
    250 ms view-model poll, not from painted frames. U3 was not scored on this backend.
  - Fonts: every launch falls back from Inter to "Sans" (36 face/size pairs).

### Blockers to a fair comparison

1. **E-HIST and wallet open (engine).**
   - Every live number carries the engine's `history_page` (100 ms on CI, 0.3–1.7 s on agentbox)
     and its wallet open. Both columns share these costs.
   - In the Cross column the engine runs in-process, so its PSS cannot be separated from the UI's.
2. **E-BAL.** The balance under test is the engine's, 150 DASH below Core's. U1 compares the UI with
   the engine, as for Tauri.
3. **The engine's "no reactor running" panic.**
   - `dw-engine/src/platform/lease/session.rs:348` (`create_background` →
     `tokio::task::spawn_blocking`) panics after every unlock, on every platform. It is logged, and
     the app continues.
   - It is main's bug (RealEngineTests fail the same way). It does not affect these numbers.
4. **agentbox's load and the lack of a GPU on every Linux machine.**
   - agentbox ran at load 98–245, so its wall times are upper bounds. The CPU columns and the quiet
     CI runner are the fairer figures.
   - Both Linux machines draw with llvmpipe. A desktop with a GPU would spend less on shaders; the
     cairo A/B bounds that saving at about 1 s of CPU on agentbox.
5. **Windows.** WinUIBackend reaches every screen with the 8 MB stack, but walking its UIA tree
   crashes the app (#787), and GtkBackend on Windows exposes no UIA tree at all: no Windows backend
   passes U4. GtkBackend's Windows shots are incomplete (Transactions and detail not on screen), so
   U3 on Windows is WinUIBackend's.
6. **What the gate changed in the app** (above).
   - The measured Cross build includes P12, P13 and the lock-state fix; main does not.
   - Without P12, main's cold start is about 1 s slower. Without P13, main crashes when a
     Transactions window closes.
7. **macOS screenshots** come from a virtual M1 with no display profile control. The colour finding
   was measured after converting the PNGs to sRGB.

### Effort to close the Cross failures

These are estimates, not measurements.

| Failure | Cause found | Fix | Size |
|---|---|---|---|
| U5 history (200–850× over) | One display-wide CSS provider per styled widget, outliving emptied rows (the larger part, shown by experiment); one widget subtree per row, no virtualization | First a backend patch: widget-scoped styles. The experiment cut the agentbox median from 33.6 s to 5.9 s, still about 270× the budget. Then a virtualized list in SwiftCrossUI (GtkListView with recycled rows, NSTableView, WinUI ListView): upstream API work in three backends | M (measured: not enough), then XL |
| U5 cold start (cold CI starts 2.56–3.20 s; agentbox 4.4 s CPU) | CPU-bound first layout and shader compiles on llvmpipe | Trim the first Overview layout; a renderer choice made by the app on GPU-less systems | M |
| U4 keyboard | GtkListBox selection follows focus, and the app changes page on selection; rows defunct after focus | Change the page on row activation (a backend patch), plus an explicit focus order | M |
| U3a / U3b (18 items) | App-level layout and components; AppKitBackend colour space; WinUIBackend defaults (font, selection, fields) | Header row, sheet detail, styled fields, the lock-screen controls, AmountText anatomy, strip layout, date formats; sRGB colours in AppKitBackend; Inter and styled selection on WinUI | L |
| U2 / U4 Windows | WinUIBackend: `FindAll` crashes the app (swift-cross-ui#787) and labels are unsupported. GtkBackend: gvsbuild's GTK is built without AccessKit, so the window exposes no UIA tree | Upstream accessibility in WinUIBackend (#787 and labels), or GtkBackend on a GTK built with `-Daccesskit=enabled` (adds Rust to the GTK build; not tried here) plus Inter registered on Windows, then U3 and U4 rerun | L (WinUI, upstream) or M (own GTK build, unmeasured) |

### Raw data

- `agentbox/` and `ci-ubuntu/`:
  - `results.txt` and `results.json`: every row above, with raw per-launch values.
  - `run.log`, plus the app's stderr per launch in `logs/`.
  - `runner.txt` and `build.json`.
  - AT-SPI walks `atspi-*.txt` (the long ones gzipped) and per-screen computed styles
    `styles-*.json`.
  - U1's `u1-app-rows.txt` and the `dwcli` outputs.
  - Screenshots.
- `agentbox/` only: `renderer-ab.txt` and `flatpak.txt`.
- `macos/`: `results.txt`, `runner.txt`, the shots, and the app's logs per launch.
- `windows/`: `results.txt`, `runner.txt`, the shots, logs, the UIA walk and the `cdb` stack.
- `u3-review.md`: the U3 review.

## Decision (G-04)

*(the manager)*
