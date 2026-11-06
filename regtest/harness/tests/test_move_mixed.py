"""move-mixed: the CoinJoin account's coins consolidated, then moved to Core.

Covers "move mixed coins" against dashd v24 regtest without mixing (no
masternodes here): the wallet's CoinJoin account is funded directly, then

1. `consolidate-mixed` sweeps its 60 coins onto one fresh address of the
   same CoinJoin account (the round a move to Shielded needs first:
   dashwallet-iOS #1221), so a move plan afterwards still finds the value in
   the CoinJoin account, as one coin;
2. `move-mixed` sweeps that coin to a BIP44 receive address of the wallet,
   after which the CoinJoin account has nothing left to move.

Needs a dwcli built with the test-only `mix-shield-test` feature (fresh
CoinJoin-account addresses): `cargo build -p dwcli --features
mix-shield-test`, then `DWCLI=/path/to/dwcli` as for test_l1_send.py. The
suite is skipped when the binary lacks the feature.

With one peer and no InstantSend quorum, dash-spv only accepts a broadcast
once it is mined, so each sweep runs in the background while the test mines
whatever reaches dashd's mempool.

`DWD_MOVE_MIXED_COINS` (default 60, at most 500: one chunk) sets the number
of coins; 490 proves a near-full chunk of confirmed coins against v24's
standardness and mempool limits.
"""

from __future__ import annotations

import os
import subprocess
import threading
import time
from decimal import Decimal

import pytest

from test_l1_send import DWCLI, Dwcli, duffs, fields, tx_fee

COINS = int(os.environ.get("DWD_MOVE_MIXED_COINS", "60"))
COIN_VALUE = Decimal("0.1")


def _has_test_feature() -> bool:
    if DWCLI is None:
        return False
    out = subprocess.run([str(DWCLI), "--help"], capture_output=True, text=True, check=False)
    return "coinjoin-test-addresses" in out.stdout


pytestmark = pytest.mark.skipif(
    not _has_test_feature(), reason="dwcli without the mix-shield-test feature (set DWCLI)"
)


def sweep_in_background(dw: Dwcli, node, *args: str) -> tuple[list[str], list[str]]:
    """Runs a sweep command, mining every block dashd's mempool needs until
    it exits. Returns dwcli's output and the txids mined meanwhile."""
    proc = subprocess.Popen(
        [*dw.base, *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    out: list[str] = []
    err: list[str] = []
    readers = [
        threading.Thread(target=lambda: out.extend(proc.stdout.read().splitlines()), daemon=True),
        threading.Thread(target=lambda: err.append(proc.stderr.read()), daemon=True),
    ]
    for r in readers:
        r.start()
    mined: list[str] = []
    deadline = time.monotonic() + 300
    while proc.poll() is None:
        if time.monotonic() > deadline:
            proc.kill()
            raise AssertionError(f"dwcli {' '.join(args)} timed out: {out}\n{''.join(err)[-4000:]}")
        pool = node.rpc.getrawmempool()
        if pool:
            mined.extend(pool)
            node.mine(1)
        time.sleep(1)
    for r in readers:
        r.join(timeout=30)
    if proc.returncode != 0:
        raise AssertionError(
            f"dwcli {' '.join(args)} exited {proc.returncode}: {out}\n{''.join(err)[-4000:]}"
        )
    return out, mined


def plan(dw: Dwcli, command: str, height: int) -> tuple[dict[str, str], list[dict[str, str]]]:
    lines = dw.run(command, dw.wallet, "--plan-only", "--sync-height", str(height))
    chunks = [fields(line) for line in lines if line.startswith("chunk ")]
    return fields(dw.line("plan", lines)), chunks


@pytest.fixture(scope="module")
def dw(regtest_node, funded_miner, tmp_path_factory) -> Dwcli:
    root = tmp_path_factory.mktemp("dwcli-move-mixed")
    passfile = root / "pass"
    passfile.write_text("move mixed passphrase\n")
    passfile.chmod(0o600)
    cli = Dwcli(root / "data", f"{regtest_node.host}:{regtest_node.p2p_port}", passfile)
    cli.run("init-vault")
    cli.wallet = cli.line("wallet_id", cli.run("create")).split()[1]
    return cli


@pytest.fixture(scope="module")
def funded(dw, regtest_node) -> tuple[list[str], int]:
    """COINS coins of 0.1 DASH on as many CoinJoin-account addresses, mined."""
    lines = dw.run("coinjoin-test-addresses", dw.wallet, "--count", str(COINS))
    addresses = [line.split()[1] for line in lines if line.startswith("cjaddress ")]
    assert len(set(addresses)) == COINS, lines
    regtest_node.wallet("miner").sendmany("", {a: COIN_VALUE for a in addresses})
    regtest_node.mine(1)
    return addresses, regtest_node.rpc.getblockcount()


def test_consolidation_keeps_the_value_in_the_coinjoin_account(dw, regtest_node, funded):
    addresses, height = funded
    total = COINS * duffs(COIN_VALUE)

    head, chunks = plan(dw, "consolidate-mixed", height)
    assert int(head["total"]) == total, head
    assert int(head["fully_mixed"]) + int(head["not_fully_mixed"]) == total, head
    assert (head["chunks"], head["coins_after"], head["unconfirmed"]) == ("1", "1", "0"), head
    assert [c["inputs"] for c in chunks] == [str(COINS)], chunks

    out, mined = sweep_in_background(
        dw, regtest_node, "consolidate-mixed", dw.wallet, "--sync-height", str(height), "--yes"
    )
    moved = fields(dw.line("moved", out))
    assert (moved["remaining"], moved["failure"]) == ("0", "-"), out
    [txid] = moved["txids"].split(",")
    assert txid in mined, (txid, mined)

    tx = regtest_node.rpc.getrawtransaction(txid, True)
    assert len(tx["vin"]) == COINS and len(tx["vout"]) == 1, tx
    output = tx["vout"][0]["scriptPubKey"]["address"]
    assert output not in addresses, "the output goes to a fresh address"
    fee = tx_fee(regtest_node, tx)
    assert duffs(tx["vout"][0]["value"]) == total - fee
    assert int(chunks[0]["fee"]) == fee

    # The move plan reads the CoinJoin account only: the value is still
    # there, as one coin.
    after, after_chunks = plan(dw, "move-mixed", regtest_node.rpc.getblockcount())
    assert int(after["total"]) == total - fee, after
    assert [c["inputs"] for c in after_chunks] == ["1"], after_chunks


def test_a_move_to_the_wallet_empties_the_coinjoin_account(dw, regtest_node, funded):
    height = regtest_node.rpc.getblockcount()
    head, _ = plan(dw, "move-mixed", height)
    value = int(head["total"])
    assert value > 0, head

    out, mined = sweep_in_background(dw, regtest_node, "move-mixed", dw.wallet, "--sync-height", str(height), "--yes")
    moved = fields(dw.line("moved", out))
    assert moved["failure"] == "-", out
    [txid] = moved["txids"].split(",")
    assert txid in mined
    tx = regtest_node.rpc.getrawtransaction(txid, True)
    fee = tx_fee(regtest_node, tx)
    assert len(tx["vout"]) == 1 and duffs(tx["vout"][0]["value"]) == value - fee
    to = tx["vout"][0]["scriptPubKey"]["address"]

    height = regtest_node.rpc.getblockcount()
    refused = subprocess.run(
        [*dw.base, "move-mixed", dw.wallet, "--plan-only", "--sync-height", str(height)],
        capture_output=True,
        text=True,
        timeout=300,
        check=False,
    )
    assert refused.returncode != 0, refused.stdout
    assert "no mixed coins to move" in (refused.stdout + refused.stderr), refused.stderr[-2000:]

    dw.sync(height)
    coins = dw.utxos()
    assert [(amount, f["address"]) for _, amount, f in coins] == [(value - fee, to)], coins
