#!/usr/bin/env bash
# Builds ci/gate/fixtures/regtest40k-dw.tar.gz, the gate's large regtest wallet (G-02, DASHPAY §2.1
# "Test data"), and the dwcli reference listings U1 compares the app with. Run on a Linux box with
# rust-dashcore's dashd test assets (`eval $(python3 contrib/setup-dashd.py)` in rust-dashcore puts
# them in ~/.rust-dashcore-test):
#
#   ci/gate/make-regtest-fixture.sh WORK_DIR [DWCLI]
#
# 1. copies the regtest-40000 chain (dashpay/regtest-blockchain v0.0.4) and serves it with dashd
#    23.1.0 (block filters on, a private port pair);
# 2. creates an encrypted vault (passphrase: ci/gate/fixtures/regtest40k-passphrase.txt) and
#    restores the chain's wallet `default` (40,153 transactions in Core) from its public mnemonic;
# 3. scans it over SPV to height 40,000, then writes `dwcli list` and `dwcli history --limit 200`;
# 4. packs the network's datadir without the SPV block store (the app opens it offline).
# Everything is public test data. WORK_DIR ends up holding the node, the datadir and the listings.
set -euo pipefail

WORK=$(realpath -m "${1:?usage: make-regtest-fixture.sh WORK_DIR [DWCLI]}")
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
DWCLI=${2:-$REPO/rust/target/release/dwcli}
ASSETS=${RUST_DASHCORE_TEST:-$HOME/.rust-dashcore-test}
CHAIN=$ASSETS/regtest-blockchain-v0.0.4/regtest-40000
BIN=$ASSETS/dashcore-23.1.0/bin
PASS=$REPO/ci/gate/fixtures/regtest40k-passphrase.txt
P2P=${FIXTURE_P2P_PORT:-28612}
RPC=${FIXTURE_RPC_PORT:-28613}
mkdir -p "$WORK"
chmod 700 "$WORK"

if [[ ! -d "$WORK/node/regtest" ]]; then
  mkdir -p "$WORK/node"
  cp -a "$CHAIN/." "$WORK/node/"
fi
cat >"$WORK/node/dash.conf" <<EOF
regtest=1
[regtest]
port=$P2P
rpcport=$RPC
rpcbind=127.0.0.1
rpcallowip=127.0.0.1
bind=127.0.0.1
EOF
cli() { "$BIN/dash-cli" -datadir="$WORK/node" "$@"; }
if ! cli getblockcount >/dev/null 2>&1; then
  "$BIN/dashd" -datadir="$WORK/node" -daemon=0 -server=1 -blockfilterindex=1 -peerblockfilters=1 \
    -wallet=default -listenonion=0 >"$WORK/dashd.out" 2>&1 &
  echo $! >"$WORK/dashd.pid"
  # A node this script started stops with it.
  trap 'cli stop >/dev/null 2>&1 || true' EXIT
  for _ in $(seq 300); do cli getblockcount >/dev/null 2>&1 && break; sleep 1; done
fi
echo "dashd: height $(cli getblockcount), Core balance $(cli -rpcwallet=default getbalance)"

DW=$WORK/dw
# Regtest needs a DAPI address even with --no-platform; nothing ever connects to it.
NET=(--datadir "$DW" --network regtest --no-platform --dapi https://127.0.0.1:1443 --peer "127.0.0.1:$P2P" --passphrase-file "$PASS")
if [[ ! -d "$DW/regtest" ]]; then
  "$DWCLI" "${NET[@]}" init-vault
  python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["mnemonic"])' "$WORK/node/wallets/default.json" |
    "$DWCLI" "${NET[@]}" import --birth-height 0 --name Default
fi
time "$DWCLI" "${NET[@]}" sync --min-height 40000 --timeout-secs 3600 | tail -n 5
"$DWCLI" "${NET[@]}" list | tee "$WORK/list.txt"
"$DWCLI" "${NET[@]}" history --limit 200 >"$WORK/history.txt"
wc -l "$WORK/history.txt"

mkdir -p "$REPO/ci/gate/fixtures"
tar -czf "$REPO/ci/gate/fixtures/regtest40k-dw.tar.gz" -C "$DW" --exclude='regtest/spv/blocks' .
ls -l "$REPO/ci/gate/fixtures/regtest40k-dw.tar.gz"
