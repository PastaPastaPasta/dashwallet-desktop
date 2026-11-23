"""Gives a dwcli regtest wallet a history for the Tauri --selftest (tauri-selftest.yml).

    python ci/github/regtest-fund.py DASHCORE_DIR DWCLI DATA_ROOT PASSPHRASE_FILE

Starts a local regtest dashd from DASHCORE_DIR (regtest/scripts/fetch-dashcore.sh), mines a
mature coinbase, pays the wallet's receive address, mines the payment into a block, and runs
`dwcli sync` until the wallet holds it with one confirmation. dwcli keeps the history in the
data root, so the app, launched later without a peer, shows the transaction. Prints the txid.

Standard library only (the regtest harness's dwd_regtest is too), so it runs on the macOS,
Windows and Linux runners as is.
"""

from __future__ import annotations

import subprocess
import sys
from decimal import Decimal
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "regtest" / "harness"))

from dwd_regtest import COINBASE_MATURITY_BLOCKS, LocalBinaryNode  # noqa: E402

AMOUNT = Decimal("1.5")
SYNC_SECS = "240"


def main() -> int:
    if len(sys.argv) != 5:
        print(__doc__, file=sys.stderr)
        return 2
    dashcore, dwcli, data_root, passphrase_file = sys.argv[1:]
    node = LocalBinaryNode(dashcore_dir=Path(dashcore))
    node.start()
    try:
        cli = [
            dwcli,
            "--datadir", data_root,
            "--network", "regtest",
            "--no-platform",
            "--dapi", "https://127.0.0.1:1",
            "--peer", f"127.0.0.1:{node.p2p_port}",
            "--passphrase-file", passphrase_file,
        ]

        def dwcli_run(*args: str) -> str:
            proc = subprocess.run([*cli, *args], capture_output=True, text=True, timeout=600)
            if proc.returncode != 0:
                raise RuntimeError(f"dwcli {' '.join(args)} failed ({proc.returncode}):\n{proc.stdout}\n{proc.stderr}")
            return proc.stdout

        miner = node.ensure_wallet("miner")
        node.mine(COINBASE_MATURITY_BLOCKS, miner.getnewaddress())
        address = dwcli_run("receive").split()[1]
        # The wallet scans the chain first, as in the L1 sync suite (regtest/harness/tests).
        dwcli_run("sync", "--min-height", str(node.rpc.getblockcount()), "--timeout-secs", SYNC_SECS)
        txid = miner.sendtoaddress(address, AMOUNT)
        node.mine(1)
        dwcli_run("sync", "--txid", txid, "--confirmations", "1", "--timeout-secs", SYNC_SECS)
        history = dwcli_run("history")
        if txid not in history:
            raise RuntimeError(f"dwcli history lacks {txid}:\n{history}")
        print(txid)
        return 0
    except Exception:
        print("--- dashd log (tail) ---", file=sys.stderr)
        print("\n".join(node.logs().splitlines()[-40:]), file=sys.stderr)
        raise
    finally:
        node.stop()


if __name__ == "__main__":
    sys.exit(main())
