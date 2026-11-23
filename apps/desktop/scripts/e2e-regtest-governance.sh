#!/usr/bin/env bash
# MG-09 acceptance: vote on a proposal and create one from the UI on the real engine, on a regtest
# masternode network (regtest/functional/dwd_governance_ui.py runs e2e-tauri/regtest-governance.ts
# and judges the result on Core).
# Builds the app (production renderer, debug binary with the real engine) and dwcli as
# scripts/e2e-tauri.sh does, then runs the suite. Not in CI: it needs Dash Core's release binaries
# and the functional framework's Python (regtest/README.md, "Masternode networks"). Needs a private
# X display in $DISPLAY (xvfb-run, or `agent-gui start N` on agentbox), WebKitWebDriver and
# tauri-driver (see scripts/e2e-tauri.sh):
#
#   DASHCORE_DIR=<dir>    the unpacked release (bin/dashd, bin/dash-cli, …)
#   DWD_PYTHON=<python>   a Python with dash_hash installed
#   DW_E2E_SKIP_BUILD=1   reuse <target>/debug/{dash-wallet,dwcli}
#   DW_E2E_OUT=<dir>      screenshots, logs and result.json (default test-results/tauri-regtest-governance)
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$(cd ../.. && pwd)

# The app's single-instance plugin wants a session bus: a private one (scripts/e2e-tauri.sh).
if [[ -z "${DW_E2E_PRIVATE_BUS:-}" ]]; then
  command -v dbus-run-session >/dev/null || { echo "e2e-regtest-governance: dbus-run-session not found (apt install dbus)" >&2; exit 2; }
  DW_E2E_PRIVATE_BUS=1 exec dbus-run-session -- "$PWD/scripts/e2e-regtest-governance.sh" "$@"
fi

if [[ -z "${DISPLAY:-}" ]]; then
  echo "e2e-regtest-governance: set DISPLAY to a private X display (xvfb-run, agent-gui)" >&2
  exit 2
fi
command -v WebKitWebDriver >/dev/null || { echo "e2e-regtest-governance: WebKitWebDriver not found (apt install webkit2gtk-driver)" >&2; exit 2; }
driver=${TAURI_DRIVER:-$(command -v tauri-driver || true)}
[[ -x "$driver" ]] || { echo "e2e-regtest-governance: tauri-driver not found; set TAURI_DRIVER" >&2; exit 2; }
[[ -x "${DASHCORE_DIR:-}/bin/dashd" ]] || { echo "e2e-regtest-governance: set DASHCORE_DIR to an unpacked Dash Core release" >&2; exit 2; }

if [[ "${DW_E2E_SKIP_BUILD:-}" != 1 ]]; then
  pnpm tauri build --debug --no-bundle
  (cd "$repo/rust" && cargo build -p dwcli --locked)
fi

target=${CARGO_TARGET_DIR:-$repo/rust/target}
DW_E2E_OUT=$(realpath -m "${DW_E2E_OUT:-test-results/tauri-regtest-governance}") \
DW_APP="$target/debug/dash-wallet" DWCLI="$target/debug/dwcli" TAURI_DRIVER="$driver" \
  "$repo/regtest/functional/run.sh" dwd_governance_ui.py "$@"
