#!/usr/bin/env bash
# A notifier for the DashPay nightly (DW_DP_NOTIFY): keeps one open issue labelled
# nightly-failure in DW_DP_ISSUE_REPO while the nightly is red, and closes it after a green run
# (ci/github/nightly-issue.sh, installed beside this script). Its one argument is the notice of
# dashpay-nightly-notify.sh: only its --green line, which ends in "result=success", closes the
# issue. The issue is public, so the artifact directory is cut to its date.
#
#   DW_DP_ISSUE_REPO=<owner/name>   the repository the issue lives in; required
set -euo pipefail
: "${DW_DP_ISSUE_REPO:?set DW_DP_ISSUE_REPO}"
notice=${1:?the notice}
case $notice in
  *"; artifacts: "*/*) notice="${notice%%; artifacts: *}; artifacts: nightly/${notice##*/} on the nightly's host" ;;
esac
state=failed
[[ $notice != *": result=success" ]] || state=green
GH_REPO=$DW_DP_ISSUE_REPO exec "$(dirname "$(readlink -f "$0")")/nightly-issue.sh" "$state" \
  "DashPay suites on testnet" "$(date -u +%F): $notice"
