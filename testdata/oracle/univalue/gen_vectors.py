#!/usr/bin/env python3
"""Builds univalue_oracle (Dash Core's own src/univalue) and writes
testdata/oracle/univalue/univalue_vectors.json (MG-04b D7).

Usage: gen_vectors.py --dash-repo <dash git checkout> [--tag v24.0.0-rc.3]

Extracts src/univalue and Core's SHA-256 at the tag with `git archive`, so the
checkout's working tree is never read. Each case in INPUTS is fed to
UniValue::read; the oracle prints read's verdict, write(), write(2), the
write(2) of dash-qt's WithoutFingerprint, its EnvelopeFingerprint, and
getInt<…> of each array element.
dw-protx's tests/oracle_univalue.rs checks the Rust reader and writer against
every case; nothing in the build or CI compiles C++.
"""
import argparse
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "univalue_vectors.json")

# A dash-qt session envelope as MnShareSession::toJson writes it
# (rc.3 src/qt/mnsharesession.cpp:276-349), with the fingerprint last.
ENVELOPE = (
    '{\n  "type": "dash-shared-mn-session",\n  "version": 1,\n  "network": "regtest",\n'
    '  "sessionId": "8f3a6c1e0b9d4a27c5e1f0a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6",\n'
    '  "revision": 2,\n  "stage": "draft",\n  "fundingTx": "",\n'
    '  "shares": [\n    {\n      "label": "Alice",\n      "amount": 50000000000,\n'
    '      "ownerAddress": "yNs5EmB8PDDMaDy1v4M4UwXgFSHd4Wi6Jn",\n'
    '      "refundAddress": "yXk2kVbJg2hLCWJ3qVfe7FoGnQ5gFyV9bM",\n      "rewardAddress": ""\n    },\n'
    '    {\n      "label": "Bob",\n      "amount": 50000000000,\n      "ownerAddress": "",\n'
    '      "refundAddress": "",\n      "rewardAddress": ""\n    }\n  ],\n'
    '  "terms": {\n    "coreP2PAddrs": "1.2.3.4:19999",\n    "operatorPubKey": "",\n'
    '    "votingAddress": "",\n    "operatorReward": 0,\n    "earlyPeriodBlocks": 52560,\n'
    '    "earlyPenalty": 1000000000\n  },\n'
    '  "contributions": [\n    {\n      "label": "Alice",\n      "inputs": [\n        {\n'
    '          "txid": "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",\n'
    '          "vout": 1,\n          "sequence": 4294967295\n        }\n      ],\n'
    '      "change": {\n        "address": "yZ7s1D8y8WjZy2s9Ue5o6m3gC1oQe8Qx2N",\n'
    '        "amount": 123456\n      },\n      "changeIndex": 0\n    }\n  ],\n'
    '  "protx": "",\n  "consentHash": "",\n  "collateralIndex": -1,\n  "sigs": [\n  ],\n'
    '  "prepareWallet": "",\n  "operatorSecretHolder": "Carol\'s VPS",\n'
    '  "coordinatorLabel": "Alice",\n  "fingerprint": "0123-ABCD"\n}'
)

INPUTS = [
    # Envelopes: as written, compact, with an unknown key, with duplicates.
    ENVELOPE,
    json.dumps(json.loads(ENVELOPE), separators=(",", ":")),
    ENVELOPE.replace('"fundingTx": "",', '"fundingTx": "", "extra": {"z": [1, 2.50, -0]},'),
    '{"type":"dash-shared-mn-sigs","fingerprint":"AAAA-AAAA","version":1,"fingerprint":"bbbb-bbbb"}',
    '{"a":1,"a":2,"b":{"fingerprint":"x","fingerprint":"y"},"fingerprint":"z"}',
    '{"fingerprint":"only"}',
    # Containers and layout.
    "{}", "[]", "[[]]", "[{}]", '{"a":[]}', '{"a":{}}', '[[[[1]]],{"x":[{}]}]',
    " \t\r\n[1 , 2 ]\n ", '{ "k" : "v" }',
    # Scalars at the top level.
    "true", "false", "null", "0", '"s"', "-",
    # Numbers: tokens kept, getInt via from_chars.
    "[0,-0,1,-1,1.0,1e3,1E+3,-1.5e-3,0.000001,-0.0]",
    "[9223372036854775807,9223372036854775808,-9223372036854775808,-9223372036854775809]",
    "[18446744073709551615,18446744073709551616,2147483647,2147483648,-2147483649,4294967295,4294967296]",
    "[007]", "[01]", "[-01]", "[1.]", "[.5]", "[+1]", "[1e]", "[1e+]", "[0x10]", "[--1]", "[1-]", "[-]", "-1",
    "[123456789012345678901234567890]",
    # Strings: escapes in and out.
    r'["\"\\\/\b\f\n\r\t"]',
    r'["\u0000\u001f\u007f\u0080\u00ff\u20ac\uFFFF"]',
    r'["\ud83d\ude00"]', r'["\ud83d"]', r'["\ude00"]', r'["\ud83dx"]', r'["\ud83d\u0041"]', r'["\u12"]',
    r'["\uzzzz"]', r'["\u00e9"]', r'"\u0041', r'"\u004',
    b'["\x7f"]', b'["\x01"]', b'["\x1f"]',
    b'["tab\there"]',
    b'["caf\xc3\xa9", "\xe2\x82\xac", "\xf0\x9f\x98\x80"]',
    # Invalid or unusual UTF-8 (passed as raw bytes below).
    b'["\xc0\xaf"]', b'["\xe0\x80\xaf"]', b'["\xed\xa0\x80"]', b'["\xff"]', b'["\xc3"]',
    b'["\xf4\x90\x80\x80"]', b'["a\x00b"]',
    # Malformed documents.
    "", " ", "[", "]", "{", '{"a"}', '{"a":}', '{"a":1,}', "[1,]", "[,1]", '{"a" 1}', "[1 2]",
    "tru", "nul", "[true false]", '"unterminated', "[1]x", "[1] [2]", "{} ", '{"a":1}}', "\"\\x\"",
    # Nesting written out, then UniValue's MAX_DEPTH (512): read only (READ_ONLY).
    "[" * 20 + "]" * 20, '{"a":' * 10 + "1" + "}" * 10,
    "[" * 511 + "]" * 511, "[" * 512 + "]" * 512, "[" * 513 + "]" * 513,
    '{"a":' * 512 + "1" + "}" * 512,
]
# Cases whose pretty rendering is quadratic in the depth: only read's verdict
# is kept.
READ_ONLY = {i for i, c in enumerate(INPUTS) if len(c) > 1000 and c[:4] in ("[[[[", '{"a"')}


PATHS = ["src/univalue", "src/crypto/sha256.cpp", "src/crypto/sha256.h", "src/crypto/common.h", "src/compat"]


def core_src_from_git(repo, tag, dest):
    data = subprocess.check_output(["git", "-C", repo, "archive", "--format=tar", tag] + PATHS)
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        tar.extractall(dest, filter="data")
    return os.path.join(dest, "src")


def build(src, out_dir):
    exe = os.path.join(out_dir, "univalue_oracle")
    lib = os.path.join(src, "univalue", "lib")
    srcs = [os.path.join(lib, f) for f in sorted(os.listdir(lib)) if f.endswith(".cpp")]
    srcs.append(os.path.join(src, "crypto", "sha256.cpp"))
    cxx = os.environ.get("CXX", "c++")
    subprocess.check_call([cxx, "-std=c++20", "-O1", "-DDISABLE_OPTIMIZED_SHA256",
                           "-I", os.path.join(src, "univalue", "include"), "-I", src, "-o", exe,
                           os.path.join(HERE, "univalue_oracle.cpp")] + srcs)
    return exe


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dash-repo", required=True)
    ap.add_argument("--tag", default="v24.0.0-rc.3")
    args = ap.parse_args()
    with tempfile.TemporaryDirectory() as tmp:
        src = core_src_from_git(args.dash_repo, args.tag, tmp)
        exe = build(src, tmp)
        lines = "".join("." + (c if isinstance(c, bytes) else c.encode()).hex() + "\n" for c in INPUTS)
        out = subprocess.run([exe], input=lines.encode(), capture_output=True, check=True).stdout
    cases = json.loads(out)
    assert len(cases) == len(INPUTS)
    for i in READ_ONLY:
        cases[i] = {"input": cases[i]["input"], "ok": cases[i]["ok"], "read_only": True}
    doc = {
        "source": f"Dash Core {args.tag} src/univalue via testdata/oracle/univalue/univalue_oracle.cpp",
        "cases": cases,
    }
    with open(OUT, "w") as f:
        json.dump(doc, f, indent=1)
        f.write("\n")
    print(f"wrote {len(cases)} cases to {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
