#!/usr/bin/env python3
"""Writes testdata/governance_objects.json: governance object hashes as
Dash Core computes them (`Governance::Object::GetHash`), for the
dw-governance golden test.

Each case is a proposal data JSON written in dash-qt's key order
(`proposalcreate.cpp` `buildJsonAndHex`). `gobject check` validates it and
`gobject prepare 0 1 <time> <hex>` builds the 1 DASH collateral
transaction, whose `OP_RETURN` output carries the object hash (internal
byte order, `ToByteVector(hash)`); the display hash is its reverse.

Usage (a fresh regtest dashd):
  dashd -regtest -datadir=$DIR -daemon -listen=0 -rpcport=29446 -rpcuser=u -rpcpassword=p -fallbackfee=0.00001
  python3 testdata/oracle/dashd/gen_governance_objects.py --rpc http://u:p@127.0.0.1:29446
"""
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gen_vectors import TESTDATA, Rpc, write_json  # noqa: E402

TIME = 1_900_000_000

CASES = [
    # (name, url, amount text, start, end, revision, time)
    ("dwd-golden-1", "https://www.dash.org/p/1", "12.50", TIME - 1_000, TIME + 2_000_000, 1, TIME),
    ("Mixed_Case-2", "https://example.org", "0.12345678", TIME, TIME + 86_400, 1, TIME + 1),
    ("a", "http://x.y", "1.00", TIME + 100, TIME + 200, 1, TIME + 2),
    ("long-name-" + "x" * 30, "https://dash.org/" + "y" * 200, "250000.00", TIME, TIME + 5_000_000, 2, TIME + 3),
]


def data_json(name, address, amount, url, start, end):
    return (
        f'{{"name":"{name}","payment_address":"{address}","payment_amount":{amount},'
        f'"url":"{url}","start_epoch":{start},"end_epoch":{end},"type":1}}'
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", required=True)
    args = ap.parse_args()
    rpc = Rpc(args.rpc)
    version = rpc.call("getnetworkinfo")["subversion"]
    if "gov" not in rpc.call("listwallets"):
        rpc.call("createwallet", "gov")
    miner = rpc.call("getnewaddress", wallet="gov")
    if rpc.call("getblockcount") < 110:
        rpc.call("generatetoaddress", 110, miner)
    payee = rpc.call("getnewaddress", wallet="gov")
    out = []
    for name, url, amount, start, end, revision, t in CASES:
        data = data_json(name, payee, amount, url, start, end)
        data_hex = data.encode().hex()
        check = rpc.call("gobject", "check", data_hex)
        txid = rpc.call("gobject", "prepare", "0", revision, t, data_hex, wallet="gov")
        tx = rpc.call("getrawtransaction", txid, True)
        op_return = [o for o in tx["vout"] if o["scriptPubKey"]["asm"].startswith("OP_RETURN")]
        assert len(op_return) == 1, tx
        internal = op_return[0]["scriptPubKey"]["asm"].split()[1]
        assert op_return[0]["value"] == 1, op_return[0]
        out.append(
            {
                "data": data,
                "data_hex": data_hex,
                "revision": revision,
                "time": t,
                "parent_hash": "0" * 64,
                "object_hash": bytes.fromhex(internal)[::-1].hex(),
                "collateral_script": op_return[0]["scriptPubKey"]["hex"],
                "check": check,
            }
        )
        rpc.call("generatetoaddress", 1, miner)
    write_json(
        os.path.join(TESTDATA, "governance_objects.json"),
        {"source": f"dashd {version} regtest: gobject check / gobject prepare", "cases": out},
    )
    print("done:", version, len(out))


if __name__ == "__main__":
    sys.exit(main())
