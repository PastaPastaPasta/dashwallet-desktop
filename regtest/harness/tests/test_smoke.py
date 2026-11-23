"""Smoke test: a fresh regtest dashd mines, funds an address and reports the expected balances."""

import socket
from decimal import Decimal

from dwd_regtest import COINBASE_MATURITY_BLOCKS, wait_until


def test_node_is_fresh_regtest(regtest_node):
    info = regtest_node.rpc.getblockchaininfo()
    assert info["chain"] == "regtest"
    network = regtest_node.rpc.getnetworkinfo()
    assert network["version"] >= 240000, network["subversion"]
    # The node runs the indexes SPV clients and the harness rely on.
    indexes = regtest_node.rpc.getindexinfo()
    assert "txindex" in indexes
    assert "basic block filter index" in indexes


def test_p2p_port_accepts_inbound_peers(regtest_node):
    """SPV clients connect to the forwarded P2P port; dashd must register them as inbound peers."""
    with socket.create_connection((regtest_node.host, regtest_node.p2p_port), timeout=10):
        wait_until(
            lambda: any(peer["inbound"] for peer in regtest_node.rpc.getpeerinfo()),
            timeout=15,
            what="an inbound peer on the P2P port",
        )


def test_mine_fund_and_balance(regtest_node, funded_miner, mine, send_to_address, wait_for_tx):
    height = regtest_node.rpc.getblockcount()
    assert height >= COINBASE_MATURITY_BLOCKS
    assert funded_miner.getbalance() > 0

    recipient = regtest_node.ensure_wallet("recipient")
    address = recipient.getnewaddress()
    amount = Decimal("12.34567891")

    txid = send_to_address(address, amount)
    mempool_tx = wait_for_tx(txid, confirmations=0)
    assert any(vout["value"] == amount for vout in mempool_tx["vout"])
    # The recipient wallet processes mempool transactions asynchronously from the RPC that sent it.
    wait_until(
        lambda: recipient.getbalances()["mine"]["untrusted_pending"] == amount,
        timeout=15,
        what="recipient wallet to see the pending payment",
    )

    mine(1)
    confirmed = wait_for_tx(txid, confirmations=1)
    assert confirmed["confirmations"] == 1
    assert regtest_node.rpc.getblockcount() == height + 1
    assert recipient.getbalance() == amount
    assert recipient.getreceivedbyaddress(address) == amount
    assert regtest_node.rpc.getmempoolinfo()["size"] == 0
