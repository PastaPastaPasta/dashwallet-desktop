#!/usr/bin/env bash
# The DashPay status card and identity chip on the real app against public testnet (DP1-08 GL
# tier): runs e2e-tauri/dashpay-status.ts through scripts/e2e-tauri.sh (same build, display,
# WebDriver and private-bus requirements) with a wallet of the testnet DashPay fixture restored.
# Reads only. Needs network access to testnet DAPI and peers.
#
#   DW_TESTNET_FIXTURE=<dir>   the fixture (`<W>.mnemonic`, `fixture.json`); required
#   DW_STATUS_WALLET, DW_STATUS_BIRTH_HEIGHT    see e2e-tauri/dashpay-status.ts
#   DW_E2E_OUT, DW_E2E_PORT, DW_E2E_SKIP_BUILD  as scripts/e2e-tauri.sh
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -n "${DW_TESTNET_FIXTURE:-}" ]] || { echo "e2e-tauri-dashpay-status: set DW_TESTNET_FIXTURE" >&2; exit 2; }

DW_E2E_SCRIPT=e2e-tauri/dashpay-status.ts \
  DW_E2E_OUT=${DW_E2E_OUT:-$PWD/test-results/tauri-dashpay-status} \
  DW_E2E_PORT=${DW_E2E_PORT:-4765} \
  scripts/e2e-tauri.sh "$@"
