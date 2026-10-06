#!/usr/bin/env python3
"""Generates the dash-qt compatibility golden vectors (R2, M2) from a regtest dashd.

Writes (relative to testdata/compat/):
  walletdat/<name>.dat         SQLite descriptor wallet.dat files, copied after unloadwallet
  walletdat/bdb_legacy_head.dat  first 4096 bytes of a legacy Berkeley DB wallet.dat (detection only)
  listdescriptors_plain.json   `listdescriptors true` of the plain descriptor wallet
  psbt/*.b64, psbt/final.hex   a PSBT made by walletcreatefundedpsbt, signed by walletprocesspsbt,
                               finalized by finalizepsbt
  manifest.json                what each file holds, from RPC calls

Usage: start a regtest dashd (regtest/scripts/regtest.sh up, or the dwd-r2 compose project), then
  gen_compat_vectors.py --rpc http://dwd:dwd@127.0.0.1:29898 --container dwd-r2-dashd-1

`--container` reads wallet files through `docker exec <name> cat` (the compose datadir is a
tmpfs that `docker cp` cannot read); without it files are read from `--datadir` on this host.
The wallet names are fixed, so the node must not already have them. Every run produces new
files (random salts, IVs and timestamps); the tests check structure and derived values only.
"""
import argparse
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TESTDATA = os.path.abspath(os.path.join(HERE, "..", ".."))
OUT = os.path.join(TESTDATA, "compat")
sys.path.insert(0, HERE)
from gen_vectors import Rpc  # noqa: E402

ABANDON = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
LEGAL = "legal winner thank year wave sausage worth useful legal winner thank yellow"
LETTER = "letter advice cage absurd amount doctor acoustic avoid letter advice cage above"
# Passes only Dash Core's weak checksum (testdata/oracle/bip39/weak_sample.txt line 1).
WEAK = "alter undo step harbor skate color young picture sand chef goat ordinary"
NON_ASCII_PASS = "pässwörd ñ € \U0001F511"
# 300 bytes: longer than the 256 bytes Dash Core keeps of the BIP39 salt.
LONG_PASS = ("long passphrase é " * 16)[:290] + "0123456789"
WALLET_PASS = "wallet pass é 🔐"
ADDRESS_COUNT = 20


class Node:
    def __init__(self, rpc, container, datadir):
        self.rpc = rpc
        self.container = container
        self.datadir = datadir

    def call(self, method, *params, wallet=None):
        return self.rpc.call(method, *params, wallet=wallet)

    def read(self, path):
        if self.container:
            return subprocess.run(["docker", "exec", self.container, "cat", path], check=True,
                                  capture_output=True).stdout
        with open(path, "rb") as f:
            return f.read()

    def wallet_path(self, name):
        info = self.call("getwalletinfo", wallet=name)
        base = self.datadir or "/home/dash/.dashcore"
        return f"{base}/regtest/wallets/{info['walletname']}/wallet.dat"


def derive(node, desc, count):
    """First `count` addresses of a ranged descriptor (public form) via deriveaddresses."""
    info = node.call("getdescriptorinfo", desc.split("#")[0])
    return node.call("deriveaddresses", info["descriptor"], [0, count - 1])


def public_descriptors(node, wallet):
    return {d["desc"]: d for d in node.call("listdescriptors", False, wallet=wallet)["descriptors"]}


def chains(node, wallet):
    """First addresses of each active chain and of the CoinJoin descriptor."""
    out = {}
    for d in node.call("listdescriptors", False, wallet=wallet)["descriptors"]:
        if d.get("coinjoin"):
            key = "coinjoin"
        elif d.get("active"):
            key = "internal" if d.get("internal") else "external"
        else:
            continue
        out[key] = derive(node, d["desc"], ADDRESS_COUNT)
    return out


def save(rel, data):
    path = os.path.join(OUT, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    mode = "wb" if isinstance(data, bytes) else "w"
    with open(path, mode) as f:
        f.write(data)


def copy_wallet(node, name, rel):
    path = node.wallet_path(name)
    node.call("unloadwallet", name)
    data = node.read(path)
    save(rel, data)
    node.call("loadwallet", name)
    return len(data)


def descriptor_wallet(node, name, mnemonic, passphrase, wallet_pass, labels):
    node.call("createwallet", name, False, True, "", False, True, False)
    if wallet_pass:
        node.call("upgradetohd", mnemonic, passphrase, wallet_pass, wallet=name)
    else:
        node.call("upgradetohd", mnemonic, passphrase, wallet=name)
    if wallet_pass:
        node.call("walletpassphrase", wallet_pass, 60, wallet=name)
    labelled = []
    for label in labels:
        addr = node.call("getnewaddress", label, wallet=name)
        labelled.append({"address": addr, "label": label})
    node.call("getrawchangeaddress", wallet=name)
    if wallet_pass:
        node.call("walletlock", wallet=name)
    entry = {
        "file": f"walletdat/{name}.dat",
        "mnemonic": mnemonic,
        "mnemonic_passphrase": passphrase,
        "wallet_passphrase": wallet_pass,
        "encrypted": bool(wallet_pass),
        "labels": labelled,
        "addresses": chains(node, name),
    }
    entry["size_bytes"] = copy_wallet(node, name, entry["file"])
    return entry


def nomnemonic_wallet(node, name, tprv):
    node.call("createwallet", name, False, True, "", False, True, False)
    descs = []
    for path, internal in (("44h/1h/0h/0/*", False), ("44h/1h/0h/1/*", True)):
        d = node.call("getdescriptorinfo", f"pkh({tprv}/{path})")
        descs.append({"desc": f"pkh({tprv}/{path})#{d['checksum']}", "timestamp": "now", "active": True,
                      "internal": internal, "range": [0, 9]})
    res = node.call("importdescriptors", descs, wallet=name)
    assert all(r["success"] for r in res), res
    entry = {"file": f"walletdat/{name}.dat", "xprv": tprv, "encrypted": False,
             "addresses": chains(node, name)}
    entry["size_bytes"] = copy_wallet(node, name, entry["file"])
    return entry


def bdb_head(node):
    node.call("createwallet", "legacy_bdb", False, False, "", False, False, False)
    data = node.read(node.wallet_path("legacy_bdb"))
    save("walletdat/bdb_legacy_head.dat", data[:4096])
    return {"file": "walletdat/bdb_legacy_head.dat", "full_size_bytes": len(data)}


def psbt_vectors(node):
    """A 2-input PSBT from a funded descriptor wallet with a known phrase."""
    node.call("createwallet", "psbt_src", False, True, "", False, True, False)
    node.call("upgradetohd", LEGAL, "", wallet="psbt_src")
    try:
        node.call("loadwallet", "miner")
    except RuntimeError:
        pass
    if "miner" not in node.call("listwallets"):
        node.call("createwallet", "miner")
    if node.call("getbalance", wallet="miner") < 10:
        node.call("generatetoaddress", 110, node.call("getnewaddress", wallet="miner"))
    a0 = node.call("getnewaddress", wallet="psbt_src")
    a1 = node.call("getnewaddress", wallet="psbt_src")
    node.call("sendtoaddress", a0, 1.5, wallet="miner")
    node.call("sendtoaddress", a1, 2.25, wallet="miner")
    node.call("generatetoaddress", 1, node.call("getnewaddress", wallet="miner"))
    dest = node.call("getnewaddress", wallet="miner")
    created = node.call("walletcreatefundedpsbt", [], [{dest: 3.0}], 0,
                        {"fee_rate": 2, "changePosition": 1, "add_inputs": True}, True, wallet="psbt_src")
    unsigned = created["psbt"]
    signed = node.call("walletprocesspsbt", unsigned, True, "ALL", True, False, wallet="psbt_src")
    final = node.call("finalizepsbt", signed["psbt"])
    save("psbt/unsigned.b64", unsigned + "\n")
    save("psbt/signed.b64", signed["psbt"] + "\n")
    save("psbt/final.hex", final["hex"] + "\n")
    return {
        "mnemonic": LEGAL,
        "mnemonic_passphrase": "",
        "destination": dest,
        "fee": int(round(created["fee"] * 100_000_000)),
        "decoded_unsigned": node.call("decodepsbt", unsigned),
        "analyze_unsigned": node.call("analyzepsbt", unsigned),
        "analyze_signed": node.call("analyzepsbt", signed["psbt"]),
        "final_txid": node.call("decoderawtransaction", final["hex"])["txid"],
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", required=True)
    ap.add_argument("--container")
    ap.add_argument("--datadir")
    args = ap.parse_args()
    node = Node(Rpc(args.rpc), args.container, args.datadir)
    version = node.call("getnetworkinfo")["subversion"]

    plain = descriptor_wallet(node, "desc_plain", ABANDON, "TREZOR", "",
                              ["", "savings", "spaces and % and # and é", "tab\there"])
    save("listdescriptors_plain.json", json.dumps(node.call("listdescriptors", True, wallet="desc_plain"),
                                                  indent=1, ensure_ascii=False) + "\n")
    manifest = {
        "source": f"regtest dashd {version}",
        "generator": "testdata/oracle/dashd/gen_compat_vectors.py",
        "wallets": [
            plain,
            descriptor_wallet(node, "desc_encrypted", LETTER, NON_ASCII_PASS, WALLET_PASS, ["enc é"]),
            descriptor_wallet(node, "desc_weak_longpass", WEAK, LONG_PASS, "", []),
            nomnemonic_wallet(node, "desc_nomnemonic",
                              # BIP32 test vector 1 master key, testnet encoding.
                              "tprv8ZgxMBicQKsPeDgjzdC36fs6bMjGApWDNLR9erAXMs5skhMv36j9MV5ecvfavji5khqjWaWSFhN3YcCUUdiKH6isR4Pwy3U5y5egddBr16m"),
        ],
        "bdb": bdb_head(node),
        "listdescriptors": {"file": "listdescriptors_plain.json", "wallet": "desc_plain"},
        "psbt": psbt_vectors(node),
    }
    save("manifest.json", json.dumps(manifest, indent=1, ensure_ascii=False, default=str) + "\n")
    print("wrote", OUT)


if __name__ == "__main__":
    main()
