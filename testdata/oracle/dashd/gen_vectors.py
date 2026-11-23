#!/usr/bin/env python3
"""Generates golden vectors from a running regtest dashd.

Writes (relative to testdata/):
  message_cases.json          signmessagewithprivkey / verifymessage results
  bip39_core_quirks.json      'dashd' section: seeds dashd derived via upgradetohd
  dumpwallet/*.txt            dumpwallet files, verbatim
  dumpwallet/manifest.json    what each dump contains, from RPC calls

Usage: start dashd, then
  gen_vectors.py --rpc http://u:p@127.0.0.1:29445 --dump-dir <dir dashd can write>

The dashd used for the committed vectors is recorded in each output.
"""
import argparse
import base64
import hashlib
import json
import os
import shutil
import subprocess
import sys
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
TESTDATA = os.path.abspath(os.path.join(HERE, "..", ".."))


class Rpc:
    def __init__(self, url):
        from urllib.parse import urlparse
        u = urlparse(url)
        self.base = f"{u.scheme}://{u.hostname}:{u.port}"
        self.auth = base64.b64encode(f"{u.username}:{u.password}".encode()).decode()

    def call(self, method, *params, wallet=None):
        path = self.base + (f"/wallet/{wallet}" if wallet else "/")
        body = json.dumps({"jsonrpc": "1.0", "id": 1, "method": method, "params": list(params)}).encode()
        req = urllib.request.Request(path, body, {"Authorization": "Basic " + self.auth, "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req) as r:
                resp = json.load(r)
        except urllib.error.HTTPError as e:
            resp = json.load(e)
        if resp.get("error"):
            raise RuntimeError(f"{method}: {resp['error']}")
        return resp["result"]


B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58check(payload: bytes) -> str:
    data = payload + hashlib.sha256(hashlib.sha256(payload).digest()).digest()[:4]
    n = int.from_bytes(data, "big")
    s = ""
    while n:
        n, r = divmod(n, 58)
        s = B58[r] + s
    return "1" * (len(data) - len(data.lstrip(b"\0"))) + s


def wif(secret: bytes, compressed: bool, prefix=239) -> str:
    return b58check(bytes([prefix]) + secret + (b"\x01" if compressed else b""))


def message_vectors(rpc, version):
    secrets = [
        (bytes.fromhex("d97f5108f11cda6eeebaaa420fef0726b1f898060b98489fa3098463c0032866"), True),
        (bytes.fromhex("d97f5108f11cda6eeebaaa420fef0726b1f898060b98489fa3098463c0032866"), False),
        (bytes([1] * 32), True),
        (bytes([1] * 32), False),
        (bytes.fromhex("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140"), True),  # n - 1
    ]
    messages = ["", "Trust no one", "This is just a test message", "héllo € \U0001F600", "a\nb\r\nc\x00d",
                "x" * 252, "y" * 253, "z" * 300]
    # A message over 65535 bytes (CompactSize 0xfe prefix), signed by the last key only.
    long_message = "w" * 65536
    rpc.call("createwallet", "msg", False, True, "", False, False, False)  # blank legacy wallet
    sign = []
    for n, (secret, compressed) in enumerate(secrets):
        key = wif(secret, compressed)
        rpc.call("importprivkey", key, "", False, wallet="msg")
        info = rpc.call("getaddressinfo", rpc.call("deriveaddresses", rpc.call("getdescriptorinfo", f"pkh({key})")["descriptor"])[0], wallet="msg")
        address = info["address"]
        for m in messages + ([long_message] if n == len(secrets) - 1 else []):
            sig = rpc.call("signmessagewithprivkey", key, m)
            sign.append({"wif": key, "network": "regtest", "secret_hex": secret.hex(), "compressed": compressed,
                         "address": address, "message": m, "signature": sig,
                         "signmessage": rpc.call("signmessage", address, m, wallet="msg"),
                         "verify": rpc.call("verifymessage", address, sig, m)})
    good = sign[1]
    verify = []
    for label, addr, sig, msg in [
        ("ok", good["address"], good["signature"], good["message"]),
        ("wrong message", good["address"], good["signature"], good["message"] + "!"),
        ("other key's address", sign[len(messages) * 2]["address"], good["signature"], good["message"]),
        ("not base64", good["address"], "not base64!", good["message"]),
        ("base64 without padding", good["address"], good["signature"].rstrip("="), good["message"]),
        ("64 bytes", good["address"], base64.b64encode(base64.b64decode(good["signature"])[:64]).decode(), good["message"]),
        ("all zero", good["address"], base64.b64encode(bytes(65)).decode(), good["message"]),
        ("header 0", good["address"], base64.b64encode(bytes([0]) + base64.b64decode(good["signature"])[1:]).decode(), good["message"]),
        ("header 35", good["address"], base64.b64encode(bytes([35 + (base64.b64decode(good["signature"])[0] - 27) % 4]) + base64.b64decode(good["signature"])[1:]).decode(), good["message"]),
        ("invalid address", "invalid_addr", good["signature"], good["message"]),
        ("p2sh address", rpc.call("addmultisigaddress", 1, [good["address"]], wallet="msg")["address"], good["signature"], good["message"]),
    ]:
        entry = {"case": label, "network": "regtest", "address": addr, "signature": sig, "message": msg}
        try:
            entry["result"] = rpc.call("verifymessage", addr, sig, msg)
        except RuntimeError as e:
            entry["error"] = str(e).split("'message': '")[1].split("'")[0]
        verify.append(entry)
    return {"source": f"regtest dashd {version}: signmessagewithprivkey, signmessage, verifymessage",
            "generator": "testdata/oracle/dashd/gen_vectors.py", "sign": sign, "verify": verify,
            "core_tests": CORE_TEST_VECTORS}


# Copied from Dash Core: src/test/util_tests.cpp (message_sign, message_verify)
# and test/functional/rpc_signmessagewithprivkey.py.
CORE_TEST_VECTORS = {
    "sign": [
        {"source": "util_tests.cpp message_sign", "network": "mainnet",
         "secret_hex": "d97f5108f11cda6eeebaaa420fef0726b1f898060b98489fa3098463c0032866", "compressed": True,
         "address": "XetGnWHsPXV9VSkWzB6Wn2KhZLD24gqa5j", "message": "Trust no one",
         "signature": "IIOzMDkvw3GtLWXkeEYRRRH53MOLHM44sJ428Nu4NNacTPJTGcKesMJ+3s3OadYK34tpSQIhu922EviNNWTsiQg="},
        {"source": "rpc_signmessagewithprivkey.py", "network": "testnet",
         "wif": "cU4zhap7nPJAWeMFu4j6jLrfPmqakDAzy8zn8Fhb3oEevdm4e5Lc", "compressed": True,
         "address": "yeMpGzMj3rhtnz48XsfpB8itPHhHtgxLc3", "message": "This is just a test message",
         "signature": "ICzMhjIUmmXcPWy2+9nw01zQMawo+s5FIy6F7VMkL+TmIeNq1j3AMEuw075os29kh5KYLbysKkDlDD+EAqERBd4="},
    ],
    "verify": [
        {"address": "invalid address", "signature": "signature should be irrelevant", "message": "message too",
         "network": "mainnet", "result": "ERR_INVALID_ADDRESS"},
        {"address": "7iRPy8FEHBzbChrktsG85YbDZ3SuiCxsNq", "signature": "signature should be irrelevant",
         "message": "message too", "network": "mainnet", "result": "ERR_ADDRESS_NO_KEY"},
        {"address": "XuXS24zs2xP1vPynvNt14kWLa64csH8Mur", "signature": "invalid signature, not in base64 encoding",
         "message": "message should be irrelevant", "network": "mainnet", "result": "ERR_MALFORMED_SIGNATURE"},
        {"address": "XuXS24zs2xP1vPynvNt14kWLa64csH8Mur",
         "signature": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
         "message": "message should be irrelevant", "network": "mainnet", "result": "ERR_PUBKEY_NOT_RECOVERED"},
        {"address": "XetGnWHsPXV9VSkWzB6Wn2KhZLD24gqa5j",
         "signature": "IPojfrX2dfPnH26UegfbGQQLrdK844DlHq5157/P6h57WyuS/Qsl+h/WSVGDF4MUi4rWSswW38oimDYfNNUBUOk=",
         "message": "I never signed this", "network": "mainnet", "result": "ERR_NOT_SIGNED"},
        {"address": "XetGnWHsPXV9VSkWzB6Wn2KhZLD24gqa5j",
         "signature": "IIOzMDkvw3GtLWXkeEYRRRH53MOLHM44sJ428Nu4NNacTPJTGcKesMJ+3s3OadYK34tpSQIhu922EviNNWTsiQg=",
         "message": "Trust no one", "network": "mainnet", "result": "OK"},
        {"address": "XenV77v8QQ3rwjyCb3j2fCwfvgbkC4Vwaj",
         "signature": "IOACalWiJTLJ2U7wTKICx5mQ2tOAJ3to8dko8FMb2XSYbmvL+yMWedyfSfaK6V8jwoociyYx628nkXXnrOhPFIY=",
         "message": "Trust me", "network": "mainnet", "result": "OK"},
    ],
}


def dump(rpc, wallet, dump_dir, name):
    path = os.path.join(dump_dir, name)
    if os.path.exists(path):
        os.remove(path)
    rpc.call("dumpwallet", path, wallet=wallet)
    dest = os.path.join(TESTDATA, "dumpwallet", name)
    shutil.copyfile(path, dest)
    return dest


def dump_vectors(rpc, dump_dir, version):
    os.makedirs(os.path.join(TESTDATA, "dumpwallet"), exist_ok=True)
    manifest = {"source": f"regtest dashd {version}: dumpwallet of legacy wallets", "generator": "testdata/oracle/dashd/gen_vectors.py", "dumps": []}

    def hd_wallet(name, mnemonic, passphrase, labels, change, imported, multisig):
        rpc.call("createwallet", name, False, True, "", False, False, False)
        rpc.call("upgradetohd", mnemonic, passphrase, wallet=name)
        addrs = []
        for label in labels:
            a = rpc.call("getnewaddress", label, wallet=name)
            addrs.append({"address": a, "label": label, "hdkeypath": rpc.call("getaddressinfo", a, wallet=name).get("hdkeypath")})
        changes = [rpc.call("getrawchangeaddress", wallet=name) for _ in range(change)]
        imports = []
        for secret, compressed, label in imported:
            key = wif(secret, compressed)
            rpc.call("importprivkey", key, label, False, wallet=name)
            imports.append({"wif": key, "label": label})
        scripts = []
        for n in range(multisig):
            ms = rpc.call("addmultisigaddress", 1, [addrs[n]["address"]], wallet=name)
            scripts.append({"address": ms["address"], "redeem_script": ms["redeemScript"]})
        hd = rpc.call("dumphdinfo", wallet=name)
        file = dump(rpc, name, dump_dir, f"{name}.txt")
        manifest["dumps"].append({
            "file": f"dumpwallet/{name}.txt", "network": "regtest", "mnemonic": hd["mnemonic"],
            "mnemonic_passphrase": hd["mnemonicpassphrase"], "hd_seed": hd["hdseed"],
            "labelled_addresses": addrs, "change_addresses": changes, "imported_keys": imports, "scripts": scripts,
        })
        return hd

    hd_wallet("dump_hd_basic",
              "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
              "TREZOR", ["", "savings", "spaces and % and # and é", "tab\there"], 2,
              [(bytes([2] * 32), True, "imported loose key"), (bytes([3] * 32), False, "")], 1)
    hd_wallet("dump_hd_nopass",
              "legal winner thank year wave sausage worth useful legal winner thank yellow", "", ["one"], 1, [], 0)
    hd_wallet("dump_hd_oddpass",
              "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
              "  two leading spaces, # and % and " + chr(0xE9) + " ", ["100%"], 0, [], 0)

    # A blank legacy wallet that never got an HD chain: loose keys only.
    name = "dump_loose_keys"
    rpc.call("createwallet", name, False, True, "", False, False, False)
    imports = []
    for secret, compressed, label in [(bytes([4] * 32), True, "first"), (bytes([5] * 32), True, ""),
                                      (bytes([6] * 32), False, "uncompressed")]:
        key = wif(secret, compressed)
        rpc.call("importprivkey", key, label, False, wallet=name)
        imports.append({"wif": key, "label": label})
    first = rpc.call("getaddressesbylabel", "first", wallet=name)
    ms = rpc.call("addmultisigaddress", 1, list(first.keys()), wallet=name)
    dump(rpc, name, dump_dir, f"{name}.txt")
    manifest["dumps"].append({"file": f"dumpwallet/{name}.txt", "network": "regtest", "imported_keys": imports,
                              "scripts": [{"address": ms["address"], "redeem_script": ms["redeemScript"]}]})
    return manifest


E_ACUTE = chr(0xE9)
COMBINING_ACUTE = chr(0x301)


def bip39_vectors(rpc, version, weak):
    """What dashd derives from mnemonics/passphrases that exercise Core's quirks.

    Each case runs `upgradetohd` on a blank legacy wallet (seed read back with
    dumphdinfo; legacy wallets reject passphrases over 256 bytes) and on a
    blank descriptor wallet (no length limit; the master xprv is read from
    `listdescriptors true`).
    """
    base = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
    tests = [
        ("strict-valid, empty passphrase", base, ""),
        ("strict-valid, TREZOR", base, "TREZOR"),
        ("passphrase 247 bytes (salt 255)", base, "p" * 247),
        ("passphrase 248 bytes (salt 256, the limit)", base, "p" * 248),
        ("passphrase 249 bytes (salt cut to 256)", base, "p" * 249),
        ("passphrase 256 bytes (legacy limit, salt cut)", base, "p" * 256),
        ("passphrase 300 bytes (salt cut to 256)", base, "p" * 300),
        ("passphrase NFC e-acute", base, "caf" + E_ACUTE),
        ("passphrase NFD e + combining acute", base, "cafe" + COMBINING_ACUTE),
        ("passphrase cut inside a multibyte character", base, "p" * 247 + E_ACUTE * 2),
    ] + [("Core-weak checksum accepted by upgradetohd", m, "") for m in weak]
    cases = []
    for i, (label, mnemonic, passphrase) in enumerate(tests):
        entry = {"case": label, "mnemonic": mnemonic, "passphrase": passphrase}
        legacy = f"b39_legacy_{i}"
        rpc.call("createwallet", legacy, False, True, "", False, False, False)
        try:
            rpc.call("upgradetohd", mnemonic, passphrase, wallet=legacy)
            entry["legacy_seed"] = rpc.call("dumphdinfo", wallet=legacy)["hdseed"]
        except RuntimeError as e:
            entry["legacy_error"] = str(e).split("'message': ")[1].strip("}'\"")
        desc = f"b39_desc_{i}"
        rpc.call("createwallet", desc, False, True, "", False, True, False)
        try:
            rpc.call("upgradetohd", mnemonic, passphrase, wallet=desc)
            descs = rpc.call("listdescriptors", True, wallet=desc)["descriptors"]
            # pkh(tprv.../44h/1h/0h/0/*)#checksum -> the master tprv
            entry["descriptor_master_xprv"] = descs[0]["desc"].split("(")[1].split("/")[0]
        except RuntimeError as e:
            entry["descriptor_error"] = str(e).split("'message': ")[1].strip("}'\"")
        cases.append(entry)
    return {"source": f"regtest dashd {version}: upgradetohd on blank legacy and descriptor wallets",
            "generator": "testdata/oracle/dashd/gen_vectors.py", "cases": cases}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", required=True)
    ap.add_argument("--dump-dir", required=True)
    ap.add_argument("--weak", nargs="*", default=[], help="Core-only-valid mnemonics to also feed upgradetohd")
    ap.add_argument("--only", choices=["message", "dump", "bip39"], nargs="*", default=["message", "dump", "bip39"])
    args = ap.parse_args()
    rpc = Rpc(args.rpc)
    version = rpc.call("getnetworkinfo")["subversion"]

    if "message" in args.only:
        write_json(os.path.join(TESTDATA, "message_cases.json"), message_vectors(rpc, version))
    if "dump" in args.only:
        write_json(os.path.join(TESTDATA, "dumpwallet", "manifest.json"), dump_vectors(rpc, args.dump_dir, version))
    if "bip39" in args.only:
        write_json(os.path.join(HERE, "dashd_bip39.json"), bip39_vectors(rpc, version, args.weak))
    print("done:", version)


def write_json(path, doc):
    """Writes `doc` in the testdata layout (see ../jsonfmt.py)."""
    with open(path, "w", encoding="utf-8") as f:
        json.dump(doc, f, ensure_ascii=False)
    subprocess.check_call([sys.executable, os.path.join(HERE, "..", "jsonfmt.py"), path])


if __name__ == "__main__":
    sys.exit(main())

