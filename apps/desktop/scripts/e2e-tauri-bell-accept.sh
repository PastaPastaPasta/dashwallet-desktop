#!/usr/bin/env bash
# A contact request accepted from the bell on the real app against public testnet (DP2-09 GL
# tier): runs e2e-tauri/bell-accept.ts through scripts/e2e-tauri.sh (same build, display,
# WebDriver and private-bus requirements). WRITES to testnet: one contact request between two
# wallets of the testnet DashPay fixture (once; `<fixture>/DP2-09.json` records it), and its
# accept. Needs network access to testnet DAPI and peers.
#
#   DW_TESTNET_FIXTURE=<dir>             the fixture (`<W>.mnemonic`, `fixture.json`); required
#   DW_BELL_SENDER, DW_BELL_RECEIVER     the wallets (default C and B); see e2e-tauri/bell-accept.ts
#   DW_E2E_SCAN_EXTRA=<dir>              also scanned for secrets at the end (the run's log)
#   DW_E2E_OUT, DW_E2E_PORT, DW_E2E_SKIP_BUILD   as scripts/e2e-tauri.sh
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -n "${DW_TESTNET_FIXTURE:-}" ]] || { echo "e2e-tauri-bell-accept: set DW_TESTNET_FIXTURE" >&2; exit 2; }

DW_E2E_SCRIPT=e2e-tauri/bell-accept.ts \
  DW_E2E_OUT=${DW_E2E_OUT:-$PWD/test-results/tauri-bell-accept} \
  DW_E2E_PORT=${DW_E2E_PORT:-4766} \
  scripts/e2e-tauri.sh "$@"
