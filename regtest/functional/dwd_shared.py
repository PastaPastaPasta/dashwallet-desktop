#!/usr/bin/env python3
# Copyright (c) 2026 The Dash Wallet Desktop developers
# Distributed under the MIT software license, see test_framework/COPYING.
"""dwd_shared.py — regtest suite `shared` (MG-04b T1, BL-501).

dw-engine's shared-masternode creation (`dwcli mn shared …`, slice 1b) against Core rc.3 with v24
active: every registration the wallets build must be accepted by Core's mempool and blocks, and
`protx info` must then show the share table the parties agreed. The sessions' messages pass
between the parties as text, as a user would paste them.

Network: as `dwd_protx.py` in its v24 run (`-vbparams=v24:0:999999999999:0:10:8:6:5:0`, as Core's
feature_protx_version.py:59 sets it). The wallet reads v24 `active` only once a version-3 entry is
in its synced list or noted (MG-04b D3), so dashd first registers one version-3 masternode with
`register_fund`; each wallet's first `capability` after that must read `active`.

Sessions:
  S1  three dwcli wallets: A coordinates (400 DASH), B and C join (300 each; C's rewards to a new
      address); start, join, lock, approve, sign, send; `protx info` shows the table.
  R   the reward change (ProUpShareTx): B moves its rewards to a new address, C resets its own to
      its refund address.
  S2  dashd's wallet as a dash-qt participant Q (300 DASH) under coordinator A (700): this script
      edits the envelope as dash-qt would (its row and contribution, the funding template, the
      fingerprint), signs its consent with `protx shared_sign` and its input with
      `signrawtransactionwithwallet`; A signs last and sends (D24).
  S3  dashd as a dash-qt coordinator P (600 DASH) with B as participant (400): the script builds
      the Invitation, absorbs B's Details, runs `protx shared_register_prepare`, signs P's consent
      (`shared_sign`), combines (`shared_combine`) and signs P's input right after, in
      `runCombineAndSign`'s order (sharedmncreatedialog.cpp:2576-2591); B's funding signature then
      completes the transaction, which B can no longer discard (D24, D17); the script broadcasts it.

Run (DWCLI = a built dwcli, DASHCORE_DIR = Dash Core v24.0.0-rc.3; on a loaded machine add
`--timeout-factor=4`, dashd's start alone can take over the default 60 s):
  DWCLI=<target>/debug/dwcli DWD_PYTHON=<venv>/bin/python DASHCORE_DIR=<dashcore> \\
      regtest/functional/run.sh dwd_shared.py
"""

import hashlib
import json
import os

from dwd_masternodes import COIN, COLLATERAL, PHRASE, DwdMasternodes, Dwcli
from dwd_protx import V24_PARAMS, DwdProtx, handed_off
from test_framework.util import assert_equal, p2p_port, softfork_active

# BIP39's test vectors: B's and C's wallets (A's is the masternodes suites' PHRASE).
PHRASE_B = "legal winner thank year wave sausage worth useful legal winner thank yellow"
PHRASE_C = "letter advice cage absurd amount doctor acoustic avoid letter advice cage above"
FEE = 50_000  # the coordinator's registration fee, duffs
EARLY_BLOCKS = 720
EARLY_PENALTY = 5 * COIN
SEQUENCE_FINAL = 0xFFFFFFFF


def dash(duffs):
    """A JSON-RPC amount (DASH, 8 decimals) for `duffs`."""
    return f"{duffs // COIN}.{duffs % COIN:08d}"


def univalue_write(v, pretty=2, indent_level=0):
    """UniValue's `write(pretty)` (rc.3 `univalue/lib/univalue_write.cpp`): an empty array or
    object still breaks the line, unlike Python's json.dumps."""
    level = max(indent_level, 1)  # `modIndent`
    pad = lambda n: " " * (pretty * n)
    if isinstance(v, (dict, list)):
        opener, closer = "{}" if isinstance(v, dict) else "[]"
        items = list(v.items()) if isinstance(v, dict) else [(None, x) for x in v]
        out = opener + "\n"
        for i, (k, x) in enumerate(items):
            out += pad(level) + ("" if k is None else json_escape(k) + ": ")
            out += univalue_write(x, pretty, level + 1)
            out += ("," if i + 1 != len(items) else "") + "\n"
        return out + pad(level - 1) + closer
    if isinstance(v, bool):
        return "true" if v else "false"
    if v is None:
        return "null"
    if isinstance(v, int):
        return str(v)
    return json_escape(v)


def json_escape(s):
    """`json_escape` (rc.3 `univalue/include/univalue_escapes.h`), quoted."""
    named = {'"': '\\"', "\\": "\\\\", "\b": "\\b", "\t": "\\t", "\n": "\\n", "\f": "\\f", "\r": "\\r"}
    out = ""
    for c in s:
        if c in named:
            out += named[c]
        elif ord(c) < 0x20 or ord(c) == 0x7F:
            out += f"\\u{ord(c):04x}"
        else:
            out += c
    return '"' + out + '"'


def seal(envelope):
    """dash-qt's `toJsonString`: the envelope without its code, the code (`EnvelopeFingerprint`:
    SHA-256 of `write(2)`, first four bytes, `XXXX-XXXX`) appended last."""
    body = {k: v for k, v in envelope.items() if k != "fingerprint"}
    digest = hashlib.sha256(univalue_write(body).encode()).hexdigest().upper()
    body["fingerprint"] = f"{digest[:4]}-{digest[4:8]}"
    return univalue_write(body)


class DwdShared(DwdProtx):
    def set_test_params(self):
        # Always the v24 network: DwdProtx adds V24_PARAMS only under DWD_MG04_V24=1.
        DwdMasternodes.set_test_params(self)
        self.extra_args = [args + [V24_PARAMS] for args in self.extra_args]

    # --- dashd's wallet as a dash-qt party ---------------------------------------------

    def own_coin(self, duffs):
        """A coin of exactly `duffs` in dashd's wallet, locked so no later send spends it."""
        node = self.nodes[0]
        addr = node.getnewaddress()
        txid = node.sendtoaddress(addr, dash(duffs))
        self.mine()
        coin = {"txid": txid, "vout": self.outpoint_of(txid, addr, duffs)}
        node.lockunspent(False, [coin])
        return coin

    def contribution(self, label, coin, change):
        """A contribution of dashd's coin `coin`, `change` duffs back to a new address (dash-qt's
        `addContribution`); `rebuild_funding` adds its `changeIndex`."""
        entry = {"label": label, "inputs": [{**coin, "sequence": SEQUENCE_FINAL}]}
        if change > 0:
            entry["change"] = {"address": self.nodes[0].getnewaddress(), "amount": change}
        return entry

    def rebuild_funding(self, envelope):
        """`rebuildFundingTx`: every contribution's inputs in order, then their change outputs;
        each `changeIndex` its output's index."""
        inputs, outputs = [], []
        for c in envelope["contributions"]:
            inputs += [{"txid": i["txid"], "vout": i["vout"], "sequence": i["sequence"]} for i in c["inputs"]]
            if "change" in c:
                c["changeIndex"] = len(outputs)
                outputs.append({c["change"]["address"]: dash(c["change"]["amount"])})
            else:
                c["changeIndex"] = -1
        envelope["fundingTx"] = self.nodes[0].createrawtransaction(inputs, outputs)

    # --- the wallets -------------------------------------------------------------------

    def send(self, w, h, sid):
        """Coordinator `w` signs last and sends, a miner running (dash-spv takes a broadcast as
        sent once it sees it in a block): the Final Record, and the tip once it is mined."""
        self.start_miner()
        try:
            sent = w.mn(h, "shared", "send", sid)
        finally:
            self.stop_miner()
        assert_equal(sent["message"]["name"], "Final Record")
        return sent, self.wait_mined(sent["session"]["pro_tx_hash"])

    def check_table(self, pro_tx_hash, rows, rewards=None):
        """Core's share table for `pro_tx_hash` is the session's `rows`; `rewards` the reward
        address of each row, by default its refund address (Core's for an empty one)."""
        st = self.core_info(pro_tx_hash)[1]
        assert_equal(st["version"], 3)
        assert_equal([(s["amount"], s["ownerAddress"], s["refundAddress"]) for s in st["shares"]],
                     [(r["amount"], r["owner_address"], r["refund_address"]) for r in rows])
        assert_equal([s["rewardAddress"] for s in st["shares"]],
                     rewards or [r["refund_address"] for r in rows])
        return st

    def wait_mined(self, txid):
        """The tip after one more block, once `txid` is in a block (120 s: the box is loaded)."""
        node = self.nodes[0]
        self.wait_until(lambda: node.getrawtransaction(txid, True).get("confirmations", 0) > 0, timeout=120)
        return self.mine()

    # --- the test ----------------------------------------------------------------------

    def run_test(self):
        node = self.nodes[0]
        a, b, c = (Dwcli(os.environ["DWCLI"], os.path.join(self.options.tmpdir, f"dwcli-{n}"), p2p_port(0),
                         self.log) for n in "abc")
        for _ in range(30):
            if softfork_active(node, "v24"):
                break
            self.mine(10)
        assert softfork_active(node, "v24"), node.getdeploymentinfo()["deployments"]["v24"]
        self.log.info(f"v24 active at height {self.tip()}")
        self.prepare_network()
        self.fund_node(6_000 * COIN)

        for w, phrase, coins in ((a, PHRASE, (450, 750)), (b, PHRASE_B, (350, 450)), (c, PHRASE_C, (350,))):
            w.run("init-vault")
            w.run("import", "--birth-height", "0", stdin=phrase)
            addr = w.run("receive", "--next").stdout.split()[1]
            for amount in coins:
                node.sendtoaddress(addr, amount)
        # register_fund takes the collateral and fee from one address (else the payout address).
        fund_v3 = node.getnewaddress()
        node.sendtoaddress(fund_v3, COLLATERAL // COIN + 1)
        h = self.mine()

        def shared(w, *args, **kw):
            return w.mn(h, "shared", *args, **kw)

        # The framework's masternodes may be version 3 already (registered once v24 is active);
        # this one is, whatever the framework does.
        self.log.info("0. dashd registers a version-3 masternode: each wallet reads v24 active (D3)")
        v3 = node.protx("register_fund", node.getnewaddress(), [f"127.0.0.1:{p2p_port(15)}"],
                        node.getnewaddress(), node.bls("generate")["public"], node.getnewaddress(), 0,
                        node.getnewaddress(), fund_v3)
        h = self.mine()
        assert_equal(self.core_info(v3)[1]["version"], 3)
        for w in (a, b, c):
            # The first call, which may run before the follower's tick notes the entry.
            cap = shared(w, "capability")
            assert_equal((cap["v24"], cap["can_send"], cap["evonode_shares"]), ("active", True, "inactive"))

        self.log.info("1. S1: three dwcli wallets")
        service = f"127.0.0.1:{p2p_port(16)}"
        operator = node.bls("generate")["public"]
        inv = shared(a, "start", "--share", f"A:{400 * COIN}", "--share", f"B:{300 * COIN}",
                     "--share", f"C:{300 * COIN}", "--service", service, "--operator-key", operator,
                     "--early-period-blocks", str(EARLY_BLOCKS), "--early-penalty", str(EARLY_PENALTY),
                     "--fee", str(FEE))
        sid = inv["session"]["id"]
        assert_equal((inv["message"]["name"], inv["session"]["role"]), ("Invitation", "coordinator"))

        def join(w, text, index, reward):
            got = shared(w, "import", stdin=text)
            assert_equal((got["kind"], got["session"]["id"], got["session"]["stage"]["kind"]),
                         ("session", sid, "invited"))
            joined = shared(w, "join", sid, "--share-index", str(index), "--reward", reward)
            assert_equal(joined["session"]["role"], "participant")
            shared(a, "import", stdin=joined["message"]["text"])

        join(b, inv["message"]["text"], 1, "refund")
        join(c, shared(a, "message", sid)["text"], 2, "new")
        s = shared(a, "show", sid)
        assert_equal((s["next"], s["revision"]), ("lock", 3))
        locked = shared(a, "lock", sid, "--revision", str(s["revision"]))
        assert_equal(locked["message"]["name"], "Locked Terms")
        for w in (b, c):
            got = shared(w, "import", stdin=locked["message"]["text"])
            assert_equal(got["session"]["next"], "approve")
            approval = shared(w, "approve", sid, "--consent-hash", got["session"]["consent_hash"])
            assert_equal(approval["message"]["name"], "Approval")
            shared(a, "import", stdin=approval["message"]["text"])
        request = shared(a, "message", sid)
        assert_equal(request["name"], "Signing Request")
        for w in (b, c):
            got = shared(w, "import", stdin=request["text"])
            assert_equal(got["session"]["next"], "sign")
            signed = shared(w, "sign", sid)
            assert_equal((signed["message"]["name"], signed["session"]["stage"]["kind"]),
                         ("Signed Contribution", "funding_signed"))
            shared(a, "import", stdin=signed["message"]["text"])
        assert_equal(shared(a, "show", sid)["next"], "send")
        sent, h = self.send(a, h, sid)
        pro_tx, rows = sent["session"]["pro_tx_hash"], sent["session"]["rows"]
        c_reward = rows[2]["reward_address"]
        assert c_reward and c_reward != rows[2]["refund_address"], rows[2]
        st = self.check_table(pro_tx, rows, [rows[0]["refund_address"], rows[1]["refund_address"], c_reward])
        assert_equal((st["pubKeyOperator"], st["addresses"]["core_p2p"]), (operator, [service]))
        assert_equal((st["earlyPeriodBlocks"], st["earlyPenalty"]), (EARLY_BLOCKS, EARLY_PENALTY))
        for w in (a, b, c):
            assert_equal(shared(w, "show", sid)["stage"]["kind"], "confirmed")

        self.log.info("2. R: B's rewards to a new address, C's back to its refund address")
        preview = shared(b, "preview-update-share", pro_tx, "--share-index", "1", "--reward", "new")
        assert_equal((preview["reward_address"], preview["reward_is_refund"]), (None, False))
        self.start_miner()
        try:
            op_b = shared(b, "update-share", pro_tx, "--share-index", "1", "--reward", "new")
            op_c = shared(c, "update-share", pro_tx, "--share-index", "2", "--reward", "refund")
        finally:
            self.stop_miner()
        assert handed_off(op_b) and handed_off(op_c)
        self.wait_mined(op_b["txid"])
        h = self.wait_mined(op_c["txid"])
        b_reward = self.core_info(pro_tx)[1]["shares"][1]["rewardAddress"]
        assert b_reward not in (rows[1]["refund_address"], rows[1]["owner_address"]), b_reward
        assert_equal(node.validateaddress(b_reward)["isvalid"], True)
        self.check_table(pro_tx, rows, [rows[0]["refund_address"], b_reward, rows[2]["refund_address"]])

        self.log.info("3. S2: dashd's wallet as a dash-qt participant under coordinator A")
        q_coin = self.own_coin(301 * COIN)
        h = self.tip()
        inv = shared(a, "start", "--share", f"A:{700 * COIN}", "--share", f"Q:{300 * COIN}",
                     "--service", f"127.0.0.1:{p2p_port(17)}", "--operator-key", node.bls("generate")["public"],
                     "--early-period-blocks", str(EARLY_BLOCKS), "--early-penalty", str(EARLY_PENALTY),
                     "--fee", str(FEE))
        sid = inv["session"]["id"]
        # Q's Details: its row and contribution, the funding template rebuilt, revision + 1.
        env = json.loads(inv["message"]["text"])
        q_row = {"ownerAddress": node.getnewaddress(), "refundAddress": node.getnewaddress(), "rewardAddress": ""}
        env["shares"][1].update(q_row)
        env["contributions"].append(self.contribution("Q", q_coin, COIN))
        self.rebuild_funding(env)
        env["revision"] += 1
        got = shared(a, "import", stdin=seal(env))
        assert got["changed"] and got["session"]["rows"][1]["contributed"], got
        assert_equal([w["kind"] for w in got["session"]["warnings"]], ["unvalued_inputs"])
        locked = shared(a, "lock", sid, "--revision", str(got["session"]["revision"]))
        # Q's Approval: its consent signature added, in share order. A's template is Q's.
        env_locked = json.loads(locked["message"]["text"])
        assert_equal(env_locked["fundingTx"], env["fundingTx"])
        env = env_locked
        sigs = node.protx("shared_sign", env["protx"])["signatures"]
        assert_equal([s["shareIndex"] for s in sigs], [1])
        env["sigs"] = sorted(env["sigs"] + sigs, key=lambda s: s["shareIndex"])
        got = shared(a, "import", stdin=seal(env))
        assert_equal(got["session"]["stage"]["kind"], "combined")
        # Q's Signed Contribution: its input signed on the Signing Request, where A's is not (D24).
        env = json.loads(shared(a, "message", sid)["text"])
        own = node.signrawtransactionwithwallet(env["protx"])
        assert not own["complete"]
        env["protx"] = own["hex"]
        shared(a, "import", stdin=seal(env))
        assert_equal(shared(a, "show", sid)["next"], "send")
        sent, h = self.send(a, h, sid)
        pro_tx, rows = sent["session"]["pro_tx_hash"], sent["session"]["rows"]
        assert_equal((rows[1]["owner_address"], rows[1]["refund_address"]),
                     (q_row["ownerAddress"], q_row["refundAddress"]))
        self.check_table(pro_tx, rows)
        assert_equal(shared(a, "show", sid)["stage"]["kind"], "confirmed")

        self.log.info("4. S3: dashd as a dash-qt coordinator, B's signature completes the transaction")
        p_coin = self.own_coin(601 * COIN)
        h = self.tip()
        p_row = {"label": "P", "amount": 600 * COIN, "ownerAddress": node.getnewaddress(),
                 "refundAddress": node.getnewaddress(), "rewardAddress": ""}
        env = {
            "type": "dash-shared-mn-session", "version": 1, "network": "regtest",
            "sessionId": os.urandom(32).hex(), "revision": 1, "stage": "draft", "fundingTx": "",
            "shares": [p_row, {"label": "B", "amount": 400 * COIN, "ownerAddress": "", "refundAddress": "",
                               "rewardAddress": ""}],
            "terms": {"coreP2PAddrs": f"127.0.0.1:{p2p_port(18)}", "operatorPubKey": node.bls("generate")["public"],
                      "votingAddress": node.getnewaddress(), "operatorReward": 0,
                      "earlyPeriodBlocks": EARLY_BLOCKS, "earlyPenalty": EARLY_PENALTY},
            "contributions": [self.contribution("P", p_coin, 601 * COIN - 600 * COIN - FEE)],
            "protx": "", "consentHash": "", "collateralIndex": -1, "sigs": [],
            "prepareWallet": node.getwalletinfo()["walletname"], "operatorSecretHolder": "dashd",
            "coordinatorLabel": "P",
        }
        self.rebuild_funding(env)
        sid = env["sessionId"]
        got = shared(b, "import", stdin=seal(env))
        assert_equal((got["session"]["role"], got["session"]["next"]), ("participant", "join"))
        details = shared(b, "join", sid)
        assert_equal(details["session"]["debit"], 400 * COIN)
        # P absorbs B's Details (its own contribution unchanged; B's template is P's rebuild), then
        # `runCombineAndSign`'s order.
        env = json.loads(details["message"]["text"])
        assert_equal(env["contributions"][0]["inputs"], [{**p_coin, "sequence": SEQUENCE_FINAL}])
        assert_equal(env["revision"], 2)
        sent_template = env["fundingTx"]
        self.rebuild_funding(env)
        assert_equal(env["fundingTx"], sent_template)
        shares = [{k: s[k] for k in ("amount", "refundAddress", "ownerAddress")} for s in env["shares"]]
        for share, s in zip(shares, env["shares"]):
            if s["rewardAddress"]:
                share["rewardAddress"] = s["rewardAddress"]
        t = env["terms"]
        prep = node.protx("shared_register_prepare", env["fundingTx"], shares, t["coreP2PAddrs"],
                          t["operatorPubKey"], t["votingAddress"], str(t["operatorReward"]),
                          t["earlyPeriodBlocks"], t["earlyPenalty"])
        env.update(stage="frozen", protx=prep["tx"], consentHash=prep["consentHash"],
                   collateralIndex=prep["collateralIndex"])
        env["sigs"] = node.protx("shared_sign", prep["tx"])["signatures"]
        assert_equal([s["shareIndex"] for s in env["sigs"]], [0])
        env["stage"] = "signing"
        got = shared(b, "import", stdin=seal(env))
        assert_equal((got["session"]["next"], got["session"]["consent_hash"]), ("approve", prep["consentHash"]))
        approval = shared(b, "approve", sid, "--consent-hash", prep["consentHash"])
        env = json.loads(approval["message"]["text"])
        assert_equal([s["shareIndex"] for s in env["sigs"]], [0, 1])
        own = node.signrawtransactionwithwallet(node.protx("shared_combine", prep["tx"], env["sigs"]))
        assert not own["complete"]
        env.update(stage="combined", protx=own["hex"])
        got = shared(b, "import", stdin=seal(env))
        assert_equal(got["session"]["next"], "sign")
        signed = shared(b, "sign", sid)
        assert_equal(signed["session"]["stage"]["kind"], "funding_signed")
        env = json.loads(signed["message"]["text"])
        assert_equal(env["stage"], "fundingSigned")
        # D17: B's signature may have completed the transaction, so B keeps the session.
        shared(b, "discard", sid, error="masternodes.shared_stage")
        pro_tx = node.sendrawtransaction(env["protx"])
        assert_equal(pro_tx, signed["session"]["pro_tx_hash"])
        h = self.mine()
        rows = signed["session"]["rows"]
        assert_equal((rows[0]["owner_address"], rows[0]["refund_address"]),
                     (p_row["ownerAddress"], p_row["refundAddress"]))
        self.check_table(pro_tx, rows)
        assert_equal(shared(b, "show", sid)["stage"]["kind"], "confirmed")


if __name__ == "__main__":
    DwdShared().main()
