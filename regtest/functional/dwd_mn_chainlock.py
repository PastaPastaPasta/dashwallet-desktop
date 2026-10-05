#!/usr/bin/env python3
# Copyright (c) 2026 The Dash Wallet Desktop developers
# Distributed under the MIT software license, see test_framework/COPYING.
"""dwd_mn_chainlock.py

Smallest masternode-enabled regtest network the dashwallet-desktop suites build on:
one controller node, four regular masternodes, and one plain (non-MN) wallet node.

Checks, using official release binaries and the vendored DashTestFramework:
  1. four masternodes register and appear ENABLED in the deterministic MN list;
  2. a rotating DIP0024 quorum (llmq_test_dip0024, InstantSend) and an llmq_test quorum
     (ChainLocks) complete DKG;
  3. a freshly mined block receives a ChainLock on every node;
  4. a wallet payment to the plain node receives an InstantSend lock and the plain node
     sees it as `instantlock: true` before the next block.
"""

from decimal import Decimal

from test_framework.test_framework import DashTestFramework
from test_framework.util import assert_equal


class DwdMasternodeChainLockTest(DashTestFramework):
    def add_options(self, parser):
        self.add_wallet_options(parser)

    def set_test_params(self):
        # DashTestFramework creates the simple nodes first: node0 = controller/miner,
        # node1 = plain wallet node, nodes 2..5 = masternodes (masternode mode runs without a wallet).
        self.set_dash_test_params(6, 4)

    def run_test(self):
        controller = self.nodes[0]
        plain = self.nodes[1]

        mn_list = controller.masternodelist("status")
        self.log.info(f"Deterministic MN list: {mn_list}")
        assert_equal(len(mn_list), 4)
        assert all(status == "ENABLED" for status in mn_list.values())

        controller.sporkupdate("SPORK_17_QUORUM_DKG_ENABLED", 0)
        self.wait_for_sporks_same()

        self.log.info("Mining rotating quorum (llmq_test_dip0024) for InstantSend")
        self.mine_cycle_quorum()
        if len(controller.quorum("list")["llmq_test"]) == 0:
            self.log.info("Mining llmq_test quorum for ChainLocks")
            self.mine_quorum(llmq_type_name="llmq_test", llmq_type=100)
        quorums = controller.quorum("list")
        self.log.info(f"Active quorums: {quorums}")
        assert len(quorums["llmq_test"]) >= 1
        assert len(quorums["llmq_test_dip0024"]) >= 1

        self.log.info("Waiting for a ChainLock on a new tip")
        tip = self.generate(controller, 1)[0]
        self.wait_for_chainlocked_block_all_nodes(tip, timeout=60)
        best_cl = plain.getbestchainlock()
        self.log.info(f"Best ChainLock on plain node: height={best_cl['height']} hash={best_cl['blockhash']}")
        assert_equal(best_cl["blockhash"], tip)

        self.log.info("Sending a payment and waiting for its InstantSend lock")
        address = plain.getnewaddress()
        txid = controller.sendtoaddress(address, Decimal("1.5"))
        self.wait_for_instantlock(txid, timeout=60)
        tx = plain.gettransaction(txid)
        self.log.info(f"Plain node sees tx {txid}: confirmations={tx['confirmations']} instantlock={tx['instantlock']}")
        assert_equal(tx["confirmations"], 0)
        assert tx["instantlock"]
        assert_equal(plain.getbalances()["mine"]["trusted"], Decimal("1.5"))

        self.log.info("Mining the locked tx into a block and ChainLocking it")
        tip = self.generate(controller, 1)[0]
        self.wait_for_chainlocked_block_all_nodes(tip, timeout=60)
        tx = plain.gettransaction(txid)
        assert tx["chainlock"]
        self.log.info("Masternode network: ChainLocks and InstantSend verified")


if __name__ == "__main__":
    DwdMasternodeChainLockTest().main()
