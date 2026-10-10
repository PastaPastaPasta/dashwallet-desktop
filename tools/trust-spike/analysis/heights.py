#!/usr/bin/env python3
"""Resolve the block height of every quorum hash in a tuple log.

usage: heights.py NETWORK TUPLES_JSONL HEIGHTS_TXT

A quorum's hash is the hash of its base block (where its DKG started), so
that block's height places the quorum. Testnet asks the
local read-only oracle (`contrib/oracle-cli testnet getblockheader`, a pure
read); mainnet uses the public Insight API, because the mainnet oracle had
not reached the tip yet. Already-resolved hashes in HEIGHTS_TXT are kept.
"""
import json
import os
import subprocess
import sys
import urllib.request

ORACLE_CLI = os.path.expanduser("~/workspace/rdc-wt/integration/contrib/oracle-cli")


def rows(path):
    """JSON lines of a log another process may still be appending to."""
    with open(path) as f:
        for line in f:
            line = line.strip()
            if line:
                try:
                    yield json.loads(line)
                except json.JSONDecodeError:
                    pass  # a line still being written


def read_heights(path):
    with open(path) as f:
        return {h: int(n) for h, n in (line.split() for line in f if line.strip())}


def height(network, block_hash):
    if network == "testnet":
        out = subprocess.run([ORACLE_CLI, "testnet", "getblockheader", block_hash],
                             capture_output=True, text=True, timeout=60, check=True).stdout
        return json.loads(out)["height"]
    url = f"https://insight.dash.org/insight-api/block/{block_hash}"
    request = urllib.request.Request(url, headers={"User-Agent": "curl/8"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)["height"]


def main():
    network, tuples, heights_path = sys.argv[1:4]
    known = read_heights(heights_path) if os.path.exists(heights_path) else {}
    hashes = {call["hash"] for row in rows(tuples) for call in row.get("ctx_calls", [])}
    with open(heights_path, "a") as out:
        for block_hash in sorted(hashes - known.keys()):
            out.write(f"{block_hash} {height(network, block_hash)}\n")


if __name__ == "__main__":
    main()
