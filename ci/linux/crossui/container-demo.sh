#!/usr/bin/env bash
# Runs INSIDE dwd-linux-crossui (see scripts/crossui-linux-demo.sh): repo at
# /src (read-only), output at /out. Builds the core and dash-wallet, then runs
# the demo under Xvfb + D-Bus + AT-SPI and records trees and screenshots.
set -euo pipefail
OUT=${OUT:-/out}

if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
  mkdir -p /work
  tar -C /src --exclude=./.build --exclude=./Artifacts --exclude=./.build-logs --exclude=./.swiftpm \
      --exclude=./docs/screenshots -cf - . | tar -C /work -xf -
  cd /work
  start=$(date +%s)
  scripts/build-core.sh --no-bindings
  echo "== build-core took $(( $(date +%s) - start ))s; target volume $(du -sh /target | cut -f1)"
  start=$(date +%s)
  swift build --scratch-path /swiftpm/.build --product dash-wallet
  echo "== swift build dash-wallet took $(( $(date +%s) - start ))s"
  bin=$(swift build --scratch-path /swiftpm/.build --show-bin-path)/dash-wallet
  echo "== binary: $(file -b "$bin" | cut -c1-80)"
  # Re-exec under a private session bus so Xvfb, AT-SPI and the app share it.
  exec dbus-run-session -- env BIN="$bin" "$0"
fi

export DISPLAY=:99
Xvfb "$DISPLAY" -screen 0 1280x860x24 -nolisten tcp >"$OUT/xvfb.log" 2>&1 &
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
/usr/libexec/at-spi-bus-launcher --launch-immediately >/dev/null 2>&1 &
for _ in $(seq 50); do
  dbus-send --session --print-reply --dest=org.a11y.Bus /org/a11y/bus org.a11y.Bus.GetAddress >/dev/null 2>&1 && break
  sleep 0.1
done

status=0
# session NAME SCRIPT-STEPS APP-ARGS...: launch, drive/dump, quit.
session() {
  local name=$1 steps=$2; shift 2
  "$BIN" "$@" >"$OUT/app-$name.log" 2>&1 &
  local pid=$!
  echo "== $name: launched dash-wallet $* (pid $pid)"
  python3 /work/ci/linux/crossui/atspi_demo.py --pid "$pid" --out "$OUT" --steps "$steps" || status=$?
  if kill -0 "$pid" 2>/dev/null; then
    echo "== $name: alive at end (rss $(ps -o rss= -p "$pid" | tr -d ' ') KB)"
    kill "$pid"; wait "$pid" 2>/dev/null || true
  else
    local code=0; wait "$pid" || code=$?; echo "== $name: EXITED EARLY with status $code"; status=2
  fi
}

# Steps: NAME (wait, dump tree, screenshot) or NAME=select:ITEM (pick a sidebar
# item through the AT-SPI Selection interface first).
session demo "1-overview,2-send=select:Send" --demo
session transactions "3-transactions" --demo --page transactions
session receive "4-receive" --demo --page receive
session onboarding "5-onboarding" --demo onboarding
# Whole flows driven through AT-SPI (typing into named fields, pressing buttons).
session onboarding-flow "6-onboarding=flow:onboarding" --demo onboarding
session send-flow "7-send=flow:send" --demo --page send

# Live mode on the real engine: data root from XDG_DATA_HOME, regtest with no
# reachable node. Creates a wallet through the onboarding flow, then closes
# the window the way a window manager would and checks that the engine was
# shut down before the process exited.
live() {
  export XDG_DATA_HOME=/tmp/xdg-live
  rm -rf "$XDG_DATA_HOME"
  "$BIN" --network regtest --dapi http://127.0.0.1:1 --connect 127.0.0.1:1 >"$OUT/app-live.log" 2>&1 &
  local pid=$!
  echo "== live: launched dash-wallet (pid $pid), XDG_DATA_HOME=$XDG_DATA_HOME"
  python3 /work/ci/linux/crossui/atspi_demo.py --pid "$pid" --out "$OUT" --steps "8-live=flow:onboarding" || status=$?
  echo "== live: data root contents"; (cd "$XDG_DATA_HOME" && find . -maxdepth 2 | sort)
  if [[ -d "$XDG_DATA_HOME/dashwallet/regtest" ]]; then
    echo "== live: PASS regtest data under \$XDG_DATA_HOME/dashwallet"
  else
    echo "== live: FAIL no \$XDG_DATA_HOME/dashwallet/regtest"; status=2
  fi
  python3 /work/ci/linux/crossui/close_window.py "Dash Wallet" || status=2
  local code=timeout
  for _ in $(seq 60); do
    if ! kill -0 "$pid" 2>/dev/null; then code=0; wait "$pid" || code=$?; break; fi
    sleep 0.5
  done
  echo "== live: exit after window close: $code"
  if [[ "$code" == timeout ]]; then kill "$pid" 2>/dev/null || true; status=2; fi
  if grep -q "engine shut down" "$OUT/app-live.log"; then
    echo "== live: PASS engine shut down before exit"
  else
    echo "== live: FAIL no engine shutdown message"; status=2
  fi
}
live
exit "$status"
