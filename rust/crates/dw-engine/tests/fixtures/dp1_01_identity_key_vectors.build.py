"""Compare the independent Python vectors with the iOS FFI harness output and
write the DP1-01 fixture. Fails loudly on any difference.

argv: vectors.json ffi-vectors.txt e0_03_signer_vectors.json out.json
"""

import json
import re
import sys

PURPOSE = {0: "AUTHENTICATION", 1: "ENCRYPTION", 2: "DECRYPTION", 3: "TRANSFER"}
LEVEL = {0: "MASTER", 1: "CRITICAL", 2: "HIGH", 3: "MEDIUM"}
TYPE = {0: "ECDSA_SECP256K1"}

py = json.load(open(sys.argv[1]))
e003 = json.load(open(sys.argv[3]))

ffi = {}
line_re = re.compile(
    r"^(base|slot) (\S+) (\S+) i=(\d+) id=(\d+)(?: type=(\d+) purpose=(\d+) sec=(\d+))? "
    r"path=(\S+) pub=([0-9a-f]{66})$")
for line in open(sys.argv[2]):
    m = line_re.match(line.strip())
    assert m, line
    kind, label, net, i, kid, kt, pur, sec, path, pub = m.groups()
    row = {"path": path, "public_key": pub, "fn": kind}
    if kind == "base":
        row.update(key_type=TYPE[int(kt)], purpose=PURPOSE[int(pur)],
                   security_level=LEVEL[int(sec)])
    ffi[(label, net, int(i), int(kid))] = row

e003_keys = {}
for c in e003["cases"]:
    for k in c["identity_keys"]:
        e003_keys[(c["seed"], k["path"])] = k["public_key"]

BASE_FN = ("rs-platform-wallet-ffi dash_sdk_derive_and_persist_identity_keys "
           "(Swift prePersistIdentityKeysForRegistration, keyCount 4)")
SLOT_FN = ("rs-platform-wallet-ffi dash_sdk_derive_identity_key_at_slot_with_resolver "
           "(Swift deriveIdentityAuthKeyAtSlot)")
SLOT_PASS_FN = ("rs-platform-wallet-ffi dash_sdk_derive_identity_key_at_slot "
                "(mnemonic+passphrase variant of the same derive_at_slot_inner)")

checked = 0
for s in py["sets"]:
    for k in s["keys"]:
        f = ffi[(s["mnemonic"], s["network"], s["identity_index"], k["id"])]
        assert f["path"] == k["path"], (s, k, f)
        assert f["public_key"] == k["public_key"], (s, k, f)
        sources = ["python-independent"]
        if f["fn"] == "base":
            for field in ("key_type", "purpose", "security_level"):
                assert f[field] == k[field], (s, k, f)
            sources.append(BASE_FN + ": public key, path, key type, purpose, security level")
        else:
            fn = SLOT_PASS_FN if "+" in s["mnemonic"] else SLOT_FN
            sources.append(fn + ": public key, path")
            sources.append("purpose, security level, key type, bounds: "
                           "DWDashPayIdentityKeys.swift (source reading)")
        e = e003_keys.get((s["seed"], k["path"]))
        if e is not None:
            assert e == k["public_key"]
            sources.append("e0_03_signer_vectors.json: public key")
        k["sources"] = sources
        checked += 1

extra = []
for x in py["abandon_testnet_i0_extra"]:
    f = ffi[("abandon", "testnet", 0, x["id"])]
    assert f["path"] == x["path"] and f["public_key"] == x["public_key"], (x, f)
    extra.append(dict(x, sources=["python-independent", SLOT_FN + ": public key, path"]))
    checked += 1

MNEMONIC_TEXT = {
    "abandon": "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    "legal": "legal winner thank year wave sausage worth useful legal winner thank yellow",
}
sets = []
for s in py["sets"]:
    label = s["mnemonic"]
    base, _, passphrase = label.partition("+")
    sets.append({
        "mnemonic": MNEMONIC_TEXT[base],
        "passphrase": passphrase,
        "seed": s["seed"],
        "network": s["network"],
        "identity_index": s["identity_index"],
        "keys": s["keys"],
    })

out = {
    "source": {
        "task": "dashwallet-desktop DP1-01",
        "ios": "dashpay/dashwallet-ios 37c0e78a2f557fa35c98c051a4ce40b77be350ee "
               "(DWDashPayIdentityKeys.swift last changed in 6836db798b)",
        "platform": "dashpay/platform bc41f1bc233dec4607d387101c1d9c2f111019b2",
        "how": "See fixtures/README.md. Every public key and path agrees between "
               "an independent from-scratch Python derivation and the "
               "rs-platform-wallet-ffi entry points SwiftDashSDK calls; keys 0-3 "
               "also take key type, purpose and security level from that FFI. "
               "The DashPay pair's metadata comes from reading the Swift source.",
        "mnemonics": "BIP39 English test vectors; throwaway.",
    },
    "dashpay_contract_id": bytes([
        162, 161, 180, 172, 111, 239, 34, 234, 42, 26, 104, 232, 18, 54, 68, 179,
        87, 135, 95, 107, 65, 44, 24, 16, 146, 129, 193, 70, 231, 178, 113, 188,
    ]).hex(),
    "dashpay_contract_id_base58": "Bwr4WHCPz5rFVAD87RqTs3izo4zpzwsEdKPWUT1NS1C7",
    "sets": sets,
    "upgrade_slots": {
        "mnemonic": MNEMONIC_TEXT["abandon"],
        "network": "testnet",
        "identity_index": 0,
        "keys": extra,
    },
}
json.dump(out, open(sys.argv[4], "w"), indent=1)
open(sys.argv[4], "a").write("\n")
print("checked", checked, "keys; wrote", sys.argv[4])
