#!/usr/bin/env bash
# One open issue per nightly source, labelled nightly-failure, for as long as that nightly is red.
#
#   nightly-issue.sh failed <source> <message>   opens "Nightly failure: <source>", or comments on it
#   nightly-issue.sh green <source> <message>    comments on every open one and closes it
#
# Needs gh (signed in, or GH_TOKEN, with issue access), jq, and GH_REPO or a checkout to take
# the repository from. The message is posted as it is: the issues are public, so it carries no
# test output and no path of the machine that ran the nightly.
set -euo pipefail
state=${1:?failed or green}
title="Nightly failure: ${2:?source}"
message=${3:?message}
numbers=$(gh issue list --state open --label nightly-failure --limit 100 --json number,title |
  jq -r --arg t "$title" '.[] | select(.title == $t) | .number')
number=$(head -n 1 <<<"$numbers")
case $state in
  failed)
    if [[ -n $number ]]; then
      gh issue comment "$number" --body "$message"
    else
      gh issue create --title "$title" --label nightly-failure --body "$message"
    fi
    ;;
  green)
    for number in $numbers; do
      gh issue close "$number" --comment "$message"
    done
    ;;
  *)
    echo "nightly-issue: unknown state: $state" >&2
    exit 2
    ;;
esac
