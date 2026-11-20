#!/usr/bin/env bash
# The DashPay suites, nightly on agentbox (DASHPAY §6 T3; ROADMAP DP1-07, DP2-06, DP3-05): builds
# dwcli at origin/main in its own checkout and runs the suites of regtest/README.md "DashPay suites"
# against testnet. It gates nothing; a failure is the exit status, which fails the unit, whose
# OnFailure= unit runs the site's notifier (DW_DP_NOTIFY, through dashpay-nightly-notify.sh) with
# the unit's status and where the artifacts are. The junit, pytest's output, this log and the
# run's dwcli logs stay under $DW_DP_STATE/nightly/<date>.
# pytest's output goes only to that directory; the journal and this log get its summary lines once
# the harness's end-of-run secret scan has passed.
#
# Install this script, with dashpay-unfunded.sh beside it, outside DW_DP_STATE and outside any
# checkout (ci/agentbox/dw-dashpay-nightly.service); it refuses to run from inside DW_DP_STATE.
#
#   DW_DP_STATE=<dir>          absolute, outside any checkout, owner-only (0700): data roots, the vault
#                              passphrase, the checkout; required
#   DW_DP_FUNDER=<file>        the funder's phrase, owner-only (0600) (agentbox's secret store); required
#   DW_DP_FUNDER_BIRTH=<n>     the funder's birth height; required
#   DW_DP_HOLD_UPSTREAM=<ip:port>  a testnet peer serving BIP157 filters (DP3-05's relay); required
#   DW_DP_NOTIFY=<file>        the site's notifier, an absolute path to an executable; required
#   DW_DP_SUITES="…"           the suite files (default: every tests/test_dp*.py)
#   DW_DP_REF=<ref>            what to test (default origin/main)
set -euo pipefail
umask 077
ulimit -c 0
: "${DW_DP_STATE:?set DW_DP_STATE}" "${DW_DP_FUNDER:?set DW_DP_FUNDER}" "${DW_DP_FUNDER_BIRTH:?set DW_DP_FUNDER_BIRTH}"
: "${DW_DP_HOLD_UPSTREAM:?set DW_DP_HOLD_UPSTREAM (the unknown-outcome relay needs it)}"
: "${DW_DP_NOTIFY:?set DW_DP_NOTIFY (the failure notifier of the site)}"
fail() {
  echo "dashpay-nightly: $*" >&2
  exit 1
}
[[ ${DW_DP_NET:-testnet} == testnet ]] || fail "DW_DP_NET=$DW_DP_NET; the nightly runs on testnet only"
[[ $DW_DP_NOTIFY == /* && -x $DW_DP_NOTIFY ]] || fail "DW_DP_NOTIFY must be an absolute path to an executable"
[[ $DW_DP_FUNDER == /* ]] || fail "DW_DP_FUNDER must be an absolute path"
funder=$(realpath -e "$DW_DP_FUNDER") || fail "DW_DP_FUNDER names no file"
# The fetch, the build and the dependency install run code of the fetched revision. They run
# without the funder's variables and through dashpay-unfunded.sh, where the state root holds only
# the checkout and the funder's phrase file reads empty. The suites then run that revision with
# the funder: it is a dedicated testnet wallet with a small capped balance (regtest/README.md).
export -n DW_DP_FUNDER DW_DP_FUNDER_BIRTH DW_DP_HOLD_UPSTREAM DW_DP_STATE
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-12}
unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR

# The state root is checked before anything is written in it: absolute, not a symlink, a
# directory of this user's with no group or other access (created so when missing), at its
# canonical path no git checkout and inside none, and not where this script lives.
[[ $DW_DP_STATE == /* ]] || fail "DW_DP_STATE must be an absolute path"
if [[ ! -e $DW_DP_STATE && ! -L $DW_DP_STATE ]]; then
  mkdir -p "$(dirname "$DW_DP_STATE")"
  mkdir -m 700 "$DW_DP_STATE"
fi
[[ ! -L $DW_DP_STATE ]] || fail "DW_DP_STATE $DW_DP_STATE is a symlink"
state=$(realpath -e "$DW_DP_STATE")
if [[ ! -d $state || ! -O $state ]] || (( 0$(stat -c %a "$state") & 077 )); then
  fail "DW_DP_STATE must be a directory owned by $(id -un), mode 0700"
fi
top=$state
while :; do
  [[ ! -e $top/.git && ! -L $top/.git ]] || fail "DW_DP_STATE $state is a git checkout or inside one ($top)"
  [[ $top != / ]] || break
  top=$(dirname "$top")
done
self=$(realpath -e "${BASH_SOURCE[0]}")
[[ $self != "$state"/* ]] || fail "this script runs from inside DW_DP_STATE; install it elsewhere"
for path in "$state/nightly.lock" "$state/nightly" "$state/repo"; do
  [[ ! -L $path ]] || fail "$path is a symlink"
done
exec 9>"$state/nightly.lock"
flock -n 9 || fail "another run holds $state/nightly.lock"

repo=$state/repo
mkdir -p "$state/nightly" "$repo"
unfunded() {
  "$(dirname "$self")/dashpay-unfunded.sh" "$state" "$repo" "$funder" "$@"
}
out=$state/nightly/$(date -u +%Y%m%d-%H%M%S)
mkdir "$out"
exec > >(tee -a "$out/nightly.log") 2>&1

if ! unfunded "$repo" bash -c '[[ ! -s $1 && ! -e $2 ]]' _ "$funder" "$state/nightly"; then
  echo "dashpay-nightly: the build's sandbox did not start, or it can read the funder's file or the state root"
  exit 1
fi
if [[ ! -d $repo/.git ]]; then
  unfunded "$repo" git clone --quiet https://github.com/PastaPastaPasta/dashwallet-desktop.git "$repo"
fi
unfunded "$repo" git fetch --quiet origin
unfunded "$repo" git checkout --quiet --detach "${DW_DP_REF:-origin/main}"
echo "dashpay-nightly: $(git -C "$repo" rev-parse HEAD) on testnet, output in $out"

DWCLI=$(unfunded "$repo/rust" cargo build -p dwcli --locked --message-format=json-render-diagnostics |
  python3 -c 'import json, sys
paths = [m["executable"] for m in map(json.loads, sys.stdin)
         if m.get("reason") == "compiler-artifact" and m["target"]["name"] == "dwcli" and m.get("executable")]
print(paths[-1] if paths else "")')
if [[ -z $DWCLI || ! -x $DWCLI ]]; then
  echo "dashpay-nightly: the build named no dwcli binary"
  exit 1
fi
echo "dashpay-nightly: dwcli $DWCLI"

cd "$repo/regtest/harness"
if [[ ! -x .venv/bin/python ]]; then
  unfunded "$PWD" uv venv .venv --python 3.12 -q
fi
unfunded "$PWD" uv pip install -q --python .venv/bin/python -r requirements.txt

export DW_DP_NET=testnet DW_DP_STATE=$state DW_DP_FUNDER DW_DP_FUNDER_BIRTH DW_DP_HOLD_UPSTREAM DWCLI
export DW_DP_STRICT=1 DW_DP_LOGS=$out/dwcli
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
  echo "dashpay-nightly: the secret scan did not run or did not pass; pytest's output stays in $out only (pytest exited $status)"
  exit 1
fi
grep -E '^(tests/|PASSED|FAILED|ERROR|XFAIL|XPASS|SKIPPED|=+ .* in [0-9.]+s)' "$out/pytest.log" || true
echo "dashpay-nightly: pytest exited $status"
exit "$status"
