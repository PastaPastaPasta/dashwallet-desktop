"""pytest fixtures for the regtest harness.

Backend selection (see dwd_regtest.node.node_from_env):
  DWD_REGTEST_BACKEND=docker  (default) docker compose service from tests/regtest/docker-compose.yml
  DWD_REGTEST_BACKEND=local   dashd from DASHCORE_DIR (unpacked release)
  DWD_REGTEST_BUILD=0         docker backend: skip `--build` (use an already built image)
"""

from __future__ import annotations

from decimal import Decimal
from typing import Callable, Iterator

import pytest

from dwd_regtest import COINBASE_MATURITY_BLOCKS, RegtestNode, node_from_env


@pytest.fixture(scope="session")
def regtest_node(request: pytest.FixtureRequest) -> Iterator[RegtestNode]:
    """One fresh regtest dashd for the whole session, stopped (and its chain discarded) afterwards."""
    node = node_from_env()
    node.start()
    try:
        yield node
    finally:
        if request.session.testsfailed:
            # Keep the tail of dashd's console log in the pytest report for failed sessions.
            print("\n--- dashd log (tail) ---\n" + "\n".join(node.logs().splitlines()[-200:]))
        node.stop()


@pytest.fixture
def mine(regtest_node: RegtestNode) -> Callable[..., list[str]]:
    """mine(blocks, address=None) -> block hashes. Defaults to paying the `miner` wallet."""
    return regtest_node.mine


@pytest.fixture
def send_to_address(regtest_node: RegtestNode) -> Callable[..., str]:
    """send_to_address(address, amount, from_wallet="miner") -> txid."""
    return regtest_node.send_to_address


@pytest.fixture
def wait_for_tx(regtest_node: RegtestNode) -> Callable[..., dict]:
    """wait_for_tx(txid, confirmations=0, timeout=30) -> decoded transaction."""
    return regtest_node.wait_for_tx


@pytest.fixture(scope="session")
def funded_miner(regtest_node: RegtestNode):
    """The `miner` wallet with at least one mature coinbase (mines 101 blocks on first use)."""
    miner = regtest_node.ensure_wallet("miner")
    if miner.getbalance() == Decimal(0):
        regtest_node.mine(COINBASE_MATURITY_BLOCKS, miner.getnewaddress())
    return miner
