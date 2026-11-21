#!/usr/bin/env bash
# A notifier for the DashPay nightly (DW_DP_NOTIFY): keeps one open issue labelled
# nightly-failure in DW_DP_ISSUE_REPO while the nightly is red, and closes it after a green run
# (ci/github/nightly-issue.sh, installed beside this script). Its one argument is the notice of
# dashpay-nightly-notify.sh: only its --green line, "<unit>: result=success" and nothing more,
# closes the issue. The issue is public and the suites can name directories, so only a notice
# of the expected form is published, with the artifact directory cut to its launcher-made name.
#
#   DW_DP_ISSUE_REPO=<owner/name>   the repository the issue lives in; required
set -euo pipefail
: "${DW_DP_ISSUE_REPO:?set DW_DP_ISSUE_REPO}"
notice=${1:?the notice}
state=failed
if [[ $notice =~ ^[A-Za-z0-9@._-]+:\ result=success$ ]]; then
  state=green
elif [[ $notice =~ ^([A-Za-z0-9@._-]+:\ result=[A-Za-z0-9-]+\ exit=[A-Za-z0-9-]+)\;\ artifacts:\ (.*)$ ]]; then
  head=${BASH_REMATCH[1]} dir=${BASH_REMATCH[2]##*/}
  if [[ ${BASH_REMATCH[2]} == "none yet" ]]; then
    notice="$head; artifacts: none yet"
  elif [[ $dir =~ ^[0-9]{8}-[0-9]{6}$ ]]; then
    notice="$head; artifacts: nightly/$dir on the nightly's host"
  else
    notice="$head; artifacts: not named here"
  fi
else
  notice="a notice of an unexpected form; see the nightly's host"
fi
GH_REPO=$DW_DP_ISSUE_REPO exec "$(dirname "$(readlink -f "$0")")/nightly-issue.sh" "$state" \
  "DashPay suites on testnet" "$(date -u +%F): $notice"
