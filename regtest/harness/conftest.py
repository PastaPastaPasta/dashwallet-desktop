"""pytest fixtures for the regtest harness.

Backend selection (see dwd_regtest.node.node_from_env):
  DWD_REGTEST_BACKEND=docker  (default) docker compose service from regtest/docker-compose.yml
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
    """One fresh regtest dashd for the whole session, stopped (and its chain discarded) afterwards.

    A test module's `DASHD_ARGS` (a tuple of dashd options) is added to its command line.
    """
    modules = {getattr(item, "module", None) for item in request.session.items}
    extra = sorted({arg for module in modules for arg in getattr(module, "DASHD_ARGS", ())})
    node = node_from_env(extra_args=tuple(extra))
    started = False
    try:
        # start() is inside the try so a failed start still tears down containers/processes.
        node.start()
        started = True
        yield node
    finally:
        if not started or request.session.testsfailed:
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


# ---- DashPay suites (DASHPAY §6 T2 and T3; regtest/README.md "DashPay suites") ----

from dwd_regtest.dashpay import Harness, befriend, dwcli_path, make_member  # noqa: E402


@pytest.fixture(scope="session")
def dp() -> Iterator[Harness]:
    """The DashPay run: skipped unless DW_DP_NET names a network and dwcli is built. At the end
    every wallet the run created is closed, and the run fails if a log holds a secret."""
    harness = Harness.from_env()
    if harness is None:
        pytest.skip("set DW_DP_NET (testnet or devnet) to run the DashPay suites")
    if dwcli_path() is None:
        pytest.skip("no dwcli binary (set DWCLI)")
    print(f"DashPay run {harness.run_id} on {harness.net.tier}, logs in {harness.logs}")
    yield harness
    leaks = harness.close()
    assert not leaks, f"a secret is in the run's logs: {leaks}"


@pytest.fixture(scope="session")
def funder(dp):
    return dp.funder()


@pytest.fixture(scope="session")
def pair(dp, funder):
    """Two members on their own data roots, A and B, each with a registered identity; not yet
    contacts."""
    a = make_member(dp, funder, "a", "dwdpa", 20_000_000)
    b = make_member(dp, funder, "b", "dwdpb", 20_000_000)
    return a, b


@pytest.fixture(scope="session")
def contacts(pair):
    """The round trip A → B → A, done once per run; what each side saw on the way."""
    return befriend(*pair)
