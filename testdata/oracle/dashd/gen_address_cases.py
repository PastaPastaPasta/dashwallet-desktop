#!/usr/bin/env python3
"""Writes testdata/address_cases.json: Dash Core's verdict (`validateaddress`)
on Base58, DIP-18 Platform and malformed strings, for mainnet and regtest.

Usage: gen_address_cases.py --main http://u:p@127.0.0.1:PORT --regtest http://u:p@127.0.0.1:PORT
(two dashd instances, the mainnet one started with -connect=0 so it never syncs).

Platform and shielded strings are built here with the BIP-350 reference
bech32m encoder; `expected_kind` records what they are by construction.
"""
import argparse
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gen_vectors import Rpc, b58check, write_json  # noqa: E402

TESTDATA = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))

# BIP-350 reference implementation (bech32 / bech32m).
CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"
BECH32_CONST, BECH32M_CONST = 1, 0x2BC830A3


def polymod(values):
    gen = [0x3B6A57B2, 0x26508E6D, 0x1EA119FA, 0x3D4233DD, 0x2A1462B3]
    chk = 1
    for v in values:
        b = chk >> 25
        chk = (chk & 0x1FFFFFF) << 5 ^ v
        for i in range(5):
            chk ^= gen[i] if ((b >> i) & 1) else 0
    return chk


def hrp_expand(hrp):
    return [ord(x) >> 5 for x in hrp] + [0] + [ord(x) & 31 for x in hrp]


def bech32_encode(hrp, data, const):
    values = hrp_expand(hrp) + data
    pm = polymod(values + [0] * 6) ^ const
    checksum = [(pm >> 5 * (5 - i)) & 31 for i in range(6)]
    return hrp + "1" + "".join(CHARSET[d] for d in data + checksum)


def convertbits(data, frombits, tobits, pad=True):
    acc, bits, ret, maxv = 0, 0, [], (1 << tobits) - 1
    for value in data:
        acc = (acc << frombits) | value
        bits += frombits
        while bits >= tobits:
            bits -= tobits
            ret.append((acc >> bits) & maxv)
    if pad and bits:
        ret.append((acc << (tobits - bits)) & maxv)
    return ret


def bech32m(hrp, payload, const=BECH32M_CONST):
    return bech32_encode(hrp, convertbits(payload, 8, 5), const)


def flip(s, i):
    c = s[i]
    return s[:i] + CHARSET[(CHARSET.index(c) + 1) % 32] + s[i + 1:]


def cases_for(network):
    hrp = "dash" if network == "main" else "tdash"
    other_hrp = "tdash" if network == "main" else "dash"
    pk, sh, sk = (76, 16, 204) if network == "main" else (140, 19, 239)
    h = hashlib.sha256(b"dw-uri address cases").digest()[:20]
    p2pkh = b58check(bytes([pk]) + h)
    p2sh = b58check(bytes([sh]) + h)
    plat_pkh = bech32m(hrp, [0xB0] + list(h))
    plat_sh = bech32m(hrp, [0x80] + list(h))
    shielded = bech32m(hrp, [0x10] + list(hashlib.sha256(b"orchard").digest() + hashlib.sha256(b"x").digest()[:11]))
    c = [
        (p2pkh, "core_p2pkh"), (p2sh, "core_p2sh"),
        ("  " + p2pkh + "\n", "core_p2pkh"),
        (b58check(bytes([140 if network == "main" else 76]) + h), "invalid"),
        (p2pkh[:-1] + ("2" if p2pkh[-1] != "2" else "3"), "invalid"),
        (b58check(bytes([pk]) + h + b"\x00"), "invalid"),
        (b58check(bytes([pk]) + h[:19]), "invalid"),
        (b58check(bytes([sk]) + bytes(range(1, 33)) + b"\x01"), "invalid"),
        (p2pkh[:10] + "0" + p2pkh[11:], "invalid"),
        (p2pkh[:10] + " " + p2pkh[10:], "invalid"),
        ("", "invalid"), ("x", "invalid"), ("invalid address", "invalid"),
        (plat_pkh, "platform_p2pkh"), (plat_sh, "platform_p2sh"), (plat_pkh.upper(), "platform_p2pkh"),
        (plat_pkh[:6] + plat_pkh[6:].upper(), "invalid"),
        (bech32m(hrp, [0xB0] + list(h), BECH32_CONST), "invalid"),
        (bech32m(hrp, [0x42] + list(h)), "invalid"),
        (bech32m(hrp, [0xB0] + list(h) + [0]), "invalid"),
        (shielded, "shielded"),
        (flip(plat_pkh, len(plat_pkh) - 3), "invalid"),
        (flip(flip(plat_pkh, len(plat_pkh) - 3), len(plat_pkh) - 9), "invalid"),
        (flip(plat_pkh, len(hrp) + 2), "invalid"),
        (bech32m(other_hrp, [0xB0] + list(h)), "invalid"),
        (hrp + "xyz", "invalid"), (hrp + "1qq", "invalid"), (hrp + "1" + "q" * 90, "invalid"),
        (hrp + "1qqqqqqqb", "invalid"), (hrp + "1qqqqqqqé", "invalid"),
    ]
    return [{"network": network, "input": s, "expected_kind": k} for s, k in c]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--main", required=True)
    ap.add_argument("--regtest", required=True)
    args = ap.parse_args()
    out = []
    versions = set()
    for network, url in (("main", args.main), ("regtest", args.regtest)):
        rpc = Rpc(url)
        versions.add(rpc.call("getnetworkinfo")["subversion"])
        for case in cases_for(network):
            r = rpc.call("validateaddress", case["input"])
            case["core_valid"] = r["isvalid"]
            if r["isvalid"]:
                case["core_script_type"] = "p2pkh" if r["scriptPubKey"].startswith("76a914") else "p2sh"
            else:
                case["core_error"] = r.get("error", "")
                case["core_error_locations"] = r.get("error_locations", [])
            out.append(case)
    doc = {"source": "dashd validateaddress (" + ", ".join(sorted(versions)) + ")",
           "generator": "testdata/oracle/dashd/gen_address_cases.py", "cases": out}
    write_json(os.path.join(TESTDATA, "address_cases.json"), doc)
    print(len(out), "cases")


if __name__ == "__main__":
    main()
