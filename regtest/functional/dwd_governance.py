#!/usr/bin/env python3
# Copyright (c) 2026 The Dash Wallet Desktop developers
# Distributed under the MIT software license, see test_framework/COPYING.
"""dwd_governance.py — regtest suite `governance` (M3 R2).

A 4-masternode DashTestFramework network and a host-built `dwcli` (env
`DWCLI`) speaking SPV/P2P to node 0:

  1. quorums for InstantSend and ChainLocks, so dash-spv syncs the
     masternode list;
  2. a fifth masternode is registered by node 0 with the dwcli wallet's
     DIP-3 voting address (`dwcli gov voting-address`); the wallet finds its
     ProRegTx (and the collateral) through the compact filters;
  3. create a proposal: `dwcli gov prepare` pays the 1 DASH collateral
     (OP_RETURN of the object hash), node 0 mines it;
  4. resume after restart: a new dwcli process lists it as pending and
     Ready;
  5. submit: dwcli relays the object, node 0 has it (`gobject get`);
  6. node 0 votes yes with its four masternodes (`gobject vote-many`);
     dwcli syncs governance objects and votes and votes yes with its
     masternode; node 0 shows the vote under its collateral and the vote
     hash dwcli computed;
  7. the tally: dwcli shows 5 weighted yes votes, as node 0's
     `FundingResult.YesCount`.

Nodes run with mocktime started at the wall clock (dwcli signs objects and
votes with the wall clock; Core refuses votes more than an hour ahead of its
time).
"""

import os
import re
import subprocess
import time
from decimal import Decimal

from test_framework.test_framework import DashTestFramework, MasternodeInfo
from test_framework.util import assert_equal, p2p_port

FILTERS = ["-blockfilterindex=1", "-peerblockfilters=1", "-peerbloomfilters=1"]


class Dwcli:
    def __init__(self, binary, datadir, passphrase_file, port, log):
        self.binary = binary
        self.base = [
            binary,
            "--datadir", datadir,
            "--network", "regtest",
            "--dapi", "http://127.0.0.1:1",
            "--quorum-url", "http://127.0.0.1:1",
            "--peer", f"127.0.0.1:{port}",
            "--passphrase-file", passphrase_file,
        ]
        self.log = log

    def run(self, *args, stdin=None, timeout=600):
        self.log.info("dwcli " + " ".join(args))
        proc = subprocess.run(self.base + list(args), input=stdin, capture_output=True, text=True, timeout=timeout)
        if proc.returncode != 0:
            raise AssertionError(f"dwcli {' '.join(args)} failed ({proc.returncode}):\n{proc.stderr[-4000:]}\n{proc.stdout}")
        self.log.info(proc.stdout.strip())
        return proc.stdout

    def start(self, *args):
        self.log.info("dwcli (background) " + " ".join(args))
        return subprocess.Popen(self.base + list(args), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)


def finish(proc, timeout=600):
    out, err = proc.communicate(timeout=timeout)
    if proc.returncode != 0:
        raise AssertionError(f"dwcli failed ({proc.returncode}):\n{err[-4000:]}\n{out}")
    return out


class DwdGovernanceTest(DashTestFramework):
    def add_options(self, parser):
        self.add_wallet_options(parser)

    def set_test_params(self):
        self.set_dash_test_params(5, 4, extra_args=[FILTERS, [], [], [], []])

    def _initialize_mocktime(self, is_genesis):
        # Start at the wall clock instead of the genesis time (see the
        # module docs).
        self.mocktime = int(time.time())
        for node in self.nodes:
            node.mocktime = self.mocktime

    def skip_test_if_missing_module(self):
        self.skip_if_no_wallet()
        if not os.environ.get("DWCLI"):
            raise AssertionError("set DWCLI to a built dwcli binary")

    def mine(self, n=1):
        node = self.nodes[0]
        hashes = self.generate(node, n)
        self.wait_for_chainlocked_block_all_nodes(hashes[-1], timeout=60)
        return node.getblockcount()

    def run_test(self):
        node = self.nodes[0]
        self.log.info("Quorums for InstantSend and ChainLocks")
        node.sporkupdate("SPORK_17_QUORUM_DKG_ENABLED", 0)
        self.wait_for_sporks_same()
        try:
            self.mine_cycle_quorum()
        except AssertionError:
            # The rotating DKG sometimes yields one quorum for both indices
            # on a loaded machine (RESULTS.md §6); the next cycle works.
            self.log.info("Rotating quorum incomplete; mining the next cycle")
            self.mine_cycle_quorum()
        if len(node.quorum("list")["llmq_test"]) == 0:
            self.mine_quorum(llmq_type_name="llmq_test", llmq_type=100)

        work = os.path.join(self.options.tmpdir, "dwcli")
        os.makedirs(work, exist_ok=True)
        passphrase = os.path.join(work, "passphrase")
        with open(passphrase, "w", encoding="utf-8") as f:
            f.write("governance suite passphrase\n")
        cli = Dwcli(os.environ["DWCLI"], os.path.join(work, "data"), passphrase, p2p_port(0), self.log)
        cli.run("init-vault")
        cli.run("create")
        voting_address = re.search(r"voting_address (\S+)", cli.run("gov", "voting-address")).group(1)
        receive = re.search(r"address (\S+)", cli.run("receive")).group(1)

        self.log.info("Fund the dwcli wallet")
        txid = node.sendtoaddress(receive, Decimal("20"))
        self.wait_for_instantlock(txid, timeout=60)
        self.mine()

        self.log.info("Register a masternode with the dwcli voting key")
        mn = MasternodeInfo(evo=False, legacy=False)
        mn.generate_addresses(node)
        # register_fund pays the collateral from the funds address only.
        funding = [node.sendtoaddress(mn.fundsAddr, mn.get_collateral_value()),
                   node.sendtoaddress(mn.fundsAddr, Decimal("0.01"))]
        self.wait_for_instantlock(*funding, timeout=60)
        pro_tx_hash = node.protx(
            "register_fund", mn.collateral_address, [], mn.ownerAddr, mn.pubKeyOperator,
            voting_address, 0, mn.rewards_address, mn.fundsAddr, True)
        self.wait_for_instantlock(pro_tx_hash, timeout=60)
        height = self.mine()
        info = node.protx("info", pro_tx_hash)
        collateral = f"{info['collateralHash']}-{info['collateralIndex']}"
        self.log.info(f"proTxHash {pro_tx_hash} collateral {collateral}")
        raw = node.getrawtransaction(pro_tx_hash, True)
        self.log.info(f"ProRegTx payload version {raw['proRegTx'].get('version')}")
        # No further DKG: the new masternode runs no node.
        node.sporkupdate("SPORK_17_QUORUM_DKG_ENABLED", 4070908800)
        self.wait_for_sporks_same()
        cli.run("sync", "--min-height", str(height), "--timeout-secs", "300")

        self.log.info("Create a proposal: the 1 DASH collateral")
        payee = node.getnewaddress()
        proc = cli.start(
            "gov", "prepare", "--sync-height", str(height), "--timeout-secs", "300",
            "--name", "dwd-governance", "--url", "https://dash.org/dwd",
            "--address", payee, "--amount", "500000000", "--count", "1", "--linger-secs", "3")
        out = finish(proc)
        m = re.search(r"proposal (\S+) collateral=(\S+)", out)
        assert m, out
        proposal, collateral_txid = m.group(1), m.group(2)
        self.wait_for_instantlock(collateral_txid, timeout=60)
        tx = node.getrawtransaction(collateral_txid, True)
        burn = [o for o in tx["vout"] if o["scriptPubKey"]["asm"].startswith("OP_RETURN")]
        assert_equal(len(burn), 1)
        assert_equal(burn[0]["value"], Decimal("1"))
        assert_equal(bytes.fromhex(burn[0]["scriptPubKey"]["asm"].split()[1])[::-1].hex(), proposal)
        height = self.mine(6)

        self.log.info("Resume after restart: a new process lists it as Ready")
        out = cli.run("gov", "pending", "--sync-height", str(height))
        m = re.search(rf"pending {proposal} .*status=(\w+) confirmations=(\d+)", out)
        assert m, out
        assert_equal(m.group(1), "Ready")
        assert int(m.group(2)) >= 6

        self.log.info("Submit")
        out = cli.run("gov", "submit", "--sync-height", str(height), proposal)
        assert f"submitted {proposal}" in out
        self.wait_until(lambda: proposal in node.gobject("list", "valid", "proposals"), timeout=60)
        assert_equal(node.gobject("get", proposal)["ObjectType"], 1)

        self.log.info("Node 0 votes yes with its four masternodes")
        many = node.gobject("vote-many", proposal, "funding", "yes")
        self.log.info(f"vote-many: {many['overall']}")
        self.wait_until(lambda: node.gobject("get", proposal)["FundingResult"]["YesCount"] == 4, timeout=60)

        self.log.info("dwcli syncs governance and votes with its masternode")
        out = cli.run("gov", "vote", "--sync-height", str(height), "--gov", "--timeout-secs", "300", proposal, "yes")
        m = re.search(r"vote (\S+) ok .*vote ([0-9a-f]{64})", out)
        assert m, out
        assert_equal(m.group(1), pro_tx_hash)
        vote_hash = m.group(2)
        self.wait_until(lambda: vote_hash in node.gobject("getcurrentvotes", proposal), timeout=60)
        current = node.gobject("getcurrentvotes", proposal)[vote_hash]
        assert current.startswith(f"{collateral}:"), current
        assert ":yes:funding" in current, current
        self.wait_until(lambda: node.gobject("get", proposal)["FundingResult"]["YesCount"] == 5, timeout=60)

        self.log.info("The tally dwcli shows equals node 0's")
        out = cli.run("gov", "list", "--sync-height", str(height), "--gov", "--timeout-secs", "300",
                      "--wait-for", proposal, "--min-yes", "5")
        m = re.search(rf"proposal hash={proposal} name=dwd-governance status=(\w+) yes=(\d+) no=(\d+) abstain=(\d+) margin=(-?\d+) my=(\S+)", out)
        assert m, out
        result = node.gobject("get", proposal)["FundingResult"]
        assert_equal(int(m.group(2)), result["YesCount"])
        assert_equal(int(m.group(3)), result["NoCount"])
        assert_equal(int(m.group(4)), result["AbstainCount"])
        assert_equal(m.group(6), "1/0/0/0")
        out = cli.run("gov", "info", "--sync-height", str(height), "--gov", "--timeout-secs", "300")
        info = dict(kv.split("=", 1) for kv in out.split("\n")[0].split()[1:])
        core = node.getgovernanceinfo()
        assert_equal(int(info["next"]), core["nextsuperblock"])
        assert_equal(int(info["last"]), core["lastsuperblock"])
        assert_equal(int(info["budget"]), int(core["governancebudget"] * 100_000_000))
        assert_equal((info["proposals"], info["passing"], info["mn_voting"]), ("1", "1", "5"))
        # dash-qt counts only masternodes that are not PoSe-banned as
        # controlled; the fifth one runs no node and may have been banned.
        banned = node.protx("info", pro_tx_hash)["state"]["PoSeBanHeight"] != -1
        assert_equal(info["controlled"], "0" if banned else "1")
        self.log.info("Governance suite passed")


if __name__ == "__main__":
    DwdGovernanceTest().main()
