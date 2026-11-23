#!/usr/bin/env bash
# Runs INSIDE dwd-linux-crossui (see scripts/crossui-linux-demo.sh): repo at
# /src (read-only), output at /out. Builds the core and dash-wallet, then runs
# the demo under Xvfb + D-Bus + AT-SPI and records trees and screenshots.
set -euo pipefail
OUT=${OUT:-/out}

if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
  mkdir -p /work
  tar -C /src --exclude=.build --exclude=./rust/target --exclude=./.derived --exclude=./.claude --exclude=./Artifacts --exclude=./.build-logs --exclude=.swiftpm \
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
  # Unit tests of the vendored SwiftCrossUI's patches. Built as the test product:
  # plain `swift test` also builds swift-winui's Windows-only C target and fails here.
  swift build --scratch-path /swiftpm/.build --product DashWalletDesktopPackageTests
  "$(dirname "$bin")/DashWalletDesktopPackageTests.xctest" --testing-library swift-testing \
    --filter SwiftCrossUIPatchTests
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

# atspi_demo.py appends to atspi-checks.json; start each run with a fresh file.
rm -f "$OUT/atspi-checks.json"

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
m1_sessions() {
  session demo "1-overview,2-send=select:Send" --demo
  session transactions "3-transactions" --demo --page transactions
  session receive "4-receive" --demo --page receive
  session onboarding "5-onboarding" --demo onboarding
  # Whole flows driven through AT-SPI (typing into named fields, pressing buttons).
  session onboarding-flow "6-onboarding=flow:onboarding" --demo onboarding
  session send-flow "7-send=flow:send" --demo --page send
  # Status-row unit selector, peers page and address-book QR code; the sync overlay.
  session tools-flow "9-tools=flow:tools" --demo
  session overlay-flow "10-overlay=flow:overlay" --demo offline
}

# M2 screens (DWD_CROSSUI_SUITE=m2): menu bar and window title, Options and
# coin selection, the Tools window, PSBT loading, Wallets / Security / About
# and transaction details.
m2_sessions() {
  session m2-menus "m2-1-menus=flow:m2-menus" --demo
  session m2-options "m2-2-options=flow:m2-options" --demo --page options
  session m2-tools "m2-3-tools=flow:m2-tools" --demo --page tools-console
  session m2-psbt "m2-4-psbt=flow:m2-psbt" --demo --page psbt
  session m2-pages "m2-5-pages=flow:m2-pages" --demo
}

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

# Live mode with dash-qt's -choosedatadir (QT-004): the Intro page, OK with
# the default directory, then the wallet opens on it.
live_chooser() {
  export XDG_DATA_HOME=/tmp/xdg-chooser
  rm -rf "$XDG_DATA_HOME"
  "$BIN" -choosedatadir --network regtest --dapi http://127.0.0.1:1 --connect 127.0.0.1:1 \
    >"$OUT/app-live-chooser.log" 2>&1 &
  local pid=$!
  echo "== live-chooser: launched dash-wallet -choosedatadir (pid $pid)"
  python3 /work/ci/linux/crossui/atspi_demo.py --pid "$pid" --out "$OUT" --steps "m2-6-live=flow:m2-chooser" || status=$?
  if [[ -d "$XDG_DATA_HOME/dashwallet/regtest" ]]; then
    echo "== live-chooser: PASS regtest data under the chosen (default) directory"
  else
    echo "== live-chooser: FAIL no \$XDG_DATA_HOME/dashwallet/regtest"; status=2
  fi
  python3 /work/ci/linux/crossui/close_window.py "Dash Wallet" || status=2
  for _ in $(seq 60); do kill -0 "$pid" 2>/dev/null || break; sleep 0.5; done
  if kill -0 "$pid" 2>/dev/null; then kill "$pid"; status=2; echo "== live-chooser: did not exit after close"; fi
  grep -q "engine shut down" "$OUT/app-live-chooser.log" && echo "== live-chooser: PASS engine shut down" \
    || { echo "== live-chooser: FAIL no engine shutdown message"; status=2; }
}

# UX restyle (DWD_CROSSUI_SUITE=ux, UX-SPEC §7): every restyled screen in
# light and dark, plus onboarding, lock, sync overlay and the gallery.
ux_sessions() {
  for appearance in light dark; do
    session "ux-$appearance-main" "ux-$appearance=flow:ux-main" --demo --appearance "$appearance"
    session "ux-$appearance-onboarding" "ux-$appearance-onboarding=flow:ux-single" --demo onboarding --appearance "$appearance"
    session "ux-$appearance-lock" "ux-$appearance-lock=flow:ux-single" --demo locked --appearance "$appearance"
    session "ux-$appearance-overlay" "ux-$appearance-overlay=flow:ux-single" --demo offline --appearance "$appearance"
  done
  session ux-gallery "ux-light-gallery=flow:ux-single" --gallery
}

if [[ "${DWD_CROSSUI_SUITE:-m1}" == ux ]]; then
  # The M1 flows and the M2 menus/options/pages first, as a regression
  # check; their screenshots stay in the container, their checks are kept.
  ux_out=$OUT
  OUT=/tmp/regression
  mkdir -p "$OUT"
  m1_sessions
  session m2-menus "m2-1-menus=flow:m2-menus" --demo
  session m2-options "m2-2-options=flow:m2-options" --demo --page options
  session m2-tools "m2-3-tools=flow:m2-tools" --demo --page tools-console
  session m2-pages "m2-5-pages=flow:m2-pages" --demo
  cp "$OUT/atspi-checks.json" "$ux_out/regression-checks.json"
  OUT=$ux_out
  ux_sessions
elif [[ "${DWD_CROSSUI_SUITE:-m1}" == m2 ]]; then
  # The M1 flows run first as a regression check; their screenshots stay in
  # the container and only their checks are kept.
  m2_out=$OUT
  OUT=/tmp/m1-regression
  mkdir -p "$OUT"
  m1_sessions
  live
  cp "$OUT/atspi-checks.json" "$m2_out/m1-regression-checks.json"
  OUT=$m2_out
  m2_sessions
  live_chooser
else
  m1_sessions
  live
fi
exit "$status"
