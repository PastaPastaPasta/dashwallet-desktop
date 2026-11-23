#!/usr/bin/env bash
# A username registered from the wizard, closed mid-flow and resumed, on the real app against
# public testnet (DP1-09 GL tier): runs e2e-tauri/registration.ts through scripts/e2e-tauri.sh
# (same build, display, WebDriver and private-bus requirements). WRITES to testnet: a new wallet E
# (its phrase in `<fixture>/E.mnemonic`), one payment from fixture wallet A to it, one asset lock,
# one identity and its username (once; `<fixture>/DP1-09.json` records each step). Needs network
# access to testnet DAPI and peers.
#
#   DW_TESTNET_FIXTURE=<dir>   the fixture (`A.mnemonic`); required
#   DW_REG_STATE=<dir>         the app's data root and passphrase between runs (0700); required
#   DW_E2E_SCAN_EXTRA=<dir>    also scanned for secrets at the end (the run's log)
#   DW_E2E_OUT, DW_E2E_PORT, DW_E2E_SKIP_BUILD   as scripts/e2e-tauri.sh
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -n "${DW_TESTNET_FIXTURE:-}" ]] || { echo "e2e-tauri-registration: set DW_TESTNET_FIXTURE" >&2; exit 2; }
[[ -n "${DW_REG_STATE:-}" ]] || { echo "e2e-tauri-registration: set DW_REG_STATE" >&2; exit 2; }

DW_E2E_SCRIPT=e2e-tauri/registration.ts \
  DW_E2E_OUT=${DW_E2E_OUT:-$PWD/test-results/tauri-registration} \
  DW_E2E_PORT=${DW_E2E_PORT:-4776} \
  scripts/e2e-tauri.sh "$@"
