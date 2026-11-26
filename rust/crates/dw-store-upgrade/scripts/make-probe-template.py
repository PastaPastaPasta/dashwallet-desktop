#!/usr/bin/env python3
"""Write data/probe-fork-v1.sqlite, the fork-shaped file the P9a probe seeds.

A copy of the released v0.1.0 fixture's regtest wallet.sqlite (fork V1, refinery
V018), with the Orchard viewing key and the second wallet removed, written with
VACUUM INTO. The fixture itself is opened read-only and never changed.
Usage: make-probe-template.py <repo root>
"""
import hashlib, os, shutil, sqlite3, sys, tempfile

root = sys.argv[1]
src = os.path.join(root, "testdata/upgrade/v0.1.0/data/regtest/wallet.sqlite")
out = os.path.join(root, "rust/crates/dw-store-upgrade/data/probe-fork-v1.sqlite")
keep = bytes.fromhex("0ff35f248c69653efce8336ce14b78e29cd3bbaf4162c42246259850af2c2add")
with tempfile.TemporaryDirectory() as tmp:
    work = os.path.join(tmp, "wallet.sqlite")
    shutil.copyfile(src, work)
    conn = sqlite3.connect(work)
    conn.execute("PRAGMA foreign_keys = ON")
    conn.execute("DELETE FROM shielded_viewing_keys")
    conn.execute("DELETE FROM wallets WHERE wallet_id != ?", (keep,))
    conn.commit()
    left = conn.execute("SELECT count(*) FROM wallets").fetchone()[0]
    assert left == 1, left
    if os.path.exists(out):
        os.remove(out)
    conn.execute("VACUUM INTO ?", (out,))
    conn.close()
digest = hashlib.sha256(open(out, "rb").read()).hexdigest()
print(digest, os.path.getsize(out))
