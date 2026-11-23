#!/usr/bin/env python3
"""Writes testdata/governance_live.json from synced mainnet / testnet dashd
nodes, with read-only RPCs issued one at a time:

- `getsuperblockbudget` at superblocks on both sides of each network's
  V20 height, the first two and the last two before the tip, and one
  height that is not a superblock (only heights the node has blocks for), and
  `getgovernanceinfo`;
- on a node whose masternode sync has finished, up to `--max-proposals`
  proposals from `gobject list valid proposals` (hash, data, creation time)
  and their tallying votes from `gobject getcurrentvotes` (vote hash and
  `outpoint:time:outcome:signal`), at most `--max-votes` per proposal.

Usage:
  python3 testdata/oracle/dashd/gen_governance_live.py \
      --node testnet=http://127.0.0.1:37998,/path/to/testnet/.cookie \
      --node mainnet=http://127.0.0.1:27998,/path/to/mainnet/.cookie
"""
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gen_vectors import TESTDATA, Rpc, write_json  # noqa: E402

# (superblock start, cycle, V20 height) per network (Core chainparams).
PARAMS = {
    "mainnet": (614_820, 16_616, 1_987_776),
    "testnet": (4_200, 24, 905_100),
}


def cookie_rpc(url, cookie_path):
    # Read the cookie on every run: dashd rewrites it on restart.
    with open(cookie_path, encoding="utf-8") as f:
        user, password = f.read().strip().split(":", 1)
    scheme, rest = url.split("://", 1)
    return Rpc(f"{scheme}://{user}:{password}@{rest}")


def budget_heights(network, tip):
    # Superblocks are the multiples of the cycle from the start height on
    # (`CSuperblock::IsValidBlockHeight`).
    start, cycle, v20 = PARAMS[network]
    first = -(-start // cycle) * cycle
    first_after_v20 = -(-v20 // cycle) * cycle
    last = tip // cycle * cycle
    heights = {
        first, first + cycle,
        first_after_v20 - cycle, first_after_v20, first_after_v20 + cycle,
        last - cycle, last,
        # Not a superblock: its budget is 0.
        last - 1,
    }
    return sorted(h for h in heights if start <= h <= tip)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--node", action="append", required=True, help="network=url,cookie")
    ap.add_argument("--max-proposals", type=int, default=5)
    ap.add_argument("--max-votes", type=int, default=40)
    args = ap.parse_args()

    networks = {}
    for spec in args.node:
        network, rest = spec.split("=", 1)
        url, cookie = rest.split(",", 1)
        rpc = cookie_rpc(url, cookie)
        info = rpc.call("getnetworkinfo")
        tip = rpc.call("getblockcount")
        entry = {
            "dashd": info.get("buildversion", info["subversion"]),
            "tip": tip,
            "governanceinfo": rpc.call("getgovernanceinfo"),
            "budgets": {str(h): rpc.call("getsuperblockbudget", h) for h in budget_heights(network, tip)},
        }
        if rpc.call("mnsync", "status")["IsSynced"]:
            listed = rpc.call("gobject", "list", "valid", "proposals")
            proposals = []
            for object_hash in sorted(listed)[: args.max_proposals]:
                o = listed[object_hash]
                votes = rpc.call("gobject", "getcurrentvotes", object_hash)
                proposals.append({
                    "object_hash": object_hash,
                    "data_hex": o["DataHex"],
                    "creation_time": o["CreationTime"],
                    "collateral_hash": o["CollateralHash"],
                    "current_votes": dict(sorted(votes.items())[: args.max_votes]),
                })
            entry["proposals"] = proposals
        networks[network] = entry

    write_json(os.path.join(TESTDATA, "governance_live.json"), {
        "source": "gen_governance_live.py (read-only RPCs, one at a time)",
        "networks": networks,
    })


if __name__ == "__main__":
    main()
