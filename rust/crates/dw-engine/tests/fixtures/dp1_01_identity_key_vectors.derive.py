"""DP1-01 independent re-derivation of the iOS DashPay identity key set.

From scratch, stdlib only: BIP-39 seed (PBKDF2-HMAC-SHA512), BIP-32 master
and hardened CKDpriv, secp256k1 scalar multiplication, DIP-13 paths. The key
policy table is transcribed from the Swift sources:

- ids 0-3: rs-platform-wallet-ffi identity_derive_and_persist.rs (what
  Swift `prePersistIdentityKeysForRegistration(keyCount: 4)` returns);
- ids 4-5: dashwallet-ios DWDashPayIdentityKeys.registrationSpecifications
  (firstKeyId: 4) -> ENCRYPTION, DECRYPTION; ECDSA_SECP256K1; MEDIUM;
  SingleContractDocumentType(DashPay, "contactRequest").

Prints public keys and paths only, never private keys.
"""

import hashlib
import hmac
import json
import sys
import unicodedata

P = 2**256 - 2**32 - 977
N = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141
G = (
    0x79BE667EF9DCBBAC55A06295CE870B07029BFCDB2DCE28D959F2815B16F81798,
    0x483ADA7726A3C4655DA4FBFC0E1108A8FD17B448A68554199C47D08FFB10D4B8,
)


def point_add(a, b):
    if a is None:
        return b
    if b is None:
        return a
    if a[0] == b[0] and (a[1] + b[1]) % P == 0:
        return None
    if a == b:
        lam = 3 * a[0] * a[0] * pow(2 * a[1], -1, P) % P
    else:
        lam = (b[1] - a[1]) * pow(b[0] - a[0], -1, P) % P
    x = (lam * lam - a[0] - b[0]) % P
    return (x, (lam * (a[0] - x) - a[1]) % P)


def point_mul(k):
    r, q = None, G
    while k:
        if k & 1:
            r = point_add(r, q)
        q = point_add(q, q)
        k >>= 1
    return r


def ser_p(pt):
    return bytes([2 + (pt[1] & 1)]) + pt[0].to_bytes(32, "big")


def seed_from_mnemonic(mnemonic, passphrase=""):
    m = unicodedata.normalize("NFKD", mnemonic).encode()
    s = unicodedata.normalize("NFKD", "mnemonic" + passphrase).encode()
    return hashlib.pbkdf2_hmac("sha512", m, s, 2048)


def master(seed):
    i = hmac.new(b"Bitcoin seed", seed, hashlib.sha512).digest()
    return int.from_bytes(i[:32], "big"), i[32:]


def ckd_hardened(k, c, index):
    data = b"\x00" + k.to_bytes(32, "big") + (index | 0x80000000).to_bytes(4, "big")
    i = hmac.new(c, data, hashlib.sha512).digest()
    il = int.from_bytes(i[:32], "big")
    assert il < N
    child = (il + k) % N
    assert child != 0
    return child, i[32:]


def coin_type(network):
    return 5 if network == "mainnet" else 1


def dip13_path(network, identity_index, key_id):
    # m / 9' / coin' / 5' (identities) / 0' (authentication) / 0' (ECDSA)
    #   / identity' / key'
    return [9, coin_type(network), 5, 0, 0, identity_index, key_id]


def derive_pubkey(seed, path):
    k, c = master(seed)
    for idx in path:
        k, c = ckd_hardened(k, c, idx)
    return ser_p(point_mul(k)).hex()


DASHPAY_ID = bytes([
    162, 161, 180, 172, 111, 239, 34, 234,
    42, 26, 104, 232, 18, 54, 68, 179,
    87, 135, 95, 107, 65, 44, 24, 16,
    146, 129, 193, 70, 231, 178, 113, 188,
])
BOUND = {"contract": DASHPAY_ID.hex(), "document_type": "contactRequest"}

# (id, purpose, security_level, key_type, bounds)
POLICY = [
    (0, "AUTHENTICATION", "MASTER", "ECDSA_SECP256K1", None),
    (1, "AUTHENTICATION", "CRITICAL", "ECDSA_SECP256K1", None),
    (2, "AUTHENTICATION", "HIGH", "ECDSA_SECP256K1", None),
    (3, "TRANSFER", "CRITICAL", "ECDSA_SECP256K1", None),
    (4, "ENCRYPTION", "MEDIUM", "ECDSA_SECP256K1", BOUND),
    (5, "DECRYPTION", "MEDIUM", "ECDSA_SECP256K1", BOUND),
]

MNEMONICS = {
    "abandon": "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    "legal": "legal winner thank year wave sausage worth useful legal winner thank yellow",
}
# (label, mnemonic, passphrase): iOS at the pin always derives with "".
SEEDS = [("abandon", MNEMONICS["abandon"], ""), ("legal", MNEMONICS["legal"], ""),
         ("legal+TREZOR", MNEMONICS["legal"], "TREZOR")]


def path_str(path):
    return "m/" + "/".join(f"{i}'" for i in path)


def main():
    out = []
    for label, mnemonic, passphrase in SEEDS:
        seed = seed_from_mnemonic(mnemonic, passphrase)
        networks = ("testnet",) if passphrase else ("mainnet", "testnet", "regtest")
        for network in networks:
            for identity_index in (0, 1):
                keys = []
                for key_id, purpose, level, key_type, bounds in POLICY:
                    path = dip13_path(network, identity_index, key_id)
                    keys.append({
                        "id": key_id,
                        "purpose": purpose,
                        "security_level": level,
                        "key_type": key_type,
                        "contract_bounds": bounds,
                        "path": path_str(path),
                        "public_key": derive_pubkey(seed, path),
                    })
                out.append({
                    "mnemonic": label,
                    "seed": seed.hex(),
                    "network": network,
                    "identity_index": identity_index,
                    "keys": keys,
                })
    # The upgrader's next slots past the registration set.
    seed = seed_from_mnemonic(MNEMONICS["abandon"])
    extra = []
    for key_id in (6, 7):
        path = dip13_path("testnet", 0, key_id)
        extra.append({"id": key_id, "path": path_str(path),
                      "public_key": derive_pubkey(seed, path)})
    json.dump({"sets": out, "abandon_testnet_i0_extra": extra}, sys.stdout,
              indent=1)


if __name__ == "__main__":
    main()
