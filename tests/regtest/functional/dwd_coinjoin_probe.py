#!/usr/bin/env python3
# Copyright (c) 2026 The Dash Wallet Desktop developers
# Distributed under the MIT software license, see test_framework/COPYING.
"""dwd_coinjoin_probe.py

Probe: can stock dashd wallets complete real CoinJoin mixing rounds against masternodes on
regtest, using release binaries and the vendored DashTestFramework? No upstream functional
test does this (rpc_coinjoin.py only fabricates mixed outputs), so this records whether the
harness can host the WS-06 interop suite.

Network: node0 controller/miner, nodes 1..3 mixing wallets, nodes 4..7 masternodes.
Pass condition: at least two mixing wallets own outputs with coinjoin_rounds >= 1, and at
least one confirmed transaction spends inputs from two different mixing wallets.
"""

import time
from decimal import Decimal

from test_framework.test_framework import DashTestFramework
from test_framework.util import assert_greater_than

MIXERS = (1, 2, 3)
# keypool/backups: the framework's keypool=1 + -createwalletbackups=0 defaults make the client's
# automatic-backup check stop mixing (see upstream rpc_coinjoin.py).
MIXER_ARGS = [
    "-keypool=200",
    "-createwalletbackups=10",
    "-coinjoinamount=4",
    "-coinjoinrounds=2",
    "-coinjoinsessions=1",
    "-coinjoindenomsgoal=5",
    "-debug=coinjoin",
]
DEADLINE_SECONDS = 25 * 60


class DwdCoinJoinProbe(DashTestFramework):
    def add_options(self, parser):
        self.add_wallet_options(parser)

    def set_test_params(self):
        extra_args = [["-debug=coinjoin"]] + [MIXER_ARGS] * len(MIXERS) + [["-debug=coinjoin"]] * 4
        self.set_dash_test_params(8, 4, extra_args)

    def mixed_outputs(self, node):
        return [u for u in node.listunspent() if u.get("coinjoin_rounds", 0) >= 1]

    def run_test(self):
        controller = self.nodes[0]
        # With ChainLocks enabled, miners only include a transaction once it has an InstantSend
        # lock or has been in the mempool for 10 minutes, so quorums come first.
        controller.sporkupdate("SPORK_17_QUORUM_DKG_ENABLED", 0)
        self.wait_for_sporks_same()
        self.mine_cycle_quorum()
        if len(controller.quorum("list")["llmq_test"]) == 0:
            self.mine_quorum(llmq_type_name="llmq_test", llmq_type=100)
        funding = []
        for idx in MIXERS:
            node = self.nodes[idx]
            for _ in range(3):
                funding.append(controller.sendtoaddress(node.getnewaddress(), Decimal("3.3")))
        self.wait_for_instantlock(*funding, timeout=60)
        self.generate(controller, 2)
        for idx in MIXERS:
            self.log.info(f"mixer {idx} balance {self.nodes[idx].getbalance()}")
            self.nodes[idx].coinjoin("start")

        start = time.time()
        last_report = 0.0
        iteration = 0
        while time.time() - start < DEADLINE_SECONDS:
            iteration += 1
            # Real time drives the client's 5-15 tick auto-denominate cadence; mocktime drives
            # queue/session timeouts (COINJOIN_QUEUE_TIMEOUT = 30 s) on clients and masternodes.
            time.sleep(1)
            self.bump_mocktime(5)
            if iteration % 10 == 0:
                self.generate(controller, 1)
            mixed = {idx: self.mixed_outputs(self.nodes[idx]) for idx in MIXERS}
            if time.time() - last_report > 30:
                last_report = time.time()
                for idx in MIXERS:
                    info = self.nodes[idx].getcoinjoininfo()
                    sessions = [(s.get("state"), s.get("entries_count"), s.get("denomination")) for s in info.get("sessions", [])]
                    balances = self.nodes[idx].getbalances()["mine"]
                    self.log.info(
                        f"t={int(time.time() - start)}s mixer {idx}: running={info.get('running')} "
                        f"queue={info.get('queue_size')} sessions={sessions} "
                        f"mixed_outputs={len(mixed[idx])} coinjoin_balance={balances.get('coinjoin')}"
                    )
            wallets_with_mixed = [idx for idx, outs in mixed.items() if outs]
            if len(wallets_with_mixed) >= 2:
                break
        else:
            raise AssertionError(f"no two wallets completed a CoinJoin round within {DEADLINE_SECONDS}s")

        elapsed = int(time.time() - start)
        self.log.info(f"wallets with mixed outputs after {elapsed}s: {wallets_with_mixed}")

        # Find a mixing transaction with inputs from two mixing wallets, then check it is mined.
        # Mixing txs are mined promptly only once InstantSend-locked (ChainLocks mining gate).
        mixed_txids = {u["txid"] for idx in wallets_with_mixed for u in self.mixed_outputs(self.nodes[idx])}
        unconfirmed = [txid for txid in mixed_txids if txid in set(controller.getrawmempool())]
        if unconfirmed:
            self.wait_for_instantlock(*unconfirmed, timeout=60)
        self.generate(controller, 1)
        shared = []
        for txid in mixed_txids:
            tx = controller.getrawtransaction(txid, True)
            owners = set()
            for vin in tx["vin"]:
                prev = controller.getrawtransaction(vin["txid"], True)
                address = prev["vout"][vin["vout"]]["scriptPubKey"].get("address")
                owners.update(idx for idx in MIXERS if self.nodes[idx].getaddressinfo(address)["ismine"])
            values = sorted({str(vout["value"]) for vout in tx["vout"]})
            self.log.info(
                f"mix tx {txid}: {len(tx['vin'])} inputs, {len(tx['vout'])} outputs of {values}, "
                f"input owners {sorted(owners)}, confirmations={tx.get('confirmations', 0)}, "
                f"instantlock={tx.get('instantlock')}, chainlock={tx.get('chainlock')}"
            )
            if len(owners) >= 2 and tx.get("confirmations", 0) >= 1:
                shared.append(txid)
        assert_greater_than(len(shared), 0)
        self.log.info(f"CoinJoin mixing on regtest verified: {len(shared)} multi-party mix tx(s), elapsed {elapsed}s")


if __name__ == "__main__":
    DwdCoinJoinProbe().main()
