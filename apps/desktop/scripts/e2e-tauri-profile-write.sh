#!/usr/bin/env bash
# One DashPay profile edit from the real app on public testnet, timed against SPV's sync phases
# (BL-162): runs e2e-tauri/profile-write.ts through scripts/e2e-tauri.sh (same display, WebDriver
# and private-bus requirements). WRITES to testnet: one profile edit of a fixture wallet (default
# A), harmless and repeatable. Needs network access to testnet DAPI and peers.
#
# It writes only under its scratch root, never into the checkout: it does not build by default
# and runs <target>/debug/{dash-wallet,dwcli} (<target>: $CARGO_TARGET_DIR, else rust/target).
# DW_E2E_SKIP_BUILD=0 builds first, as scripts/e2e-tauri.sh does (that writes dist/, and
# rust/target unless CARGO_TARGET_DIR is set).
#
#   DW_TESTNET_FIXTURE=<dir>          the fixture (`<W>.mnemonic`, `fixture.json`); required
#   DW_PROFILE_WRITE_AT=early|synced|never   when to write (default synced; never: reads only)
#   DW_PROFILE_ROOT=<dir>             the scratch root (default $TMPDIR/dw-profile-write)
#   DW_E2E_OUT=<dir>                  a new directory under it (default <run>/out)
#   DW_E2E_SCAN_EXTRA=<dir>           also scanned for secrets, read-only
#   DW_LOG=<EnvFilter>                the engine's log, in <DW_E2E_OUT>/tauri-driver.log
#   DW_E2E_PORT                       as scripts/e2e-tauri.sh (default 4770)
#   see e2e-tauri/profile-write.ts for the rest
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -n "${DW_TESTNET_FIXTURE:-}" ]] || { echo "e2e-tauri-profile-write: set DW_TESTNET_FIXTURE" >&2; exit 2; }
# Only an explicit 0 builds.
skip=1
[[ "${DW_E2E_SKIP_BUILD:-1}" == 0 ]] && skip=0
if [[ "$skip" == 1 ]]; then
  target=${CARGO_TARGET_DIR:-$(cd ../.. && pwd)/rust/target}
  for bin in dash-wallet dwcli; do
    [[ -x "$target/debug/$bin" ]] || {
      echo "e2e-tauri-profile-write: no $target/debug/$bin; build it, or set DW_E2E_SKIP_BUILD=0" >&2
      exit 2
    }
  done
fi

DW_E2E_SCRIPT=e2e-tauri/profile-write.ts \
  DW_E2E_SKIP_BUILD=$skip \
  DW_E2E_PORT=${DW_E2E_PORT:-4770} \
  scripts/e2e-tauri.sh "$@"
