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

Refusals before anything is signed: the collateral already in use, a listed service, a listed
operator key, a version-3 service; and a wrong external signature.

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
from test_framework.util import assert_equal, p2p_port


def state(op):
    return op["state"]["kind"]


def handed_off(op):
    """`sent`, or already `confirmed`: dash-spv may see the block before dwcli returns."""
    assert state(op) in ("sent", "confirmed"), op
    return True


class DwdProtx(DwdMasternodes):
    def set_test_params(self):
        # The framework wants it defined here; the network is the masternodes suite's.
        super().set_test_params()

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

    def check_listed(self, pro_tx_hash, terms, collateral):
        """Core's list entry for `pro_tx_hash` carries the terms the wallet reported."""
        info, st = self.core_info(pro_tx_hash)
        assert_equal(f"{info['collateralHash']}-{info['collateralIndex']}", collateral)
        assert_equal(st["ownerAddress"], terms["owner_address"])
        assert_equal(st["votingAddress"], terms["voting_address"])
        assert_equal(st["payoutAddress"], terms["payout_address"])
        assert_equal(st["pubKeyOperator"], terms["operator_public_key"])
        assert_equal(round(info["operatorReward"] * 100), terms["operator_reward"])
        assert terms["service"] in json.dumps(st), (terms["service"], st)
        assert_equal(info["type"], "Regular")
        return info

    def run_test(self):
        node = self.nodes[0]
        dw = Dwcli(os.environ["DWCLI"], os.path.join(self.options.tmpdir, "dwcli"), p2p_port(0), self.log)
        evidence = os.environ.get("DWD_MG04_EVIDENCE")
        self.prepare_network()

        dw.run("init-vault")
        dw.run("import", "--birth-height", "0", stdin=PHRASE)
        funds = dw.run("receive", "--next").stdout.split()[1]
        existing_addr = dw.run("receive", "--next").stdout.split()[1]
        node.sendtoaddress(funds, COLLATERAL // COIN + 10)
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

        self.log.info("5. The wallet's view: every operation confirmed, every masternode its own")
        ops = {o["id"]: o for o in dw.mn(h, "operations")}
        assert_equal(len(ops), 3)
        assert all(state(o) == "confirmed" for o in ops.values()), ops
        overview = dw.mn(h, "list")
        mine = {r["pro_tx_hash"]: r for r in overview["masternodes"] if r["mine"]}
        assert {f["pro_tx_hash"], e["pro_tx_hash"], x["pro_tx_hash"]} <= set(mine)
        assert all(mine[f["pro_tx_hash"]]["roles"][k] for k in ("owner", "voting", "payout", "collateral"))
        assert mine[e["pro_tx_hash"]]["roles"]["collateral"] and not mine[e["pro_tx_hash"]]["roles"]["payout"]
        assert mine[x["pro_tx_hash"]]["roles"]["owner"] and not mine[x["pro_tx_hash"]]["roles"]["collateral"]
        listed = {m["proTxHash"] for m in node.protx("list", "registered", True)}
        assert {f["pro_tx_hash"], e["pro_tx_hash"], x["pro_tx_hash"]} <= listed

        if evidence:
            os.makedirs(evidence, exist_ok=True)
            with open(os.path.join(evidence, "protx-operations.json"), "w", encoding="utf8") as out:
                json.dump({"operations": list(ops.values()),
                           "core": {h_: node.protx("info", h_) for h_ in
                                    (f["pro_tx_hash"], e["pro_tx_hash"], x["pro_tx_hash"])}},
                          out, indent=1, default=str)
            shutil.copy(os.path.join(self.options.tmpdir, "test_framework.log"), evidence)
        self.log.info("ProTx suite passed")


if __name__ == "__main__":
    DwdProtx().main()
