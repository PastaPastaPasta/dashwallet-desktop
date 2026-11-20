#!/usr/bin/env bash
# Runs one command of the DashPay nightly where the funder's credentials cannot be read
# (dashpay-nightly.sh: the fetch, the build and the dependency install, which run code from the
# fetched revision). In the command's mount namespace the state root is an empty directory holding
# only the checkout, and the funder's phrase file is empty. There is no fallback: without
# bubblewrap, or where it cannot start, this fails and so does the nightly.
#
#   dashpay-unfunded.sh <state> <repo> <funder> <dir> <command> [argument...]
#
# <state> and <repo> are canonical paths, <repo> a directory inside <state>; <funder> is the
# phrase file, an absolute path, with no other hard link (one elsewhere would stay readable);
# <dir> is where the command starts.
set -euo pipefail
[[ $# -ge 5 ]] || { echo "usage: dashpay-unfunded.sh <state> <repo> <funder> <dir> <command> [argument...]" >&2; exit 2; }
[[ $3 == /* ]] || { echo "dashpay-unfunded: the funder's file must be an absolute path" >&2; exit 1; }
state=$1 repo=$2 funder=$(realpath -e "$3") dir=$4
shift 4
[[ $(stat -c %h "$funder") == 1 ]] || { echo "dashpay-unfunded: the funder's file has another hard link, which the sandbox cannot hide" >&2; exit 1; }
command -v bwrap > /dev/null || { echo "dashpay-unfunded: bubblewrap (bwrap) is not installed" >&2; exit 1; }
[[ -d $repo && $repo == "$state"/* ]] || { echo "dashpay-unfunded: $repo is not a directory inside $state" >&2; exit 1; }
exec bwrap --die-with-parent --dev-bind / / --tmpfs "$state" --bind "$repo" "$repo" \
  --ro-bind /dev/null "$funder" --chdir "$dir" -- "$@"
