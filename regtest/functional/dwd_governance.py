#!/usr/bin/env python3
# Copyright (c) 2026 The Dash Wallet Desktop developers
# Distributed under the MIT software license, see test_framework/COPYING.
"""dwd_governance.py — regtest suite `governance` (MG-02 T1).

Our governance sync (`dwcli governance`, built on dw-engine's governance facade over dw-p2p)
against 4 regtest masternodes, one of them an evonode (vote weight 4).

Network: node0 controller/miner, SPV peer of dwcli (BIP157 filters) and holder of the
masternodes' voting keys; nodes 1..3 regular masternodes, node 4 an evonode added after setup
(the framework's static masternodes are all regular). Real time (no mocktime): the wallet checks
object and vote times against the wall clock, as Core checks them against adjusted time. With
real time, InstantSend and ChainLocks are switched off again: without quorums nothing is
IS-locked, and enforced ChainLocks keep unlocked transactions out of blocks for 10 minutes.
Default regtest budget parameters (no -budgetparams): the wallet cannot see them, so the first
superblock is Core's regtest default, height 1500, cycle 20, maturity window 10.

Checks, in order:
 1. after govsync, the wallet lists the two proposals with Core's weighted Yes/No counts
    (`gobject get` FundingResult), exact weights (every voting key is one masternode's), the
    evonode's vote at weight 4, and the passing/failing statuses;
 2. the console answers `getgovernanceinfo`, `getsuperblockbudget` and `gobject count`/`get`
    as Core does for the same chain;
 3. `voteraw`: a vote signed outside (`signmessage` with the voting key, regtest's message
    signing) relayed by the wallet reaches Core's nodes;
 4. the superblock: the masternodes' trigger pays the passing proposal at height 1500, and the
    wallet, synced again, shows it Funded at that height.

Run (DWCLI = a built dwcli):
  DWCLI=<target>/debug/dwcli DWD_PYTHON=<venv>/bin/python DASHCORE_DIR=<dashcore> \\
      regtest/functional/run.sh dwd_governance.py
"""

import json
import os
import subprocess
import time
from decimal import Decimal

from test_framework.governance import prepare_object
from test_framework.messages import uint256_to_string
from test_framework.test_framework import EVONODE_COLLATERAL, DashTestFramework
from test_framework.util import assert_equal, assert_greater_than, p2p_port

SPORK_OFF = 4070908800  # 2099-01-01: a spork time that never comes
CONTROLLER_ARGS = ["-peerblockfilters=1", "-blockfilterindex=1", "-debug=gobject"]
MN_ARGS = ["-debug=gobject"]
FIRST_SUPERBLOCK = 1500
SUPERBLOCK_CYCLE = 20
MATURITY_WINDOW = 10
PASSPHRASE = "dwd governance regtest"
OUTCOME_CODES = {"yes": 1, "no": 2, "abstain": 3}


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

    def run(self, *args, timeout=900):
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
        env.setdefault("DWCLI_LOG", "warn,dw_governance=debug,dw_engine::governance=debug")
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, env=env)
        self.log.info(f"dwcli {' '.join(args)} -> {p.returncode}\n{p.stdout}{p.stderr[-6000:]}")
        if p.returncode != 0:
            raise AssertionError(f"dwcli {args} failed: {p.stderr[-3000:]}")
        return p.stdout.splitlines()

    def governance(self, *args, timeout=900):
        """Runs `dwcli governance`; returns its lines grouped by kind."""
        out = {"govstate": [], "govinfo": [], "offinfo": [], "proposal": [], "votes": {}, "relayed": [],
               "relay_error": [], "console": []}
        for line in self.run("governance", *args, timeout=timeout):
            kind, _, rest = line.partition(" ")
            if kind in ("govstate", "govinfo", "offinfo", "proposal", "relayed"):
                out[kind].append(json.loads(rest))
            elif kind == "votes":
                h, _, body = rest.partition(" ")
                out["votes"][h] = json.loads(body)
            elif kind == "relay_error":
                out["relay_error"].append(rest)
            elif kind == "console":
                out["console"].append([rest, None])
            elif kind in ("result", "error") and out["console"]:
                out["console"][-1][1] = (kind, rest)
        return out


def duffs(amount):
    return int(Decimal(str(amount)) * 100_000_000)


class DwdGovernance(DashTestFramework):
    def add_options(self, parser):
        self.add_wallet_options(parser)

    def set_test_params(self):
        self.set_dash_test_params(4, 3, [CONTROLLER_ARGS] + [MN_ARGS] * 3, evo_count=1)
        self.disable_mocktime = True

    def skip_test_if_missing_module(self):
        self.skip_if_no_wallet()
        if not os.environ.get("DWCLI"):
            raise SystemExit("set DWCLI to the dwcli binary")

    # --- helpers -----------------------------------------------------------------------

    def mine_to(self, height):
        node = self.nodes[0]
        while node.getblockcount() < height:
            n = min(50, height - node.getblockcount())
            self.generate(node, n)

    def core_counts(self, h):
        r = self.nodes[1].gobject("get", h)["FundingResult"]
        return r["YesCount"], r["NoCount"], r["AbstainCount"]

    def mn(self, i):
        return self.mninfo[i]

    # --- the test ----------------------------------------------------------------------

    def run_test(self):
        node = self.nodes[0]
        dwcli = Dwcli(os.environ["DWCLI"], os.path.join(self.options.tmpdir, "dwcli"),
                      p2p_port(0), self.log)
        # Peers keep a spork signed in the same second as the one they have; setup just signed these.
        time.sleep(1.5)
        for spork in ("SPORK_2_INSTANTSEND_ENABLED", "SPORK_19_CHAINLOCKS_ENABLED"):
            node.sporkupdate(spork, SPORK_OFF)
        self.wait_for_sporks_same()
        while node.getbalance() < EVONODE_COLLATERAL + 10:
            self.generate(node, 10)
        self.dynamically_add_masternode(evo=True)
        evo = [m for m in self.mninfo if m.evo]
        assert_equal((len(self.mninfo), len(evo)), (4, 1))
        self.log.info(f"masternodes: {[(m.proTxHash[:8], m.evo) for m in self.mninfo]}")

        self.log.info("Mine to 30 blocks before the first superblock's maturity window")
        self.mine_to(FIRST_SUPERBLOCK - MATURITY_WINDOW - 30)
        budget = node.getsuperblockbudget(FIRST_SUPERBLOCK)
        self.log.info(f"budget of {FIRST_SUPERBLOCK}: {budget}")

        self.log.info("Prepare and submit two proposals")
        now = int(time.time())
        p0_addr, p1_addr = node.getnewaddress(), node.getnewaddress()
        p0_amount = Decimal("1.1")
        p1_amount = Decimal("3.3")
        assert p0_amount + p1_amount <= budget
        prepared = [
            prepare_object(node, 1, uint256_to_string(0), now, 1, "dwd_p0", p0_amount, p0_addr),
            prepare_object(node, 1, uint256_to_string(0), now, 1, "dwd_p1", p1_amount, p1_addr),
        ]
        self.generate(node, 6)
        p0, p1 = [node.gobject("submit", "0", 1, now, p["hex"], p["collateralHash"]) for p in prepared]
        for n in self.nodes:
            self.wait_until(lambda: len(n.gobject("list", "all", "proposals")) == 2, timeout=30)

        self.log.info("Vote: every masternode yes on p0; p1 one no (the evonode) and one yes")
        node.gobject("vote-many", p0, "funding", "yes")
        node.gobject("vote-alias", p1, "funding", "no", evo[0].proTxHash)
        regular = [m for m in self.mninfo if not m.evo]
        node.gobject("vote-alias", p1, "funding", "yes", regular[0].proTxHash)
        self.wait_until(lambda: self.core_counts(p0) == (3 + 4, 0, 0), timeout=30)
        self.wait_until(lambda: self.core_counts(p1) == (1, 4, 0), timeout=30)

        self.log.info("1. govsync: the wallet's tallies equal Core's")
        tip = node.getblockcount()
        out = dwcli.governance(
            "--sync-height", str(tip), "--offline-info", "--min-proposals", "2", "--min-voters", "6",
            "--votes", p0, "--votes", p1,
            "--console", "getgovernanceinfo",
            "--console", f"getsuperblockbudget {FIRST_SUPERBLOCK}",
            "--console", "gobject count",
            "--console", f"gobject get {p0}",
        )
        # With sync off the reads already follow the list; no counts yet.
        off = out["offinfo"][0]
        assert_equal((off["valid_masternodes"], off["valid_evonodes"]), (4, 1))
        assert_equal(off["valid_weight"], 3 + 4)
        assert_equal(off["proposals"], None)
        rows = {r["hash"]: r for r in out["proposal"]}
        assert_equal(set(rows), {p0, p1})
        for h, amount in ((p0, p0_amount), (p1, p1_amount)):
            r, (yes, no, abstain) = rows[h], self.core_counts(h)
            t = r["tally"]
            assert_equal((t["yes"]["estimate"], t["no"]["estimate"], t["abstain"]["estimate"]),
                         (yes, no, abstain))
            for k in ("yes", "no", "abstain"):
                assert_equal(t[k]["min"], t[k]["max"])  # every key is one masternode's
            assert_equal(t["ranged_votes"], 0)
            assert_equal(r["payment_amount"], duffs(amount))
        assert_equal(rows[p0]["tally"]["voters"], 4)
        assert_equal(rows[p1]["tally"]["voters"], 2)
        assert_equal(rows[p0]["status"], "passing")
        assert rows[p1]["status"] in ("voting", "failing"), rows[p1]["status"]
        assert_equal(len(out["votes"][p0]), 4)
        evo_rows = [v for v in out["votes"][p1] if v["outcome"] == "no"]
        assert_equal([v["weight"]["estimate"] for v in evo_rows], [4])
        state = out["govstate"][-1]
        assert_equal(state["phase"], "synced")
        assert_greater_than(state["peers"], 0)

        self.log.info("2. console answers as Core")
        console = dict(out["console"])
        info = json.loads(console["getgovernanceinfo"][1])
        core_info = node.getgovernanceinfo()
        for k in ("governanceminquorum", "proposalfee", "superblockcycle", "superblockmaturitywindow",
                  "lastsuperblock", "nextsuperblock", "fundingthreshold", "governancebudget"):
            if k in info:
                assert_equal(Decimal(str(info[k])), Decimal(str(core_info[k])))
        assert "fundingthreshold" in info and "nextsuperblock" in info, info
        kind, budget_text = console[f"getsuperblockbudget {FIRST_SUPERBLOCK}"]
        assert_equal(kind, "result")
        assert_equal(Decimal(budget_text), budget)
        kind, get_text = console[f"gobject get {p0}"]
        assert_equal(kind, "result")
        got = json.loads(get_text)
        assert_equal(got["FundingResult"]["YesCount"], self.core_counts(p0)[0])
        assert_equal(got["Hash"], p0)

        self.log.info("3. voteraw: a vote signed outside, relayed by the wallet, reaches Core")
        voter = regular[1]
        vote_time = int(time.time())
        message = f"{voter.collateral_txid}-{voter.collateral_vout}|{p1}|1|{OUTCOME_CODES['abstain']}|{vote_time}"
        sig = node.signmessage(voter.votingAddr, message)
        out = dwcli.governance(
            "--min-proposals", "2",
            "--relay", f"{voter.collateral_txid}-{voter.collateral_vout}:{p1}:abstain:{vote_time}:{sig}",
            "--linger-secs", "20",
        )
        assert_equal(out["relay_error"], [])
        assert_equal(len(out["relayed"]), 1)
        assert_greater_than(out["relayed"][0]["peers"], 0)
        self.wait_until(lambda: self.core_counts(p1) == (1, 4, 1), timeout=30)

        self.log.info("4. the superblock pays p0; the wallet shows it funded")
        self.mine_to(FIRST_SUPERBLOCK - MATURITY_WINDOW)
        # The elected masternode creates the trigger inside the window, the others vote.
        for _ in range(MATURITY_WINDOW - 1):
            triggers = node.gobject("list", "valid", "triggers")
            if any(t["YesCount"] > 1 for t in triggers.values()):
                break
            self.generate(node, 1)
            time.sleep(2)
        self.wait_until(lambda: any(t["YesCount"] > 1 for t in node.gobject("list", "valid", "triggers").values()),
                        timeout=60)
        self.mine_to(FIRST_SUPERBLOCK)
        coinbase = node.getblock(node.getbestblockhash(), 2)["tx"][0]["vout"]
        paid = [o for o in coinbase if o["scriptPubKey"].get("address") == p0_addr]
        assert_equal(len(paid), 1)
        assert_equal(Decimal(str(paid[0]["value"])), p0_amount)

        out = dwcli.governance("--sync-height", str(FIRST_SUPERBLOCK), "--min-proposals", "2")
        rows = {r["hash"]: r for r in out["proposal"]}
        assert_equal(rows[p0]["status"], "funded")
        assert_equal(rows[p0]["funded_height"], FIRST_SUPERBLOCK)
        assert rows[p1]["funded_height"] is None
        self.log.info("governance suite passed")


if __name__ == "__main__":
    DwdGovernance().main()
