#!/usr/bin/env bash
# The DashPay suites, nightly on agentbox (DASHPAY §6 T3; ROADMAP DP1-07, DP2-06): builds
# dwcli at origin/main in its own checkout and runs the suites of regtest/README.md "DashPay suites"
# against testnet. Alerting, not gating: the exit status is pytest's, and the junit, the log and
# the run's dwcli logs stay under $DW_DP_STATE/nightly/<date>.
#
#   DW_DP_STATE=<dir>          persistent state (data roots, vault passphrase, the checkout); required
#   DW_DP_FUNDER=<file>        the funder's phrase (agentbox's secret store); required
#   DW_DP_FUNDER_BIRTH=<n>     the funder's birth height; required
#   DW_DP_SUITES="…"           the suite files (default: every tests/test_dp*.py)
#   DW_DP_REF=<ref>            what to test (default origin/main)
set -euo pipefail
: "${DW_DP_STATE:?set DW_DP_STATE}" "${DW_DP_FUNDER:?set DW_DP_FUNDER}" "${DW_DP_FUNDER_BIRTH:?set DW_DP_FUNDER_BIRTH}"
export DW_DP_NET=${DW_DP_NET:-testnet} DW_DP_STATE DW_DP_FUNDER DW_DP_FUNDER_BIRTH
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-12}

repo=$DW_DP_STATE/repo
out=$DW_DP_STATE/nightly/$(date -u +%Y%m%d-%H%M)
mkdir -p "$out"
export DW_DP_LOGS=$out/dwcli
exec > >(tee -a "$out/nightly.log") 2>&1

if [[ ! -d $repo/.git ]]; then
  git clone --quiet https://github.com/PastaPastaPasta/dashwallet-desktop.git "$repo"
fi
git -C "$repo" fetch --quiet origin
git -C "$repo" checkout --quiet --detach "${DW_DP_REF:-origin/main}"
echo "dashpay-nightly: $(git -C "$repo" rev-parse HEAD) on $DW_DP_NET, output in $out"

(cd "$repo/rust" && cargo build -p dwcli --locked)
export DWCLI=$repo/rust/target/debug/dwcli

cd "$repo/regtest/harness"
if [[ ! -x .venv/bin/python ]]; then
  uv venv .venv --python 3.12 -q
fi
uv pip install -q --python .venv/bin/python -r requirements.txt
read -r -a suites <<< "${DW_DP_SUITES:-$(cd "$repo/regtest/harness" && ls tests/test_dp*.py 2>/dev/null | tr '\n' ' ')}"
if [[ ${#suites[@]} -eq 0 ]]; then
  echo "dashpay-nightly: no DashPay suites at this revision"
  exit 0
fi
status=0
.venv/bin/python -m pytest -v -rA "${suites[@]}" --junitxml="$out/junit.xml" || status=$?
echo "dashpay-nightly: pytest exited $status"
exit "$status"
