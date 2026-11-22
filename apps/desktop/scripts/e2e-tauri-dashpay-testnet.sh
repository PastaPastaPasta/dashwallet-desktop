#!/usr/bin/env bash
# The seven DashPay GL flows on the real app against public testnet, in one run (H-04 O4a): builds
# the app and dwcli once, then runs the flows against the fixture scripts/testnet-dashpay-fixture.sh
# made, reads first and writes after:
#
#   dashpay-status (DP1-08, A)   contacts (DP2-07, A)        reads
#   registration (DP1-09, A→E)   add-contact (DP2-08, A→D)   writes
#   bell-accept (DP2-09, C→B)    pay-contact (DP3-06, A→B)
#   profile-write (BL-162, A)
#
# A failed flow does not stop the run. Each flow's output and log go to <out>/<flow>/; at the end
# <out>/summary.md and summary.json give each flow's result, duration and first error line, and
# the phrase and key check (e2e-tauri/artifact-scan.ts: the fixture's phrases and passphrases as
# plain text, BIP39 runs of 12+, xprv/tprv, screenshots through tesseract) runs over everything
# kept. The writes are the flows' own, each guarded by its ledger in the fixture, so a rerun does
# not repeat them; registration is skipped once its ledger says done. Wallet E's phrase (which
# registration.ts writes into the fixture) and the registration's passphrase (in DW_REG_STATE,
# beside the fixture, not in it: registration.ts fails on a secret among the fixture's files) are
# moved into the secret store as <prefix>-E-<wallet id>-{mnemonic,reg-passphrase} after it runs.
#
# No window opens: without $DISPLAY the run starts under xvfb-run, and each flow gets a private
# session bus (scripts/e2e-tauri.sh). Needs WebKitWebDriver, tauri-driver, xvfb-run, dbus-run-session
# and `secret`, and network access to testnet peers and DAPI.
#
#   DW_TESTNET_FIXTURE=<dir>       the fixture (fixture.json, <W>.mnemonic); required
#   DW_REG_STATE=<dir>             registration's app data root and passphrase (default <fixture>.reg-state)
#   DW_SUITE_OUT=<dir>             default test-results/dashpay-testnet (emptied first; not the fixture)
#   DW_SUITE_FLOWS="a b …"         a subset, in the order given (default all seven)
#   DW_SUITE_FLOW_TIMEOUT=<s>      per flow, then SIGTERM (default 7200)
#   DW_E2E_SKIP_BUILD=1            reuse <target>/debug/{dash-wallet,dwcli}
#   DW_FIXTURE_SECRET_PREFIX       store names for E (default dwd-qa-testnet-<basename of the fixture>,
#                                  as scripts/testnet-dashpay-fixture.sh)
set -euo pipefail
[[ -n "${DW_TESTNET_FIXTURE:-}" ]] || { echo "e2e-tauri-dashpay-testnet: set DW_TESTNET_FIXTURE" >&2; exit 2; }
fixture=$(cd "$DW_TESTNET_FIXTURE" && pwd)
[[ -f "$fixture/fixture.json" ]] || { echo "e2e-tauri-dashpay-testnet: $fixture has no fixture.json; run scripts/testnet-dashpay-fixture.sh" >&2; exit 2; }
export DW_TESTNET_FIXTURE=$fixture
export DW_REG_STATE=${DW_REG_STATE:-$fixture.reg-state}
out=${DW_SUITE_OUT:-$(cd "$(dirname "$0")/.." && pwd)/test-results/dashpay-testnet}
mkdir -p "$out"
out=$(cd "$out" && pwd)
export DW_SUITE_OUT=$out
case "$out/" in "$fixture"/* | "$DW_REG_STATE"/*) echo "e2e-tauri-dashpay-testnet: $out lies in the fixture" >&2; exit 2 ;; esac
case "$fixture/" in "$out"/*) echo "e2e-tauri-dashpay-testnet: the fixture lies in $out, which the run empties" >&2; exit 2 ;; esac
cd "$(dirname "$0")/.."
repo=$(cd ../.. && pwd)

if [[ -z "${DISPLAY:-}" ]]; then
  command -v xvfb-run >/dev/null || { echo "e2e-tauri-dashpay-testnet: no DISPLAY and no xvfb-run" >&2; exit 2; }
  exec xvfb-run -a -s "-screen 0 1440x960x24" "$PWD/scripts/e2e-tauri-dashpay-testnet.sh" "$@"
fi

if [[ "${DW_E2E_SKIP_BUILD:-}" != 1 ]]; then
  pnpm tauri build --debug --no-bundle
  (cd "$repo/rust" && cargo build -p dwcli --locked)
fi
target=${CARGO_TARGET_DIR:-$repo/rust/target}
for bin in dash-wallet dwcli; do
  [[ -x "$target/debug/$bin" ]] || { echo "e2e-tauri-dashpay-testnet: no $target/debug/$bin" >&2; exit 2; }
done
env_lines=$(node e2e-tauri/testnet-suite.ts env "$fixture")
eval "$env_lines"
find "$out" -mindepth 1 -delete
: >"$out/runs.tsv"
profile_root=${TMPDIR:-/tmp}/dwd-dashpay-testnet-profile-$(date -u +%Y%m%dT%H%M%S)
prefix=${DW_FIXTURE_SECRET_PREFIX:-dwd-qa-testnet-$(basename "$fixture")}
set +e

# Moves a file registration.ts wrote into the store, leaving a symlink.
adopt() {
  local path=$1 name=$2
  [[ -f "$path" && ! -L "$path" ]] || return 0
  secret adopt --name "$name" --note "dwd QA testnet fixture (testnet only)" "$path" >/dev/null ||
    echo "e2e-tauri-dashpay-testnet: could not move $path into the secret store" >&2
}

flows=${DW_SUITE_FLOWS:-dashpay-status contacts registration add-contact bell-accept pay-contact profile-write}
for flow in $flows; do
  dir="$out/$flow"
  mkdir -p "$dir/logs"
  log="$dir/logs/run.log"
  if [[ "$flow" == registration && -f "$fixture/DP1-09.json" ]] && grep -q '"state": "done"' "$fixture/DP1-09.json"; then
    echo "registration: already done in this fixture (DP1-09.json); skipped" >"$log"
    printf '%s\tskip\t0\t%s\n' "$flow" "$log" >>"$out/runs.tsv"
    continue
  fi
  case $flow in
    registration) extra=(DW_E2E_OUT="$dir/out") ;;
    profile-write) extra=(DW_PROFILE_ROOT="$profile_root" DW_E2E_OUT="$profile_root/out") ;;
    *) extra=(DW_E2E_OUT="$dir/out") ;;
  esac
  echo "== $flow ($(date -u +%H:%M:%SZ)); log $log"
  start=$(date +%s)
  env DW_E2E_SKIP_BUILD=1 DW_E2E_SCAN_EXTRA="$dir/logs" "${extra[@]}" \
    timeout --signal=TERM --kill-after=120 "${DW_SUITE_FLOW_TIMEOUT:-7200}" "scripts/e2e-tauri-$flow.sh" >"$log" 2>&1
  rc=$?
  secs=$(($(date +%s) - start))
  printf '%s\t%s\t%s\t%s\n' "$flow" "$rc" "$secs" "$log" >>"$out/runs.tsv"
  echo "   exit $rc after $secs s"
  if [[ "$flow" == registration && -f "$fixture/DP1-09.json" ]]; then
    e=$(node -e 'console.log((JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).walletId ?? "").slice(0, 8))' "$fixture/DP1-09.json")
    if [[ "$e" =~ ^[0-9a-f]{8}$ ]]; then
      adopt "$fixture/E.mnemonic" "$prefix-E-$e-mnemonic"
      adopt "$DW_REG_STATE/passphrase" "$prefix-E-$e-reg-passphrase"
    fi
  fi
done

node e2e-tauri/testnet-suite.ts summary "$out" "$fixture" "$profile_root"
