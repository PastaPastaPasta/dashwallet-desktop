#!/usr/bin/env sh
# Drive the docker-compose regtest node from a shell.
#
#   regtest.sh up                    build + start the node, wait until RPC answers
#   regtest.sh down                  stop the node and discard its chain
#   regtest.sh status                height, peers, miner balance
#   regtest.sh mine <n> [address]    mine n blocks (default payee: the `miner` wallet)
#   regtest.sh fund <address> <amt>  send amt DASH from the `miner` wallet, then mine 1 block
#                                    (mines 101 blocks first if the miner has no mature coins)
#   regtest.sh cli <args...>         raw dash-cli (node-level RPCs)
#   regtest.sh wcli <args...>        raw dash-cli against the `miner` wallet
#
# Environment: the same DWD_* variables as docker-compose.yml (project name, host ports, RPC creds).
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
compose() {
    docker compose -f "$here/docker-compose.yml" "$@"
}
cli() {
    compose exec -T dashd dash-cli -regtest -rpcport=19898 \
        -rpcuser="${DWD_RPC_USER:-dwd}" -rpcpassword="${DWD_RPC_PASSWORD:-dwd}" "$@"
}
wcli() {
    cli -rpcwallet=miner "$@"
}
ensure_miner() {
    if ! cli listwallets | grep -q '"miner"'; then
        cli loadwallet miner >/dev/null 2>&1 || cli createwallet miner >/dev/null
    fi
}

cmd=${1:-}
[ -n "$cmd" ] && shift
case "$cmd" in
    up)
        compose up -d --build --wait --wait-timeout 120 dashd
        cli getblockchaininfo | grep -E '"(chain|blocks)"'
        ;;
    down)
        compose down --volumes --timeout 30
        ;;
    status)
        echo "height: $(cli getblockcount)"
        echo "peers:  $(cli getconnectioncount)"
        ensure_miner
        echo "miner balance: $(wcli getbalance)"
        ;;
    mine)
        n=${1:?usage: regtest.sh mine <n> [address]}
        if [ "$#" -ge 2 ]; then
            address=$2
        else
            ensure_miner
            address=$(wcli getnewaddress)
        fi
        cli generatetoaddress "$n" "$address" >/dev/null
        echo "mined $n block(s) to $address; height $(cli getblockcount)"
        ;;
    fund)
        address=${1:?usage: regtest.sh fund <address> <amount>}
        amount=${2:?usage: regtest.sh fund <address> <amount>}
        ensure_miner
        balance=$(wcli getbalance)
        if [ "$balance" = "0.00000000" ]; then
            cli generatetoaddress 101 "$(wcli getnewaddress)" >/dev/null
        fi
        txid=$(wcli sendtoaddress "$address" "$amount")
        cli generatetoaddress 1 "$(wcli getnewaddress)" >/dev/null
        echo "$txid"
        ;;
    cli)
        cli "$@"
        ;;
    wcli)
        ensure_miner
        wcli "$@"
        ;;
    *)
        sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
        exit 2
        ;;
esac
