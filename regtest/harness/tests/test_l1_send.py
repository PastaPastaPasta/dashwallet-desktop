"""l1-send: dwcli (dw-engine over SPV) pays dashd on regtest.

Covers: receive and sync over SPV, a payment back to dashd at a custom fee
rate, coin control with a chosen outpoint, a locked coin left out of
automatic selection, subtract-fee-from-amount, dashd receiving exact
amounts, and signmessage / verifymessage in both directions.

Needs a dwcli binary built from this tree: `DWCLI=/path/to/dwcli` (default:
`$CARGO_TARGET_DIR/debug/dwcli`, else `rust/target/debug/dwcli`). The suite
is skipped when none exists. Run it with its own compose project, e.g.

    DWD_COMPOSE_PROJECT=dwd-e2 DWD_REGTEST_BUILD=0 DWCLI=... \\
        .venv/bin/python -m pytest -v tests/test_l1_send.py

Every dwcli call is a separate process that opens the session, starts SPV,
waits for the wallet to reach the chain tip and shuts down again, the way a
user restarts the app. With one peer and no InstantSend quorum, dash-spv
only accepts a broadcast once it is mined, so payments run in the
background while the test mines the transaction from dashd's mempool.
"""

from __future__ import annotations

import os
import queue
import re
import subprocess
import threading
import time
from decimal import Decimal
from pathlib import Path

import pytest

from dwd_regtest import wait_until

COIN = 100_000_000
REPO = Path(__file__).resolve().parents[3]


def _dwcli_path() -> Path | None:
    candidates = []
    if os.environ.get("DWCLI"):
        candidates.append(Path(os.environ["DWCLI"]))
    if os.environ.get("CARGO_TARGET_DIR"):
        candidates.append(Path(os.environ["CARGO_TARGET_DIR"]) / "debug" / "dwcli")
    candidates.append(REPO / "rust" / "target" / "debug" / "dwcli")
    return next((c for c in candidates if c.exists()), None)


DWCLI = _dwcli_path()
pytestmark = pytest.mark.skipif(DWCLI is None, reason="no dwcli binary (set DWCLI)")


def duffs(amount: Decimal) -> int:
    return int((amount * COIN).to_integral_exact())


def fields(line: str) -> dict[str, str]:
    """`key=value` pairs of a dwcli output line."""
    return dict(re.findall(r"(\w+)=(\S+)", line))


class Dwcli:
    """One wallet driven through dwcli processes."""

    def __init__(self, datadir: Path, peer: str, passfile: Path):
        self.base = [
            str(DWCLI),
            "--datadir",
            str(datadir),
            "--network",
            "regtest",
            "--dapi",
            "http://127.0.0.1:1",
            "--quorum-url",
            "http://127.0.0.1:1",
            "--peer",
            peer,
            "--passphrase-file",
            str(passfile),
        ]
        self.wallet = ""

    def run(self, *args: str, timeout: float = 300) -> list[str]:
        proc = subprocess.run(
            [*self.base, *args], capture_output=True, text=True, timeout=timeout, check=False
        )
        if proc.returncode != 0:
            raise AssertionError(
                f"dwcli {' '.join(args)} exited {proc.returncode}\n"
                f"stdout:\n{proc.stdout}\nstderr (tail):\n{proc.stderr[-4000:]}"
            )
        return proc.stdout.splitlines()

    def line(self, prefix: str, lines: list[str]) -> str:
        found = [line for line in lines if line.startswith(prefix + " ")]
        assert found, f"no {prefix!r} line in {lines}"
        return found[0]

    def address(self) -> str:
        return self.line("address", self.run("address", self.wallet)).split()[1]

    def sync(self, height: int) -> dict[str, str]:
        return fields(self.line("synced", self.run("sync-wallet", self.wallet, "--height", str(height))))

    def utxos(self, *extra: str) -> list[tuple[str, int, dict[str, str]]]:
        out = []
        for line in self.run("utxos", self.wallet, *extra):
            if line.startswith("utxo "):
                parts = line.split()
                out.append((parts[1], int(parts[2]), fields(line)))
        return out

    def pay(self, node, height: int, *args: str) -> tuple[list[str], dict]:
        """Runs `send` in the background, mines its transaction once dashd
        has it, and returns dwcli's output and dashd's decoded transaction."""
        proc = subprocess.Popen(
            [*self.base, "send", self.wallet, "--sync-height", str(height), *args],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        lines: list[str] = []
        stderr: list[str] = []
        threading.Thread(target=lambda: stderr.append(proc.stderr.read()), daemon=True).start()
        txid = None
        deadline = time.monotonic() + 300
        for line in proc.stdout:
            lines.append(line.rstrip("\n"))
            if line.startswith("prepared "):
                txid = fields(line)["txid"]
                break
            if time.monotonic() > deadline:
                break
        if txid is None:
            proc.wait(timeout=30)
            raise AssertionError(f"send never prepared: {lines}\n{''.join(stderr)[-4000:]}")
        wait_until(
            lambda: txid in node.rpc.getrawmempool(),
            timeout=90,
            what=f"tx {txid} in dashd's mempool",
        )
        node.mine(1)
        rest, _ = proc.communicate(timeout=180)
        lines.extend(rest.splitlines())
        if proc.returncode != 0:
            raise AssertionError(f"send exited {proc.returncode}: {lines}\n{''.join(stderr)[-4000:]}")
        assert f"broadcast {txid}" in lines, lines
        return lines, node.rpc.getrawtransaction(txid, True)


def tx_fee(node, tx: dict) -> int:
    spent = 0
    for vin in tx["vin"]:
        prev = node.rpc.getrawtransaction(vin["txid"], True)
        spent += duffs(prev["vout"][vin["vout"]]["value"])
    return spent - sum(duffs(v["value"]) for v in tx["vout"])


def paid_to(tx: dict, address: str) -> int:
    return sum(
        duffs(v["value"])
        for v in tx["vout"]
        if v["scriptPubKey"].get("address") == address
        or address in v["scriptPubKey"].get("addresses", [])
    )


@pytest.fixture(scope="module")
def dw(regtest_node, funded_miner, tmp_path_factory) -> Dwcli:
    root = tmp_path_factory.mktemp("dwcli")
    passfile = root / "pass"
    passfile.write_text("l1 send passphrase\n")
    cli = Dwcli(root / "data", f"{regtest_node.host}:{regtest_node.p2p_port}", passfile)
    cli.run("init-vault")
    created = cli.run("create")
    cli.wallet = cli.line("wallet_id", created).split()[1]
    return cli


@pytest.fixture(scope="module")
def recipient(regtest_node):
    return regtest_node.ensure_wallet("l1-recipient")


@pytest.fixture(scope="module")
def funded(dw, regtest_node):
    """10 DASH and 5 DASH on two receive addresses, confirmed and synced."""
    for amount in (Decimal(10), Decimal(5)):
        regtest_node.send_to_address(dw.address(), amount)
    regtest_node.mine(1)
    height = regtest_node.rpc.getblockcount()
    synced = dw.sync(height)
    assert int(synced["total"]) == 15 * COIN, synced
    return height


def test_receive_and_sync(dw, funded):
    coins = dw.utxos()
    assert sorted(amount for _, amount, _ in coins) == [5 * COIN, 10 * COIN]
    assert all(int(f["conf"]) >= 1 for _, _, f in coins)


def test_send_back_with_custom_fee(dw, funded, regtest_node, recipient):
    address = recipient.getnewaddress()
    lines, tx = dw.pay(regtest_node, funded, "--to", f"{address}:{COIN}", "--fee-per-kb", "5000")
    estimate = fields(dw.line("estimate", lines))
    prepared = fields(dw.line("prepared", lines))
    assert paid_to(tx, address) == COIN
    fee = tx_fee(regtest_node, tx)
    assert fee == int(prepared["fee"]) == int(estimate["fee"])
    # 5 duff/byte on key-wallet's size estimate (148 per input, 34 per output, +10).
    assert fee == 5 * int(estimate["size"]), (fee, estimate)
    assert recipient.getreceivedbyaddress(address, 0) == Decimal(1)


def test_coin_control_spends_the_chosen_outpoint(dw, regtest_node, recipient):
    height = regtest_node.rpc.getblockcount()
    dw.sync(height)
    coins = dw.utxos()
    chosen, value, _ = min(coins, key=lambda c: c[1])
    address = recipient.getnewaddress()
    _, tx = dw.pay(regtest_node, height, "--to", f"{address}:{COIN // 2}", "--coin", chosen)
    spent = [f"{vin['txid']}:{vin['vout']}" for vin in tx["vin"]]
    assert spent == [chosen], (spent, chosen, value)
    assert paid_to(tx, address) == COIN // 2


def test_locked_coin_is_not_selected(dw, regtest_node, recipient):
    height = regtest_node.rpc.getblockcount()
    dw.sync(height)
    coins = dw.utxos()
    assert len(coins) >= 2, coins
    largest, _, _ = max(coins, key=lambda c: c[1])
    dw.run("lock", dw.wallet, largest)
    assert dw.line("locked", dw.run("locked", dw.wallet)).split()[1] == largest
    # The locked coin is hidden unless asked for, and flagged.
    assert largest not in [c[0] for c in dw.utxos()]
    assert any(c[0] == largest and c[2]["locked"] == "1" for c in dw.utxos("--all"))
    address = recipient.getnewaddress()
    _, tx = dw.pay(regtest_node, height, "--to", f"{address}:{COIN // 10}")
    spent = [f"{vin['txid']}:{vin['vout']}" for vin in tx["vin"]]
    assert largest not in spent, (largest, spent)
    dw.run("unlock", dw.wallet, largest)
    assert not [line for line in dw.run("locked", dw.wallet) if line.startswith("locked ")]


def test_subtract_fee_from_amount(dw, regtest_node, recipient):
    height = regtest_node.rpc.getblockcount()
    dw.sync(height)
    address = recipient.getnewaddress()
    lines, tx = dw.pay(regtest_node, height, "--to", f"{address}:{2 * COIN}:subtract")
    fee = tx_fee(regtest_node, tx)
    prepared = fields(dw.line("prepared", lines))
    assert fee == int(prepared["fee"])
    assert paid_to(tx, address) == 2 * COIN - fee
    assert int(prepared["sent"]) == 2 * COIN - fee
    assert recipient.getreceivedbyaddress(address, 0) == Decimal(2 * COIN - fee) / COIN


def test_foreign_change_address_counts_against_the_cap(dw, regtest_node, recipient):
    height = regtest_node.rpc.getblockcount()
    dw.sync(height)
    address = recipient.getnewaddress()
    change = recipient.getnewaddress()
    lines, tx = dw.pay(regtest_node, height, "--to", f"{address}:{COIN // 10}", "--change", change)
    estimate = fields(dw.line("estimate", lines))
    prepared = fields(dw.line("prepared", lines))
    assert paid_to(tx, address) == COIN // 10
    assert paid_to(tx, change) == int(estimate["change"])
    # The foreign change is spent from the wallet's point of view.
    assert int(prepared["external"]) == COIN // 10 + int(estimate["change"])
    assert int(prepared["debit"]) == int(prepared["external"]) + tx_fee(regtest_node, tx)


def test_rebroadcast_after_unknown_outcome(dw, regtest_node, recipient):
    """A broadcast with no acceptance verdict stays reserved and can be sent
    again through the same prepared transaction (review M3).

    With one peer and no InstantSend quorum nothing accepts the payment
    until it is mined, so leaving it unmined makes dash-spv report the
    outcome uncertain after its 60 s acceptance timeout: the engine returns
    send.broadcast_unknown. dwcli then checks the inputs are still held and
    abandon is refused, and broadcasts the same handle again; mining the
    transaction makes that repeat succeed with the same txid."""
    height = regtest_node.rpc.getblockcount()
    dw.sync(height)
    address = recipient.getnewaddress()
    proc = subprocess.Popen(
        [
            *dw.base,
            "send",
            dw.wallet,
            "--sync-height",
            str(height),
            "--to",
            f"{address}:{COIN // 10}",
            "--rebroadcast-unknown",
            "1",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    lines: queue.Queue[str] = queue.Queue()
    stderr: list[str] = []
    threading.Thread(
        target=lambda: [lines.put(line.rstrip("\n")) for line in proc.stdout], daemon=True
    ).start()
    threading.Thread(target=lambda: stderr.append(proc.stderr.read()), daemon=True).start()
    seen: list[str] = []

    def read_until(prefix: str, timeout: float) -> str:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                line = lines.get(timeout=1)
            except queue.Empty:
                if proc.poll() is not None and lines.empty():
                    break
                continue
            seen.append(line)
            if line.startswith(prefix):
                return line
        raise AssertionError(f"no {prefix!r} line: {seen}\n{''.join(stderr)[-4000:]}")

    try:
        txid = fields(read_until("prepared ", 300))["txid"]
        wait_until(
            lambda: txid in regtest_node.rpc.getrawmempool(),
            timeout=90,
            what=f"tx {txid} in dashd's mempool",
        )
        # No block: no verdict within the acceptance timeout.
        unknown = read_until("unknown ", 120)
        assert unknown.split()[1] == txid, unknown
        held = fields(read_until("held ", 30))
        inputs = sum(1 for line in seen if line.startswith("input "))
        assert inputs >= 1, seen
        # Still reserved, or already seen spent by the transaction itself;
        # never offered to another payment.
        assert int(held["reserved"]) + int(held["unlisted"]) == inputs, (held, seen)
        read_until("abandon refused", 30)
        read_until("rebroadcast 1", 30)
        regtest_node.mine(1)
        assert read_until("broadcast ", 120) == f"broadcast {txid}"
        assert proc.wait(timeout=60) == 0, f"{seen}\n{''.join(stderr)[-4000:]}"
    finally:
        if proc.poll() is None:
            proc.kill()
    tx = regtest_node.rpc.getrawtransaction(txid, True)
    assert paid_to(tx, address) == COIN // 10


def test_balance_after_payments(dw, regtest_node):
    height = regtest_node.rpc.getblockcount()
    synced = dw.sync(height)
    total = sum(amount for _, amount, _ in dw.utxos("--all"))
    assert int(synced["total"]) == total


def test_messages_round_trip_with_dashd(dw, regtest_node, funded_miner):
    address = dw.address()
    signature = dw.line("signature", dw.run("sign", dw.wallet, address, "dwd l1 message")).split()[1]
    assert regtest_node.rpc.verifymessage(address, signature, "dwd l1 message") is True
    assert regtest_node.rpc.verifymessage(address, signature, "another message") is False

    theirs = funded_miner.getnewaddress()
    their_sig = funded_miner.signmessage(theirs, "signed by dashd")
    assert "verified" in dw.run("verify", theirs, "signed by dashd", their_sig)
    with pytest.raises(AssertionError, match="not verified"):
        dw.run("verify", theirs, "tampered", their_sig)
