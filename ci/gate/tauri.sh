#!/usr/bin/env bash
# Gate G-UI, the Tauri candidate (G-02): the measurements a GitHub runner makes (DASHPAY §2.1).
# Run by .github/workflows/gate.yml (`gh workflow run gate.yml --ref <branch> -f candidate=tauri`)
# with GATE_OS (linux|macos|windows) and GATE_OUT set; everything lands in GATE_OUT.
#
# Every OS:
#   runner.json            the runner's specs (image, CPU, memory, webview version)
#   build.log, bundles     `pnpm tauri build` with the OS's bundles (U7) and the build time
#   selftest-<os>.json     --selftest on the TypeScript fixture: U3(a) style checks, U3(c) boxes,
#                          U6 probes, IPC timings; shots/<theme>-<screen>.png of each screen (U3, U8)
#   selftest-<os>-engine.json  --selftest on the copied regtest wallet (U1 rows, real history_page)
# Windows (ci/gate/tauri-windows.ps1): NSIS and MSI installs, the installed app on the copied wallet
#   for 10 minutes (U2), the UIA walk (U4).
# macOS: the dmg's .app copied to ~/Applications and launched (U7, U8).
# Linux: apps/desktop/scripts/gate/linux-gate.sh under Xvfb (U4 AT-SPI, U5) as on agentbox.
#
# The wallet is ci/gate/fixtures/regtest40k-dw.tar.gz: dw-engine's datadir for the wallet `default`
# of rust-dashcore's regtest-40000 test chain (dashpay/regtest-blockchain v0.0.4), restored and
# scanned with dwcli on agentbox; its vault passphrase is ci/gate/fixtures/regtest40k-passphrase.txt
# (a public test value). The app opens it offline (no --sync): no node is needed on the runner.
set -euo pipefail

: "${GATE_OUT:?GATE_OUT is not set}" "${GATE_OS:?GATE_OS is not set}"
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
OUT=$GATE_OUT
TMP=${RUNNER_TEMP:-$OUT/tmp}
# Git Bash: drive-letter paths ("D:\a\_temp") make GNU tar look for a remote host; use /d/a/_temp.
if [[ "$GATE_OS" == windows ]]; then
  OUT=$(cygpath -u "$OUT")
  TMP=$(cygpath -u "$TMP")
fi
mkdir -p "$OUT/shots" "$TMP"
exe=dash-wallet
[[ "$GATE_OS" != windows ]] || exe=dash-wallet.exe
APP=$REPO/rust/target/release/$exe
FIXTURE=(--fixture --network testnet)
# Self-tests run at one viewport on every OS (U3c compares boxes); it fits the runners' 1024x768
# desktops and stays above the 980 px layout breakpoint. Playwright: DW_VIEWPORT=1000x620.
export GATE_WINDOW_SIZE=${GATE_WINDOW_SIZE:-1000x620}
SIZE=(--window-size "$GATE_WINDOW_SIZE")
# Screenshot pause per self-test screen (ms): long enough for a capture on a slow runner.
PAUSE_MS=${PAUSE_MS:-2500}

# macOS runners have no coreutils `timeout`; perl's alarm survives the exec.
run_limited() {
  local s=$1
  shift
  if command -v timeout >/dev/null; then timeout "$s" "$@"; else perl -e 'alarm shift; exec @ARGV' "$s" "$@"; fi
}

echo "== runner specs"
{
  echo "{"
  echo "  \"os\": \"$GATE_OS\","
  echo "  \"runnerImage\": \"${ImageOS:-?} ${ImageVersion:-?}\","
  echo "  \"runnerName\": \"${RUNNER_NAME:-?}\","
  echo "  \"arch\": \"$(uname -m)\","
  case $GATE_OS in
    linux)
      echo "  \"cpu\": \"$(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs)\","
      echo "  \"cores\": $(nproc),"
      echo "  \"memoryMB\": $(($(awk '/MemTotal/ {print $2}' /proc/meminfo) / 1024)),"
      echo "  \"system\": \"$(. /etc/os-release && echo "$PRETTY_NAME")\","
      echo "  \"webview\": \"WebKitGTK $(pkg-config --modversion webkit2gtk-4.1 2>/dev/null || echo ?)\","
      ;;
    macos)
      echo "  \"cpu\": \"$(sysctl -n machdep.cpu.brand_string)\","
      echo "  \"cores\": $(sysctl -n hw.ncpu),"
      echo "  \"memoryMB\": $(($(sysctl -n hw.memsize) / 1048576)),"
      echo "  \"system\": \"macOS $(sw_vers -productVersion) ($(sw_vers -buildVersion))\","
      echo "  \"webview\": \"WKWebView (Safari $(defaults read /Applications/Safari.app/Contents/Info.plist CFBundleShortVersionString 2>/dev/null || echo ?))\","
      ;;
    windows)
      powershell.exe -NoProfile -Command '
        $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
        $os = Get-CimInstance Win32_OperatingSystem
        $key = "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
        $wv = (Get-ItemProperty -Path $key -ErrorAction SilentlyContinue).pv
        "  `"cpu`": `"$($cpu.Name.Trim())`","
        "  `"cores`": $([Environment]::ProcessorCount),"
        "  `"memoryMB`": $([int]($os.TotalVisibleMemorySize / 1024)),"
        "  `"system`": `"$($os.Caption) $($os.Version)`","
        "  `"webview`": `"WebView2 $wv`","' | tr -d '\r'
      ;;
  esac
  echo "  \"load\": \"$(uptime 2>/dev/null | sed 's/.*load average[s]*: //' || echo n/a)\""
  echo "}"
} >"$OUT/runner.json"
cat "$OUT/runner.json"

echo "== U7: pnpm tauri build"
case $GATE_OS in
  windows) bundles=nsis,msi ;;
  macos) bundles=app,dmg ;;
  linux) bundles=deb,appimage ;;
esac
t0=$(date +%s)
(cd "$REPO/apps/desktop" && pnpm tauri build --bundles "$bundles") >"$OUT/build.log" 2>&1 \
  || { tail -n 60 "$OUT/build.log"; echo "::error::pnpm tauri build failed"; exit 1; }
build_s=$(($(date +%s) - t0))
echo "built in ${build_s}s"
bundle_dir=$REPO/rust/target/release/bundle
find "$bundle_dir" -maxdepth 2 -type f \( -name '*.exe' -o -name '*.msi' -o -name '*.dmg' -o -name '*.deb' -o -name '*.AppImage' \) \
  -exec ls -l {} \; | tee "$OUT/bundles.txt"
ls -l "$APP" | tee -a "$OUT/bundles.txt"

echo "== the copied wallet"
WALLET=$TMP/dw
rm -rf "$WALLET"
mkdir -p "$WALLET"
tar -xzf "$REPO/ci/gate/fixtures/regtest40k-dw.tar.gz" -C "$WALLET"
PASSPHRASE_FILE=$REPO/ci/gate/fixtures/regtest40k-passphrase.txt
# Regtest needs a DAPI address even with --no-platform; the app never connects to it.
ENGINE=(--datadir "$WALLET" --network regtest --no-platform --dapi https://127.0.0.1:1443)
du -sh "$WALLET" | tee "$OUT/wallet-size.txt"

# The window of PID as an image, by CGWindowID (macOS only: Linux and Windows capture in their own
# harnesses).
window_id_tool=$TMP/macos-window-id
shot() { # PID FILE
  local w
  w=$("$window_id_tool" "$1" || true)
  if [[ -n "$w" ]]; then screencapture -x -o -l "$w" "$2"; else screencapture -x "$2"; fi
}

# --selftest with a capture of each screen while it pauses (macOS).
selftest_with_shots() { # REPORT LOG ARGS...
  local report=$1 log=$2 seen=0 count screen pid deadline=$(($(date +%s) + 900))
  shift 2
  "$APP" "$@" "${SIZE[@]}" --selftest "$report" --selftest-pause "$PAUSE_MS" >"$log" 2>&1 &
  pid=$!
  # macOS ships bash 3.2: no mapfile, and empty arrays trip `set -u`, so read the log line by line.
  while kill -0 "$pid" 2>/dev/null && (($(date +%s) < deadline)); do
    count=$(grep -c 'selftest-screen [a-zA-Z/]' "$log" || true)
    while ((seen < count)); do
      screen=$(grep -o 'selftest-screen [a-zA-Z/]*' "$log" | sed -n "$((seen + 1))p" | awk '{print $2}')
      sleep 0.8
      shot "$pid" "$OUT/shots/${screen//\//-}.png" || true
      seen=$((seen + 1))
    done
    sleep 0.2
  done
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

# Summarises a self-test report; fails unless it is the final report and no check failed.
judge_selftest() { # REPORT NAME
  [[ -s "$1" ]] || { echo "::error::$2 wrote no report"; return 1; }
  jq -r --arg n "$2" '
    "### \($n)",
    "- checks: \(.checksTotal) total, \(.checksFailed) failed, \(.checksNotApplicable) not applicable",
    "- first paint: \(.firstPaintMs // "n/a") ms; shell paint: \(.shellPaintMs // "n/a") ms",
    (.ipc // {} | to_entries[] | "- ipc \(.key): p50 \(.value.p50) ms, p95 \(.value.p95) ms"),
    (.ipcSplit // {} | to_entries[] | "- \(.key) split p95: round trip \(.value.roundTrip.p95), engine \(.value.engine.p95), ipc \(.value.ipc.p95) ms"),
    (.security // {} | to_entries[] | "- security \(.key): \(.value)")' "$1" | tee -a "${GITHUB_STEP_SUMMARY:-/dev/null}"
  jq -e '.final == true and .checksFailed == 0' "$1" >/dev/null \
    || { echo "::error::$2: not the final report, or a check failed"; return 1; }
}

status=0
case $GATE_OS in
  macos)
    swiftc -O -o "$window_id_tool" "$REPO/ci/github/macos-window-id.swift"
    # The runner has no trackpad, so macOS draws classic scroll bars that take 15-17 px of layout
    # width; a Mac with a trackpad (the default) overlays them, as Playwright's engines do. U3c
    # compares layout, so the gate uses the trackpad default (GATE_MAC_SCROLLBARS=Automatic keeps
    # the runner's).
    defaults write -g AppleShowScrollBars -string "${GATE_MAC_SCROLLBARS:-WhenScrolling}"
    echo "AppleShowScrollBars: $(defaults read -g AppleShowScrollBars)" | tee "$OUT/macos-scrollbars.txt"
    echo "== U3/U8: self-test with screenshots (fixture)"
    selftest_with_shots "$OUT/selftest-macos.json" "$OUT/selftest-macos.log" "${FIXTURE[@]}"
    judge_selftest "$OUT/selftest-macos.json" "Tauri --selftest on macOS (fixture)" || status=1
    echo "== U1/U5: self-test on the copied wallet"
    run_limited 900 "$APP" "${ENGINE[@]}" "${SIZE[@]}" --selftest "$OUT/selftest-macos-engine.json" >"$OUT/selftest-macos-engine.log" 2>&1 || true
    judge_selftest "$OUT/selftest-macos-engine.json" "Tauri --selftest on macOS (regtest wallet)" || status=1
    echo "== U7: the dmg installs and launches"
    dmg=$(ls "$bundle_dir"/dmg/*.dmg | head -1)
    mnt=$TMP/dmg
    mkdir -p "$mnt" "$HOME/Applications"
    hdiutil attach -nobrowse -readonly -mountpoint "$mnt" "$dmg" | tee "$OUT/dmg-attach.txt"
    rm -rf "$HOME/Applications/Dash Wallet.app"
    cp -R "$mnt/Dash Wallet.app" "$HOME/Applications/"
    hdiutil detach "$mnt"
    for theme in light dark; do
      "$REPO/ci/github/macos-launch-screenshot.sh" "dmg-app-fixture-$theme" "$OUT/shots" 60 \
        "$HOME/Applications/Dash Wallet.app" "${FIXTURE[@]}" --theme "$theme" || status=1
    done
    "$REPO/ci/github/macos-launch-screenshot.sh" "dmg-app-wallet" "$OUT/shots" 90 \
      "$HOME/Applications/Dash Wallet.app" "${ENGINE[@]}" || status=1
    ;;
  windows)
    echo "== U3: self-test, U2 install and 10-minute run, U4 UIA walk (tauri-windows.ps1)"
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$REPO/ci/gate/tauri-windows.ps1")" \
      -Repo "$(cygpath -w "$REPO")" -OutDir "$(cygpath -w "$OUT")" -App "$(cygpath -w "$APP")" \
      -Wallet "$(cygpath -w "$WALLET")" -PassphraseFile "$(cygpath -w "$PASSPHRASE_FILE")" -PauseMs "$PAUSE_MS" -WindowSize "$GATE_WINDOW_SIZE" || status=1
    judge_selftest "$OUT/selftest-windows.json" "Tauri --selftest on Windows (fixture)" || status=1
    judge_selftest "$OUT/selftest-windows-engine.json" "Tauri --selftest on Windows (regtest wallet)" || status=1
    ;;
  linux)
    echo "== the agentbox harness under Xvfb"
    # The runner is not shared: drop the page cache before each of the first 4 of 8 starts (real
    # cold starts), and keep 4 warm ones.
    RUNS=8 DROP_CACHES=4 PASSPHRASE_FILE=$PASSPHRASE_FILE "$REPO/apps/desktop/scripts/gate/linux-gate.sh" "$OUT/linux" "${ENGINE[@]}" || status=1
    ;;
esac

echo "build seconds: $build_s" >>"$OUT/bundles.txt"
exit "$status"
