#!/usr/bin/env python3
"""Slim Dash Core v24 regtest vectors to the blocks BL-187's decode tests read.

Input: the `core_v24_regtest.json` that rust-dashcore's `dash/contrib/core-vectors/`
tooling extracts from the regtest chains Core's own functional tests build
(`run-functional-tests.sh <dash> v24.0.0-rc.3 <bin> <dir>`, then `extract.py` and
`slim.py`). Every block and transaction in it is dashd's: raw `getblock`/
`getrawtransaction` hex with Core's JSON for it.

Output: the blocks listed in BLOCKS, each with Core's hash, merkle root and txids, and,
for its provider transactions, Core's `decoderawtransaction` payload JSON.

    python3 -I make_payload_blocks.py core_v24_regtest.json ../../core_v24_payload_blocks.json
"""
import json
import sys

# Functional test @ height: what the block carries.
BLOCKS = [
    ("feature_masternode_payout_shares@121", "ProRegTx v3 with DIP-0026 payouts"),
    ("feature_masternode_shares@273", "ProRegTx v3 with a shared collateral (2 shares)"),
    ("rpc_netinfo@263", "Evo ProRegTx v3, extended net info (IPv4 and IPv6)"),
    ("feature_masternode_payout_shares@125", "ProUpServTx v3"),
    ("rpc_netinfo@269", "Evo ProUpServTx v3 with domain, I2P and Tor entries"),
    ("feature_masternode_payout_shares@124", "ProUpRegTx v3 with payouts"),
    ("feature_masternode_shares@292", "type 10, ProDisTx"),
    ("feature_masternode_shares@276", "type 11, ProUpShareTx"),
    ("feature_masternode_shares@279", "type 12, ProUpSharedRegTx"),
]

PAYLOAD_KEYS = {1: "proRegTx", 2: "proUpServTx", 3: "proUpRegTx", 10: "proDisTx",
                11: "proUpShareTx", 12: "proUpSharedRegTx"}


def main(src, dst):
    with open(src) as f:
        data = json.load(f)
    txs = {t["core"]["txid"]: t for t in data["transactions"]}
    blocks = {b["source"]: b for b in data["blocks"]}
    out = []
    for source, carries in BLOCKS:
        b = blocks[source]
        special = []
        for txid in b["txids"]:
            t = txs.get(txid)
            if t is None or t["core"]["type"] not in PAYLOAD_KEYS:
                continue
            key = PAYLOAD_KEYS[t["core"]["type"]]
            special.append({"txid": txid, "type": t["core"]["type"], "payload": t["core"][key]})
        if not special:
            sys.exit(f"{source}: no provider transaction")
        out.append({"source": source, "carries": carries, "height": b["height"],
                    "hash": b["hash"], "merkleroot": b["merkleroot"], "txids": b["txids"],
                    "special": special, "hex": b["hex"]})
    doc = {"generator": data["generator"], "blocks": out}
    with open(dst, "w") as f:
        json.dump(doc, f, indent=1)
        f.write("\n")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
