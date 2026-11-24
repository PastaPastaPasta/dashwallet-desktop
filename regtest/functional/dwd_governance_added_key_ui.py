#!/usr/bin/env python3
# Copyright (c) 2026 The Dash Wallet Desktop developers
# Distributed under the MIT software license, see test_framework/COPYING.
"""dwd_governance_added_key_ui.py — regtest suite `governance_added_key_ui`.

From the app's UI on regtest, track a masternode the wallet did not derive, add its voting key
and vote with it: this suite builds the chain, starts
`apps/desktop/e2e-tauri/regtest-vote-added-key.ts` (tauri-driver on the real app) and judges the
outcome on Core.

Network, sporks, budget and the app's wallet as in dwd_governance_ui.py, except that the wallet
holds no coin and no masternode votes with one of its keys.

Setup:
 - Core generates a fresh voting key in a legacy wallet of node0 (`getnewaddress`,
   `dumpprivkey`); `protx update_registrar` makes it masternode 2's voting key. The key is a
   regtest key made for this run; it reaches the UI script by environment only;
 - Core submits proposal `dwd_ak_p0` to vote on.

Checks, after the UI run:
 1. every node holds a funding Yes from masternode 2's collateral on dwd_ak_p0, and Core's Yes
    count went from 0 to 1;
 2. neither the run's output (logs, result.json, screenshots) nor the app's data directory holds
    the key in plain text (WIF, hex or raw bytes).

Run (DW_APP, TAURI_DRIVER and DISPLAY as for scripts/e2e-regtest-vote-added-key.sh):
  DWCLI=<target>/debug/dwcli DW_APP=<target>/debug/dash-wallet TAURI_DRIVER=<tauri-driver> \\
  DWD_PYTHON=<venv>/bin/python DASHCORE_DIR=<dashcore> regtest/functional/run.sh dwd_governance_added_key_ui.py
"""

import contextlib
import json
import os
import signal
import subprocess
import time
from decimal import Decimal

from dwd_governance_ui import DESKTOP, FIRST_SUPERBLOCK, MATURITY_WINDOW, PASSPHRASE, PHRASE, SPORK_OFF, Dwcli, DwdGovernanceUi, source_head
from test_framework.address import base58_to_byte
from test_framework.governance import prepare_object
from test_framework.messages import uint256_to_string
from test_framework.util import assert_equal, p2p_port, rpc_url

PROPOSAL = "dwd_ak_p0"
UI_TIMEOUT = 3600


def leaks(root, secrets):
    """Files below `root` holding any of `secrets` (bytes)."""
    found = []
    for d, _, names in os.walk(root):
        for name in names:
            p = os.path.join(d, name)
            if os.path.islink(p) or not os.path.isfile(p):
                continue
            with open(p, "rb") as f:
                data = f.read()
            found += [p for s in secrets if s in data]
    return found


class DwdGovernanceAddedKeyUi(DwdGovernanceUi):
    def run_test(self):
        node = self.nodes[0]
        time.sleep(1.5)  # peers keep a spork signed in the same second as the one they have
        for spork in ("SPORK_2_INSTANTSEND_ENABLED", "SPORK_19_CHAINLOCKS_ENABLED"):
            node.sporkupdate(spork, SPORK_OFF)
        self.wait_for_sporks_same()

        self.log.info("Setup: the app's wallet; Core gives masternode 2 a voting key of its own")
        dwcli = Dwcli(os.environ["DWCLI"], os.path.join(self.options.tmpdir, "dwcli"), p2p_port(0), self.log)
        dwcli.run("init-vault")
        wallet = " ".join(dwcli.run("import", "--birth-height", "0", stdin=PHRASE)).split()[-1]
        while node.getbalance() < 20:
            self.generate(node, 10)
        node.createwallet(wallet_name="voting", descriptors=False)
        keys = node.get_wallet_rpc("voting")
        voting = keys.getnewaddress()
        wif = keys.dumpprivkey(voting)
        node.unloadwallet("voting")  # the calls below go to the default wallet again
        payload, _ = base58_to_byte(wif)
        secret = payload[:32]
        assert_equal(len(payload), 33)  # a compressed WIF
        voter = self.mninfo[1]
        fee_source = node.getnewaddress()
        node.sendtoaddress(fee_source, 1)
        self.generate(node, 1)
        node.protx("update_registrar", voter.proTxHash, "", voting, "", fee_source)
        self.generate(node, 1)
        info = node.protx("info", voter.proTxHash)
        assert_equal(info["state"]["votingAddress"], voting)
        collateral = f"{info['collateralHash']}-{info['collateralIndex']}"

        self.log.info(f"Mine to 30 blocks before the first superblock's maturity window; submit {PROPOSAL}")
        self.mine_to(FIRST_SUPERBLOCK - MATURITY_WINDOW - 30)
        now = int(time.time())
        prepared = prepare_object(node, 1, uint256_to_string(0), now, 1, PROPOSAL, Decimal("1.5"), node.getnewaddress())
        self.generate(node, 6)
        p0 = node.gobject("submit", "0", 1, now, prepared["hex"], prepared["collateralHash"])
        for n in self.nodes:
            self.wait_until(lambda: p0 in n.gobject("list", "all", "proposals"), timeout=30)
        assert_equal(self.yes_count(node, p0), 0)

        source = source_head()
        self.log.info(f"The UI run on {source['head']}{' (dirty tree)' if source['dirty'] else ''}: track masternode 2, add its voting key, vote Yes on {PROPOSAL}")
        out = os.environ.get("DW_E2E_OUT", os.path.join(self.options.tmpdir, "ui"))
        os.makedirs(out, exist_ok=True)
        env = dict(os.environ, DW_E2E_OUT=out, DW_E2E_DATADIR=dwcli.root, DW_E2E_PASSPHRASE=PASSPHRASE,
                   DW_E2E_SPV_PEER=f"127.0.0.1:{p2p_port(0)}", DW_E2E_RPC=rpc_url(node.datadir, 0, self.chain),
                   DW_E2E_PROPOSAL=PROPOSAL, DW_E2E_VOTER=voter.proTxHash, DW_E2E_VOTING_KEY=wif)
        env.setdefault("RUST_LOG", "warn,dw_engine::governance=debug,dw_governance=debug")
        with open(os.path.join(out, "app.log"), "w", encoding="utf8") as log:
            # Its own session, so a timeout also ends tauri-driver, WebKitWebDriver and the app.
            p = subprocess.Popen(["node", "e2e-tauri/regtest-vote-added-key.ts"], cwd=DESKTOP, env=env,
                                 stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                p.wait(timeout=UI_TIMEOUT)
            except subprocess.TimeoutExpired:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(p.pid, signal.SIGKILL)
                p.wait()
                raise
        with open(os.path.join(out, "app.log"), encoding="utf8") as log:
            self.log.info("".join(line for line in log if line.startswith("regtest-vote-added-key:")))
        assert_equal(p.returncode, 0)

        self.log.info(f"1. Core holds the app's Yes from masternode 2 on {PROPOSAL}")
        for n in self.nodes:
            self.wait_until(lambda: self.yes_count(n, p0) == 1, timeout=60)
            votes = n.gobject("getcurrentvotes", p0)
            ours = [v for v in votes.values() if v.startswith(collateral + ":")]
            assert_equal(len(ours), 1)
            # `<outpoint>:<time>:yes:funding:<weight>`: a regular masternode weighs 1.
            assert_equal(ours[0].split(":")[2:], ["yes", "funding", "1"])
            self.log.info(f"node{n.index}: {ours[0]}")

        self.log.info("2. No file of the run or of the app's data holds the key in plain text")
        secrets = [wif.encode(), secret.hex().encode(), secret.hex().upper().encode(), secret]
        assert_equal(leaks(out, secrets) + leaks(dwcli.root, secrets), [])

        with open(os.path.join(out, "result.json"), encoding="utf8") as f:
            result = json.load(f)
        result["evidence"]["core"] = {"vote": ours[0], "votingAddress": voting}
        result["source"] = source
        with open(os.path.join(out, "result.json"), "w", encoding="utf8") as f:
            json.dump(result, f, indent=2)
        self.log.info("governance_added_key_ui suite passed")


if __name__ == "__main__":
    DwdGovernanceAddedKeyUi().main()
