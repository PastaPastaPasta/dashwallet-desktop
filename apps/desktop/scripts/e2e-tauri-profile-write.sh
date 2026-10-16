#!/usr/bin/env bash
# One DashPay profile edit from the real app on public testnet, timed against SPV's sync phases
# (BL-162): runs e2e-tauri/profile-write.ts through scripts/e2e-tauri.sh (same build, display,
# WebDriver and private-bus requirements). WRITES to testnet: one profile edit of a fixture
# wallet (default A), harmless and repeatable. Needs network access to testnet DAPI and peers.
#
#   DW_TESTNET_FIXTURE=<dir>          the fixture (`<W>.mnemonic`, `fixture.json`); required
#   DW_PROFILE_WRITE_AT=early|synced  when to write (default synced); see e2e-tauri/profile-write.ts
#   DW_LOG=<EnvFilter>                the engine's log, in <DW_E2E_OUT>/tauri-driver.log
#   DW_E2E_OUT, DW_E2E_PORT, DW_E2E_SKIP_BUILD   as scripts/e2e-tauri.sh
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -n "${DW_TESTNET_FIXTURE:-}" ]] || { echo "e2e-tauri-profile-write: set DW_TESTNET_FIXTURE" >&2; exit 2; }

DW_E2E_SCRIPT=e2e-tauri/profile-write.ts \
  DW_E2E_OUT=${DW_E2E_OUT:-$PWD/test-results/tauri-profile-write} \
  DW_E2E_PORT=${DW_E2E_PORT:-4770} \
  scripts/e2e-tauri.sh "$@"
