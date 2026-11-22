#!/usr/bin/env bash
# Builds (or resumes) the testnet DashPay fixture the seven e2e-tauri GL flows read
# (scripts/e2e-tauri-dashpay-testnet.sh runs them): four new wallets A–D, each with an identity
# and a non-contested username, and the contact graph A:{B,C}, B:{A}, C:{A,D}, D:{C} (DP2-09 D20),
# leaving B–C, A–D and B–D unrelated. WRITES to testnet: one faucet payout to A, one payment
# A→B,C,D, four asset locks, identities and names, three contact requests and their accepts. Each
# write is recorded before it is made, and a rerun reads the network first, so it never pays,
# registers or sends twice; see e2e-tauri/testnet-fixture.ts.
#
# Needs dwcli (built here unless DW_E2E_SKIP_BUILD=1), `secret` (the shared secret store; each
# phrase and the vault passphrase go there, linked into the fixture), python3 and the dash-faucet
# skill's helper (its API key, if configured, is read by the helper only), and network access to
# testnet peers, DAPI, Insight and the faucet. Opens no window.
#
#   DW_TESTNET_FIXTURE=<dir>   the fixture, under $HOME; required. New or a fixture this script made
#   DW_E2E_SKIP_BUILD=1        reuse <target>/debug/dwcli (<target>: $CARGO_TARGET_DIR, else rust/target)
#   see e2e-tauri/testnet-fixture.ts for the rest (secret names, amounts, timeouts)
set -euo pipefail
[[ -n "${DW_TESTNET_FIXTURE:-}" && "$DW_TESTNET_FIXTURE" != /* ]] && export DW_TESTNET_FIXTURE=$PWD/$DW_TESTNET_FIXTURE
cd "$(dirname "$0")/.."
repo=$(cd ../.. && pwd)
[[ -n "${DW_TESTNET_FIXTURE:-}" ]] || { echo "testnet-dashpay-fixture: set DW_TESTNET_FIXTURE" >&2; exit 2; }
command -v secret >/dev/null || { echo "testnet-dashpay-fixture: the secret store's \`secret\` is not on PATH" >&2; exit 2; }
if [[ "${DW_E2E_SKIP_BUILD:-}" != 1 ]]; then
  (cd "$repo/rust" && cargo build -p dwcli --locked)
fi
target=${CARGO_TARGET_DIR:-$repo/rust/target}
DWCLI="$target/debug/dwcli" exec node e2e-tauri/testnet-fixture.ts
