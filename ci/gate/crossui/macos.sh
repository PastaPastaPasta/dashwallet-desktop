#!/usr/bin/env bash
# Gate G-UI on macos-latest (ci/gate/swiftcrossui.sh): SwiftCrossUI's dash-wallet with AppKitBackend.
#   U8  build, launch, a screenshot of each screen in light and dark; no crash
#   U3  the same screenshots, for review against UX-SPEC's Cross column
#   U7  an unsigned .app and dmg: the dmg mounts, the app installs to /Applications and launches
# The app opens the gate wallet (GATE_WALLET) on regtest with no reachable peer. With no AT-SPI
# here, GateProbe's hooks (Sources/DashWalletCross/GateProbe.swift) unlock the vault and open a
# transaction's details; its marks say when each screen is up.
set -euo pipefail
: "${GATE_ROOT:?}" "${GATE_OUT:?}" "${GATE_WORK:?}" "${GATE_WALLET:?}" "${GATE_PASSPHRASE_FILE:?}"
cd "$GATE_ROOT"
OUT="$GATE_OUT"
results="$OUT/results.txt"
: >"$results"
result() { echo "$*" | tee -a "$results"; }

# --- build (release, AppKitBackend) ---
t0=$(date +%s)
scripts/build-core.sh --profile release --no-bindings
t1=$(date +%s)
swift build -c release --product dash-wallet
t2=$(date +%s)
bin_dir=$(swift build -c release --show-bin-path)
result "build: core $((t1 - t0)) s, swift $((t2 - t1)) s (release)"

# --- U7: unsigned .app and dmg ---
app="$GATE_WORK/Dash Wallet.app"
rm -rf "$app" && mkdir -p "$app/Contents/MacOS"
cp "$bin_dir/dash-wallet" "$app/Contents/MacOS/dash-wallet"
# SwiftPM's generated Bundle.module looks for the resource bundles at Bundle.main.bundleURL, the
# .app's root, not Contents/Resources (where codesign wants them; a signed app needs Swift
# Bundler or a custom accessor).
for bundle in "$bin_dir"/*.bundle; do cp -R "$bundle" "$app/"; done
cat >"$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>dash-wallet</string>
  <key>CFBundleIdentifier</key><string>org.dash.wallet.desktop.crossui</string>
  <key>CFBundleName</key><string>Dash Wallet</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
dmg="$OUT/DashWallet-crossui-unsigned.dmg"
hdiutil create -volname "Dash Wallet" -srcfolder "$app" -ov -format UDZO "$dmg" >/dev/null
mount="$GATE_WORK/dmg-mount"
mkdir -p "$mount"
hdiutil attach -nobrowse -readonly -mountpoint "$mount" "$dmg" >/dev/null
rm -rf "/Applications/Dash Wallet.app"
cp -R "$mount/Dash Wallet.app" /Applications/
hdiutil detach "$mount" >/dev/null
# The build directory's bundles would mask a bundle missing from the app (SwiftPM's accessor
# falls back to them): move them away so the installed app must use its own.
for bundle in "$bin_dir"/*.bundle; do mv "$bundle" "$bundle.moved"; done
installed="/Applications/Dash Wallet.app/Contents/MacOS/dash-wallet"
result "U7 dmg: $(stat -f %z "$dmg") bytes; app $(du -sk "/Applications/Dash Wallet.app" | cut -f1) KB; installed from the dmg"

window_id="$GATE_WORK/window-id"
swiftc -O -o "$window_id" ci/github/macos-window-id.swift

# shows LOG MARK [AFTER]: MARK is GateProbe's latest screen (`*-paint`) mark in LOG, written after
# its first AFTER mark, if given.
shows() {
  awk -v m="$2" -v a="${3:-}" '$1 != "gate-mark" { next } a == "" || $2 == a { on = 1 }
    on && $2 ~ /-paint$/ { last = $2 } END { exit last != m }' "$1"
}

# screen NAME APPEARANCE MARK [ENV=VALUE...] -- [APP ARGS...]: launch the installed app on a
# fresh copy of the gate wallet, wait until GateProbe's latest screen mark is MARK, screenshot the
# window, check it is still alive, quit. A run that unlocks counts only marks after its unlock:
# before the vault reports it is locked, the app can show Overview or Transactions (and the sync
# overlay, which GateProbe hides) and only then the lock screen.
status=0
screen() {
  local name=$1 appearance=$2 mark=$3
  shift 3
  local envs=()
  while (($#)) && [[ "$1" != -- ]]; do envs+=("$1"); shift; done
  [[ "${1:-}" == -- ]] && shift
  local after=""
  [[ " ${envs[*]-} " == *" DWD_GATE_UNLOCK_FILE="* ]] && after=unlock-requested
  local datadir="$GATE_WORK/run-datadir"
  rm -rf "$datadir" && cp -R "$GATE_WALLET" "$datadir"
  local log="$OUT/$name.log" err="$OUT/$name.err.log"
  local start
  start=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
  env DWD_GATE=1 DWD_GATE_HIDE_SYNC_OVERLAY=1 ${envs[@]+"${envs[@]}"} "$installed" --network regtest --datadir "$datadir" \
    --connect 127.0.0.1:1 --dapi http://127.0.0.1:1 --appearance "$appearance" "$@" >"$log" 2>"$err" &
  local pid=$!
  local seen=""
  for _ in $(seq 360); do
    if shows "$err" "$mark" "$after"; then seen=1; break; fi
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.5
  done
  local waited
  waited=$(perl -MTime::HiRes=time -e "printf '%.1f', time - $start")
  if [[ -z "$seen" ]]; then
    if kill -0 "$pid" 2>/dev/null; then
      result "$name: FAIL no '$mark' in ${waited}s"
    else
      wait "$pid" && code=0 || code=$?
      result "$name: FAIL exited ($code) before '$mark'"
    fi
    screencapture -x "$OUT/$name-screen.png" || true
    tail -n 30 "$err" || true
    status=1
  else
    sleep 3  # the frame after the model change, and AppKit's layout pass
    shows "$err" "$mark" "$after" || result "$name: WARN the screen changed after '$mark'"
    local window
    window=$("$window_id" "$pid" || true)
    if [[ -n "$window" ]]; then
      screencapture -x -o -l "$window" "$OUT/$name.png"
    else
      screencapture -x "$OUT/$name-screen.png"
      "$window_id" --list "$pid" || true
    fi
    if kill -0 "$pid" 2>/dev/null; then
      result "$name: ok '$mark' after ${waited}s; window ${window:-none}; rss $(ps -o rss= -p "$pid" | tr -d ' ') KB"
    else
      result "$name: FAIL exited after '$mark'"
      status=1
    fi
  fi
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

unlock="DWD_GATE_UNLOCK_FILE=$GATE_PASSPHRASE_FILE"
for appearance in light dark; do
  screen "$appearance-unlock" "$appearance" lock-paint
  screen "$appearance-overview" "$appearance" overview-paint "$unlock"
  screen "$appearance-transactions" "$appearance" transactions-paint "$unlock" -- --page transactions
  screen "$appearance-detail" "$appearance" detail-paint "$unlock" DWD_GATE_OPEN_DETAIL=1 -- --page transactions
done

# U7: the installed app through LaunchServices, as a user starts it.
open -n "/Applications/Dash Wallet.app" --args --demo
sleep 20
if pid=$(pgrep -n -f "^$installed"); then
  w=$("$window_id" "$pid" || true)
  [[ -n "$w" ]] && screencapture -x -o -l "$w" "$OUT/u7-launchservices-demo.png"
  result "U7 launch through LaunchServices: running (pid $pid), window ${w:-none}"
  kill "$pid" || true
else
  result "U7 launch through LaunchServices: FAIL not running after 20 s"
  status=1
fi
ls -1 ~/Library/Logs/DiagnosticReports/ 2>/dev/null | grep -i dash && cp ~/Library/Logs/DiagnosticReports/*dash* "$OUT/" 2>/dev/null || true
exit "$status"
