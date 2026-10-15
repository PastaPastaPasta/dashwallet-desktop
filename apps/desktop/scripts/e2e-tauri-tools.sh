#!/usr/bin/env bash
# U-05 acceptance: the Tools tabs on the real engine against a dashd regtest chain (e2e-tauri/tools.ts).
# Builds the app (production renderer, debug binary with the real engine) and dwcli as
# scripts/e2e-tauri.sh does, then runs the Tools flow against dashd. Not in CI: it needs dashd. Needs a
# private X display in $DISPLAY (xvfb-run, or `agent-gui start N` on agentbox), WebKitWebDriver,
# tauri-driver (see scripts/e2e-tauri.sh), xdotool, and a dashd with dash-cli next to it:
#
#   DASHD=<dir>/dashd     else $DASHD_PATH, else dashd on PATH (rust-dashcore's
#                         contrib/setup-dashd.py caches one in ~/.rust-dashcore-test/)
#   DW_E2E_SKIP_BUILD=1   reuse <target>/debug/{dash-wallet,dwcli}
#   DW_E2E_OUT=<dir>      screenshots, logs and result.json (default test-results/tauri-tools)
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$(cd ../.. && pwd)

# The app's single-instance plugin wants a session bus: a private one (scripts/e2e-tauri.sh).
if [[ -z "${DW_E2E_PRIVATE_BUS:-}" ]]; then
  command -v dbus-run-session >/dev/null || { echo "e2e-tauri-tools: dbus-run-session not found (apt install dbus)" >&2; exit 2; }
  DW_E2E_PRIVATE_BUS=1 exec dbus-run-session -- "$PWD/scripts/e2e-tauri-tools.sh" "$@"
fi

if [[ -z "${DISPLAY:-}" ]]; then
  echo "e2e-tauri-tools: set DISPLAY to a private X display (xvfb-run, agent-gui)" >&2
  exit 2
fi
command -v WebKitWebDriver >/dev/null || { echo "e2e-tauri-tools: WebKitWebDriver not found (apt install webkit2gtk-driver)" >&2; exit 2; }
command -v xdotool >/dev/null || { echo "e2e-tauri-tools: xdotool not found (apt install xdotool)" >&2; exit 2; }
driver=${TAURI_DRIVER:-$(command -v tauri-driver || true)}
[[ -x "$driver" ]] || { echo "e2e-tauri-tools: tauri-driver not found; set TAURI_DRIVER" >&2; exit 2; }
dashd=${DASHD:-${DASHD_PATH:-$(command -v dashd || true)}}
[[ -x "$dashd" && -x "$(dirname "$dashd")/dash-cli" ]] || { echo "e2e-tauri-tools: set DASHD to a dashd with dash-cli next to it" >&2; exit 2; }

if [[ "${DW_E2E_SKIP_BUILD:-}" != 1 ]]; then
  pnpm tauri build --debug --no-bundle
  (cd "$repo/rust" && cargo build -p dwcli --locked)
fi

target=${CARGO_TARGET_DIR:-$repo/rust/target}
DW_APP="$target/debug/dash-wallet" DWCLI="$target/debug/dwcli" TAURI_DRIVER="$driver" DASHD="$dashd" \
  node e2e-tauri/tools.ts
