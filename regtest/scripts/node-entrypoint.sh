#!/usr/bin/env sh
# Container entrypoint: run a single dashd in regtest mode in the foreground.
# Any arguments are appended to the dashd command line (e.g. extra -debug categories).
#
# Environment:
#   DWD_RPC_USER / DWD_RPC_PASSWORD  RPC credentials (regtest-only defaults: dwd / dwd)
#   DWD_DATADIR                      data directory (default: /home/dash/.dashcore)
set -eu

datadir=${DWD_DATADIR:-/home/dash/.dashcore}
mkdir -p "$datadir"

exec dashd \
    -regtest \
    -datadir="$datadir" \
    -printtoconsole \
    -server \
    -txindex=1 \
    -blockfilterindex=1 \
    -peerblockfilters=1 \
    -peerbloomfilters=1 \
    -listen=1 \
    -port=19899 \
    -bind=0.0.0.0:19899 \
    -rpcport=19898 \
    -rpcbind=0.0.0.0:19898 \
    -rpcallowip=0.0.0.0/0 \
    -rpcuser="${DWD_RPC_USER:-dwd}" \
    -rpcpassword="${DWD_RPC_PASSWORD:-dwd}" \
    -fallbackfee=0.00001 \
    -debug=net -debug=mempool -debug=rpc \
    "$@"
