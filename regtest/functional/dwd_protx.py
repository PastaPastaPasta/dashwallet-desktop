#!/usr/bin/env python3
# Copyright (c) 2026 The Dash Wallet Desktop developers
# Distributed under the MIT software license, see test_framework/COPYING.
"""dwd_protx.py — regtest suite `protx` (MG-04 T1).

dw-engine's ProTx owner flows (`dwcli mn register` / `register-submit`) against Core: every
registration the wallet signs must be accepted by Core's mempool and blocks, and Core's
deterministic masternode list must then show the keys and payee the wallet chose. The owner and
voting keys are derived here from the mnemonic (BIP39 + DIP9 `m/9'/1'/3'/{1,2}'/i`), so the keys
are checked against an oracle that isn't the engine's own derivation. Payload bytes are pinned
against dashd separately (`dw-protx/tests/oracle_protx_owner.rs`).

Network: as `dwd_masternodes.py`: node0 controller/miner and dwcli's SPV peer (BIP157 filters),
nodes 1..2 the framework's static masternodes; real time; InstantSend, ChainLocks and DKG off.

Registrations, all version 2 (MG-04 D1):
  F  `FundNew`: the collateral is an output of the ProRegTx, funded by the wallet; the operator
     key given (Core's `bls generate`), owner and voting keys the keychain's first, the payout a
     new wallet address.
  E  `ExistingUtxo`: a wallet coin of exactly 1,000 DASH; its key signs Core's message; every
     key the keychain's next unused; the payout another wallet's address (Core's); a reward.
  X  `External`: Core's coin of 1,000 DASH; the wallet prepares, Core's `signmessage` signs the
     message, the wallet submits.
  W  an evonode, `ExistingUtxo`: a wallet coin of exactly 4,000 DASH, the node id and ports given.
  V  an evonode, `FundNew` (4,000 DASH): the keychain's platform node id, the network's default
     Platform ports.

ProUpRegTx of F, each signed by its keychain owner key (MG-04 slice 4):
  U1 the voting key changed (keychain index 7), the operator key and payout kept;
  U2 the payout changed to a new wallet address, the keys kept from the wallet's list;
  U3 the operator key changed to one dashd generates: Core PoSe-bans F; dashd, the operator,
     sends `protx update_service` with the new secret, and Core revives it.

Refusals before anything is signed: the collateral already in use, a listed service, a listed
operator key, a version-3 service; and a wrong external signature. For the evonode: a regular
collateral, Platform ports equal to the Core port, and a listed platform node id. For an update:
a hash not listed, a listed masternode without an owner key the wallet has, the owner address
as payee, and another masternode's operator key.

Run (DWCLI = a built dwcli):
  DWCLI=<target>/debug/dwcli DWD_PYTHON=<venv>/bin/python DASHCORE_DIR=<dashcore> \\
      regtest/functional/run.sh dwd_protx.py
"""

import json
import os
import shutil
import time

from dwd_masternodes import (COIN, COLLATERAL, OWNER, PHRASE, SPORK_OFF, VOTING, DwdMasternodes, Dwcli,
                             address, provider_key)

EVO_COLLATERAL = 4_000 * COIN
# chainparams.cpp:873-874 (rc.3): regtest's nDefaultPlatformP2PPort / nDefaultPlatformHTTPPort.
PLATFORM_PORTS = (22_200, 22_201)
from test_framework.util import assert_equal, p2p_port, softfork_active


def state(op):
    return op["state"]["kind"]


def handed_off(op):
    """`sent`, or already `confirmed`: dash-spv may see the block before dwcli returns."""
    assert state(op) in ("sent", "confirmed"), op
    return True


# DWD_MG04_V24=1: the second run (MG-04 §7, D1), with v24 active from early in the chain, as Core's
# feature tests set it (rc.3 test/functional/feature_protx_version.py:59): plain BIP9 (no EHF), a
# 10-block window. dashd's own RPCs then build version-3 payloads; the wallet's stay version 2.
V24 = os.environ.get("DWD_MG04_V24") == "1"
V24_PARAMS = "-vbparams=v24:0:999999999999:0:10:8:6:5:0"


class DwdProtx(DwdMasternodes):
    def set_test_params(self):
        # The framework wants it defined here; the network is the masternodes suite's.
        super().set_test_params()
        if V24:
            self.extra_args = [args + [V24_PARAMS] for args in self.extra_args]

    def prepare_network(self):
        """As `dwd_masternodes.py`: InstantSend and ChainLocks off, then whole DIP24 cycles until
        Core answers dash-spv's QRInfo."""
        node = self.nodes[0]
        time.sleep(1.5)  # peers keep a spork signed in the same second as the one they have
        for spork in ("SPORK_2_INSTANTSEND_ENABLED", "SPORK_19_CHAINLOCKS_ENABLED"):
            node.sporkupdate(spork, SPORK_OFF)
        self.wait_for_sporks_same()
        self.mine_until_qrinfo()

    def core_info(self, pro_tx_hash):
        info = self.nodes[0].protx("info", pro_tx_hash)
        return info, info["state"]

    def check_listed(self, pro_tx_hash, terms, collateral, kind="Regular"):
        """Core's list entry for `pro_tx_hash` carries the terms the wallet reported."""
        info, st = self.core_info(pro_tx_hash)
        assert_equal(f"{info['collateralHash']}-{info['collateralIndex']}", collateral)
        assert_equal(st["ownerAddress"], terms["owner_address"])
        assert_equal(st["votingAddress"], terms["voting_address"])
        assert_equal(st["payoutAddress"], terms["payout_address"])
        assert_equal(st["pubKeyOperator"], terms["operator_public_key"])
        assert_equal(round(info["operatorReward"] * 100), terms["operator_reward"])
        assert terms["service"] in json.dumps(st), (terms["service"], st)
        assert_equal(info["type"], kind)
        platform = terms.get("platform")
        if kind == "Evo":
            # A version-2 state's ports, at the Core address (evo/dmnstate.cpp:146-168, rc.3).
            port = lambda purpose: int(st["addresses"][purpose][0].rsplit(":", 1)[1])
            assert_equal((st["platformNodeID"], port("platform_p2p"), port("platform_https")),
                         (platform["node_id"], platform["p2p_port"], platform["http_port"]))
        else:
            assert platform is None and "platformNodeID" not in st, (terms, st)
        return info

    def fund_node(self, duffs):
        """Mines until node0 can spend `duffs` (each coinbase matures after 100 blocks)."""
        node = self.nodes[0]
        for _ in range(30):
            if node.getbalance() * COIN >= duffs:
                return self.tip()
            self.mine(20)
        raise AssertionError(f"node0 holds {node.getbalance()} DASH, short of {duffs / COIN}")

    def run_test(self):
        node = self.nodes[0]
        dw = Dwcli(os.environ["DWCLI"], os.path.join(self.options.tmpdir, "dwcli"), p2p_port(0), self.log)
        evidence = os.environ.get("DWD_MG04_EVIDENCE")
        if V24:
            for _ in range(30):
                if softfork_active(node, "v24"):
                    break
                self.mine(10)
            assert softfork_active(node, "v24"), node.getdeploymentinfo()["deployments"]["v24"]
            self.log.info(f"v24 active at height {self.tip()}")
        self.prepare_network()
        self.fund_node(2 * EVO_COLLATERAL + 3 * COLLATERAL + 100 * COIN)

        dw.run("init-vault")
        dw.run("import", "--birth-height", "0", stdin=PHRASE)
        funds = dw.run("receive", "--next").stdout.split()[1]
        existing_addr = dw.run("receive", "--next").stdout.split()[1]
        evo_addr = dw.run("receive", "--next").stdout.split()[1]
        node.sendtoaddress(funds, COLLATERAL // COIN + 10)
        node.sendtoaddress(funds, EVO_COLLATERAL // COIN + 500)
        node.sendtoaddress(funds, 5)
        existing = node.sendtoaddress(existing_addr, COLLATERAL // COIN)
        external_addr = node.getnewaddress()
        external = node.sendtoaddress(external_addr, COLLATERAL // COIN)
        h = self.mine()
        existing = f"{existing}-{self.outpoint_of(existing, existing_addr, COLLATERAL)}"
        external = f"{external}-{self.outpoint_of(external, external_addr, COLLATERAL)}"
        listed_service = f"127.0.0.1:{self.mninfo[0].nodePort}"
        listed_operator = node.protx("info", self.mninfo[0].proTxHash)["state"]["pubKeyOperator"]

        self.log.info("1. Refusals before anything is signed")
        def refused(code, *args):
            return dw.mn(h, "register", "--preview", *args, error=code)
        refused("masternodes.protx_refused", "--collateral", "fund-new", "--service", listed_service)
        refused("masternodes.protx_refused", "--collateral", "fund-new", "--service", "127.0.0.1:30001",
                "--operator-key", listed_operator)
        e = refused("masternodes.unsupported", "--collateral", "fund-new", "--service", "[::1]:30001")
        assert "protx_v3" in e["message"] and "service" in e["message"], e
        refused("invalid_argument", "--collateral", "fund-new", "--service", "127.0.0.1:30001",
                "--operator-reward", "10001")
        refused("masternodes.collateral_unavailable", "--collateral", external, "--service", "127.0.0.1:30001")
        refused("masternodes.collateral_unavailable", "--evonode", "--collateral", existing,
                "--service", "127.0.0.1:30001")
        refused("masternodes.protx_refused", "--evonode", "--collateral", "fund-new",
                "--service", "127.0.0.1:30001", "--platform-p2p-port", "30001")

        self.log.info("2. F: FundNew, the operator key given")
        operator_f = node.bls("generate")["public"]
        service_f = f"127.0.0.1:{p2p_port(14)}"
        preview = dw.mn(h, "register", "--preview", "--collateral", "fund-new", "--service", service_f,
                        "--operator-key", operator_f)
        assert_equal(preview["collateral_amount"], COLLATERAL)
        assert 0 < preview["fee"] == preview["total_debit"] < 100_000, preview
        self.start_miner()
        try:
            f = dw.mn(h, "register", "--collateral", "fund-new", "--service", service_f, "--operator-key", operator_f)
        finally:
            self.stop_miner()
        assert handed_off(f)
        assert_equal((f["kind"], f["pro_tx_hash"], f["fee"]), ("registration", f["txid"], preview["fee"]))
        self.wait_until(lambda: node.getrawtransaction(f["txid"], True).get("confirmations", 0) > 0, timeout=60)
        h = self.mine()
        terms = f["terms"]
        assert_equal((terms["owner_address"], terms["owner_index"]), (address(provider_key(OWNER, 0)), 0))
        assert_equal((terms["voting_address"], terms["voting_index"]), (address(provider_key(VOTING, 0)), 0))
        assert_equal((terms["operator_public_key"], terms["operator_index"]), (operator_f, None))
        assert_equal((terms["version"], terms["collateral"]), (2, "fund_new"))
        decoded = node.getrawtransaction(f["txid"], True)
        assert_equal((decoded["version"], decoded["type"]), (3, 1))
        assert_equal(decoded["proRegTx"]["version"], 2)
        info = self.check_listed(f["pro_tx_hash"], terms, terms["collateral_outpoint"])
        assert_equal(info["collateralHash"], f["pro_tx_hash"])

        self.log.info("3. E: ExistingUtxo, every key the keychain's next unused, a reward")
        payee_e = node.getnewaddress()
        self.start_miner()
        try:
            e = dw.mn(h, "register", "--collateral", existing, "--service", f"127.0.0.1:{p2p_port(15)}",
                      "--payout", payee_e, "--operator-reward", "1250")
        finally:
            self.stop_miner()
        assert handed_off(e)
        self.wait_until(lambda: node.getrawtransaction(e["txid"], True).get("confirmations", 0) > 0, timeout=60)
        h = self.mine()
        terms = e["terms"]
        assert_equal((terms["owner_index"], terms["voting_index"], terms["operator_index"]), (1, 1, 0))
        assert_equal(terms["owner_address"], address(provider_key(OWNER, 1)))
        assert_equal(terms["voting_address"], address(provider_key(VOTING, 1)))
        assert_equal(terms["collateral_outpoint"], existing)
        assert_equal(terms["payout_address"], payee_e)
        self.check_listed(e["pro_tx_hash"], terms, existing)
        # The collateral is in use now: refused again, and kept out of coin selection.
        refused("masternodes.collateral_unavailable", "--collateral", existing, "--service", "127.0.0.1:30002")

        self.log.info("4. X: External, signed by Core's signmessage")
        prepared = dw.mn(h, "register", "--collateral", external, "--external-address", external_addr,
                         "--service", f"127.0.0.1:{p2p_port(16)}")
        assert_equal(state(prepared), "needs_signature")
        ask = prepared["sign_request"]
        assert_equal(ask["address"], external_addr)
        wrong = node.signmessage(node.getnewaddress(), ask["message"])
        dw.mn(h, "register-submit", prepared["id"], "--signature", wrong, error="masternodes.signature_mismatch")
        signature = node.signmessage(external_addr, ask["message"])
        self.start_miner()
        try:
            x = dw.mn(h, "register-submit", prepared["id"], "--signature", signature)
        finally:
            self.stop_miner()
        assert handed_off(x)
        self.wait_until(lambda: node.getrawtransaction(x["txid"], True).get("confirmations", 0) > 0, timeout=60)
        h = self.mine()
        self.check_listed(x["pro_tx_hash"], x["terms"], external)
        assert_equal(x["terms"]["operator_index"], 1)

        # The coin arrives now and W runs before V: until it is registered, coin selection may spend
        # a collateral-sized coin, largest first, as Core's does (D37).
        self.log.info("5. W: an evonode, ExistingUtxo of 4,000 DASH, the node id and ports given")
        evo_existing = node.sendtoaddress(evo_addr, EVO_COLLATERAL // COIN)
        h = self.mine()
        evo_existing = f"{evo_existing}-{self.outpoint_of(evo_existing, evo_addr, EVO_COLLATERAL)}"
        node_w = "0123456789abcdef0123456789abcdef01234567"  # asymmetric: Core shows uint160 reversed
        self.start_miner()
        try:
            w = dw.mn(h, "register", "--evonode", "--collateral", evo_existing,
                      "--service", f"127.0.0.1:{p2p_port(18)}", "--platform-node-id", node_w,
                      "--platform-p2p-port", "26100", "--platform-http-port", "26101")
        finally:
            self.stop_miner()
        assert handed_off(w)
        self.wait_until(lambda: node.getrawtransaction(w["txid"], True).get("confirmations", 0) > 0, timeout=60)
        h = self.mine()
        assert_equal(w["terms"]["platform"], {"node_id": node_w, "p2p_port": 26100, "http_port": 26101})
        assert_equal(w["terms"]["platform_node_index"], None)
        self.check_listed(w["pro_tx_hash"], w["terms"], evo_existing, kind="Evo")

        self.log.info("6. V: an evonode, FundNew, the keychain's node id and the default ports")
        service_v = f"127.0.0.1:{p2p_port(17)}"
        preview = dw.mn(h, "register", "--preview", "--evonode", "--collateral", "fund-new",
                        "--service", service_v)
        assert_equal(preview["collateral_amount"], EVO_COLLATERAL)
        assert_equal(preview["terms"]["platform_node_index"], 0)
        node_v = {"index": 0, "node_id": preview["terms"]["platform"]["node_id"]}
        self.start_miner()
        try:
            v = dw.mn(h, "register", "--evonode", "--collateral", "fund-new", "--service", service_v)
        finally:
            self.stop_miner()
        assert handed_off(v)
        self.wait_until(lambda: node.getrawtransaction(v["txid"], True).get("confirmations", 0) > 0, timeout=60)
        h = self.mine()
        terms = v["terms"]
        assert_equal((terms["masternode_type"], terms["version"]), ("evonode", 2))
        assert_equal(terms["platform"], {"node_id": node_v["node_id"], "p2p_port": PLATFORM_PORTS[0],
                                         "http_port": PLATFORM_PORTS[1]})
        assert_equal(terms["platform_node_index"], node_v["index"])
        decoded = node.getrawtransaction(v["txid"], True)
        assert_equal((decoded["proRegTx"]["version"], decoded["proRegTx"]["type"]), (2, 1))
        self.check_listed(v["pro_tx_hash"], terms, terms["collateral_outpoint"], kind="Evo")
        # Its node id is listed now: given again, it is Core's duplicate.
        refused("masternodes.protx_refused", "--evonode", "--collateral", "fund-new",
                "--service", "127.0.0.1:30003", "--platform-node-id", node_v["node_id"])

        self.log.info("7. The wallet's view: every operation confirmed, every masternode its own")
        ops = {o["id"]: o for o in dw.mn(h, "operations")}
        assert_equal(len(ops), 5)
        assert all(state(o) == "confirmed" for o in ops.values()), ops
        overview = dw.mn(h, "list")
        mine = {r["pro_tx_hash"]: r for r in overview["masternodes"] if r["mine"]}
        ours = {f["pro_tx_hash"], e["pro_tx_hash"], x["pro_tx_hash"], v["pro_tx_hash"], w["pro_tx_hash"]}
        assert ours <= set(mine)
        assert all(mine[f["pro_tx_hash"]]["roles"][k] for k in ("owner", "voting", "payout", "collateral"))
        assert mine[e["pro_tx_hash"]]["roles"]["collateral"] and not mine[e["pro_tx_hash"]]["roles"]["payout"]
        assert mine[x["pro_tx_hash"]]["roles"]["owner"] and not mine[x["pro_tx_hash"]]["roles"]["collateral"]
        assert all(mine[h_]["roles"][k] for h_ in (v["pro_tx_hash"], w["pro_tx_hash"])
                   for k in ("owner", "voting", "collateral")), mine
        listed = {m["proTxHash"] for m in node.protx("list", "registered", True)}
        assert ours <= listed

        hash_f = f["pro_tx_hash"]
        payout_f = f["terms"]["payout_address"]

        def mn_unless_pending(*args):
            """`dw.mn` on F, or None while D43 and D49 refuse an update: the wallet's history shows
            an update in a block the list hasn't reached yet (masternodes.update_pending)."""
            p = dw.run("mn", "--sync-height", str(h), "--sync-timeout", "600", "update-registrar",
                       hash_f, *args, check=False)
            line = json.loads(p.stdout.strip().splitlines()[-1])
            if not line["ok"] and line["error"]["code"] == "masternodes.update_pending":
                return None
            assert line["ok"] and p.returncode == 0, (line, p.stderr[-2000:])
            return line["result"]

        def kept_preview(*args):
            """The preview once the wallet's list shows Core's current keys of F: a kept key is
            the list's (D10, D43), and the list may trail the tip by a block."""
            want = self.core_info(hash_f)[1]
            kept = [(field, want[core]) for field, core, flag in
                    (("voting_address", "votingAddress", "--voting"),
                     ("operator_public_key", "pubKeyOperator", "--operator"))
                    if not any(a.startswith(flag) for a in args)]
            p = None
            for _ in range(30):
                p = mn_unless_pending("--preview", *args)
                if p is not None and all(p["terms"][field] == value for field, value in kept):
                    return p
                time.sleep(2)
            raise AssertionError(f"the wallet's list never showed F's keys: {p and p['terms']}")

        def update(*args):
            self.start_miner()
            try:
                for _ in range(30):
                    u = mn_unless_pending(*args)
                    if u is not None:
                        break
                    time.sleep(2)
                else:
                    raise AssertionError("the update stayed refused as update_pending")
            finally:
                self.stop_miner()
            assert handed_off(u)
            assert_equal((u["kind"], u["pro_tx_hash"]), ("update_registrar", hash_f))
            self.wait_until(lambda: node.getrawtransaction(u["txid"], True).get("confirmations", 0) > 0,
                            timeout=60)
            decoded = node.getrawtransaction(u["txid"], True)
            assert_equal((decoded["version"], decoded["type"], decoded["proUpRegTx"]["version"]), (3, 3, 2))
            return u

        self.log.info("8. U1: F's voting key changed, signed by its keychain owner key")
        refused_u = lambda code, *args: dw.mn(h, "update-registrar", *args, error=code)
        refused_u("masternodes.not_found", "ab" * 32, "--payout", payout_f)
        # Under v24 Core's own masternodes hold a version-3 state (extended addresses), which the
        # wallet refuses before it looks for an owner key (D46).
        refused_u("masternodes.unsupported" if V24 else "masternodes.no_owner_key", self.mninfo[0].proTxHash,
                  "--payout", payout_f)
        refused_u("masternodes.protx_refused", hash_f, "--payout", f["terms"]["owner_address"])
        refused_u("masternodes.protx_refused", hash_f, "--payout", payout_f, "--operator-key", listed_operator)
        p = kept_preview("--voting-index", "7", "--payout", payout_f)
        assert_equal(p["owner_key"], {"kind": "keychain", "index": 0})
        assert_equal((p["voting_changes"], p["operator_changes"], p["payout_changes"], p["pose_bans"]),
                     (True, False, False, False))
        assert 0 < p["fee"] == p["total_debit"] < 100_000, p
        u1 = update("--voting-index", "7", "--payout", payout_f)
        assert_equal(u1["fee"], p["fee"])
        h = self.mine()
        st = self.core_info(hash_f)[1]
        assert_equal(st["votingAddress"], address(provider_key(VOTING, 7)))
        assert_equal((st["ownerAddress"], st["payoutAddress"], st["pubKeyOperator"]),
                     (f["terms"]["owner_address"], payout_f, operator_f))

        self.log.info("9. U2: F's payout changed to a new wallet address, the keys kept")
        p = kept_preview("--payout", "new")
        assert_equal((p["voting_changes"], p["operator_changes"], p["payout_changes"]), (False, False, True))
        u2 = update("--payout", "new")
        h = self.mine()
        st = self.core_info(hash_f)[1]
        assert_equal(st["payoutAddress"], u2["terms"]["payout_address"])
        assert_equal((st["votingAddress"], st["pubKeyOperator"]),
                     (address(provider_key(VOTING, 7)), operator_f))
        payout_f = u2["terms"]["payout_address"]

        self.log.info("10. U3: F's operator key changed to one dashd generates; dashd's operator revives it")
        new_operator = node.bls("generate")
        p = kept_preview("--operator-key", new_operator["public"], "--payout", payout_f)
        assert_equal((p["operator_changes"], p["pose_bans"], p["voting_changes"], p["payout_changes"]),
                     (True, True, False, False))
        u3 = update("--operator-key", new_operator["public"], "--payout", payout_f)
        h = self.mine()
        st = self.core_info(hash_f)[1]
        assert_equal(st["pubKeyOperator"], new_operator["public"])
        assert st["PoSeBanHeight"] > 0, st
        assert_equal((st["votingAddress"], st["payoutAddress"]), (address(provider_key(VOTING, 7)), payout_f))

        def operations(height):
            ops = {o["id"]: o for o in dw.mn(height, "operations")}
            assert_equal(len(ops), 8)
            assert all(state(o) == "confirmed" for o in ops.values()), ops
            assert_equal(sorted(o["kind"] for o in ops.values()), ["registration"] * 5 + ["update_registrar"] * 3)
            return ops

        if V24:
            # D48: the wallet's dashcore pin can't decode the version-3 ProUpServTx dashd builds
            # below, so a block with it that the wallet's filter matches stalls the wallet's sync.
            # The wallet's part ends here; the revive is dashd's alone.
            ops = operations(h)
        # The operator (dashd, holding the new secret) restores the service, and with it the
        # masternode; the wallet signs nothing as operator.
        fee_source = node.getnewaddress()
        node.sendtoaddress(fee_source, 1)
        self.mine()
        revive = node.protx("update_service", hash_f, service_f, new_operator["secret"], "", fee_source)
        h = self.mine()
        # dashd builds its own payloads at the tip's version (D1): 3 once v24 is active.
        assert_equal(node.getrawtransaction(revive, True)["proUpServTx"]["version"], 3 if V24 else 2)
        st = self.core_info(hash_f)[1]
        assert_equal(st["PoSeBanHeight"], -1)
        assert st["PoSeRevivedHeight"] > 0, st
        assert service_f in json.dumps(st["addresses"]), st

        if not V24:
            ops = operations(h)

        if evidence:
            os.makedirs(evidence, exist_ok=True)
            with open(os.path.join(evidence, "protx-operations.json"), "w", encoding="utf8") as out:
                json.dump({"operations": list(ops.values()),
                           "core": {h_: node.protx("info", h_) for h_ in sorted(ours)}},
                          out, indent=1, default=str)
            shutil.copy(os.path.join(self.options.tmpdir, "test_framework.log"), evidence)
        self.log.info("ProTx suite passed")


if __name__ == "__main__":
    DwdProtx().main()
