#!/usr/bin/env bash
# Settings on the real app against a dashd regtest node (U-04 T1): brings up the regtest node of
# regtest/docker-compose.yml under its own compose project and host ports, runs
# e2e-tauri/settings.ts through scripts/e2e-tauri.sh (same build, display, WebDriver and
# private-bus requirements), and takes the node down again. Needs Docker, the regtest image
# (`regtest/scripts/regtest.sh up` builds it on first use) and xdotool (the menu shortcut that
# opens Settings).
#
#   DWD_COMPOSE_PROJECT, DWD_RPC_HOST_PORT, DWD_P2P_HOST_PORT   the node (default dw-e2e-settings,
#                                                               29908, 29909)
#   DW_E2E_OUT, DW_E2E_PORT, DW_E2E_SKIP_BUILD                  as scripts/e2e-tauri.sh
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$(cd ../.. && pwd)

export DWD_COMPOSE_PROJECT=${DWD_COMPOSE_PROJECT:-dw-e2e-settings}
export DWD_RPC_HOST_PORT=${DWD_RPC_HOST_PORT:-29908}
export DWD_P2P_HOST_PORT=${DWD_P2P_HOST_PORT:-29909}
regtest="$repo/regtest/scripts/regtest.sh"

command -v xdotool >/dev/null || { echo "e2e-tauri-settings: needs xdotool" >&2; exit 1; }
sh "$regtest" up
trap 'sh "$regtest" down >/dev/null 2>&1 || true' EXIT

DW_E2E_SCRIPT=e2e-tauri/settings.ts \
  DW_E2E_OUT=${DW_E2E_OUT:-$PWD/test-results/tauri-settings} \
  DW_E2E_PORT=${DW_E2E_PORT:-4755} \
  DW_REGTEST_SH="$regtest" \
  DW_REGTEST_P2P="127.0.0.1:$DWD_P2P_HOST_PORT" \
  scripts/e2e-tauri.sh "$@"
