#!/usr/bin/env python3
# Copyright (c) 2026 The Dash Wallet Desktop developers
# Distributed under the MIT software license, see test_framework/COPYING.
"""dwd_governance_ui.py — regtest suite `governance_ui` (MG-09 GL acceptance).

From the app's UI on regtest, vote on a proposal and create a proposal: this suite builds the
chain, starts `apps/desktop/e2e-tauri/regtest-governance.ts` (tauri-driver on the real app) and
judges the outcome on Core.

Network: node0 controller/miner and the app's SPV peer (BIP157 filters); nodes 1..3 the
framework's regular masternodes. Real time (no mocktime), InstantSend and ChainLocks off, as in
dwd_governance.py, and Core's regtest budget defaults (superblock 1500, cycle 20).

Setup:
 - dwcli makes the vault and imports the BIP39 test vector (regtest coins only), so the app's
   DIP3 voting key 0 is known here (dwd_masternodes.provider_key);
 - the wallet gets one 1,000 DASH coin, so Masternodes shows in the sidebar without the setting
   (MG-09 D3);
 - `protx update_registrar` gives masternode 1 the wallet's voting key 0 (node0 holds the
   framework's owner keys);
 - Core submits proposal `dwd_ui_p0` to vote on.

Checks, after the UI run:
 1. the vote: every node holds a funding Yes from masternode 1's collateral on dwd_ui_p0, and
    Core's Yes count went from 0 to 1;
 2. the created proposal: its fee transaction has at least 6 confirmations, every node lists the
    object with the hash and fee txid the app reported, the name, link, payment address and
    amount entered in the UI, the address being the wallet's (dwcli lists it).

Run (DW_APP, TAURI_DRIVER and DISPLAY as for scripts/e2e-regtest-governance.sh):
  DWCLI=<target>/debug/dwcli DW_APP=<target>/debug/dash-wallet TAURI_DRIVER=<tauri-driver> \\
  DWD_PYTHON=<venv>/bin/python DASHCORE_DIR=<dashcore> regtest/functional/run.sh dwd_governance_ui.py
"""

import contextlib
import json
import os
import signal
import subprocess
import time
from decimal import Decimal

from dwd_masternodes import VOTING, address, provider_key
from test_framework.governance import prepare_object
from test_framework.messages import uint256_to_string
from test_framework.test_framework import DashTestFramework
from test_framework.util import assert_equal, p2p_port, rpc_url

SPORK_OFF = 4070908800  # 2099-01-01: a spork time that never comes
CONTROLLER_ARGS = ["-peerblockfilters=1", "-blockfilterindex=1", "-txindex=1", "-debug=gobject"]
MN_ARGS = ["-debug=gobject"]
FIRST_SUPERBLOCK = 1500
MATURITY_WINDOW = 10
PASSPHRASE = "dwd governance ui regtest"
PHRASE = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
PROPOSAL = "dwd_ui_p0"
HERE = os.path.dirname(os.path.abspath(__file__))
DESKTOP = os.path.join(HERE, "..", "..", "apps", "desktop")
# Above the UI run's own worst case (its polls at DW_E2E_TIMEOUT_MS each, plus the chain's waits).
UI_TIMEOUT = 5400


def source_head():
    """The commit under test and whether the tree differs from it, for result.json."""
    git = lambda *a: subprocess.run(["git", "-C", HERE, *a], capture_output=True, text=True, check=True).stdout.strip()
    return {"head": git("rev-parse", "HEAD"), "dirty": git("status", "--porcelain") != ""}


class Dwcli:
    def __init__(self, binary, datadir, peer_port, log):
        self.binary = binary
        self.root = os.path.join(datadir, "data")
        self.peer_port = peer_port
        self.log = log
        self.passfile = os.path.join(datadir, "pass")
        os.makedirs(datadir, mode=0o700, exist_ok=True)
        with open(self.passfile, "w", encoding="utf8") as f:
            f.write(PASSPHRASE + "\n")

    def run(self, *args, stdin=None, timeout=900):
        cmd = [self.binary, "--datadir", self.root, "--network", "regtest", "--dapi", "http://127.0.0.1:1",
               "--quorum-url", "http://127.0.0.1:1", "--no-platform", "--peer", f"127.0.0.1:{self.peer_port}",
               "--passphrase-file", self.passfile, *args]
        p = subprocess.run(cmd, input=stdin, capture_output=True, text=True, timeout=timeout)
        self.log.info(f"dwcli {' '.join(args)} -> {p.returncode}\n{p.stdout}{p.stderr[-3000:]}")
        if p.returncode != 0:
            raise AssertionError(f"dwcli {args} failed: {p.stderr[-3000:]}")
        return p.stdout.splitlines()


class DwdGovernanceUi(DashTestFramework):
    def add_options(self, parser):
        self.add_wallet_options(parser)

    def set_test_params(self):
        self.set_dash_test_params(4, 3, [CONTROLLER_ARGS] + [MN_ARGS] * 3)
        self.disable_mocktime = True

    def skip_test_if_missing_module(self):
        self.skip_if_no_wallet()
        for name in ("DWCLI", "DW_APP", "TAURI_DRIVER", "DISPLAY"):
            if not os.environ.get(name):
                raise SystemExit(f"set {name}")

    def mine_to(self, height):
        node = self.nodes[0]
        while node.getblockcount() < height:
            self.generate(node, min(50, height - node.getblockcount()))

    def yes_count(self, n, h):
        return n.gobject("get", h)["FundingResult"]["YesCount"]

    def run_test(self):
        node = self.nodes[0]
        time.sleep(1.5)  # peers keep a spork signed in the same second as the one they have
        for spork in ("SPORK_2_INSTANTSEND_ENABLED", "SPORK_19_CHAINLOCKS_ENABLED"):
            node.sporkupdate(spork, SPORK_OFF)
        self.wait_for_sporks_same()

        self.log.info("Setup: the app's wallet, one 1,000 DASH coin, masternode 1 votes with its key 0")
        dwcli = Dwcli(os.environ["DWCLI"], os.path.join(self.options.tmpdir, "dwcli"), p2p_port(0), self.log)
        dwcli.run("init-vault")
        wallet = " ".join(dwcli.run("import", "--birth-height", "0", stdin=PHRASE)).split()[-1]
        coin_address = dwcli.run("receive", "--next")[-1].split()[1]
        while node.getbalance() < 1_000 + 20:
            self.generate(node, 10)
        node.sendtoaddress(coin_address, 1_000)
        voter = self.mninfo[0]
        voting = address(provider_key(VOTING, 0))
        fee_source = node.getnewaddress()
        node.sendtoaddress(fee_source, 1)
        self.generate(node, 1)
        node.protx("update_registrar", voter.proTxHash, "", voting, "", fee_source)
        self.generate(node, 1)
        assert_equal(node.protx("info", voter.proTxHash)["state"]["votingAddress"], voting)
        info = node.protx("info", voter.proTxHash)
        collateral = f"{info['collateralHash']}-{info['collateralIndex']}"

        self.log.info("Mine to 30 blocks before the first superblock's maturity window; submit dwd_ui_p0")
        self.mine_to(FIRST_SUPERBLOCK - MATURITY_WINDOW - 30)
        now = int(time.time())
        prepared = prepare_object(node, 1, uint256_to_string(0), now, 1, PROPOSAL, Decimal("1.5"), node.getnewaddress())
        self.generate(node, 6)
        p0 = node.gobject("submit", "0", 1, now, prepared["hex"], prepared["collateralHash"])
        for n in self.nodes:
            self.wait_until(lambda: p0 in n.gobject("list", "all", "proposals"), timeout=30)
        assert_equal(self.yes_count(node, p0), 0)

        source = source_head()
        self.log.info(f"The UI run on {source['head']}{' (dirty tree)' if source['dirty'] else ''}: vote Yes on dwd_ui_p0, create dwd_ui_p1")
        out = os.environ.get("DW_E2E_OUT", os.path.join(self.options.tmpdir, "ui"))
        os.makedirs(out, exist_ok=True)
        env = dict(os.environ, DW_E2E_OUT=out, DW_E2E_DATADIR=dwcli.root, DW_E2E_PASSPHRASE=PASSPHRASE,
                   DW_E2E_WALLET=wallet, DW_E2E_SPV_PEER=f"127.0.0.1:{p2p_port(0)}",
                   DW_E2E_RPC=rpc_url(node.datadir, 0, self.chain), DW_E2E_MINER=node.getnewaddress(),
                   DW_E2E_PROPOSAL=PROPOSAL, DW_E2E_VOTER=voter.proTxHash)
        env.setdefault("RUST_LOG", "warn,dw_engine::governance=debug,dw_governance=debug")
        with open(os.path.join(out, "app.log"), "w", encoding="utf8") as log:
            # Its own session, so a timeout also ends tauri-driver, WebKitWebDriver and the app.
            p = subprocess.Popen(["node", "e2e-tauri/regtest-governance.ts"], cwd=DESKTOP, env=env,
                                 stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                p.wait(timeout=UI_TIMEOUT)
            except subprocess.TimeoutExpired:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(p.pid, signal.SIGKILL)
                p.wait()
                raise
        with open(os.path.join(out, "app.log"), encoding="utf8") as log:
            self.log.info("".join(line for line in log if line.startswith("regtest-governance:")))
        assert_equal(p.returncode, 0)
        with open(os.path.join(out, "result.json"), encoding="utf8") as f:
            result = json.load(f)
        evidence = result["evidence"]

        self.log.info("1. Core holds the app's Yes from masternode 1 on dwd_ui_p0")
        for n in self.nodes:
            self.wait_until(lambda: self.yes_count(n, p0) == 1, timeout=60)
            votes = n.gobject("getcurrentvotes", p0)
            ours = [v for v in votes.values() if v.startswith(collateral + ":")]
            assert_equal(len(ours), 1)
            # `<outpoint>:<time>:yes:funding:<weight>`: a regular masternode weighs 1.
            assert_equal(ours[0].split(":")[2:], ["yes", "funding", "1"])

        self.log.info("2. Every node lists the created proposal with the app's hash and fee txid")
        h, fee_txid = evidence["createdHash"], evidence["feeTxid"]
        assert node.getrawtransaction(fee_txid, True)["confirmations"] >= 6
        wallet_addresses = {line.split()[2] for line in dwcli.run("addresses", wallet) if line.startswith("addr ")}
        for n in self.nodes:
            self.wait_until(lambda: h in n.gobject("list", "all", "proposals"), timeout=120)
            got = n.gobject("get", h)
            assert_equal(got["CollateralHash"], fee_txid)
            data = json.loads(got["DataString"])
            assert_equal((data["name"], data["url"]), (result["created"], "https://example.org/dwd-ui-p1"))
            assert_equal(Decimal(str(data["payment_amount"])), Decimal("2.5"))
            assert data["payment_address"] in wallet_addresses, (data["payment_address"], wallet_addresses)
        evidence["core"] = {"vote": ours[0], "created": data}
        result["source"] = source
        with open(os.path.join(out, "result.json"), "w", encoding="utf8") as f:
            json.dump(result, f, indent=2)
        self.log.info("governance_ui suite passed")


if __name__ == "__main__":
    DwdGovernanceUi().main()
