# Continuous integration (GitHub Actions)

DEC-13 and roadmap task T-03. `main` is pushed to
[`PastaPastaPasta/dashwallet-desktop`](https://github.com/PastaPastaPasta/dashwallet-desktop), which is
public, so the hosted Linux, macOS and Windows runners cost nothing. No workflow needs a secret, and every
job's `GITHUB_TOKEN` is read-only (`contents: read`).

| Workflow | Triggers | What it runs |
|---|---|---|
| [`ci.yml`](../.github/workflows/ci.yml) | push to `main` and `dw/**`, pull requests, manual | T0 on Linux, macOS and Windows; UI tests (Playwright, axe, tauri-driver); app launches with screenshots |
| [`nightly.yml`](../.github/workflows/nightly.yml) | 03:17 UTC daily (default branch only), manual | T1 regtest suites; the engine with its `shielded` feature; the Linux GUI (AT-SPI) demo |
| [`gate.yml`](../.github/workflows/gate.yml) | manual only | G-02 / G-03 measurement scripts on the chosen OSes |
| [`tauri-selftest.yml`](../.github/workflows/tauri-selftest.yml) | manual only | the Tauri app (G-01) built, run with `--selftest` and launched for a screenshot on macOS and Windows |

Every action a workflow uses directly is pinned to a commit SHA (the tag is in a comment). Actions those
actions call are not all pinned: `compnerd/gha-setup-swift` (pinned) is a composite action that refers to
`actions/cache/restore@v6`, `actions/cache/save@v6` and `actions/upload-artifact@v4` by tag. Its cache steps
are off by default, and the artifact upload runs only when its installer fails. Accepted, as for its
unverified installer below; pinning them would mean vendoring the action.

A new push to a branch cancels that branch's running CI. Each distinct commit on `main` has its own
concurrency group, so a running `main` run is never cancelled and a queued one is not superseded by a later
commit's. Two runs of the same `main` commit (a re-run or a manual dispatch) still share a group. Every `run` step uses bash with `pipefail` (Git Bash on
Windows) unless it names another shell. A pull request from a `dw/*` branch runs CI twice, once for the
push and once for the pull request. Checkouts do not keep the job token (`persist-credentials: false`).

**Downloads are pinned and verified** before anything runs them, except the Windows Swift installer (last
row):

| What | Where | Check |
|---|---|---|
| protoc 29.3 (release zip, every OS) | `ci/github/install-protoc.sh`; the `ci/linux` Dockerfiles | SHA-256 per asset, kept in the script and the Dockerfiles (protobuf publishes none) |
| swiftly 1.1.1 (Linux) | `ci/github/linux-swiftly.sh` | SHA-256 per architecture; swiftly then GPG-verifies the Swift toolchain |
| Windows App Runtime 1.5 installer | `ci.yml`, windows-swift | the resolved `download.microsoft.com` URL, SHA-256 (`Get-FileHash`) and a valid Microsoft Authenticode signature |
| Dash Core v24.0.0-rc.2 | `regtest/scripts/fetch-dashcore.sh` | SHA-256 per platform, also on every cache restore |
| dash_hash 1.4.0 | `nightly.yml`; `regtest/Dockerfile` | SHA-256 of the commit archive; of the tag archive in the image |
| uv 0.12.23 | `nightly.yml` | version pinned through `astral-sh/setup-uv` |
| Swift 6.3.3 for Windows | `compnerd/gha-setup-swift` | exact release from swift.org over HTTPS; **the action checks neither a hash nor a signature** (accepted: pinned version, non-blocking and gate jobs only) |

## `ci.yml`

| Job | Runner | Steps |
|---|---|---|
| Rust lint | `ubuntu-24.04` | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| Rust tests (Linux) | `ubuntu-24.04` | `cargo test --workspace --locked`; then the Tauri bindings regenerated (`DW_UPDATE_BINDINGS=1 cargo test -p dw-app` for `codegen` and `coverage`) and `git diff --exit-code` over `apps/desktop/src/api/generated` and `docs/design/E0-13-facade-coverage.md` (E0-13) |
| Tauri renderer | `ubuntu-24.04`, Node 24, pnpm | `apps/desktop`: `pnpm install --frozen-lockfile`, `pnpm tokens:check`, `pnpm typecheck`, `pnpm test` (vitest, including the per-facade command test and the fixture backend against a mocked IPC), then `pnpm build` and `pnpm build:fixture`, each checked by `scripts/check-bundle.mjs` (see "UI tests") |
| UI tests (Playwright, axe) | `ubuntu-24.04` in the container `mcr.microsoft.com/playwright:v1.64.0-noble` | no Rust and no engine: `pnpm build:fixture`, then `playwright test` in Chromium and WebKit on the fixture backend (see "UI tests"); `test-results/` and the HTML report uploaded as `ui-playwright` |
| Tauri E2E (tauri-driver, Linux) | `ubuntu-24.04` | `linux-host-deps.sh tauri` (WebKitGTK, `webkit2gtk-driver`, Xvfb), `cargo install tauri-driver --version 2.1.0 --locked`, then `xvfb-run -a pnpm e2e:tauri`: the debug app with the production renderer and the real engine, driven through one smoke flow; screenshots and logs uploaded as `e2e-tauri-linux` |
| Swift + UI T0 (Linux) | `ubuntu-24.04`, Swift 6.3.3 (swiftly) | `scripts/build-core.sh --check-bindings`; the Swift suites of `scripts/linux-docker-test.sh` (`SwiftCrossUIPatchTests`, the vendored SwiftCrossUI patches, among them) plus `DashUICrossTests`; `dash-wallet --demo` launched under Xvfb, with a screenshot of its window |
| macOS | `macos-latest` (arm64) | `cargo test --workspace --locked`; `build-core.sh --check-bindings`; `swift build` and `swift test` (every suite; `MacUITests` write their offscreen renders with `DWD_WRITE_SCREENSHOTS=1`); the app generated with XcodeGen, built with `xcodebuild`, installed to `/Applications`, launched through LaunchServices with `--demo` in light and dark, with screenshots of its window; then, **non-blocking** (see "Known gaps"), `dash-wallet --demo` (SwiftCrossUI, AppKitBackend) |
| Windows (engine) | `windows-latest` | `cargo build --workspace --locked`, `cargo test --workspace --exclude dw-app --locked` (see "Known gaps") and `cargo clippy --workspace --all-targets -- -D warnings` (MSVC; clippy lints the Windows code paths, which the Linux job never compiles) |
| Windows Swift | `windows-latest` | **non-blocking**, see "Known gaps": the Windows core bundle, the headless Swift suites, `dash-wallet` with WinUIBackend, then a launch screenshot if it built |

The Linux Rust jobs install the WebKitGTK development packages first (`ci/github/linux-host-deps.sh rust`,
the list in G-01's `apps/desktop/README.md`): once the Tauri crate `rust/crates/dw-app` lands,
`cargo build --workspace` needs them. `ci/linux/Dockerfile`, the Rust builder image, has them too. The
Swift images build only `dw-ffi` and do not.

The Linux Swift job installs Swift 6.3.3 (the version of the `swift:6.3.3-noble` image the Docker scripts
use) with swiftly, and GTK 4 with `ci/github/linux-host-deps.sh swiftcrossui`. It builds the package's test
product (`DashWalletDesktopPackageTests`) and `dash-wallet` by name, because a plain
`swift build --build-tests` would also compile swift-winui's Windows-only C targets, which fail on Linux.
It then runs the `.xctest` bundle directly with `--testing-library swift-testing` (every suite uses
swift-testing).

It runs on the runner rather than in a `swift:6.3.3-noble` job container, so it shares the free-disk step
and caching with the other Linux jobs.

`build-core.sh` reads the system libraries from rustc's `native-static-libs:` line. Under
`CARGO_TERM_COLOR=always`, which every workflow sets, that line ends in an ANSI reset. The script
dropped `-lc` from the module map, but `-lc<ESC>[0m` slipped through, so every object importing the core
asked the linker for a library named `c<ESC>[0m`. gold and lld both reported `cannot find -lc`, with the
escape invisible in the log. The script now strips colour codes before parsing.

**Screenshots.** Each run uploads `screenshots-linux`, `screenshots-macos` and (once the app builds on
Windows) `screenshots-windows`, with the app logs next to the PNGs. `screenshots-macos/offscreen/` holds
the `MacUITests` renders (`docs/screenshots/ux/mac` in the run's checkout).

```sh
gh run list --workflow ci.yml --branch main --limit 1           # the latest run on main
gh run download <run-id> -n screenshots-macos -D /path/to/out    # or -n screenshots-linux
```

The launch screenshots come from these helpers:

- `ci/github/linux-launch-screenshot.sh`: Xvfb and `xwd` of the largest "Dash Wallet" window, taken
  `DWD_SHOT_SETTLE` seconds after it appears (60 in CI, because the demo shows "Loading wallet…" for 10–45 s
  on agentbox before the Overview);
- `ci/github/macos-launch-screenshot.sh`: `screencapture -l` of the process's largest on-screen window, found
  with `ci/github/macos-window-id.swift`, plus the whole screen;
- `ci/github/windows-launch-screenshot.ps1`: the primary screen through `System.Drawing`.

The Linux and macOS helpers fail the step when the app exits early or shows no window; the Windows one when
it exits early. They run only on CI runners and in containers, never on a developer's Mac (`CLAUDE.md`).

**Caching.** `Swatinem/rust-cache` caches `~/.cargo` and the dependency part of `rust/target` per job,
keyed on `Cargo.lock` and the toolchain. `actions/cache` caches SwiftPM's repository cache, keyed on
`Package.resolved`. A cold run builds the whole platform graph: it takes about 25–60 minutes per job,
depending on the runner, and a warm one much less (see "Timings").

**Disk.** The hosted runners have less free disk than agentbox, so the workflows set
`DWD_MIN_FREE_GB=5` for `scripts/disk-guard.sh`. The Linux test, nightly and gate jobs delete the
preinstalled .NET, Android, GHC and CodeQL trees first (`ci/github/linux-free-disk.sh`).

## UI tests (U-07)

The renderer is tested in a browser against the **fixture backend** (`apps/desktop/src/fixture/`): Tauri's
mock IPC answers the generated commands (`src/api/generated/commands.ts`, typed by `CommandArgs` and
`CommandResults`) from a scenario's data and emits `engine://event` like dw-app's bridge. Nothing of the
engine is built or linked. Scenarios: `funded` (default), `empty`, `no-wallet`, `locked`, `large`, `xss`,
`dashpay-no-identity`, `dashpay-registering`, `dashpay-registered`, `lock-report-omitted`,
`lock-report-pending`, `lock-report-resolved`, and `error-<class>` for each `ErrorData` class (see
`apps/desktop/README.md` for the URL parameters).

**No fixture code in production.** The renderer imports the fixture only under the build-time constant
`__DW_FIXTURES__`, which is true for `vite` (dev server) and `vite build --mode fixture` and false for
`vite build`, so the production bundle has no fixture chunk. `scripts/check-bundle.mjs` runs at the end of
both builds: `dist/` fails if it contains the fixture's sentinel string or Tauri's mock IPC, and
`dist-fixture/` fails if it does not (the positive control). `pnpm tauri build` runs `pnpm build`, so
every app binary passes the check. dw-app has no fixture mode (`--fixture` is an unknown option).

| Suite | Where | What |
|---|---|---|
| `e2e/scenarios.spec.ts` | Chromium, WebKit | every scenario boots on the fixture alone; 8 states (empty wallet, no wallet, locked, loading, open failed, wallets error, history error, unresolved lock report) in light and dark, each with a screenshot artifact and 0 serious or critical axe issues |
| `e2e/screens.spec.ts`, `e2e/a11y.spec.ts`, `e2e/style.spec.ts` | Chromium, WebKit | the four gate screens in light and dark: screenshots, axe and keyboard reach, the UX-SPEC §2 style checks |
| `e2e/errors.spec.ts` | Chromium | each `ErrorData` class rendered with its message, axe-clean; the splash's retry |
| `e2e/lockreport.spec.ts` | Chromium, WebKit | a lock report with omitted flows, artifacts or wallets stays unresolved until the leases end |
| `e2e/security.spec.ts` | Chromium, WebKit | hostile strings render as text |
| `e2e/visual.spec.ts` | Chromium, `DW_VISUAL=1` only | `toHaveScreenshot` baselines (below) |
| `e2e-tauri/smoke.ts` | WebKitGTK via tauri-driver | the real app on the real engine: lock screen, wrong passphrase refused, unlock, Overview and Transactions of a new regtest wallet, lock again |

Run them locally with the CI commands:

```sh
cd apps/desktop
pnpm e2e                          # host browsers (pnpm exec playwright install chromium webkit)
scripts/e2e-docker.sh             # the CI job: same image, same commands, as the calling user
xvfb-run -a pnpm e2e:tauri        # the tauri-driver smoke (or DISPLAY from `agent-gui start N`)
```

**Visual baselines.** `e2e/visual.spec.ts` compares the four gate screens in light and dark against
`e2e/visual.spec.ts-snapshots/` (Chromium), and runs only with `DW_VISUAL=1`. The baselines were made in the
Playwright image with `scripts/e2e-docker.sh --update-snapshots`. Three more runs there matched all eight
pixel for pixel on agentbox at load 50–60. On the host, the same Chromium build differed on two screens
(about 1,470 pixels each, font rasterisation), so they are only meaningful inside the image. Whether the
hosted runners' CPUs rasterise identically is unproven, so CI runs the comparison as a **non-blocking**
step. A mismatch leaves the actual and diff PNGs in the `ui-playwright` artifact. Make the step blocking
once it has passed on the runners; regenerate the baselines in the image whenever a screen changes on
purpose. The per-state screenshots of the other suites are artifacts only.

## `nightly.yml`

| Job | What |
|---|---|
| Regtest suites (T1) | `dwcli` built on the runner; the regtest image built; then, once that setup has passed, each as its own step so one failure does not hide the rest: the harness smoke tests, `l1-sync`, `l1-send`, `l2-tools`, `restore`, the functional tests `dwd_mn_chainlock.py` and `dwd_coinjoin_probe.py` (Docker), and the `coinjoin` suite (`dwd_coinjoin_client.py`) on the host against the verified v24 release from `regtest/scripts/fetch-dashcore.sh`. JUnit XML and logs are uploaded as `regtest-logs`. |
| Engine with shielded feature (E0-12) | `dw-engine`'s non-default `shielded` feature (`platform-wallet/shielded` and `platform-wallet-storage/shielded`: halo2, orchard), which no PR job builds: `cargo build -p dwcli --features dw-engine/shielded --locked`; the `shielded_warmup` example; `cargo clippy -p dw-engine --all-targets --features shielded --locked -- -D warnings`; then the example run, which builds the Halo 2 proving key in a cold process (no network). Cold cost: [`docs/research/shielded-cost.md`](research/shielded-cost.md). |
| SwiftCrossUI AT-SPI demo (GUI, Linux) | **non-blocking** (see "Known gaps"): `scripts/crossui-linux-demo.sh` with `DWD_CROSSUI_SUITE=m2` (the M1 flows, then the M2 flows), uploaded as `crossui-linux-demo` |

GitHub runs `schedule` triggers only from the repository's default branch (see "Known gaps"). Until then,
start it by hand: `gh workflow run nightly.yml --ref main`. The nightly's overall status is the regtest
and shielded jobs': the AT-SPI demo shows its own red X without failing the run, so check its log
separately.

## `gate.yml` (UI gate, G-02 and G-03)

Run it on a gate branch:

```sh
gh workflow run gate.yml --ref <gate-branch> -f candidate=tauri -f os=all
gh workflow run gate.yml --ref <gate-branch> -f candidate=swiftcrossui -f os=windows -f args="--u2 --u4"
```

The workflow provides the runners and toolchains; the gate task provides the measurement script
**`ci/gate/<candidate>.sh`** on its branch, run with `bash` on every OS (Git Bash on Windows). Without that
script the job fails and says so. `gate.yml` is registered on GitHub, so it can be dispatched on any branch
that has it.

| Set for the script | |
|---|---|
| `GATE_CANDIDATE` | `tauri` or `swiftcrossui` |
| `GATE_OS` | `linux`, `macos` or `windows` |
| `GATE_OUT` | an empty directory; everything in it is uploaded as the artifact `gate-<candidate>-<os>` |
| arguments | the `args` input, split on whitespace (no globbing) |
| every OS | Rust from `rust/rust-toolchain.toml` with a warm cache, protoc 29.3 |
| Linux, `tauri` | WebKitGTK 4.1 development packages, Xvfb, AT-SPI, `xdotool`, ImageMagick, `webkit2gtk-driver` (`ci/github/linux-host-deps.sh tauri`) |
| Linux, `swiftcrossui` | GTK 4, Xvfb, AT-SPI (`linux-host-deps.sh swiftcrossui`); Swift 6.3.3 through swiftly with its system packages (`ci/github/linux-swiftly.sh`) |
| `tauri` | Node 24 and the pnpm version pinned in `apps/desktop/package.json`; `pnpm install --frozen-lockfile` already run in `apps/desktop`. Not exercised yet: `apps/desktop` exists only on the G-01 branch. |
| `swiftcrossui`, Windows | Swift 6.3.3 (`compnerd/gha-setup-swift`) |
| `swiftcrossui`, macOS | Xcode's Swift on `macos-latest` |

The scripts can use the launch-and-screenshot helpers in `ci/github/`. Playwright browsers, the
10,000-transaction regtest wallet and anything else a measurement needs are fetched by the script itself.

## `tauri-selftest.yml` (G-02)

```sh
gh workflow run tauri-selftest.yml --ref <branch with G-01> -f os=both            # or macos / windows
gh workflow run tauri-selftest.yml --ref <branch> -f os=windows -f theme=dark \
    -f selftest_args="--datadir D:/a/wallet --network regtest"                   # instead of the new wallet
```

Per OS (`macos-latest`, `windows-latest`): Node 24 and pnpm from `apps/desktop/package.json`,
`pnpm install --frozen-lockfile`, `pnpm tauri build --no-bundle` (the renderer and the release binary
`rust/target/release/dash-wallet`). Without `selftest_args` the job builds `dwcli`, which makes an
encrypted vault and a wallet in a fresh regtest data root (`--no-platform`); the app opens it on the real
engine, locked. (Until U-07 the default was `--fixture`, an in-app TypeScript wallet; the fixture backend
now runs in browsers only.) Then:

1. `dash-wallet <app args> --selftest selftest-<os>.json` within 10 minutes. The app renders the four
   screens in light and dark and writes the `data-testid` boxes, style checks, IPC timings and security
   probes. The app exits 0 whatever it found, so the step judges the report. It fails when the app fails,
   times out or writes no report; when the report is a checkpoint (`final: false`) rather than the final
   one; when it has no `security.navigation` probe; when a style check fails on an element that is there; or
   when a security probe says `NOT REFUSED`. The new wallet has no history, so the checks of history rows
   and the hero breakdown find no element; they are counted apart (72 of 160 on Linux, locked or not) and
   do not fail the run. The populated screens are checked by Playwright in `ci.yml`, in Chromium and
   WebKit. The counts, paint times and probes go to the run's step summary.
2. `dash-wallet <app args> --theme <theme>` launched again, with a screenshot (the helpers above). The window
   is 1280×820, and the Windows helper captures the primary screen, which may be smaller.

On Windows a step first logs the WebView2 runtime version, or warns that none is registered.

Both, with the logs, are uploaded as `tauri-<os>`. Installers (MSI/NSIS, dmg) are not built here: that is
U7 in G-02's `ci/gate/tauri.sh`, or PK-02 / W-01 later. On a ref without `apps/desktop` the job fails at its
first step and says so.

## Known gaps

1. **No working Windows app yet.** MacUI is macOS-only. SwiftCrossUI's `dash-wallet` builds with WinUIBackend
   in CI, but its launch crashes (below). The gate decides the Windows stack (G-03 fixes WinUIBackend, G-02
   the Tauri build, W-01 the installer after G-04; DASHPAY §2.1). Until then:
   - the blocking Windows job builds and tests the engine only;
   - the non-blocking "Windows Swift" job records how far the Swift side gets, and uploads
     `screenshots-windows` as soon as `dash-wallet` builds and launches. The screenshot helper itself is
     proven on `windows-latest`. So far:
     - the Rust core bundle builds for `x86_64-pc-windows-msvc`, and the committed bindings match.
       `build-core.sh` merges the crate import libraries the core needs (`windows.0.52.0.lib` from
       `windows_x86_64_msvc`) into `dashwallet_core.lib` with `llvm-lib`, because a Swift link does not
       search the crate's directory;
     - the headless Swift packages build, and 526 of 529 tests pass (run
       [37796510022](https://github.com/PastaPastaPasta/dashwallet-desktop/actions/runs/37796510022)). The
       three failures are portability gaps in the tests: the two `LintImportsTests` run
       `scripts/lint-imports.sh`, a bash script Windows cannot execute, and
       `DesktopDataDirectoryTests.dataLocationCreatesTheXDGRoot` checks the Linux XDG layout;
     - one timing test, `LifecycleTests.doneOnlyAfterThePeakDelay`, failed in one run of two (a 9 s
       `eventually` on the slower runner);
     - `dash-wallet.exe` builds with WinUIBackend (about 14 minutes). It needs the Windows App Runtime 1.5,
       which `windows-latest` lacks (it stopped at "This application requires the Windows App Runtime",
       run [37804634973](https://github.com/PastaPastaPasta/dashwallet-desktop/actions/runs/37804634973)), so
       the job installs it. Then the app opens its window and exits with `0xC00000FD`
       (STATUS_STACK_OVERFLOW), the same main-thread stack overflow in SwiftCrossUI's layout as on macOS
       (gap 3; run [37811923871](https://github.com/PastaPastaPasta/dashwallet-desktop/actions/runs/37811923871)).
       G-03 owns the fix. When the app stays up, the step uploads the screenshot. The Windows helper captures
       the screen whatever is on it, so look at `screenshots-windows` before reading a green step as the app
       rendering.
2. **The nightly schedule does not fire yet.** The repository's default branch on GitHub is
   `m3/r2-governance`, not `main`, and GitHub runs `schedule` only from the default branch. Setting the
   default branch to `main` (repository settings; needs an admin) turns the nightly on. Manual runs work
   meanwhile.
3. **dash-wallet (SwiftCrossUI) on macOS.** The debug build of `dash-wallet` with AppKitBackend crashes at
   launch on `macos-latest`. The cause is a main-thread stack overflow: `EXC_BAD_ACCESS` in
   `___chkstk_darwin`, 11,110 frames deep in SwiftCrossUI's recursive layout (`ViewGraphNode.computeLayout` →
   `LayoutSystem.computeStackLayout` → … → `AppKitBackend.size(text:)`; run
   [37769597095](https://github.com/PastaPastaPasta/dashwallet-desktop/actions/runs/37769597095),
   `screenshots-macos/macos-crossui-demo-backtrace.txt` and `crash-reports/`). The step is non-blocking, and
   on a crash it uploads the lldb backtrace and any crash report. G-03 measures SwiftCrossUI on macOS and owns
   the fix; a release build or a larger main-thread stack is the obvious first test. The macOS app (MacUI)
   is unaffected. The WinUIBackend build on Windows fails the same way (gap 1).
4. **The macOS app opens no window at launch on the runner.** Launched through LaunchServices with `--demo`, a
   fresh "Dash Wallet" becomes the active app (its menu bar shows) but presents no main window: it owns only
   four off-screen 1024×30 windows. A reopen event (`open -a`, what a Dock click sends) brings the main window
   up, and the screenshots are taken then. The helper logs a `::warning::` with the window list each time.
   First seen in run [37762511023](https://github.com/PastaPastaPasta/dashwallet-desktop/actions/runs/37762511023).
   It looks like an app issue (the main `Window` scene next to a `MenuBarExtra`), not a runner one; not fixed
   here.
5. **rs-x11-hash on MSVC.** dashcore's X11 hash (`rs-x11-hash 0.1.8`) includes `<unistd.h>`, which MSVC
   lacks, though it uses nothing from it. The Windows jobs put an empty one on the include path
   (`ci/windows/unistd-shim`, through `CFLAGS_x86_64_pc_windows_msvc`). A real Windows release needs the same
   shim, an upstream fix, or the `x86_64-pc-windows-gnu` target that dash-evo-tool ships with.

   The shim is on the include path of every C file the Windows jobs compile, not only rs-x11-hash's.
6. **`dw-app`'s tests are left out on Windows.** Tauri's Windows binaries need the Common Controls v6
   manifest, and `tauri_build` embeds it into the app's bins only, so test binaries that link Tauri fail to
   load (STATUS_ENTRYPOINT_NOT_FOUND, tauri#13419). The Windows job runs
   `cargo test --workspace --exclude dw-app` (cargo only warns while `dw-app` does not exist). The fix belongs
   in G-01's `rust/crates/dw-app/build.rs`: embed the manifest for every target of the crate, as the projects
   in tauri#13419 do. Then drop the `--exclude`. `cargo build` and `clippy` include `dw-app` on Windows.
7. **The nightly AT-SPI demo is not green on the runner yet, so its job is non-blocking.** Its first run (on a throwaway branch, run
   [37753227825](https://github.com/PastaPastaPasta/dashwallet-desktop/actions/runs/37753227825)) passed 172
   checks and failed 4 hard ones: "send: the sent payment is listed on the Transactions page", "onboarding:
   the new wallet's Overview is shown", and the two transaction-details checks. The first and last are the
   ones `docs/screenshots/m2/linux/RESULTS.md` already records as passing in some runs and failing in others
   (an AT-SPI walk of about 1,500 nodes per `wait_for` poll), and the runner has 4 CPUs. The regtest job of
   the same run passed every suite. Make the job blocking again once the demo passes on the runner.
8. **The Tauri jobs are untested end to end.** `tauri-selftest.yml` and the `tauri` setup of `gate.yml` need a
   ref that contains G-01, and G-01 is not on GitHub yet. Their first dispatch after G-01 lands is their
   first real run.
9. **The U-07 jobs have not run on GitHub yet.** `ui-playwright` and `e2e-tauri-linux` were run locally
   with the same commands (the Playwright image; a private X display on agentbox). Their first
   run on GitHub is the first push of U-07.
