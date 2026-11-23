#!/usr/bin/env bash
# A payment to a contact from Pay ▸ To a contact on the real app against public testnet (DP3-06 GL
# tier): runs e2e-tauri/pay-contact.ts through scripts/e2e-tauri.sh (same build, display, WebDriver
# and private-bus requirements). WRITES to testnet: one payment between two wallets of the testnet
# DashPay fixture (once; `<fixture>/DP3-06.json` records it). Needs network access to testnet DAPI
# and peers, and a funded payer.
#
#   DW_TESTNET_FIXTURE=<dir>             the fixture (`<W>.mnemonic`, `fixture.json`); required
#   DW_PAY_PAYER, DW_PAY_PAYEE           the wallets (default A and B); see e2e-tauri/pay-contact.ts
#   DW_PAY_USERNAME, DW_PAY_AMOUNT       the payee's username (default dwrecoverb2), the DASH paid
#   DW_E2E_SCAN_EXTRA=<dir>              also scanned for secrets at the end (the run's log)
#   DW_E2E_OUT, DW_E2E_PORT, DW_E2E_SKIP_BUILD   as scripts/e2e-tauri.sh
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -n "${DW_TESTNET_FIXTURE:-}" ]] || { echo "e2e-tauri-pay-contact: set DW_TESTNET_FIXTURE" >&2; exit 2; }

DW_E2E_SCRIPT=e2e-tauri/pay-contact.ts \
  DW_E2E_OUT=${DW_E2E_OUT:-$PWD/test-results/tauri-pay-contact} \
  DW_E2E_PORT=${DW_E2E_PORT:-4786} \
  scripts/e2e-tauri.sh "$@"
