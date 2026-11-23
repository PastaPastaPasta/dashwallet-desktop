#!/usr/bin/env python3
"""Generates testdata/core_v23_long_passphrase.json from a released regtest dashd.

Dash Core 23.0.0 to 23.1.x let a descriptor wallet take a BIP39 passphrase of any length through
`upgradetohd` and cut the PBKDF2 salt at 256 bytes; legacy wallets refuse over 256 bytes from 19.2.0
on, and descriptor wallets took a mnemonic only from 23.0.0 (v23.1.10 src/wallet/bip39.cpp:140-145,
src/wallet/wallet.cpp:3916-3928). v24.0.0 refuses one over 256 bytes, with or without
-allow-deprecated-long-mnemonics (v24.0.0 src/wallet/rpc/wallet.cpp:384-392), so the regtest
harness can no longer create such a wallet. This records one wallet made by the last v23 release
for the desktop's restore test.

Usage: start a regtest dashd from the release archive, then
  gen_long_passphrase.py --rpc http://u:p@127.0.0.1:<port> --archive <archive file name> \\
      --sha256 <archive SHA-256 from SHA256SUMS.asc> --mnemonic-file <f> --passphrase-file <f>

The mnemonic and passphrase are read from files so they never appear in a process list or log.
"""
import argparse
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TESTDATA = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
from gen_vectors import Rpc  # noqa: E402

COUNT = 5


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", required=True)
    ap.add_argument("--archive", required=True)
    ap.add_argument("--sha256", required=True)
    ap.add_argument("--mnemonic-file", required=True)
    ap.add_argument("--passphrase-file", required=True)
    args = ap.parse_args()
    rpc = Rpc(args.rpc)
    with open(args.mnemonic_file, encoding="utf-8") as f:
        mnemonic = f.read().strip()
    with open(args.passphrase_file, encoding="utf-8") as f:
        passphrase = f.read().rstrip("\n")
    version = rpc.call("getnetworkinfo")["subversion"]
    name = "long_passphrase"
    rpc.call("createwallet", name, False, True, "", False, True, False)  # blank descriptor wallet
    rpc.call("upgradetohd", mnemonic, passphrase, wallet=name)
    descs = rpc.call("listdescriptors", True, wallet=name)["descriptors"]
    external = [rpc.call("getnewaddress", wallet=name) for _ in range(COUNT)]
    internal = [rpc.call("getrawchangeaddress", wallet=name) for _ in range(COUNT)]
    doc = {
        "source": f"regtest dashd {version}: upgradetohd on a blank descriptor wallet",
        "generator": "testdata/oracle/dashd/gen_long_passphrase.py",
        "archive": args.archive,
        "archive_sha256": args.sha256,
        "mnemonic": mnemonic,
        "passphrase": passphrase,
        "passphrase_bytes": len(passphrase.encode()),
        "descriptor_master_xprv": descs[0]["desc"].split("(")[1].split("/")[0],
        "external": external,
        "internal": internal,
    }
    out = os.path.join(TESTDATA, "core_v23_long_passphrase.json")
    with open(out, "w", encoding="utf-8") as f:
        json.dump(doc, f, indent=2, ensure_ascii=False)
        f.write("\n")
    print(f"wrote {out} ({version})", file=sys.stderr)


if __name__ == "__main__":
    main()
