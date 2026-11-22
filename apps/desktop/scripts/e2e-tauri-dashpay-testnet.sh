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
# A failed flow does not stop the run. Each run gets a new directory <out>/<UTC time>-XXXX/ (the
# symlink <out>/latest is pointed at it); nothing is ever emptied or deleted. Each
# flow's output and log go to <run>/<flow>/; at the end <run>/summary.md and summary.json give each
# flow's result, duration and first error line, and the phrase and key check
# (e2e-tauri/artifact-scan.ts: the fixture's phrases and passphrases as plain text and in
# screenshots through tesseract, BIP39 runs of 12+, xprv/tprv) runs over everything kept. The
# writes are the flows' own, each guarded by its ledger in the fixture, so a rerun does not repeat
# them; registration is skipped once its ledger says done, and profile-write once BL-162.json
# says done (profile-write.ts writes it as soon as the edit is confirmed; DW_PROFILE_WRITE_AGAIN=1
# asks for another edit). Each secret a flow makes (its app passphrase, registration's E phrase and
# passphrase) goes to the store as it is made and its store name to <run>/secrets.manifest
# (e2e-tauri/flow-secrets.ts), which the closing check reads with the fixture's. Wallet E's phrase
# (<fixture>/E.mnemonic) and the registration's passphrase (in DW_REG_STATE, beside the fixture,
# not in it: registration.ts fails on a secret among the fixture's files) are thus links into the
# store from the start, as <prefix>-run-<run>-registration-E-{mnemonic,passphrase}; a plain file
# left by registration.ts run alone is moved in after it runs, as
# <prefix>-E-<wallet id>-{mnemonic,reg-passphrase}, and if that fails the run stops. Every run first checks that each phrase and passphrase file of the
# fixture and its registration state is a link into the secret store, and that every one the
# ledgers say exists is there, and stops if not.
#
# No window opens: without $DISPLAY the run starts under xvfb-run, and each flow gets a private
# session bus (scripts/e2e-tauri.sh). Needs WebKitWebDriver, tauri-driver, xvfb-run, dbus-run-session
# and `secret`, and network access to testnet peers and DAPI.
#
#   DW_TESTNET_FIXTURE=<dir>       the fixture (fixture.json, <W>.mnemonic); required
#   DW_REG_STATE=<dir>             registration's app data root and passphrase (default <fixture>.reg-state)
#   DW_SUITE_OUT=<dir>             runs go to <dir>/<UTC time>-XXXX/ (default test-results/dashpay-testnet);
#                                  in a git work tree it must be git-ignored; not in the fixture
#   DW_SUITE_FLOWS="a b …"         a subset, in the order given (default all seven)
#   DW_SUITE_FLOW_TIMEOUT=<s>      per flow, then SIGTERM (default 7200)
#   DW_E2E_SKIP_BUILD=1            reuse <target>/debug/{dash-wallet,dwcli}
#   DW_PROFILE_WRITE_AGAIN=1       run profile-write although BL-162.json says it already wrote
#   DW_SECRET_STORE=<dir>          where the store's links point (default ~/.secrets)
#   DW_FIXTURE_SECRET_PREFIX       store names for E (default dwd-qa-testnet-<basename of the fixture>,
#                                  as scripts/testnet-dashpay-fixture.sh)
set -euo pipefail
[[ -n "${DW_TESTNET_FIXTURE:-}" ]] || { echo "e2e-tauri-dashpay-testnet: set DW_TESTNET_FIXTURE" >&2; exit 2; }
fixture=$(cd "$DW_TESTNET_FIXTURE" && pwd -P)
[[ -f "$fixture/fixture.json" ]] || { echo "e2e-tauri-dashpay-testnet: $fixture has no fixture.json; run scripts/testnet-dashpay-fixture.sh" >&2; exit 2; }
export DW_TESTNET_FIXTURE=$fixture
reg=${DW_REG_STATE:-$fixture.reg-state}
mkdir -p "$reg" && chmod 700 "$reg"
export DW_REG_STATE
DW_REG_STATE=$(cd "$reg" && pwd -P)
app=$(cd "$(dirname "$0")/.." && pwd -P)
root=${DW_SUITE_OUT:-$app/test-results/dashpay-testnet}
[[ "$root" == /* ]] || root=$PWD/$root
# Inside a git work tree the output must be git-ignored (checked before anything is created).
near=$root
until [[ -d "$near" ]]; do near=$(dirname "$near"); done
near_real=$(cd "$near" && pwd -P)
root=$near_real${root#"$near"}
[[ "$root" != / ]] || { echo "e2e-tauri-dashpay-testnet: / may not hold the runs" >&2; exit 2; }
# Only "not a git repository" counts as outside one: any other git error refuses.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_CEILING_DIRECTORIES
if where=$(LC_ALL=C git -C "$near_real" rev-parse --is-inside-git-dir --is-inside-work-tree 2>&1); then
  [[ "${where%%$'\n'*}" != true ]] || { echo "e2e-tauri-dashpay-testnet: $root lies in a .git directory" >&2; exit 2; }
  if [[ "${where##*$'\n'}" == true ]] && ! git -C "$near_real" check-ignore -q -- "$root/"; then
    echo "e2e-tauri-dashpay-testnet: $root lies in a git work tree and is not git-ignored" >&2; exit 2
  fi
elif [[ "$where" != *"not a git repository"* ]]; then
  echo "e2e-tauri-dashpay-testnet: cannot tell whether $root lies in a git work tree: $where" >&2; exit 2
fi
for keep in "$fixture" "$DW_REG_STATE"; do
  case "$root/" in "$keep"/*) echo "e2e-tauri-dashpay-testnet: $root lies in $keep" >&2; exit 2 ;; esac
  case "$keep/" in "$root"/*) echo "e2e-tauri-dashpay-testnet: $keep lies in $root" >&2; exit 2 ;; esac
done
export DW_SUITE_OUT=$root
store=${DW_SECRET_STORE:-$HOME/.secrets}
[[ -d "$store" ]] || { echo "e2e-tauri-dashpay-testnet: no secret store at $store" >&2; exit 2; }
store=$(cd "$store" && pwd -P)
export DW_SECRET_STORE=$store
cd "$app"
repo=$(cd ../.. && pwd)

if [[ -z "${DISPLAY:-}" ]]; then
  command -v xvfb-run >/dev/null || { echo "e2e-tauri-dashpay-testnet: no DISPLAY and no xvfb-run" >&2; exit 2; }
  exec xvfb-run -a -s "-screen 0 1440x960x24" "$PWD/scripts/e2e-tauri-dashpay-testnet.sh" "$@"
fi

# Before hours of flows: every phrase and passphrase file is a link into the store that resolves,
# and every one the ledgers say exists is there (the closing check reads them all).
node e2e-tauri/testnet-suite.ts secrets "$fixture" || exit 2
env_lines=$(node e2e-tauri/testnet-suite.ts env "$fixture")
eval "$env_lines"
in_store() { [[ -L "$1" && "$(readlink -f "$1")" == "$store"/* ]]; }

if [[ "${DW_E2E_SKIP_BUILD:-}" != 1 ]]; then
  pnpm tauri build --debug --no-bundle
  (cd "$repo/rust" && cargo build -p dwcli --locked)
fi
target=${CARGO_TARGET_DIR:-$repo/rust/target}
for bin in dash-wallet dwcli; do
  [[ -x "$target/debug/$bin" ]] || { echo "e2e-tauri-dashpay-testnet: no $target/debug/$bin" >&2; exit 2; }
done
mkdir -p "$root"
out=$(mktemp -d "$root/$(date -u +%Y%m%dT%H%M%SZ)-XXXX") || exit 2
if [[ -L "$root/latest" || ! -e "$root/latest" ]]; then ln -sfn "$(basename "$out")" "$root/latest"; fi
echo "e2e-tauri-dashpay-testnet: this run's results go to $out"
: >"$out/runs.tsv"
profile_root=${TMPDIR:-/tmp}/dwd-dashpay-testnet-profile-$(date -u +%Y%m%dT%H%M%S)
prefix=${DW_FIXTURE_SECRET_PREFIX:-dwd-qa-testnet-$(basename "$fixture")}
set +e

# Moves a file registration.ts wrote into the store, leaving a symlink.
adopt() {
  local path=$1 name=$2
  [[ -f "$path" && ! -L "$path" ]] || return 0
  if ! secret adopt --name "$name" --note "dwd QA testnet fixture (testnet only)" "$path" >/dev/null || ! in_store "$path"; then
    echo "e2e-tauri-dashpay-testnet: could not move $path into the secret store" >&2
    return 1
  fi
}

# Logs why this flow is skipped and adds its row.
skip() {
  echo "$1" >"$log"
  printf '%s\tskip\t0\t%s\n' "$flow" "$log" >>"$out/runs.tsv"
}

flows=${DW_SUITE_FLOWS:-dashpay-status contacts registration add-contact bell-accept pay-contact profile-write}
for flow in $flows; do
  dir="$out/$flow"
  mkdir -p "$dir/logs"
  log="$dir/logs/run.log"
  if [[ "$flow" == registration && -f "$fixture/DP1-09.json" ]] && grep -q '"state": "done"' "$fixture/DP1-09.json"; then
    skip "registration: already done in this fixture (DP1-09.json); skipped"
    continue
  fi
  if [[ "$flow" == profile-write && "${DW_PROFILE_WRITE_AT:-synced}" != never && "${DW_PROFILE_WRITE_AGAIN:-}" != 1 && -f "$fixture/BL-162.json" ]] && grep -q '"state": "done"' "$fixture/BL-162.json"; then
    skip "profile-write: already wrote in this fixture (BL-162.json); skipped (DW_PROFILE_WRITE_AGAIN=1 for another edit)"
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
    DW_SECRET_MANIFEST="$out/secrets.manifest" DW_SECRET_RUN_PREFIX="$prefix-run-$(basename "$out")-$flow" \
    timeout --signal=TERM --kill-after=120 "${DW_SUITE_FLOW_TIMEOUT:-7200}" "scripts/e2e-tauri-$flow.sh" >"$log" 2>&1
  rc=$?
  secs=$(($(date +%s) - start))
  printf '%s\t%s\t%s\t%s\n' "$flow" "$rc" "$secs" "$log" >>"$out/runs.tsv"
  echo "   exit $rc after $secs s"
  if [[ "$flow" == registration ]]; then
    # Named by E's wallet id; a run that stopped before the ledger had one still goes to the store.
    e=$(node -e 'try { console.log((JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).walletId ?? "").slice(0, 8)) } catch { console.log("") }' "$fixture/DP1-09.json")
    [[ "$e" =~ ^[0-9a-f]{8}$ ]] || e=pending-$(date -u +%Y%m%dT%H%M%S)
    if ! { adopt "$fixture/E.mnemonic" "$prefix-E-$e-mnemonic" && adopt "$DW_REG_STATE/passphrase" "$prefix-E-$e-reg-passphrase" && node e2e-tauri/testnet-suite.ts secrets "$fixture"; } >"$dir/logs/secret-store.log" 2>&1; then
      echo "secret-store: FAIL: wallet E's phrase or passphrase is not in the secret store; the run stops" >>"$dir/logs/secret-store.log"
      printf 'secret-store\t1\t0\t%s\n' "$dir/logs/secret-store.log" >>"$out/runs.tsv"
      echo "   wallet E's secrets did not reach the store; stopping (see $dir/logs/secret-store.log)"
      break
    fi
  fi
done

node e2e-tauri/testnet-suite.ts summary "$out" "$fixture" "$profile_root"
