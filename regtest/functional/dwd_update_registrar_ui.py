#!/usr/bin/env python3
# Copyright (c) 2026 The Dash Wallet Desktop developers
# Distributed under the MIT software license, see test_framework/COPYING.
"""dwd_update_registrar_ui.py — regtest suite `update_registrar_ui` (MG-10 S3 GL, `--part update`).

From the app's UI on regtest: change a masternode's payout and voting key with Change keys or
payout (a ProUpRegTx signed with the wallet's owner key), then judge the result on Core. This
suite builds the chain, starts `apps/desktop/e2e-tauri/regtest-update-registrar.ts` (tauri-driver
on the real app) and checks `protx info`.

Network: as `dwd_protx.py` (node0 controller/miner and the app's SPV peer, nodes 1..2 the
framework's static masternodes; real time; InstantSend, ChainLocks and DKG off).

Setup:
 - dwcli makes the vault and imports the BIP39 test vector (regtest coins only), so the owner
   and voting keys are known here (dwd_masternodes.provider_key);
 - dwcli registers F: `FundNew`, the operator key Core's `bls generate`, the owner and voting
   keys the keychain's first, the payout a new wallet address. The wallet holds F's ProRegTx, so
   the app reads Change keys or payout as `available{keychain}`.

The UI run: Masternodes ▸ F ▸ Change keys or payout…: the voting key the keychain's next, the
payout Core's address (another wallet's), passphrase, Send change → "Change sent".

Checks on Core, after the UI run:
 1. the update's transaction is a ProUpRegTx (type 3) for F, version 2, in a block;
 2. `protx info F`: the payout is Core's address, the voting address is the keychain's voting
    key 1 (derived here, not by the engine), and the owner and operator keys are unchanged.

Run (DW_APP, TAURI_DRIVER and DISPLAY as for scripts/e2e-regtest-update-registrar.sh):
  DWCLI=<target>/debug/dwcli DW_APP=<target>/debug/dash-wallet TAURI_DRIVER=<tauri-driver> \\
  DWD_PYTHON=<venv>/bin/python DASHCORE_DIR=<dashcore> regtest/functional/run.sh dwd_update_registrar_ui.py
"""

import contextlib
import json
import os
import signal
import subprocess

from dwd_masternodes import COLLATERAL, COIN, OWNER, PASSPHRASE, PHRASE, VOTING, Dwcli, address, provider_key
from dwd_protx import DwdProtx, handed_off
from test_framework.util import assert_equal, p2p_port, rpc_url

HERE = os.path.dirname(os.path.abspath(__file__))
DESKTOP = os.path.join(HERE, "..", "..", "apps", "desktop")
UI_TIMEOUT = 3600


def source_head():
    """The commit under test and whether the tree differs from it, for result.json."""
    git = lambda *a: subprocess.run(["git", "-C", HERE, *a], capture_output=True, text=True, check=True).stdout.strip()
    return {"head": git("rev-parse", "HEAD"), "dirty": git("status", "--porcelain") != ""}


class DwdUpdateRegistrarUi(DwdProtx):
    def set_test_params(self):
        # The framework wants it defined here; the network is dwd_protx.py's.
        super().set_test_params()

    def skip_test_if_missing_module(self):
        super().skip_test_if_missing_module()
        for name in ("DW_APP", "TAURI_DRIVER", "DISPLAY"):
            if not os.environ.get(name):
                raise SystemExit(f"set {name}")

    def run_test(self):
        node = self.nodes[0]
        dw = Dwcli(os.environ["DWCLI"], os.path.join(self.options.tmpdir, "dwcli"), p2p_port(0), self.log)
        self.prepare_network()
        self.fund_node(COLLATERAL + 100 * COIN)

        self.log.info("Setup: the app's wallet registers F (FundNew, Core's operator key)")
        dw.run("init-vault")
        wallet = dw.run("import", "--birth-height", "0", stdin=PHRASE).stdout.split()[-1]
        funds = dw.run("receive", "--next").stdout.split()[1]
        node.sendtoaddress(funds, COLLATERAL // COIN + 10)
        h = self.mine()
        operator = node.bls("generate")["public"]
        self.start_miner()
        try:
            f = dw.mn(h, "register", "--collateral", "fund-new", "--service", f"127.0.0.1:{p2p_port(14)}",
                      "--operator-key", operator)
        finally:
            self.stop_miner()
        assert handed_off(f)
        self.wait_until(lambda: node.getrawtransaction(f["txid"], True).get("confirmations", 0) > 0, timeout=60)
        self.mine()
        hash_f = f["pro_tx_hash"]
        info, before = self.core_info(hash_f)
        assert_equal(before["ownerAddress"], address(provider_key(OWNER, 0)))
        assert_equal(before["votingAddress"], address(provider_key(VOTING, 0)))
        payout = node.getnewaddress()
        assert before["payoutAddress"] != payout

        source = source_head()
        self.log.info(f"The UI run on {source['head']}{' (dirty tree)' if source['dirty'] else ''}: F's payout to Core, voting key 1")
        out = os.environ.get("DW_E2E_OUT", os.path.join(self.options.tmpdir, "ui"))
        os.makedirs(out, exist_ok=True)
        env = dict(os.environ, DW_E2E_OUT=out, DW_E2E_DATADIR=os.path.join(dw.datadir, "data"), DW_E2E_PASSPHRASE=PASSPHRASE,
                   DW_E2E_WALLET=wallet, DW_E2E_SPV_PEER=f"127.0.0.1:{p2p_port(0)}",
                   DW_E2E_RPC=rpc_url(node.datadir, 0, self.chain), DW_E2E_MINER=node.getnewaddress(),
                   DW_E2E_MASTERNODE=hash_f, DW_E2E_PAYOUT=payout)
        env.setdefault("RUST_LOG", "warn,dw_engine::masternodes=debug")
        with open(os.path.join(out, "app.log"), "w", encoding="utf8") as log:
            # Its own session, so a timeout also ends tauri-driver, WebKitWebDriver and the app.
            p = subprocess.Popen(["node", "e2e-tauri/regtest-update-registrar.ts"], cwd=DESKTOP, env=env,
                                 stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                p.wait(timeout=UI_TIMEOUT)
            except subprocess.TimeoutExpired:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(p.pid, signal.SIGKILL)
                p.wait()
                raise
        with open(os.path.join(out, "app.log"), encoding="utf8") as log:
            self.log.info("".join(line for line in log if line.startswith("regtest-update-registrar:")))
        assert_equal(p.returncode, 0)
        with open(os.path.join(out, "result.json"), encoding="utf8") as fh:
            result = json.load(fh)
        txid = result["evidence"]["txid"]

        self.log.info("1. The app's transaction is F's ProUpRegTx, version 2, in a block")
        self.wait_until(lambda: node.getrawtransaction(txid, True).get("confirmations", 0) > 0, timeout=120)
        decoded = node.getrawtransaction(txid, True)
        assert_equal((decoded["version"], decoded["type"]), (3, 3))
        assert_equal(decoded["proUpRegTx"]["proTxHash"], hash_f)
        assert_equal(decoded["proUpRegTx"]["version"], 2)

        self.log.info("2. protx info: the payout is Core's address, the voting key the keychain's key 1")
        _, after = self.core_info(hash_f)
        assert_equal(after["payoutAddress"], payout)
        assert_equal(after["votingAddress"], address(provider_key(VOTING, 1)))
        assert_equal(after["ownerAddress"], before["ownerAddress"])
        assert_equal(after["pubKeyOperator"], before["pubKeyOperator"])
        result["evidence"]["core"] = {"before": before, "after": after}
        result["source"] = source
        with open(os.path.join(out, "result.json"), "w", encoding="utf8") as fh:
            json.dump(result, fh, indent=2)
        self.log.info("update_registrar_ui suite passed")


if __name__ == "__main__":
    DwdUpdateRegistrarUi().main()
