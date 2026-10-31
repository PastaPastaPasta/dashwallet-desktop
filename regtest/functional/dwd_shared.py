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
  K1  changing S1's keys together (ProUpSharedRegTx, slice 2d): A prepares a rotation of the
      operator and voting keys to keys dashd holds, funded by A (charged nothing yet); B and C
      import the Request, approve and reply; B can't send it (only the funder re-signs its fee
      inputs); A inserts the signatures and sends; Core mines it, `protx info` shows the new keys
      and the PoSe ban, every wallet's session reads `confirmed`, and dashd, the operator, revives
      it with `protx update_service` (the wallet never signs as operator). A pending key change
      holds A's next maintenance call until it is buried (D105).
  D   dissolve now (ProDisTx, slice 2a): A dissolves S1's masternode alone in the early period, so
      it pays the penalty, split over B and C; the outputs Core mines are the preview's, the
      masternode leaves the list, and A's operation reads `confirmed`.
  S2  dashd's wallet as a dash-qt participant Q (300 DASH) under coordinator A (700): this script
      edits the envelope as dash-qt would (its row and contribution, the funding template, the
      fingerprint), signs its consent with `protx shared_sign` and its input with
      `signrawtransactionwithwallet`; A signs last and sends (D24).
  S   standbys of S2's masternode (slice 2b): A creates its pair in the early period and exports
      Core's file, whose immediate variant dashd accepts at once and whose full one it refuses
      (`bad-prodis-penalty-floor`); A imports Q's full variant from dashd's `protx shared_dissolve`
      (`submit=false`); past the period A sends Q's, which Core mines byte for byte, and the
      masternode leaves the list.
  S3  dashd as a dash-qt coordinator P (600 DASH) with B as participant (400): the script builds
      the Invitation, absorbs B's Details, runs `protx shared_register_prepare`, signs P's consent
      (`shared_sign`), combines (`shared_combine`) and signs P's input right after, in
      `runCombineAndSign`'s order (sharedmncreatedialog.cpp:2576-2591); B's funding signature then
      completes the transaction, which B can no longer discard (D24, D17); the script broadcasts it.
  K2  changing S3's keys together, both ways (slice 2d). dashd as P prepares a voting-key change
      (`protx shared_update_registrar_prepare`, sequences 0xfffffffe and its prepare-time
      scriptSigs) and signs; B imports it, approves (charged nothing) and replies; dashd combines
      (`shared_combine`, which re-signs its fee input) and sends; B's session reads `confirmed` by
      the list. Then B prepares an operator-key change it funds, dashd signs it (`shared_sign`), B
      imports the signature as a bare array for its session and sends; dashd revives it.
  T   dissolving S3's masternode together (slice 2c), both ways. dashd as P prepares
      (`protx shared_dissolve_prepare`, P the actor) and signs; B imports the request, approves
      (charged nothing) and replies; dashd's `shared_combine` of B's reply passes
      `testmempoolaccept`, unsent. Then B prepares with its share the actor at exactly the relay
      minimum (one duff less is refused), dashd signs it (`shared_sign`) and B imports the
      signature as a bare array for its session, combines and sends: Core mines dashd's
      `shared_combine` bytes, the masternode leaves the list, and P's unsent request reads dead.

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

        self.log.info("2a. K1: S1's operator and voting keys changed together, funded by A")
        # D23: B's and C's reward changes hold their approvals until buried.
        h = self.mine(6)
        before = self.core_info(pro_tx)[1]
        new_operator, new_voting = node.bls("generate"), node.getnewaddress()
        started = shared(a, "key-change", pro_tx, "--operator-key", new_operator["public"],
                         "--voting-address", new_voting)
        k = started["session"]
        assert_equal((started["message"]["name"], k["kind"], k["role"], k["stage"]["kind"], k["next"]),
                     ("Request", "key_change", "coordinator", "signing", "copy"))
        assert_equal(k["base_keys"], {"operator_public_key": before["pubKeyOperator"],
                                      "voting_address": before["votingAddress"]})
        assert_equal(k["key_change"], {
            "requested": {"operator_public_key": new_operator["public"], "voting_address": new_voting},
            "current": k["base_keys"], "funder": True})
        assert_equal([w["kind"] for w in k["warnings"]], ["operator_reset", "usable_by_funder"])
        assert k["fee"] > 0 and k["debit"] == k["fee"] and len(k["reserved"]) > 0, k
        for w in (b, c):
            got = shared(w, "import", stdin=started["message"]["text"])["session"]
            assert_equal((got["id"], got["role"], got["stage"]["kind"], got["next"], got["fee"]),
                         (k["id"], "participant", "signing", "approve", None))
            assert_equal((got["key_change"]["requested"], got["key_change"]["funder"]),
                         (k["key_change"]["requested"], False))
            reply = shared(w, "approve-maintenance", k["id"])
            assert_equal(reply["message"]["name"], "Reply")
            shared(a, "import", stdin=reply["message"]["text"])
        # Only the funder can sign its fee inputs again after the insert (CombineShared).
        shared(b, "send-maintenance", k["id"], error="masternodes.shared_stage")
        assert_equal(shared(a, "show", k["id"])["next"], "send")
        self.start_miner()
        try:
            sent = shared(a, "send-maintenance", k["id"])
        finally:
            self.stop_miner()
        assert_equal(sent["kind"], "session")
        assert sent["session"]["stage"]["kind"] in ("sent", "confirmed"), sent
        h = self.wait_mined(sent["session"]["txid"])
        mined = node.getrawtransaction(sent["session"]["txid"], True)
        assert_equal((mined["version"], mined["type"]), (3, 12))
        assert all(i["sequence"] == SEQUENCE_FINAL for i in mined["vin"]), mined["vin"]
        st = self.core_info(pro_tx)[1]
        assert_equal((st["pubKeyOperator"], st["votingAddress"]), (new_operator["public"], new_voting))
        assert st["PoSeBanHeight"] > 0, st
        for w in (a, b, c):
            assert_equal(shared(w, "show", k["id"])["stage"]["kind"], "confirmed")
        # D105: the key change holds A's next maintenance call until it is buried.
        shared(a, "dissolve", pro_tx, "--share-index", "0", error="masternodes.update_pending")
        fee_source = node.getnewaddress()
        node.sendtoaddress(fee_source, 1)
        self.mine()
        node.protx("update_service", pro_tx, service, new_operator["secret"], "", fee_source)
        h = self.mine()
        st = self.core_info(pro_tx)[1]
        assert_equal(st["PoSeBanHeight"], -1)
        assert st["PoSeRevivedHeight"] > 0, st
        h = self.mine(6)  # RECOMMENDED_CONFIRMATIONS: buried

        self.log.info("2b. D: A dissolves S1 alone in the early period, paying the penalty")
        preview = shared(a, "preview-dissolve", pro_tx, "--share-index", "0")
        assert_equal((preview["early"], preview["penalty"], preview["fee"]), (True, EARLY_PENALTY, 1_000))
        assert_equal(preview["total_debit"], EARLY_PENALTY + 1_000)
        assert_equal(preview["valid_from_height"], self.core_info(pro_tx)[1]["registeredHeight"] + EARLY_BLOCKS)
        half = EARLY_PENALTY // 2
        want = [(400 * COIN - EARLY_PENALTY - 1_000, rows[0]["refund_address"], True),
                (300 * COIN + half, rows[1]["refund_address"], False),
                (300 * COIN + EARLY_PENALTY - half, rows[2]["refund_address"], False)]
        assert_equal([(r["amount"], r["refund_address"], r["mine"]) for r in preview["returns"]], want)
        assert_equal(preview["warnings"], [])
        self.start_miner()
        try:
            op = shared(a, "dissolve", pro_tx, "--share-index", "0")
        finally:
            self.stop_miner()
        assert handed_off(op)
        assert_equal((op["kind"], op["fee"]), ("dissolve", 1_000))
        h = self.wait_mined(op["txid"])
        # Core's builder order: the non-actors in share order, then the actor.
        mined = node.getrawtransaction(op["txid"], True)
        assert_equal(mined["type"], 10)
        got = [(round(o["value"] * COIN), o["scriptPubKey"]["address"]) for o in mined["vout"]]
        assert_equal(got, [(amount, address) for amount, address, _ in want[1:] + want[:1]])
        assert pro_tx not in {m["proTxHash"] for m in node.protx("list", "registered", True)}, pro_tx
        ops = {o["id"]: o for o in a.mn(h, "operations")}
        assert_equal(ops[op["id"]]["state"]["kind"], "confirmed")
        shared(a, "preview-dissolve", pro_tx, "--share-index", "0", error="masternodes.shared_too_early")

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

        self.log.info("3b. S: standbys of S2's masternode, created, exported, imported and sent")
        valid_from = self.core_info(pro_tx)[1]["registeredHeight"] + EARLY_BLOCKS
        assert_equal(shared(a, "show", sid)["next"], "create_standby")
        created = shared(a, "create-standby", pro_tx)
        assert_equal([(s["share_index"], s["fee"], s["full"], s["immediate"], s["source"], s["full_valid_from"])
                      for s in created], [(0, 1_000, True, True, "created", valid_from)])
        # Signing again stores nothing new (D84); the prompt is answered (D91).
        assert_equal(shared(a, "create-standby", pro_tx), created)
        assert_equal(shared(a, "standby", pro_tx), created)
        assert_equal(shared(a, "show", sid)["next"], "nothing")
        out = os.path.join(self.options.tmpdir, "standby.txt")
        exported = shared(a, "export-standby", pro_tx, "--share-index", "0", "--out", out)
        assert_equal(exported["file_name"], f"standby-dissolution-{pro_tx[:8]}-share1.txt")
        with open(out, encoding="utf-8") as f:
            assert_equal(f.read(), exported["text"])
        lines = exported["text"].split("\n")
        hex_after = lambda prefix: lines[1 + next(i for i, l in enumerate(lines) if l.startswith(prefix))]
        full_hex, immediate_hex = hex_after("FULL PRINCIPAL — valid from block"), hex_after("IMMEDIATE — pays")
        assert_equal(node.testmempoolaccept([immediate_hex])[0]["allowed"], True)
        assert_equal(node.testmempoolaccept([full_hex])[0]["reject-reason"], "bad-prodis-penalty-floor")
        # dashd as Q (share 1) signs its full variant without sending it; A stores it.
        q_full = node.protx("shared_dissolve", pro_tx, 1, 1_000, False, False)
        got = shared(a, "import", stdin=q_full)
        assert_equal((got["kind"], got["stored"]), ("standby", True))
        d = got["description"]
        assert_equal((d["variant"], d["penalty"], d["returns_principal"]), ("full", 0, True))
        assert_equal((d["standby"]["share_index"], d["standby"]["source"]), (1, "imported"))
        assert_equal(shared(a, "import", stdin=q_full)["stored"], False)
        assert_equal([(s["share_index"], s["full"], s["immediate"]) for s in shared(a, "standby", pro_tx)],
                     [(0, True, True), (1, True, False)])
        send_q = ("send-standby", pro_tx, "--share-index", "1", "--variant", "full")
        shared(a, *send_q, error="masternodes.shared_too_early")
        # The full variants are valid in the block at `valid_from` (D85).
        h = self.mine(valid_from - 1 - self.tip())
        shared(a, "send-standby", pro_tx, "--share-index", "0", "--variant", "immediate",
               error="masternodes.shared_stage")
        assert_equal(node.testmempoolaccept([full_hex])[0]["allowed"], True)
        self.start_miner()
        try:
            op = shared(a, *send_q)
        finally:
            self.stop_miner()
        assert handed_off(op)
        assert_equal(op["kind"], "dissolve")
        h = self.wait_mined(op["txid"])
        assert_equal(node.getrawtransaction(op["txid"]), q_full)
        assert pro_tx not in {m["proTxHash"] for m in node.protx("list", "registered", True)}, pro_tx
        assert_equal({o["id"]: o for o in a.mn(h, "operations")}[op["id"]]["state"]["kind"], "confirmed")
        # Delisted: nothing left to prompt. A's own standby is held while the dissolution may be
        # reorged out (D23), then refused as unlisted.
        assert_equal(shared(a, "show", sid)["next"], "nothing")
        own_full = ("send-standby", pro_tx, "--share-index", "0", "--variant", "full")
        shared(a, *own_full, error="masternodes.update_pending")
        h = self.mine(6)  # RECOMMENDED_CONFIRMATIONS: buried
        shared(a, *own_full, error="masternodes.shared_too_early")

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

        self.log.info("4b. K2: S3's keys changed together, prepared by dashd, then by B")
        # m-16: the list one block above the registration.
        h = self.mine()
        fee_source = node.getnewaddress()
        node.sendtoaddress(fee_source, 1)
        h = self.mine()
        p_voting = node.getnewaddress()
        prep = node.protx("shared_update_registrar_prepare", pro_tx, "", p_voting, fee_source)
        p_sigs = node.protx("shared_sign", prep["tx"])["signatures"]
        assert_equal([s["shareIndex"] for s in p_sigs], [0])
        got = shared(b, "import", stdin=seal({"type": "dash-shared-mn-sigs", "version": 1, "network": "regtest",
                                              "kind": "registrar", "proTxHash": pro_tx, "tx": prep["tx"],
                                              "signatures": p_sigs}))
        k2 = got["session"]
        assert_equal((k2["kind"], k2["role"], k2["stage"]["kind"], k2["next"], k2["fee"], k2["debit"]),
                     ("key_change", "participant", "signing", "approve", None, None))
        assert_equal(k2["key_change"]["requested"]["voting_address"], p_voting)
        assert_equal([w["kind"] for w in k2["warnings"]], ["usable_by_funder"])
        reply = shared(b, "approve-maintenance", k2["id"])
        assert_equal((reply["message"]["name"], reply["session"]["stage"]["kind"]), ("Reply", "combined"))
        b_sigs = json.loads(reply["message"]["text"])["signatures"]
        assert_equal([s["shareIndex"] for s in b_sigs], [0, 1])
        shared(b, "send-maintenance", k2["id"], error="masternodes.shared_stage")
        combined = node.protx("shared_combine", prep["tx"], b_sigs, False)
        txid = node.sendrawtransaction(combined)
        h = self.mine()
        assert node.getrawtransaction(txid, True)["confirmations"] > 0
        assert_equal(self.core_info(pro_tx)[1]["votingAddress"], p_voting)
        assert_equal(shared(b, "show", k2["id"])["stage"]["kind"], "confirmed")

        b_operator = node.bls("generate")
        started = shared(b, "key-change", pro_tx, "--operator-key", b_operator["public"])
        k3 = started["session"]
        assert_equal((k3["role"], k3["next"], k3["base_keys"]["voting_address"]), ("coordinator", "copy", p_voting))
        b_request = json.loads(started["message"]["text"])
        assert_equal([s["shareIndex"] for s in b_request["signatures"]], [1])
        p_sig = node.protx("shared_sign", b_request["tx"])["signatures"]
        got = shared(b, "import", "--target", k3["id"], stdin=json.dumps(p_sig))
        assert_equal((got["changed"], got["session"]["stage"]["kind"], got["session"]["next"]),
                     (True, "combined", "send"))
        self.start_miner()
        try:
            sent = shared(b, "send-maintenance", k3["id"])
        finally:
            self.stop_miner()
        assert_equal(sent["kind"], "session")
        h = self.wait_mined(sent["session"]["txid"])
        st = self.core_info(pro_tx)[1]
        assert_equal((st["pubKeyOperator"], st["votingAddress"]), (b_operator["public"], p_voting))
        assert st["PoSeBanHeight"] > 0, st
        assert_equal(shared(b, "show", k3["id"])["stage"]["kind"], "confirmed")
        fee_source = node.getnewaddress()
        node.sendtoaddress(fee_source, 1)
        self.mine()
        node.protx("update_service", pro_tx, f"127.0.0.1:{p2p_port(18)}", b_operator["secret"], "", fee_source)
        h = self.mine()
        assert_equal(self.core_info(pro_tx)[1]["PoSeBanHeight"], -1)
        h = self.mine(6)  # buried (D105)

        self.log.info("5. T: S3's masternode dissolved together, prepared by dashd, then by B")
        # m-16: the list one block above the registration.
        h = self.mine()
        # dashd as P: P's share pays the default fee; B's approval moves none of its coins.
        prep = node.protx("shared_dissolve_prepare", pro_tx, 0)
        p_sigs = node.protx("shared_sign", prep["tx"])["signatures"]
        assert_equal([s["shareIndex"] for s in p_sigs], [0])
        request = seal({"type": "dash-shared-mn-sigs", "version": 1, "network": "regtest", "kind": "dissolve",
                        "proTxHash": pro_tx, "tx": prep["tx"], "signatures": p_sigs})
        got = shared(b, "import", stdin=request)
        t1 = got["session"]
        assert_equal((t1["kind"], t1["role"], t1["stage"]["kind"], t1["next"]),
                     ("dissolve_together", "participant", "signing", "approve"))
        assert_equal((t1["fee"], t1["debit"], t1["actor_fee"]), (100_000, 0, None))
        reply = shared(b, "approve-maintenance", t1["id"])
        assert_equal((reply["message"]["name"], reply["session"]["stage"]["kind"]), ("Reply", "combined"))
        b_sigs = json.loads(reply["message"]["text"])["signatures"]
        assert_equal([s["shareIndex"] for s in b_sigs], [0, 1])
        p_combined = node.protx("shared_combine", prep["tx"], b_sigs, False)
        assert_equal(node.testmempoolaccept([p_combined])[0]["allowed"], True)

        # D96's relay minimum is dashd's: with P's share the actor, B refuses dashd's request one duff
        # below it, and dashd's mempool refuses that request combined for its fee. PreChecks checks
        # the fee before the payload's signatures (validation.cpp:1058, :1103), so share 1's slot
        # holds P's signature; at the minimum the same bytes fail only on that signature.
        def dashd_request(fee):
            return node.protx("shared_dissolve_prepare", pro_tx, 0, fee)["tx"]

        def sealed(tx):
            return seal({"type": "dash-shared-mn-sigs", "version": 1, "network": "regtest", "kind": "dissolve",
                         "proTxHash": pro_tx, "tx": tx, "signatures": []})

        low = shared(b, "import", stdin=sealed(dashd_request(1)), error="invalid_argument")
        p_floor = int(low["message"].split("relay minimum of ")[1].split()[0])
        for fee, reason in ((p_floor - 1, "min relay fee not met"), (p_floor, "bad-prodis-sig")):
            tx = dashd_request(fee)
            if fee < p_floor:
                shared(b, "import", stdin=sealed(tx), error="invalid_argument")
            sig = node.protx("shared_sign", tx)["signatures"][0]["signature"]
            filled = node.protx("shared_combine", tx, [{"shareIndex": 0, "signature": sig},
                                                      {"shareIndex": 1, "signature": sig}], False)
            assert_equal(node.testmempoolaccept([filled])[0]["reject-reason"], reason)

        # B prepares with its own share the actor, at the relay minimum of the combined size.
        low = shared(b, "dissolve-together", pro_tx, "--actor", "1", "--fee", "1",
                     error="invalid_argument")
        floor = int(low["message"].split("relay minimum of ")[1].split()[0])
        shared(b, "dissolve-together", pro_tx, "--actor", "1", "--fee", str(floor - 1), error="invalid_argument")
        started = shared(b, "dissolve-together", pro_tx, "--actor", "1", "--fee", str(floor))
        t2 = started["session"]
        assert_equal((started["message"]["name"], t2["role"], t2["stage"]["kind"], t2["next"]),
                     ("Request", "coordinator", "signing", "copy"))
        assert_equal((t2["fee"], t2["debit"], t2["actor_fee"]), (floor, floor, floor))
        b_request = json.loads(started["message"]["text"])
        assert_equal([s["shareIndex"] for s in b_request["signatures"]], [1])
        # dashd signs B's request; B takes P's signature as a bare array for its session.
        p_sig = node.protx("shared_sign", b_request["tx"])["signatures"]
        got = shared(b, "import", "--target", t2["id"], stdin=json.dumps(p_sig))
        assert_equal((got["changed"], got["session"]["stage"]["kind"], got["session"]["next"]),
                     (True, "combined", "send"))
        want = node.protx("shared_combine", b_request["tx"], p_sig + b_request["signatures"], False)
        assert_equal(node.testmempoolaccept([want])[0]["allowed"], True)
        self.start_miner()
        try:
            sent = shared(b, "send-maintenance", t2["id"])
        finally:
            self.stop_miner()
        # A dissolution is sent as an operation (MG-04b D95, D99).
        assert_equal(sent["kind"], "operation")
        op = sent["operation"]
        assert handed_off(op)
        assert_equal((op["kind"], op["fee"]), ("dissolve", floor))
        h = self.wait_mined(op["txid"])
        assert_equal(node.getrawtransaction(op["txid"]), want)
        assert pro_tx not in {m["proTxHash"] for m in node.protx("list", "registered", True)}, pro_tx
        assert_equal({o["id"]: o for o in b.mn(h, "operations")}[op["id"]]["state"]["kind"], "confirmed")
        assert_equal(shared(b, "show", t2["id"])["stage"]["kind"], "confirmed")
        assert_equal(shared(b, "show", t1["id"])["stage"], {"kind": "dead", "reason": "delisted"})


if __name__ == "__main__":
    DwdShared().main()
