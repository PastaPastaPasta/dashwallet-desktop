#!/usr/bin/env bash
# Gate G-UI pre-measurements of the Tauri app on Linux (DASHPAY §2.1: U3, U4, U5, U6, U7).
#
# Runs on the agentbox host under a private Xvfb display and a private D-Bus session with the
# AT-SPI bus; nothing appears on a real display. Needs the release build
# (`pnpm tauri build`), xdotool, ImageMagick, python3-pyatspi and at-spi2-core.
#
# Usage (from the repo root):
#   apps/desktop/scripts/gate/linux-gate.sh OUT_DIR [ENGINE_ARGS...]
# ENGINE_ARGS, when given, are passed to the engine-mode runs (U1 rows, real IPC, engine PSS),
# for example: --datadir /tmp/dw --network regtest --spv-peer 127.0.0.1:29899 --dapi https://127.0.0.1:1443
# With PASSPHRASE_FILE set, the engine run unlocks the vault with keystrokes (first line of the
# file) and screenshots the unlocked Overview and Transactions.
#
# With DW_GATE_DISPLAY=:N (and XAUTHORITY) set, the harness uses that X display (an `agent-gui`
# session) instead of starting its own Xvfb.
#
# Every start-up sample records the 1-minute load and, next to the wall-clock times, the CPU time
# (user + system, all processes of the app) spent until the first paint: on a loaded machine the
# wall clock swings with the load, the CPU time much less.
#
# Writes OUT_DIR/linux-gate.json plus screenshots, AT-SPI dumps and the self-test reports.
set -euo pipefail

if [[ -z "${DW_GATE_INNER:-}" ]]; then
  exec dbus-run-session -- env DW_GATE_INNER=1 "$0" "$@"
fi

OUT=$(realpath -m "${1:?usage: linux-gate.sh OUT_DIR [ENGINE_ARGS...]}")
shift
ENGINE_ARGS=("$@")
REPO=$(cd "$(dirname "$0")/../../../.." && pwd)
BIN=${BIN:-$REPO/rust/target/release/dash-wallet}
RUNS=${RUNS:-5}
# DROP_CACHES=N makes the first N runs cold starts by dropping the page cache before each (needs
# sudo). Off by default: on a shared machine it disturbs every other process's measurements.
DROP_CACHES=${DROP_CACHES:-0}
mkdir -p "$OUT"
cd "$OUT"

if [[ -n "${DW_GATE_DISPLAY:-}" ]]; then
  export DISPLAY=$DW_GATE_DISPLAY
else
  export DISPLAY=:77
  Xvfb "$DISPLAY" -screen 0 1440x900x24 -nolisten tcp >xvfb.log 2>&1 &
  XVFB=$!
  trap 'kill $XVFB 2>/dev/null || true' EXIT
fi
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
# Services D-Bus starts (the xdg-desktop-portal backends) need the display, as in a real session.
dbus-update-activation-environment DISPLAY ${XAUTHORITY:+XAUTHORITY}
/usr/libexec/at-spi-bus-launcher --launch-immediately >/dev/null 2>&1 &
for _ in $(seq 50); do
  dbus-send --session --print-reply --dest=org.a11y.Bus /org/a11y/bus org.a11y.Bus.GetAddress >/dev/null 2>&1 && break
  sleep 0.1
done

FIXTURE=(--fixture --network testnet)
UPTIME_START=$(uptime)

# Monotonic ms (CLOCK_BOOTTIME via /proc/uptime, 10 ms steps): the wall clock can step on a busy VM.
now_ms() {
  local up
  read -r up _ </proc/uptime
  echo $((10#${up/./} * 10))
}

# PSS (kB) of the process and all its descendants (UI + engine, WebKitWebProcess, WebKitNetworkProcess),
# as JSON: {"pssKb": total, "processes": {"comm[pid]": kB, ...}}.
pss_tree() {
  local total=0 kb parts="" p
  for p in $(tree_pids "$1"); do
    kb=$(awk '/^Pss:/ {print $2}' "/proc/$p/smaps_rollup" 2>/dev/null || echo 0)
    total=$((total + kb))
    parts+="\"$(cat "/proc/$p/comm" 2>/dev/null)[$p]\":$kb,"
  done
  echo "{\"pssKb\":$total,\"processes\":{${parts%,}}}"
}

load1() { cut -d' ' -f1 /proc/loadavg; }

# Descendants of PID, PID first.
tree_pids() {
  local pids=("$1") i=0
  while ((i < ${#pids[@]})); do
    mapfile -t -O "${#pids[@]}" pids < <(pgrep -P "${pids[$i]}" || true)
    i=$((i + 1))
  done
  echo "${pids[@]}"
}

# User + system CPU ms of PID and its descendants (processes that already exited are not counted).
cpu_ms_tree() {
  local tck total=0 p t
  tck=$(getconf CLK_TCK)
  for p in $(tree_pids "$1"); do
    # Fields 14 and 15 of /proc/PID/stat, counted after the parenthesised command name.
    t=$(awk '{sub(/.*\) /, ""); print $12 + $13}' "/proc/$p/stat" 2>/dev/null) || continue
    total=$((total + t))
  done
  echo $((total * 1000 / tck))
}
mark_ms() { grep -o "startup $2 [0-9]*" "$1" | awk '{print $3}'; } # LOG MARK

wait_line() { # FILE PATTERN TIMEOUT_S
  local deadline=$(($(date +%s) + $3))
  while (($(date +%s) < deadline)); do
    grep -q "$2" "$1" 2>/dev/null && return 0
    sleep 0.05
  done
  return 1
}

window_shot() { # NAME
  local win
  win=$(xdotool search --onlyvisible --name "Dash Wallet" 2>/dev/null | head -1 || true)
  if [[ -n "$win" ]]; then import -window "$win" "shot-$1.png"; else import -window root "shot-$1.png"; fi
}

echo "== U3: self-test (fixture) with screenshots"
# U3c compares boxes across engines at one viewport that fits every CI runner's desktop.
SIZE=(--window-size "${GATE_WINDOW_SIZE:-1000x620}")
log=selftest-fixture.log
"$BIN" "${FIXTURE[@]}" "${SIZE[@]}" --selftest selftest-webkitgtk.json --selftest-pause 1500 >"$log" 2>&1 &
pid=$!
seen=0
while kill -0 $pid 2>/dev/null; do
  # Complete lines only: a line read while the app writes it would give a cut-off name.
  mapfile -t screens < <(head -n "$(wc -l <"$log")" "$log" | grep -o 'selftest-screen [a-zA-Z/]*' | awk '{print $2}')
  while ((seen < ${#screens[@]})); do
    sleep 0.4
    window_shot "${screens[$seen]//\//-}"
    seen=$((seen + 1))
  done
  sleep 0.1
done
wait $pid || echo "self-test exited with $?"

echo "== U4: AT-SPI walk (fixture)"
"$BIN" "${FIXTURE[@]}" >app-atspi.log 2>&1 &
pid=$!
atspi_status=0
python3 "$REPO/apps/desktop/scripts/gate/atspi_walk.py" --pid $pid --out atspi || atspi_status=$?
kill $pid 2>/dev/null || true
wait $pid 2>/dev/null || true

echo "== U5: cold and warm start ($RUNS runs, fixture)"
start_json="["
for i in $(seq "$RUNS"); do
  if ((i <= DROP_CACHES)) && sudo -n true 2>/dev/null; then
    sync
    sudo -n sh -c 'echo 3 > /proc/sys/vm/drop_caches'
    cold=true
  else
    cold=false
  fi
  log=start-$i.log
  t0=$(now_ms)
  "$BIN" "${FIXTURE[@]}" >"$log" 2>&1 &
  pid=$!
  load=$(load1)
  if wait_line "$log" "startup first-paint" 60; then
    t1=$(now_ms)
    cpu=$(cpu_ms_tree $pid)
    start_json+="{\"run\":$i,\"dropCaches\":$cold,\"load1\":$load,\"shellMs\":$(mark_ms "$log" shell),\"firstPaintMs\":$(mark_ms "$log" first-paint),\"spawnToFirstPaintMs\":$((t1 - t0)),\"cpuMsAtFirstPaint\":$cpu},"
  else
    start_json+="{\"run\":$i,\"dropCaches\":$cold,\"load1\":$load,\"error\":\"no first paint in 60 s\"},"
  fi
  if ((i == RUNS)); then
    sleep 20
    fixture_pss="{\"load1\":$(load1),\"tree\":$(pss_tree $pid)}"
  fi
  kill $pid 2>/dev/null || true
  wait $pid 2>/dev/null || true
done
start_json="${start_json%,}]"

engine_json=null
if ((${#ENGINE_ARGS[@]})); then
  echo "== U1/U5: engine mode ${ENGINE_ARGS[*]}"
  timeout 600 "$BIN" "${ENGINE_ARGS[@]}" "${SIZE[@]}" --selftest selftest-engine.json >selftest-engine.log 2>&1 || echo "engine self-test exited with $?"
  # Start-up on the wallet: the window, then Overview with balance and history (the engine opens
  # the wallet's history first: engine-bound, see the report).
  engine_starts="["
  for i in $(seq "${ENGINE_RUNS:-3}"); do
    log=engine-start-$i.log
    t0=$(now_ms)
    "$BIN" "${ENGINE_ARGS[@]}" >"$log" 2>&1 &
    pid=$!
    load=$(load1)
    if wait_line "$log" "startup first-paint" 180; then
      t1=$(now_ms)
      engine_starts+="{\"run\":$i,\"load1\":$load,\"shellMs\":$(mark_ms "$log" shell),\"firstPaintMs\":$(mark_ms "$log" first-paint),\"spawnToFirstPaintMs\":$((t1 - t0)),\"cpuMsAtFirstPaint\":$(cpu_ms_tree $pid)},"
    else
      engine_starts+="{\"run\":$i,\"load1\":$load,\"error\":\"no first paint in 180 s\"},"
    fi
    kill $pid 2>/dev/null || true
    wait $pid 2>/dev/null || true
  done
  engine_starts="${engine_starts%,}]"
  log=engine-idle.log
  t0=$(now_ms)
  "$BIN" "${ENGINE_ARGS[@]}" >"$log" 2>&1 &
  pid=$!
  load=$(load1)
  if wait_line "$log" "startup first-paint" 180; then
    t1=$(now_ms)
    window_shot engine-locked
    if [[ -n "${PASSPHRASE_FILE:-}" ]]; then
      xdotool type --delay 15 "$(head -1 "$PASSPHRASE_FILE")"
      xdotool key Return
      sleep 8
      window_shot engine-overview
      xdotool key Tab Tab Tab Tab Tab Tab Return # toolbar (2) + sidebar: Overview, Send, Receive, Transactions
      sleep 6
      window_shot engine-transactions
    fi
    sleep 30
    engine_json="{\"starts\":$engine_starts,\"load1\":$load,\"loadIdle\":$(load1),\"shellMs\":$(mark_ms "$log" shell),\"firstPaintMs\":$(mark_ms "$log" first-paint),\"spawnToFirstPaintMs\":$((t1 - t0)),\"idle\":$(pss_tree $pid)}"
  else
    engine_json="{\"starts\":$engine_starts,\"error\":\"no first paint in 180 s\"}"
  fi
  kill $pid 2>/dev/null || true
  wait $pid 2>/dev/null || true
fi

echo "== U7: artefact sizes"
sizes="{"
for f in "$BIN" "$REPO"/rust/target/release/bundle/deb/*.deb "$REPO"/rust/target/release/bundle/appimage/*.AppImage; do
  [[ -e "$f" ]] && sizes+="\"$(basename "$f")\":$(stat -c %s "$f"),"
done
sizes="${sizes%,}}"

cat >linux-gate.json <<JSON
{
  "binary": "$BIN",
  "webkit2gtk": "$(pkg-config --modversion webkit2gtk-4.1)",
  "kernel": "$(uname -r)",
  "cpus": $(nproc),
  "uptimeStart": "$UPTIME_START",
  "uptimeEnd": "$(uptime)",
  "startup": $start_json,
  "fixtureIdle": ${fixture_pss:-null},
  "engine": $engine_json,
  "atspiExit": $atspi_status,
  "sizesBytes": $sizes
}
JSON
echo "== wrote $OUT/linux-gate.json"
cat linux-gate.json

# Exit status: the AT-SPI walk and every self-test report (final, no failed check) must pass.
selftest_ok() { python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); sys.exit(not (r.get("final") is True and r["checksFailed"] == 0))' "$1" 2>/dev/null; }
status=0
((atspi_status == 0)) || { echo "FAIL: the AT-SPI walk exited with $atspi_status"; status=1; }
reports=(selftest-webkitgtk.json)
((${#ENGINE_ARGS[@]})) && reports+=(selftest-engine.json)
for r in "${reports[@]}"; do
  selftest_ok "$r" || { echo "FAIL: $r is missing, not final, or has failed checks"; status=1; }
done
exit $status
