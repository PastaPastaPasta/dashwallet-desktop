#!/usr/bin/env python3
# Copyright (c) 2026 The Dash Wallet Desktop developers
# Distributed under the MIT software license, see test_framework/COPYING.
"""dwd_masternodes.py — regtest suite `masternodes` (MG-03 T1).

dw-engine's masternodes facade (`dwcli mn`) against Core's deterministic masternode list. The
wallet's keys are derived here from the mnemonic (BIP39 + DIP9 `m/9'/1'/3'/{1,2}'/i`), so the
roles are checked against an oracle that isn't the engine's own derivation.

Network: node0 controller/miner and the SPV peer of dwcli (BIP157 filters); nodes 1..2 the
framework's static masternodes. Real time (no mocktime), so the 30-day payout window holds the
payouts; InstantSend and ChainLocks are off (no quorums). DKG stays off
(SPORK_17), so masternodes registered without a running node are never PoSe-punished. Core
still stores a quorum snapshot per DIP24 cycle without DKG, and answers dash-spv's QRInfo once
enough cycles have one, so the test first mines whole cycles until `quorum rotationinfo` works.

Masternodes registered by the test:
  A  external collateral funded to the wallet (`protx register_prepare`, the collateral signed
     by `dwcli sign`, `register_submit`); owner and voting keys are the wallet's keys 0, the
     payout a wallet address, the operator key Core's. All four wallet roles.
  B  Core's collateral inside the ProRegTx (`register_fund`); owner key the wallet's key 1,
     everything else Core's. The owner-key match only, and the internal collateral resolved.
  C  Core's collateral; owner and voting keys generated here, the payout a wallet address. The
     payout match; then tracked, its voting and owner keys attached (and a wrong key refused).
  D  nothing of the wallet's; Core holds its owner key. Tracked with its voting key attached,
     then `protx update_registrar` rotates the voting key.

Checks, in order:
 1. the funded 1,000 DASH coin is a collateral-shaped coin before A is registered;
 2. A, B and C (D is registered too) are listed `enabled` with the expected roles, collateral and registration facts;
 3. tracking: `find` by service address, `track`, `already_tracked`, attach and check keys
    (`matched`, `key_mismatch`, `unchecked`), labels, `untrack` deleting the keys (none attached
    once tracked again); every `mn` subcommand given a secret-shaped 64 hex repeats it nowhere;
    after D's voting key is rotated, its attached key reads `key_mismatch` and `voting: false`;
 4. payouts: the wallet's payouts of A and C equal Core's coinbase payments to their payout
    addresses, newest first, and mature after 100 blocks;
 5. status transitions of A: `protx revoke` (Core bans a revoked masternode) makes it
    `pose_banned`; the wallet spending the collateral makes it `removed`.

Run (DWCLI = a built dwcli):
  DWCLI=<target>/debug/dwcli DWD_PYTHON=<venv>/bin/python DASHCORE_DIR=<dashcore> \\
      regtest/functional/run.sh dwd_masternodes.py
"""

import hashlib
import hmac
import json
import os
import shutil
import subprocess
import threading
import time

from test_framework.address import key_to_p2pkh
from test_framework.key import ECKey, ORDER
from test_framework.script import hash160
from test_framework.test_framework import DashTestFramework
from test_framework.util import assert_equal, assert_greater_than, assert_greater_than_or_equal, p2p_port
from test_framework.wallet_util import bytes_to_wif

SPORK_OFF = 4070908800  # 2099-01-01: a spork time that never comes
CONTROLLER_ARGS = ["-peerblockfilters=1", "-blockfilterindex=1"]
PASSPHRASE = "dwd masternodes regtest"
PHRASE = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
COIN = 100_000_000
COLLATERAL = 1000 * COIN
HARDENED = 0x80000000
# A private key's shape (MG-03 D23): no dwcli output may repeat it, in either byte order.
SECRET = "e9873d79c6d87dc0fb6a5778633389f4453213303da61f20bd67fc233aa33262"
VOTING, OWNER = 1, 2  # DIP3 sub-accounts under m/9'/coin'/3'


def provider_key(sub_account, index):
    """The wallet's DIP3 key `m/9'/1'/3'/<sub>'/<index>` (BIP32 over the BIP39 seed)."""
    seed = hashlib.pbkdf2_hmac("sha512", PHRASE.encode(), b"mnemonic", 2048)
    i = hmac.new(b"Bitcoin seed", seed, hashlib.sha512).digest()
    k, chain = int.from_bytes(i[:32], "big"), i[32:]
    for n in [9 | HARDENED, 1 | HARDENED, 3 | HARDENED, sub_account | HARDENED, index]:
        if n & HARDENED:
            data = b"\0" + k.to_bytes(32, "big")
        else:
            key = ECKey()
            key.set(k.to_bytes(32, "big"), True)
            data = key.get_pubkey().get_bytes()
        i = hmac.new(chain, data + n.to_bytes(4, "big"), hashlib.sha512).digest()
        k, chain = (int.from_bytes(i[:32], "big") + k) % ORDER, i[32:]
    key = ECKey()
    key.set(k.to_bytes(32, "big"), True)
    return key


def address(key):
    return key_to_p2pkh(key.get_pubkey().get_bytes())


def wif(key):
    return bytes_to_wif(key.get_bytes())


class Dwcli:
    def __init__(self, binary, datadir, peer_port, log):
        self.binary = binary
        self.datadir = datadir
        self.peer_port = peer_port
        self.log = log
        self.passfile = os.path.join(datadir, "pass")
        os.makedirs(datadir, mode=0o700, exist_ok=True)
        with open(self.passfile, "w", encoding="utf8") as f:
            f.write(PASSPHRASE + "\n")

    def run(self, *args, stdin=None, check=True, timeout=600):
        cmd = [
            self.binary,
            "--datadir", os.path.join(self.datadir, "data"),
            "--network", "regtest",
            "--dapi", "http://127.0.0.1:1",
            "--quorum-url", "http://127.0.0.1:1",
            "--no-platform",
            "--peer", f"127.0.0.1:{self.peer_port}",
            "--passphrase-file", self.passfile,
        ] + list(args)
        env = dict(os.environ)
        env.setdefault("DWCLI_LOG", "warn,dw_engine::masternodes=debug")
        p = subprocess.run(cmd, input=stdin, capture_output=True, text=True, timeout=timeout, env=env)
        shown = " ".join("<key>" if a == stdin else a for a in args)
        self.log.info(f"dwcli {shown} -> {p.returncode}\n{p.stdout}{p.stderr[-4000:]}")
        if check and p.returncode != 0:
            raise AssertionError(f"dwcli {shown} failed: {p.stderr[-2000:]}")
        return p

    def mn(self, height, *args, stdin=None, error=None):
        """`dwcli mn <args>` once the list is synced and the wallet scanned `height`: the result,
        or, with `error`, checks the refusal's code."""
        p = self.run("mn", "--sync-height", str(height), "--sync-timeout", "600", *args, stdin=stdin,
                     check=False)
        line = json.loads(p.stdout.strip().splitlines()[-1])
        if error is not None:
            assert not line["ok"], line
            assert_equal(line["error"]["code"], error)
            return line["error"]
        assert line["ok"] and p.returncode == 0, (line, p.stderr[-2000:])
        return line["result"]


class DwdMasternodes(DashTestFramework):
    def add_options(self, parser):
        self.add_wallet_options(parser)

    def set_test_params(self):
        self.set_dash_test_params(3, 2, [CONTROLLER_ARGS, [], []])
        self.disable_mocktime = True

    def skip_test_if_missing_module(self):
        self.skip_if_no_wallet()
        if not os.environ.get("DWCLI"):
            raise SystemExit("set DWCLI to the dwcli binary")

    # --- helpers -----------------------------------------------------------------------

    def tip(self):
        return self.nodes[0].getblockcount()

    def mine(self, n=1):
        self.generate(self.nodes[0], n)
        return self.tip()

    def start_miner(self):
        """A block every 5 s while dwcli broadcasts: dash-spv takes a broadcast as sent once it
        sees it in a block."""
        self.mining = True

        def loop():
            while self.mining:
                time.sleep(5)
                try:
                    self.generate(self.nodes[0], 1, sync_fun=self.no_op)
                except Exception as e:  # the test is ending
                    self.log.info(f"miner: {e}")
        self.miner = threading.Thread(target=loop, daemon=True)
        self.miner.start()

    def stop_miner(self):
        self.mining = False
        self.miner.join()
        self.sync_blocks()

    def mine_until_qrinfo(self, max_cycles=12):
        """Mines whole DIP24 cycles until Core answers QRInfo at the tip. Core stores a cycle's
        quorum snapshot only once it has enabled masternodes and the snapshots of the earlier
        cycles, and refuses `getqrinfo` (dash-spv's masternode-list sync) without them."""
        node = self.nodes[0]
        for _ in range(max_cycles):
            try:
                node.quorum("rotationinfo", node.getbestblockhash(), True)
                self.log.info(f"QRInfo answered at height {self.tip()}")
                return
            except Exception as e:
                if "Cannot find quorum snapshot" not in str(e):
                    raise
            self.mine(24 - self.tip() % 24 + 1)
        raise AssertionError(f"no QRInfo after {max_cycles} cycles")

    def coinbase_payments(self, addr, since_height):
        """Core's coinbase outputs to `addr` from `since_height` on: {txid: duffs}."""
        node = self.nodes[0]
        out = {}
        for h in range(since_height, self.tip() + 1):
            cb = node.getblock(node.getblockhash(h), 2)["tx"][0]
            for o in cb["vout"]:
                if o["scriptPubKey"].get("address") == addr:
                    out[cb["txid"]] = out.get(cb["txid"], 0) + o["valueSat"]
        return out

    def outpoint_of(self, txid, addr, amount):
        for o in self.nodes[0].getrawtransaction(txid, True)["vout"]:
            if o["valueSat"] == amount and o["scriptPubKey"].get("address") == addr:
                return o["n"]
        raise AssertionError(f"no {amount} output to {addr} in {txid}")

    def row(self, overview, pro_tx_hash):
        rows = [r for r in overview["masternodes"] if r["pro_tx_hash"] == pro_tx_hash]
        assert_equal(len(rows), 1)
        return rows[0]

    # --- the test ----------------------------------------------------------------------

    def run_test(self):
        node = self.nodes[0]
        dw = Dwcli(os.environ["DWCLI"], os.path.join(self.options.tmpdir, "dwcli"), p2p_port(0), self.log)
        evidence = os.environ.get("DWD_MG03_EVIDENCE")
        time.sleep(1.5)  # peers keep a spork signed in the same second as the one they have
        for spork in ("SPORK_2_INSTANTSEND_ENABLED", "SPORK_19_CHAINLOCKS_ENABLED"):
            node.sporkupdate(spork, SPORK_OFF)
        self.wait_for_sporks_same()
        self.mine_until_qrinfo()

        dw.run("init-vault")
        wallet = dw.run("import", "--birth-height", "0", stdin=PHRASE).stdout.split()[-1]
        owner0, voting0, owner1 = provider_key(OWNER, 0), provider_key(VOTING, 0), provider_key(OWNER, 1)
        payout_a = dw.run("receive", "--next").stdout.split()[1]
        payout_c = dw.run("receive", "--next").stdout.split()[1]
        collateral_addr = dw.run("receive", "--next").stdout.split()[1]
        fee_addr = node.getnewaddress()
        node.sendtoaddress(fee_addr, 10)
        # register_fund takes the collateral and fee from one address (else the payout address)
        fund_b, fund_c, fund_d = node.getnewaddress(), node.getnewaddress(), node.getnewaddress()
        for fund in (fund_b, fund_c, fund_d):
            node.sendtoaddress(fund, COLLATERAL // COIN + 1)

        self.log.info("1. A funded coin of exactly 1,000 DASH is collateral-shaped")
        funding = node.sendtoaddress(collateral_addr, COLLATERAL // COIN)
        h = self.mine()
        vout = self.outpoint_of(funding, collateral_addr, COLLATERAL)
        overview = dw.mn(h, "list")
        assert_equal(overview["list"]["synced"], True)
        assert_equal(overview["masternodes"], [])
        coins = overview["collateral_coins"]
        assert_equal([(c["outpoint"], c["kind"], c["amount"]) for c in coins],
                     [(f"{funding}-{vout}", "regular", COLLATERAL)])

        self.log.info("2. Register A, B and C")
        bls = [node.bls("generate") for _ in range(4)]
        prepared = node.protx("register_prepare", funding, vout, [f"127.0.0.1:{p2p_port(10)}"], address(owner0),
                              bls[0]["public"], address(voting0), 0, payout_a, fee_addr)
        assert_equal(prepared["collateralAddress"], collateral_addr)
        signature = dw.run("sign", wallet, collateral_addr, prepared["signMessage"]).stdout.split()[-1]
        mn_a = node.protx("register_submit", prepared["tx"], signature)
        mn_b = node.protx("register_fund", node.getnewaddress(), [f"127.0.0.1:{p2p_port(11)}"], address(owner1),
                          bls[1]["public"], node.getnewaddress(), 0, node.getnewaddress(), fund_b)
        owner_c, voting_c = ECKey(), ECKey()
        owner_c.generate()
        voting_c.generate()
        mn_c = node.protx("register_fund", node.getnewaddress(), [f"127.0.0.1:{p2p_port(12)}"], address(owner_c),
                          bls[2]["public"], address(voting_c), 0, payout_c, fund_c)
        # D: nothing of the wallet's; Core holds its owner key, so it can rotate D's voting key (step 3)
        voting_d = ECKey()
        voting_d.generate()
        mn_d = node.protx("register_fund", node.getnewaddress(), [f"127.0.0.1:{p2p_port(13)}"], node.getnewaddress(),
                          bls[3]["public"], address(voting_d), 0, node.getnewaddress(), fund_d)
        registered = self.mine()
        listed = node.protx("list", "registered", True)
        assert {mn_a, mn_b, mn_c, mn_d} <= {m["proTxHash"] for m in listed}

        overview = dw.mn(registered, "list")
        assert_equal(overview["collateral_coins"], [])
        a, b, c = (self.row(overview, h) for h in (mn_a, mn_b, mn_c))
        assert_equal([r["status"] for r in (a, b, c)], ["enabled"] * 3)
        assert all(r["mine"] and not r["tracked"] and r["kind"] == "regular" for r in (a, b, c))
        roles = lambda r: {k for k in ("collateral", "owner", "voting", "payout", "operator") if r["roles"][k]}
        assert_equal(roles(a), {"collateral", "owner", "voting", "payout"})
        assert_equal(roles(b), {"owner"})
        assert_equal(roles(c), {"payout"})
        assert_equal(a["collateral"], {"kind": "known", "outpoint": f"{funding}-{vout}"})
        b_vout = node.protx("info", mn_b)["collateralIndex"]
        assert_equal(b["collateral"], {"kind": "known", "outpoint": f"{mn_b}-{b_vout}"})
        assert_equal((a["payout_address"], c["payout_address"]), (payout_a, payout_c))
        assert_equal(sorted(r["type_index"] for r in (a, b, c)), [1, 2, 3])

        detail = dw.mn(registered, "status", mn_a)
        reg = detail["registration"]
        assert reg is not None, detail
        assert_equal(reg["registered_height"], registered)
        assert_equal((reg["owner_address"], reg["voting_address"], reg["payout_address"]),
                     (address(owner0), address(voting0), payout_a))
        assert_equal((reg["operator_reward"], reg["revoked"]), (0, False))
        core = node.protx("info", mn_a)["state"]
        assert_equal(detail["list"]["voting_address"], core["votingAddress"])
        assert_equal(detail["list"]["operator_public_key"], core["pubKeyOperator"])
        assert_equal(detail["list"]["service"], f"127.0.0.1:{p2p_port(10)}")
        assert_equal(detail["payout_checked_height"], None)

        self.log.info("3. Tracking and attached keys")
        # every subcommand, on the synced list, with a secret-shaped argument (D23)
        back = bytes.fromhex(SECRET)[::-1].hex()
        for args, stdin in [
            (["list", "--wallet", SECRET], None),
            (["status", SECRET], None),
            (["find", SECRET], None),
            (["track", SECRET], None),
            (["untrack", SECRET], None),
            (["label", SECRET, "--label", "x"], None),
            (["attach", SECRET, "--role", "voting"], SECRET),
            (["detach", SECRET, "--role", "voting"], None),
            (["payouts", SECRET], None),
            (["credits", SECRET], None),
        ]:
            p = dw.run("mn", "--sync-height", str(registered), "--sync-timeout", "600", *args, stdin=stdin,
                       check=False)
            line = json.loads(p.stdout.strip().splitlines()[-1])
            # a refusal, or (find) no match
            assert line["ok"] == (p.returncode == 0) and line.get("result", []) == [], line
            out = (p.stdout + p.stderr).lower()
            assert SECRET not in out and back not in out, (args, out[-2000:])

        static = self.mninfo[0]
        found = dw.mn(registered, "find", f"127.0.0.1:{static.nodePort}")
        assert_equal([f["pro_tx_hash"] for f in found], [static.proTxHash])
        assert_greater_than_or_equal(len(dw.mn(registered, "find", "127.0.0.1")), 5)
        dw.mn(registered, "track", static.proTxHash, "--label", " Static ")
        dw.mn(registered, "track", static.proTxHash, error="masternodes.already_tracked")
        dw.mn(registered, "track", "ab" * 32, error="masternodes.not_found")
        dw.mn(registered, "attach", mn_c, "--role", "voting", stdin=wif(voting_c), error="masternodes.not_tracked")
        dw.mn(registered, "track", mn_c)
        dw.mn(registered, "attach", mn_c, "--role", "voting", stdin=wif(owner_c), error="masternodes.key_mismatch")
        assert_equal(dw.mn(registered, "attach", mn_c, "--role", "voting", stdin=wif(voting_c))["check"], "matched")
        assert_equal(dw.mn(registered, "attach", mn_c, "--role", "owner", stdin=wif(owner_c))["check"], "matched")
        # a key that doesn't pay C's payout script: unchecked, not refused (D19)
        assert_equal(dw.mn(registered, "attach", mn_c, "--role", "payout", stdin=wif(owner_c))["check"], "unchecked")
        # nothing to check the static masternode's owner key against
        stranger = ECKey()
        stranger.generate()
        assert_equal(dw.mn(registered, "attach", static.proTxHash, "--role", "owner", stdin=wif(stranger))["check"],
                     "unchecked")
        dw.mn(registered, "label", mn_a, "--label", "Home node")
        overview = dw.mn(registered, "list")
        assert len(overview["masternodes"]) == 4, "C is listed once though tracked and owned"
        s, c, a = (self.row(overview, h) for h in (static.proTxHash, mn_c, mn_a))
        assert_equal((s["status"], s["mine"], s["tracked"], s["label"], s["type_index"]),
                     ("enabled", False, True, "Static", 0))
        attached = lambda r: {k["role"]: k["check"] for k in r["roles"]["attached"]}
        key_id = lambda key: hash160(key.get_pubkey().get_bytes()).hex()
        assert_equal((roles(s), attached(s)), ({"owner"}, {"owner": "unchecked"}))
        assert_equal((c["tracked"], attached(c)), (True, {"owner": "matched", "payout": "unchecked", "voting": "matched"}))
        assert_equal({k["role"]: k["key_id"] for k in c["roles"]["attached"]},
                     {"owner": key_id(owner_c), "payout": key_id(owner_c), "voting": key_id(voting_c)})
        assert_equal(roles(c), {"owner", "voting", "payout"})
        assert_equal(a["label"], "Home node")
        assert_equal(dw.mn(registered, "untrack", mn_c), {"keys_deleted": 3})
        assert_equal(dw.mn(registered, "untrack", static.proTxHash), {"keys_deleted": 1})
        dw.mn(registered, "untrack", mn_c, error="masternodes.not_tracked")
        # the keys are gone: tracked again, C has none attached
        dw.mn(registered, "track", mn_c)
        assert_equal(self.row(dw.mn(registered, "list"), mn_c)["roles"]["attached"], [])
        assert_equal(dw.mn(registered, "untrack", mn_c), {"keys_deleted": 0})

        self.log.info("3b. A ProUpRegTx rotates D's voting key: the attached key no longer holds (D24)")
        dw.mn(registered, "track", mn_d)
        assert_equal(dw.mn(registered, "attach", mn_d, "--role", "voting", stdin=wif(voting_d))["check"], "matched")
        d = self.row(dw.mn(registered, "list"), mn_d)
        assert_equal(d["roles"]["voting"], True)
        assert_equal(d["roles"]["attached"], [{"role": "voting", "key_id": key_id(voting_d), "check": "matched"}])
        rotated_to = ECKey()
        rotated_to.generate()
        node.protx("update_registrar", mn_d, "", address(rotated_to), "", fee_addr)
        rotated = self.mine()
        assert_equal(node.protx("info", mn_d)["state"]["votingAddress"], address(rotated_to))
        # a fresh process: the id came back from app.sqlite, the list is the new one
        d = self.row(dw.mn(rotated, "list"), mn_d)
        assert_equal(d["roles"]["attached"], [{"role": "voting", "key_id": key_id(voting_d), "check": "key_mismatch"}])
        assert_equal(d["roles"]["voting"], False)
        assert_equal(dw.mn(rotated, "untrack", mn_d), {"keys_deleted": 1})

        self.log.info("4. Payouts equal Core's coinbase payments")
        for _ in range(60):
            h = self.mine(5)
            paid_a = self.coinbase_payments(payout_a, registered)
            paid_c = self.coinbase_payments(payout_c, registered)
            if len(paid_a) >= 2 and len(paid_c) >= 2:
                break
        assert_greater_than_or_equal(len(paid_a), 2)
        payouts = dw.mn(h, "payouts", mn_a)
        assert_equal({p["txid"]: p["amount"] for p in payouts}, paid_a)
        assert_equal([p["height"] for p in payouts], sorted((p["height"] for p in payouts), reverse=True))
        assert all(p["address"] == payout_a and not p["matured"] for p in payouts)
        assert_equal({p["txid"]: p["amount"] for p in dw.mn(h, "payouts", mn_c)}, paid_c)
        overview = dw.mn(h, "list")
        a = self.row(overview, mn_a)
        assert_equal(a["payouts_30d"], sum(paid_a.values()))
        assert_equal(a["last_payout"], payouts[0])
        assert_equal(overview["payouts_30d"], sum(paid_a.values()) + sum(paid_c.values()))
        assert_equal(dw.mn(h, "payouts", mn_b), [])
        first = payouts[-1]
        h = self.mine(100)
        matured = {p["txid"]: p["matured"] for p in dw.mn(h, "payouts", mn_a)}
        assert_equal(matured[first["txid"]], True)
        if evidence:
            os.makedirs(evidence, exist_ok=True)
            with open(os.path.join(evidence, "mn-list.json"), "w", encoding="utf8") as f:
                json.dump(dw.mn(h, "list"), f, indent=1)
            with open(os.path.join(evidence, "mn-a-payouts.json"), "w", encoding="utf8") as f:
                json.dump({"wallet": dw.mn(h, "payouts", mn_a), "core": paid_a}, f, indent=1)

        self.log.info("5. Status transitions: enabled -> pose_banned -> removed")
        node.protx("revoke", mn_a, bls[0]["secret"], 0, fee_addr)
        h = self.mine()
        assert_equal(node.protx("info", mn_a)["state"]["PoSeBanHeight"], h)
        detail = dw.mn(h, "status", mn_a)
        assert_equal(detail["row"]["status"], "pose_banned")
        assert_equal(detail["list"]["valid"], False)
        self.start_miner()
        try:
            out = dw.run("send", wallet, "--sync-height", str(h), "--coin", f"{funding}:{vout}",
                         "--to", f"{node.getnewaddress()}:{COLLATERAL - COIN}").stdout
        finally:
            self.stop_miner()
        spend = out.split("broadcast ")[1].split()[0]
        self.wait_until(lambda: node.getrawtransaction(spend, True).get("confirmations", 0) > 0, timeout=60)
        h = self.mine()
        assert mn_a not in {m["proTxHash"] for m in node.protx("list", "registered", True)}
        detail = dw.mn(h, "status", mn_a)
        assert_equal((detail["row"]["status"], detail["list"]), ("removed", None))
        assert_equal(detail["row"]["roles"]["collateral"], False)
        assert_equal(self.row(dw.mn(h, "list"), mn_b)["status"], "enabled")
        if evidence:
            with open(os.path.join(evidence, "mn-a-removed.json"), "w", encoding="utf8") as f:
                json.dump(detail, f, indent=1)
            shutil.copy(os.path.join(self.options.tmpdir, "test_framework.log"), evidence)
        self.log.info("Masternodes suite passed")


if __name__ == "__main__":
    DwdMasternodes().main()
