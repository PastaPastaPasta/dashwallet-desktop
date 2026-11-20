#!/usr/bin/env bash
# The OnFailure= handler of the DashPay nightly (ci/agentbox/dw-dashpay-nightly-failed.service):
# runs the site's notifier, DW_DP_NOTIFY, with one line: the nightly unit's result and exit
# status, and the newest artifact directory. Never any test output.
#
#   DW_DP_NOTIFY=<file>   an absolute path to an executable that delivers its one argument; required
#   DW_DP_STATE=<dir>     the nightly's state root (artifacts under nightly/<date>); required
set -euo pipefail
: "${DW_DP_NOTIFY:?set DW_DP_NOTIFY}" "${DW_DP_STATE:?set DW_DP_STATE}"
unit=${1:-dw-dashpay-nightly.service}
result=$(systemctl --user show -p Result --value "$unit" 2>/dev/null || true)
code=$(systemctl --user show -p ExecMainStatus --value "$unit" 2>/dev/null || true)
last=$(find "$DW_DP_STATE/nightly" -mindepth 1 -maxdepth 1 -type d -name '[0-9]*' 2>/dev/null | sort | tail -n 1)
exec "$DW_DP_NOTIFY" "$unit: result=${result:-unknown} exit=${code:-unknown}; artifacts: ${last:-none yet}"
