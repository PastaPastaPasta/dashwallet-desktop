#!/usr/bin/env bash
# The DashPay suites, nightly on agentbox (DASHPAY §6 T3; ROADMAP DP1-07, DP2-06, DP3-05): builds
# dwcli at origin/main in its own checkout and runs the suites of regtest/README.md "DashPay suites"
# against testnet. Log-only: it gates nothing and notifies no one; a failure is the exit status
# (the unit's failed status) and the kept files. The junit, pytest's output, this log and the
# run's dwcli logs stay under $DW_DP_STATE/nightly/<date>.
# pytest's output goes only to that directory; the journal and this log get its summary lines once
# the harness's end-of-run secret scan has passed.
#
#   DW_DP_STATE=<dir>          absolute, outside any checkout, owner-only (0700): data roots, the vault
#                              passphrase, the checkout; required
#   DW_DP_FUNDER=<file>        the funder's phrase, owner-only (0600) (agentbox's secret store); required
#   DW_DP_FUNDER_BIRTH=<n>     the funder's birth height; required
#   DW_DP_HOLD_UPSTREAM=<ip:port>  a testnet peer serving BIP157 filters (DP3-05's relay); required
#   DW_DP_SUITES="…"           the suite files (default: every tests/test_dp*.py)
#   DW_DP_REF=<ref>            what to test (default origin/main)
set -euo pipefail
umask 077
: "${DW_DP_STATE:?set DW_DP_STATE}" "${DW_DP_FUNDER:?set DW_DP_FUNDER}" "${DW_DP_FUNDER_BIRTH:?set DW_DP_FUNDER_BIRTH}"
: "${DW_DP_HOLD_UPSTREAM:?set DW_DP_HOLD_UPSTREAM (the unknown-outcome relay needs it)}"
export DW_DP_NET=${DW_DP_NET:-testnet} DW_DP_STATE DW_DP_FUNDER DW_DP_FUNDER_BIRTH DW_DP_HOLD_UPSTREAM
export DW_DP_STRICT=1
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-12}
unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR

mkdir -p "$DW_DP_STATE"
exec 9>"$DW_DP_STATE/nightly.lock"
if ! flock -n 9; then
  echo "dashpay-nightly: another run holds $DW_DP_STATE/nightly.lock" >&2
  exit 1
fi

repo=$DW_DP_STATE/repo
out=$DW_DP_STATE/nightly/$(date -u +%Y%m%d-%H%M%S)
mkdir -p "$out"
export DW_DP_LOGS=$out/dwcli
exec > >(tee -a "$out/nightly.log") 2>&1

if [[ ! -d $repo/.git ]]; then
  git clone --quiet https://github.com/PastaPastaPasta/dashwallet-desktop.git "$repo"
fi
git -C "$repo" fetch --quiet origin
git -C "$repo" checkout --quiet --detach "${DW_DP_REF:-origin/main}"
echo "dashpay-nightly: $(git -C "$repo" rev-parse HEAD) on $DW_DP_NET, output in $out"

DWCLI=$(cd "$repo/rust" && cargo build -p dwcli --locked --message-format=json-render-diagnostics |
  python3 -c 'import json, sys
paths = [m["executable"] for m in map(json.loads, sys.stdin)
         if m.get("reason") == "compiler-artifact" and m["target"]["name"] == "dwcli" and m.get("executable")]
print(paths[-1] if paths else "")')
if [[ -z $DWCLI || ! -x $DWCLI ]]; then
  echo "dashpay-nightly: the build named no dwcli binary"
  exit 1
fi
export DWCLI
echo "dashpay-nightly: dwcli $DWCLI"

cd "$repo/regtest/harness"
if [[ ! -x .venv/bin/python ]]; then
  uv venv .venv --python 3.12 -q
fi
uv pip install -q --python .venv/bin/python -r requirements.txt
if [[ -n ${DW_DP_SUITES:-} ]]; then
  read -r -a suites <<< "$DW_DP_SUITES"
else
  shopt -s nullglob
  suites=(tests/test_dp*.py)
  shopt -u nullglob
fi
if [[ ${#suites[@]} -eq 0 ]]; then
  echo "dashpay-nightly: no DashPay suites at this revision"
  exit 1
fi
export DW_DP_SCAN=$out/pytest.log:$out/nightly.log
status=0
.venv/bin/python -m pytest -v -rA -l "${suites[@]}" --junitxml="$out/junit.xml" > "$out/pytest.log" 2>&1 || status=$?
if [[ ! -f $DW_DP_LOGS/secret-scan.ok ]]; then
  echo "dashpay-nightly: the secret scan did not pass; pytest's output stays in $out only (pytest exited $status)"
  exit 1
fi
grep -E '^(tests/|PASSED|FAILED|ERROR|XFAIL|XPASS|SKIPPED|=+ .* in [0-9.]+s)' "$out/pytest.log" || true
echo "dashpay-nightly: pytest exited $status"
exit "$status"
