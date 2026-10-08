#!/usr/bin/env sh
# Run one Dash Core functional test (vendored test_framework) against release binaries.
#
# Usage: run.sh <test.py> [test_framework options...]
#   e.g. run.sh dwd_mn_chainlock.py --loglevel=INFO
#
# Environment:
#   DASHCORE_DIR  unpacked release (bin/dashd, bin/dash-cli, ...); default /opt/dashcore
#   DWD_FUNC_TMP  scratch root for config.ini, cache and node datadirs; default ${TMPDIR:-/tmp}/dwd-functional
#   DWD_PYTHON    Python interpreter with dash_hash installed; default python3
#
# The functional framework normally reads config.ini produced by ./configure in a source build.
# Release binaries have no build tree, so this script writes an equivalent config.ini that declares
# the components the official release ships, and points DASHD/DASHCLI/... at the release binaries.
set -eu

if [ "$#" -lt 1 ]; then
    echo "usage: $0 <test.py> [options...]" >&2
    exit 2
fi
test_script=$1
shift

here=$(cd "$(dirname "$0")" && pwd)
# Everything below the scratch root is created owner-only, whatever the caller's umask: dwcli's
# engine refuses a database below a group-writable directory (0775 under Ubuntu's umask 002).
umask 077
dashcore=${DASHCORE_DIR:-/opt/dashcore}
scratch=${DWD_FUNC_TMP:-${TMPDIR:-/tmp}/dwd-functional}
mkdir -p "$scratch"

for bin in dashd dash-cli dash-util dash-wallet; do
    if [ ! -x "$dashcore/bin/$bin" ]; then
        echo "missing $dashcore/bin/$bin (set DASHCORE_DIR)" >&2
        exit 1
    fi
done

config="$scratch/config.ini"
cat > "$config" <<EOF
[environment]
PACKAGE_NAME=Dash Core
PACKAGE_BUGREPORT=https://github.com/dashpay/dash/issues
SRCDIR=$here
BUILDDIR=$dashcore
EXEEXT=
RPCAUTH=$dashcore/share/rpcauth/rpcauth.py

[components]
ENABLE_WALLET=true
USE_SQLITE=true
USE_BDB=true
ENABLE_CLI=true
ENABLE_UTIL_TOOL=true
ENABLE_WALLET_TOOL=true
ENABLE_BITCOIND=true
ENABLE_ZMQ=true
EOF

export DASHD="$dashcore/bin/dashd"
export DASHCLI="$dashcore/bin/dash-cli"
export DASHUTIL="$dashcore/bin/dash-util"
export DASHWALLET="$dashcore/bin/dash-wallet"

# --tmpdir must not exist yet; options given on the command line come later and win.
exec "${DWD_PYTHON:-python3}" "$here/$test_script" \
    --configfile="$config" \
    --cachedir="$scratch/cache" \
    --tmpdir="$scratch/run-$(date +%s)-$$" \
    "$@"
