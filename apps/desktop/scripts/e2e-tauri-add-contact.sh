#!/usr/bin/env bash
# A contact request sent from Add Contact on the real app against public testnet (DP2-08 GL tier):
# runs e2e-tauri/add-contact.ts through scripts/e2e-tauri.sh (same build, display, WebDriver and
# private-bus requirements). WRITES to testnet: one contact request between two wallets of the
# testnet DashPay fixture (once; `<fixture>/DP2-08.json` records it), left unaccepted. Needs
# network access to testnet DAPI and peers.
#
#   DW_TESTNET_FIXTURE=<dir>             the fixture (`<W>.mnemonic`, `fixture.json`); required
#   DW_ADD_SENDER, DW_ADD_RECIPIENT      the wallets (default A and D); see e2e-tauri/add-contact.ts
#   DW_ADD_USERNAME                      the recipient's username (default dwtdashd2, wallet D's)
#   DW_E2E_SCAN_EXTRA=<dir>              also scanned for secrets at the end (the run's log)
#   DW_E2E_OUT, DW_E2E_PORT, DW_E2E_SKIP_BUILD   as scripts/e2e-tauri.sh
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -n "${DW_TESTNET_FIXTURE:-}" ]] || { echo "e2e-tauri-add-contact: set DW_TESTNET_FIXTURE" >&2; exit 2; }

DW_E2E_SCRIPT=e2e-tauri/add-contact.ts \
  DW_E2E_OUT=${DW_E2E_OUT:-$PWD/test-results/tauri-add-contact} \
  DW_E2E_PORT=${DW_E2E_PORT:-4768} \
  scripts/e2e-tauri.sh "$@"
