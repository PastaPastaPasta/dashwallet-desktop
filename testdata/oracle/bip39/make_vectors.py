#!/usr/bin/env python3
"""Builds bip39_oracle (Dash Core's own bip39.cpp) and writes
testdata/bip39_core_quirks.json.

Usage: make_vectors.py --core-src <dash checkout>/src

Sections of the output:
  trezor   Core's src/test/data/bip39_vectors.json, re-run through Core
           (FromData, Check, ToSeed with passphrase "TREZOR").
  check    Core's CMnemonic::Check verdict next to strict BIP39, for
           phrases that probe the XOR-mask checksum bug and the word parser.
  seed     CMnemonic::ToSeed output next to the BIP39 standard seed (NFKD,
           untruncated salt) for passphrases that probe the 256-byte salt
           cut and the missing NFKD step.
  dashd    Seeds a regtest dashd stored after `upgradetohd` (copied from
           testdata/oracle/dashd/dashd_bip39.json when present).
"""
import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unicodedata

HERE = os.path.dirname(os.path.abspath(__file__))
TESTDATA = os.path.abspath(os.path.join(HERE, "..", ".."))
WORDS = open(os.path.join(TESTDATA, "..", "rust", "crates", "dw-compat", "src", "bip39_english.txt")).read().split()
assert len(WORDS) == 2048


def build(core_src, out_dir):
    exe = os.path.join(out_dir, "bip39_oracle")
    srcs = ["wallet/bip39.cpp", "crypto/sha256.cpp", "crypto/sha512.cpp", "crypto/hmac_sha512.cpp",
            "crypto/pkcs5_pbkdf2_hmac_sha512.cpp", "support/lockedpool.cpp", "support/cleanse.cpp"]
    cxx = os.environ.get("CXX", "clang++")
    subprocess.check_call([cxx, "-std=c++20", "-O1", "-DDISABLE_OPTIMIZED_SHA256", "-I", core_src, "-o", exe,
                           os.path.join(HERE, "bip39_oracle.cpp")] + [os.path.join(core_src, s) for s in srcs])
    return exe


class Oracle:
    def __init__(self, exe):
        self.p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE)

    def ask(self, *fields):
        self.p.stdin.write(("\t".join(fields) + "\n").encode())
        self.p.stdin.flush()
        return self.p.stdout.readline().decode().rstrip("\n")

    def check(self, m: bytes) -> bool:
        return self.ask("CHECK", m.hex()) == "1"

    def seed(self, m: bytes, p: bytes) -> str:
        return self.ask("SEED", m.hex(), p.hex())

    def from_data(self, e: bytes) -> str:
        return self.ask("FROMDATA", e.hex())


def strict_valid(phrase: str) -> bool:
    words = phrase.split(" ")
    if len(words) not in (12, 15, 18, 21, 24) or any(w not in WORDS for w in words):
        return False
    bits = "".join(format(WORDS.index(w), "011b") for w in words)
    cs = len(words) // 3
    ent = int(bits[:-cs], 2).to_bytes(len(words) * 4 // 3, "big")
    return bits[-cs:] == format(hashlib.sha256(ent).digest()[0], "08b")[:cs]


def standard_seed(phrase: str, passphrase: str) -> str:
    m = unicodedata.normalize("NFKD", phrase).encode()
    s = ("mnemonic" + unicodedata.normalize("NFKD", passphrase)).encode()
    return hashlib.pbkdf2_hmac("sha512", m, s, 2048).hex()


def with_checksum(entropy: bytes, cs_value: int) -> str:
    """The phrase for `entropy` with its checksum bits replaced by `cs_value`."""
    cs = len(entropy) * 8 // 32
    bits = format(int.from_bytes(entropy, "big"), f"0{len(entropy) * 8}b") + format(cs_value, f"0{cs}b")
    return " ".join(WORDS[int(bits[i:i + 11], 2)] for i in range(0, len(bits), 11))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--core-src", required=True)
    args = ap.parse_args()
    with tempfile.TemporaryDirectory() as tmp:
        o = Oracle(build(args.core_src, tmp))

        trezor = []
        for e_hex, phrase, seed, xprv in json.load(open(os.path.join(TESTDATA, "core", "bip39_vectors.json"))):
            e = bytes.fromhex(e_hex)
            assert o.from_data(e) == phrase
            assert o.check(phrase.encode())
            assert o.seed(phrase.encode(), b"TREZOR") == seed
            trezor.append({"entropy": e_hex, "mnemonic": phrase, "passphrase": "TREZOR", "seed": seed, "xprv": xprv})

        check = []

        def add_check(case, phrase, entropy=None):
            entry = {"case": case, "mnemonic": phrase, "core_check": o.check(phrase.encode()),
                     "strict_check": strict_valid(phrase)}
            if entropy is not None:
                entry["entropy"] = entropy.hex()
            check.append(entry)

        # Every checksum value for one entropy per length: Core's mask checks
        # only some bits, so several wrong checksums pass.
        for n_bytes in (16, 20, 24, 28, 32):
            entropy = hashlib.sha256(b"dw-compat bip39 %d" % n_bytes).digest()[:n_bytes]
            cs = n_bytes * 8 // 32
            for value in range(1 << cs):
                add_check(f"{n_bytes * 3 // 4} words, checksum {value:0{cs}b}", with_checksum(entropy, value), entropy)
        base = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        for case, phrase in [
            ("valid", base),
            ("uppercase word", base.replace("about", "About")),
            ("double space", base.replace(" about", "  about")),
            ("leading space", " " + base),
            ("trailing space", base + " "),
            ("tab separator", base.replace(" about", "\tabout")),
            ("11 words", " ".join(base.split()[:11])),
            ("13 words", base + " abandon"),
            ("24 words all abandon+art", " ".join(["abandon"] * 23 + ["art"])),
            ("25 words", " ".join(["abandon"] * 25)),
            ("9 words", " ".join(["abandon"] * 9)),
            ("27 words", " ".join(["abandon"] * 27)),
            ("unknown word", base.replace("about", "aboutt")),
            ("word longer than 8 letters", base.replace("about", "abandonzz")),
            ("non-ASCII word", base.replace("about", "ábout")),
            ("empty", ""),
            ("Core platformkeys_tests bad mnemonic",
             "birth kingdom trash renew flavor utility donkey gasp regular alert pave kingdom"),
        ]:
            add_check(case, phrase)

        seed = []
        for case, phrase, passphrase in [
            ("empty passphrase", base, ""),
            ("TREZOR", base, "TREZOR"),
            ("passphrase 247 bytes (salt 255)", base, "p" * 247),
            ("passphrase 248 bytes (salt 256, the limit)", base, "p" * 248),
            ("passphrase 249 bytes (salt cut to 256)", base, "p" * 249),
            ("passphrase 300 bytes (salt cut to 256)", base, "p" * 300),
            ("passphrase NFC e-acute", base, "café"),
            ("passphrase NFD e + combining acute", base, "café"),
            ("passphrase fullwidth (NFKD would fold)", base, "ＡＢ"),
            ("passphrase cut inside a multibyte character", base, "p" * 247 + "éé"),
            ("passphrase with NUL", base, "a\u0000b"),
            ("mnemonic is hashed as given (double space)", base.replace(" about", "  about"), ""),
        ]:
            core = o.seed(phrase.encode(), passphrase.encode())
            seed.append({"case": case, "mnemonic": phrase, "passphrase": passphrase, "core_seed": core,
                         "bip39_seed": standard_seed(phrase, passphrase)})

    out = {
        "source": "Dash Core develop src/wallet/bip39.cpp (CMnemonic), compiled and run by testdata/oracle/bip39",
        "notes": [
            "core_check is CMnemonic::Check; strict_check is BIP39 with the full checksum.",
            "core_seed is CMnemonic::ToSeed: PBKDF2-HMAC-SHA512(mnemonic bytes, ('mnemonic'+passphrase)[..256], 2048), no NFKD.",
            "bip39_seed is the BIP39 standard seed (NFKD, untruncated salt) for comparison.",
        ],
        "trezor": trezor,
        "check": check,
        "seed": seed,
    }
    dashd = os.path.join(TESTDATA, "oracle", "dashd", "dashd_bip39.json")
    if os.path.exists(dashd):
        out["dashd"] = json.load(open(dashd))
    path = os.path.join(TESTDATA, "bip39_core_quirks.json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False)
    subprocess.check_call([sys.executable, os.path.join(HERE, "..", "jsonfmt.py"), path])
    weak = [c["mnemonic"] for c in check if c["core_check"] and not c["strict_check"]]
    print(f"{len(trezor)} trezor, {len(check)} check ({len(weak)} weak-accepted), {len(seed)} seed")
    with open(os.path.join(HERE, "weak_sample.txt"), "w") as f:
        f.write("\n".join(weak[:3] + weak[-3:]) + "\n")


if __name__ == "__main__":
    main()
