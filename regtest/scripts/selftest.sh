#!/usr/bin/env sh
# End-to-end check of regtest.sh: start the compose node on non-default ports, mine, fund a
# second wallet, verify its balance, and tear down. Exits non-zero on any mismatch.
set -eu

R="$(cd "$(dirname "$0")" && pwd)/regtest.sh"
export DWD_COMPOSE_PROJECT=${DWD_COMPOSE_PROJECT:-dwd-regtest-selftest}
export DWD_RPC_HOST_PORT=${DWD_RPC_HOST_PORT:-29898}
export DWD_P2P_HOST_PORT=${DWD_P2P_HOST_PORT:-29899}
trap '"$R" down >/dev/null 2>&1 || true' EXIT

"$R" up
"$R" mine 5
"$R" cli createwallet ext >/dev/null
addr=$("$R" cli -rpcwallet=ext getnewaddress)
txid=$("$R" fund "$addr" 3.25)
echo "fund txid: $txid"
balance=$("$R" cli -rpcwallet=ext getbalance)
"$R" status
height=$("$R" cli getblockcount)
# 5 mined + 101 maturity blocks + 1 confirmation block
[ "$height" = "107" ] || { echo "FAIL: height $height, want 107" >&2; exit 1; }
[ "$balance" = "3.25000000" ] || { echo "FAIL: ext balance $balance, want 3.25000000" >&2; exit 1; }
echo "PASS: regtest.sh selftest (height $height, ext balance $balance)"
