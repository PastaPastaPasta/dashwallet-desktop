#!/usr/bin/env python3
# Copyright (c) 2026 The Dash Wallet Desktop developers
# Distributed under the MIT software license, see test_framework/COPYING.
"""dwd_governance.py — regtest suite `governance` (MG-02 T1).

Our governance sync (`dwcli governance`, built on dw-engine's governance facade over dw-p2p)
against 4 regtest masternodes, one of them an evonode (vote weight 4).

Network: node0 controller/miner, SPV peer of dwcli (BIP157 filters), holder of the
masternodes' voting keys and, with -txindex, the source of the ProRegTxs dwcli resolves vote
outpoints from in check 4 (`--collateral-rpc`, regtest's stand-in for DAPI, MG-02 D80); nodes 1..3 regular masternodes, node 4 an evonode added after setup
(the framework's static masternodes are all regular). Real time (no mocktime): the wallet checks
object and vote times against the wall clock, as Core checks them against adjusted time. With
real time, InstantSend and ChainLocks are switched off again: without quorums nothing is
IS-locked, and enforced ChainLocks keep unlocked transactions out of blocks for 10 minutes.
Default regtest budget parameters (no -budgetparams): the wallet cannot see them, so the first
superblock is Core's regtest default, height 1500, cycle 20, maturity window 10.

Checks, in order:
 1. after govsync, the wallet lists the two proposals with Core's weighted Yes/No counts
    (`gobject get` FundingResult) as the estimates (every voting key is one masternode's),
    each vote ranged 0..4 as no outpoint is resolved (MG-02 D64), the evonode's vote
    estimated at weight 4, and the passing/failing statuses;
 2. the console answers `getgovernanceinfo`, `getsuperblockbudget` and `gobject count`/`get`
    as Core does for the same chain;
 3. `voteraw`: a vote signed outside (`signmessage` with the voting key, regtest's message
    signing) relayed by the wallet reaches Core's nodes;
 4. resolved outpoints: a masternode votes no on p1, then changes to yes; dwcli, resolving
    every voter's outpoint from its ProRegTx, counts each vote exactly (no ranged vote, no close
    call) and its tally and `gobject getcurrentvotes` follow Core's;
 5. the superblock: the masternodes' trigger pays the passing proposal at height 1500, and the
    wallet, synced again, shows it Funded at that height;
 6. the other signals (MG-02 D82–D89): Core takes a proposal's valid, delete and endorsed votes
    signed by the operator key only, and v24 has no RPC that signs one (`vote-conf` is gone,
    #5552), so it refuses the voting-key votes `vote-alias` signs; after Core's maintenance pass
    (`mockscheduler`) the wallet's flags, signal results and `gobject list` filters are Core's
    (operator votes themselves: unit tests only, D90).

Run (DWCLI = a built dwcli):
  DWCLI=<target>/debug/dwcli DWD_PYTHON=<venv>/bin/python DASHCORE_DIR=<dashcore> \\
      regtest/functional/run.sh dwd_governance.py
"""

import json
import os
import sqlite3
import subprocess
import time
from decimal import Decimal

from test_framework.governance import prepare_object
from test_framework.messages import uint256_to_string
from test_framework.test_framework import EVONODE_COLLATERAL, DashTestFramework
from test_framework.authproxy import JSONRPCException
from test_framework.util import assert_equal, assert_greater_than, p2p_port, rpc_url

SPORK_OFF = 4070908800  # 2099-01-01: a spork time that never comes
CONTROLLER_ARGS = ["-peerblockfilters=1", "-blockfilterindex=1", "-txindex=1", "-debug=gobject"]
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

    def run(self, *args, timeout=900, options=()):
        cmd = [
            self.binary,
            "--datadir", os.path.join(self.datadir, "data"),
            "--network", "regtest",
            "--dapi", "http://127.0.0.1:1",
            "--quorum-url", "http://127.0.0.1:1",
            "--no-platform",
            "--peer", f"127.0.0.1:{self.peer_port}",
            "--passphrase-file", self.passfile,
            *options,
        ] + list(args)
        env = dict(os.environ)
        env.setdefault("DWCLI_LOG", "warn,dw_governance=debug,dw_engine::governance=debug")
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, env=env)
        self.log.info(f"dwcli {' '.join(args)} -> {p.returncode}\n{p.stdout}{p.stderr[-6000:]}")
        if p.returncode != 0:
            raise AssertionError(f"dwcli {args} failed: {p.stderr[-3000:]}")
        return p.stdout.splitlines()

    def governance(self, *args, timeout=900, options=()):
        """Runs `dwcli governance` (after the global `options`); returns its lines grouped by kind."""
        out = {"govstate": [], "govinfo": [], "offinfo": [], "proposal": [], "votes": {}, "relayed": [],
               "relay_error": [], "console": []}
        for line in self.run("governance", *args, timeout=timeout, options=options):
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

    def maintenance_pass(self, nodes):
        """Runs Core's governance maintenance (`CheckAndRemove`, every 5 minutes) now, and waits
        for it: the scheduler runs the task on its own thread."""
        for n in nodes:
            with n.assert_debug_log(["UpdateCachesAndClean -- Governance Objects:"], timeout=30):
                n.mockscheduler(300)

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
            "--console", f"gobject getcurrentvotes {p0}",
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
                # Unresolved, a vote proves no weight (D64).
                assert_equal(t[k]["min"], 0)
                assert t[k]["estimate"] <= t[k]["max"], t
            assert_equal(t["ranged_votes"], t["voters"])
            assert_equal(r["payment_amount"], duffs(amount))
        assert_equal(rows[p0]["tally"]["voters"], 4)
        assert_equal(rows[p1]["tally"]["voters"], 2)
        assert_equal(rows[p0]["status"], "passing")
        assert_equal(rows[p0]["close_call"], True)
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
            assert k in info, (k, info)
            assert_equal(Decimal(str(info[k])), Decimal(str(core_info[k])))
        kind, budget_text = console[f"getsuperblockbudget {FIRST_SUPERBLOCK}"]
        assert_equal(kind, "result")
        assert_equal(Decimal(budget_text), budget)
        kind, get_text = console[f"gobject get {p0}"]
        assert_equal(kind, "result")
        got = json.loads(get_text)
        assert_equal(got["FundingResult"]["YesCount"], self.core_counts(p0)[0])
        assert_equal(got["Hash"], p0)
        # Every vote, keyed by its hash, in Core's outpoint order.
        kind, votes_text = console[f"gobject getcurrentvotes {p0}"]
        assert_equal(kind, "result")
        ours, core = json.loads(votes_text), node.gobject("getcurrentvotes", p0)
        assert_equal(list(ours.items()), list(core.items()))

        self.log.info("3. voteraw: a vote signed outside, relayed by the wallet, reaches Core")
        voter = regular[1]
        vote_time = int(time.time())
        collateral = (voter.collateral_txid, voter.collateral_vout)

        def signed(outcome, when):
            message = f"{collateral[0]}-{collateral[1]}|{p1}|1|{OUTCOME_CODES[outcome]}|{when}"
            return [*collateral, p1, "funding", outcome, when, node.signmessage(voter.votingAddr, message)]

        relayed = signed("abstain", vote_time)
        # Then through the console: the same vote again, an older one, and a
        # newer one within the hour; Core must answer them alike.
        refused = [relayed, signed("no", vote_time - 1), signed("no", vote_time + 1)]
        lines = ["voteraw " + " ".join(str(a) for a in args) for args in refused]
        out = dwcli.governance(
            "--min-proposals", "2",
            "--relay", f"{collateral[0]}-{collateral[1]}:{p1}:abstain:{vote_time}:{relayed[-1]}",
            *[a for line in lines for a in ("--console", line)],
            "--linger-secs", "20",
        )
        assert_equal(out["relay_error"], [])
        assert_equal(len(out["relayed"]), 1)
        assert_greater_than(out["relayed"][0]["peers"], 0)
        self.wait_until(lambda: self.core_counts(p1) == (1, 4, 1), timeout=30)
        console = dict(out["console"])
        for line, args in zip(lines, refused):
            try:
                node.voteraw(*args)
                raise AssertionError(f"Core accepted {line}")
            except JSONRPCException as e:
                core = f"{e.error['message']} (code {e.error['code']})"
            kind, ours = console[line]
            assert_equal(kind, "error")
            # Core's rate message ends with its own clock readings (D37).
            cut = core.find(", last update =")
            if cut >= 0:
                core, ours = core[:cut], ours[:cut]
            assert_equal(ours, core)

        self.log.info("4. resolved outpoints: a masternode changes its vote, the tally follows Core")
        mover = regular[2]
        # Core's own record of the collateral: a masternode funded by its ProRegTx has the
        # registration's output (an internal collateral), whatever the framework noted.
        info = node.protx("info", mover.proTxHash)
        outpoint = (info["collateralHash"], info["collateralIndex"])

        def vote_as_mover(outcome, when):
            message = f"{outpoint[0]}-{outpoint[1]}|{p1}|1|{OUTCOME_CODES[outcome]}|{when}"
            node.voteraw(*outpoint, p1, "funding", outcome, when, node.signmessage(mover.votingAddr, message))

        vote_as_mover("no", int(time.time()))
        self.wait_until(lambda: self.core_counts(p1) == (1, 4 + 1, 1), timeout=30)
        rpc = rpc_url(node.datadir, 0, self.chain)

        def resolved_sync(voters):
            out = dwcli.governance(
                "--sync-height", str(node.getblockcount()), "--min-proposals", "2",
                "--min-voters", str(voters), "--max-ranged", "0", "--votes", p1,
                "--console", f"gobject getcurrentvotes {p1}", options=("--collateral-rpc", rpc),
            )
            rows = {r["hash"]: r for r in out["proposal"]}
            for h in (p0, p1):
                t, counts = rows[h]["tally"], self.core_counts(h)
                for k, core in zip(("yes", "no", "abstain"), counts):
                    # Resolved, every vote proves its weight.
                    assert_equal((t[k]["min"], t[k]["estimate"], t[k]["max"]), (core, core, core))
                assert_equal(t["ranged_votes"], 0)
                assert_equal(rows[h]["close_call"], False)
            kind, votes_text = dict(out["console"])[f"gobject getcurrentvotes {p1}"]
            assert_equal(kind, "result")
            assert_equal(list(json.loads(votes_text).items()),
                         list(node.gobject("getcurrentvotes", p1).items()))
            return out

        out = resolved_sync(4 + 4)
        assert_equal(sorted(v["outcome"] for v in out["votes"][p1]), ["abstain", "no", "no", "yes"])
        # Each listed masternode's ProRegTx is kept for the next session.
        dbs = [os.path.join(d, f) for d, _, fs in os.walk(dwcli.datadir) for f in fs if f == "app.sqlite"]
        assert_equal(len(dbs), 1)
        with sqlite3.connect(dbs[0]) as db:
            assert_equal(db.execute("SELECT COUNT(*) FROM governance_registrations").fetchone()[0], 4)

        # Core takes a masternode's new vote an hour after it accepted the last one, by its own
        # clock (GOVERNANCE_UPDATE_MIN, `last_update`). The nodes' clocks move ahead in steps
        # shorter than the inactivity timeout, so their peers keep pinging; the vote is dated
        # by the real clock dwcli checks it against, and the clocks go back after.
        ahead = int(time.time())
        for _ in range(6):
            ahead += 650
            for n in self.nodes:
                n.setmocktime(ahead)
            time.sleep(2)
        vote_as_mover("yes", int(time.time()) + 60)
        self.wait_until(lambda: self.core_counts(p1) == (1 + 1, 4, 1), timeout=30)
        self.wait_until(lambda: all(n.gobject("getcurrentvotes", p1) == node.gobject("getcurrentvotes", p1)
                                    for n in self.nodes), timeout=30)
        for n in self.nodes:
            n.setmocktime(0)
        out = resolved_sync(4 + 4)
        assert_equal(sorted(v["outcome"] for v in out["votes"][p1]), ["abstain", "no", "yes", "yes"])

        self.log.info("5. the superblock pays p0; the wallet shows it funded")
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

        self.log.info("6. other signals: Core refuses voting-key votes on them; flags and lists agree")
        # Core checks these against the operator key (`ProcessVote`, D89); the wallet relays
        # funding votes only (D30) and stores none signed so (D89).
        for signal in ("valid", "delete", "endorsed"):
            r = node.gobject("vote-alias", p0, signal, "yes", regular[0].proTxHash)
            assert_equal(r["overall"], "Voted successfully 0 time(s) and failed 1 time(s).")
            r2 = node.gobject("vote-many", p1, signal, "no")
            assert_equal(r2["overall"], f"Voted successfully 0 time(s) and failed {len(self.mninfo)} time(s).")
            for d in [*r["detail"].values(), *r2["detail"].values()]:
                assert "-- Invalid vote," in d["errorMessage"], d
        self.maintenance_pass([node])
        lists = {signal: f"gobject list {signal} proposals" for signal in ("valid", "funding", "delete", "endorsed")}
        lines = [f"gobject get {p0}", f"gobject get {p1}", *lists.values()]
        out = dwcli.governance(
            "--sync-height", str(node.getblockcount()), "--min-proposals", "2", "--max-ranged", "0",
            *[a for line in lines for a in ("--console", line)],
            options=("--collateral-rpc", rpc),
        )
        console = dict(out["console"])
        for h in (p0, p1):
            kind, text = console[f"gobject get {h}"]
            assert_equal(kind, "result")
            ours, core = json.loads(text), node.gobject("get", h)
            for k in ("fCachedValid", "fCachedFunding", "fCachedDelete", "fCachedEndorsed", "FundingResult",
                      "ValidResult", "DeleteResult", "EndorsedResult"):
                assert_equal((h, k, ours[k]), (h, k, core[k]))
            for result in ("ValidResult", "DeleteResult", "EndorsedResult"):
                assert_equal(core[result]["YesCount"] + core[result]["NoCount"], 0)
        for signal, line in lists.items():
            kind, text = console[line]
            assert_equal(kind, "result")
            assert_equal(sorted(json.loads(text)), sorted(node.gobject("list", signal, "proposals")))
        assert_equal(sorted(node.gobject("list", "valid", "proposals")), sorted([p0, p1]))
        self.log.info("governance suite passed")


if __name__ == "__main__":
    DwdGovernance().main()
