"""L1 suite `l1-sync`: the engine syncs against dashd over SPV and keeps history across restarts.

Drives the `dwcli` binary (set DWCLI to its path; the suite is skipped otherwise):

1. create an encrypted vault and import a known mnemonic through it (the vault path, review H-1);
2. SPV-sync to the node's tip; the balance becomes known and is zero;
3. dashd pays the wallet's first receive address and mines a block;
4. the payment is in the history as a dash-qt "Received with" record with the right amount,
   and the confirmed balance equals it;
5. a fresh dwcli process (no sync) still shows the record and the balance (restart keeps history).

Sync waits on heights and txids, not on `caught_up`: a single regtest node has no quorums, so
dash-spv's masternode phase never finishes (dashd answers its `getqrinfo` with "Cannot find quorum
snapshot"), and dash-spv never reaches its steady state there.

    DWD_COMPOSE_PROJECT=dwd-e1 DWD_REGTEST_BUILD=0 DWCLI=<target>/debug/dwcli \\
        .venv/bin/python -m pytest -v tests/test_l1_sync.py
"""

from __future__ import annotations

import os
import subprocess
from decimal import Decimal
from pathlib import Path

import pytest

DWCLI = os.environ.get("DWCLI")
pytestmark = pytest.mark.skipif(not DWCLI, reason="set DWCLI to the dwcli binary to run the l1-sync suite")

# BIP39 test vector (all-zero entropy); its first regtest receive address is m/44'/1'/0'/0/0.
MNEMONIC = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
AMOUNT = Decimal("1.5")
AMOUNT_DUFFS = 150_000_000


class Dwcli:
    def __init__(self, datadir: Path, passphrase_file: Path, p2p_port: int):
        self.base = [
            DWCLI,
            "--datadir", str(datadir),
            "--network", "regtest",
            # Nothing listens on these: L1 sync needs no Platform.
            "--dapi", "http://127.0.0.1:1",
            "--quorum-url", "http://127.0.0.1:1",
            "--peer", f"127.0.0.1:{p2p_port}",
            "--passphrase-file", str(passphrase_file),
        ]

    def run(self, *args: str, stdin: str | None = None, timeout: float = 300) -> str:
        proc = subprocess.run(
            [*self.base, *args], input=stdin, capture_output=True, text=True, timeout=timeout
        )
        if proc.returncode != 0:
            raise AssertionError(f"dwcli {' '.join(args)} failed ({proc.returncode}):\n{proc.stdout}\n{proc.stderr}")
        # Shown by pytest for failing tests, or always with -s.
        print(f"$ dwcli {' '.join(args)}\n{proc.stdout}{proc.stderr}")
        return proc.stdout


def wallet_line(output: str) -> str:
    lines = [line for line in output.splitlines() if " name=" in line]
    assert len(lines) == 1, output
    return lines[0]


def history_rows(output: str) -> list[list[str]]:
    return [line.split(" ") for line in output.splitlines() if line.strip()]


def test_import_sync_receive_and_restart(tmp_path, regtest_node, funded_miner, mine):
    passphrase = tmp_path / "passphrase"
    passphrase.write_text("l1 sync passphrase\n")
    cli = Dwcli(tmp_path / "data", passphrase, regtest_node.p2p_port)

    cli.run("init-vault")
    wallet_id = cli.run("import", "--birth-height", "0", stdin=MNEMONIC).split()[1]
    assert len(wallet_id) == 64

    address = cli.run("receive").split()[1]
    assert regtest_node.rpc.validateaddress(address)["isvalid"]

    tip = regtest_node.rpc.getblockcount()
    out = cli.run("sync", "--min-height", str(tip), "--timeout-secs", "240")
    assert "balance=unknown" not in out, out
    assert "confirmed=0 " in wallet_line(out), out

    txid = funded_miner.sendtoaddress(address, AMOUNT)
    mine(1)
    out = cli.run("sync", "--txid", txid, "--confirmations", "1", "--timeout-secs", "240")
    line = wallet_line(out)
    assert f"confirmed={AMOUNT_DUFFS} " in line, out
    assert f"total={AMOUNT_DUFFS} " in line, out

    rows = history_rows(cli.run("history"))
    assert len(rows) == 1, rows
    row = rows[0]
    assert row[0] == txid
    assert row[2] == "RecvWithAddress"
    assert int(row[3]) == AMOUNT_DUFFS
    assert row[4] == "Confirming"
    assert int(row[5]) >= 1
    assert row[6] == address

    # The paid address is used now: the current receive address moved on.
    assert cli.run("receive").split()[1] != address

    # Restart: a new process without SPV still has the history and the balance.
    rows_after = history_rows(cli.run("history"))
    assert [r[0] for r in rows_after] == [txid]
    assert int(rows_after[0][3]) == AMOUNT_DUFFS
    assert f"confirmed={AMOUNT_DUFFS} " in wallet_line(cli.run("list"))

    # Six more blocks: the record is Confirmed after another sync.
    mine(6)
    cli.run("sync", "--txid", txid, "--confirmations", "7", "--timeout-secs", "240")
    rows = history_rows(cli.run("history"))
    assert rows[0][4] == "Confirmed", rows
