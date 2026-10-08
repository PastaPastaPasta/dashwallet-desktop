#!/usr/bin/env python3
"""Gate U1: what the Tauri screens show equals `dwcli` on the same datadir.

Compares the engine-mode self-test report (`shown`: the hero amount on Overview, its
full-precision tooltip, and the first 200 rows of Transactions, read from the DOM) with `dwcli list` and `dwcli history --limit 200`.
Amounts are compared in duffs after parsing the displayed strings, so the whole display path
(IPC, DTOs, formatting) is covered.

Usage: u1_compare.py --selftest selftest-engine.json --list list.txt --history history.txt
Exit status 0 when the balance and all 200 rows match; prints a JSON summary.
"""

import argparse
import json
import re
import sys
from decimal import Decimal

THIN = " "


def duffs(shown: str) -> int:
    """'+1 234.5' (thin-space grouped, compact) → signed duffs."""
    text = shown.replace(THIN, "").replace(" ", "").replace("+", "")
    return int((Decimal(text) * 100_000_000).to_integral_exact())


def floored(total: int, digits: int = 2) -> str:
    """The hero's `.floored(2)` string of a duff amount, without grouping."""
    whole, frac = divmod(total, 100_000_000)
    return f"{whole}.{str(frac).zfill(8)[:digits]}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", required=True)
    ap.add_argument("--list", required=True)
    ap.add_argument("--history", required=True)
    args = ap.parse_args()

    shown = json.load(open(args.selftest))["shown"]
    m = re.search(r"total=(\d+)", open(args.list).read())
    cli_total = int(m.group(1)) if m else None
    cli_rows = []
    for line in open(args.history):
        parts = line.split()
        if len(parts) >= 4 and re.fullmatch(r"[0-9a-f]{64}", parts[0]):
            cli_rows.append({"txid": parts[0], "recordIndex": int(parts[1]), "amount": int(parts[3])})

    hero = (shown["heroAmount"] or "").replace(THIN, "").replace(" ", "")
    # The tooltip carries every duff ("206 014.36392896 tDASH"): compare it exactly.
    full = (shown.get("heroFull") or "").rsplit(" ", 1)[0]
    full_duffs = duffs(full) if full else None
    balance_ok = cli_total is not None and hero == floored(cli_total) and full_duffs == cli_total
    app_rows = [{"txid": r["txid"], "recordIndex": r["recordIndex"], "amount": duffs(r["amount"])} for r in shown["rows"]]
    n = min(200, len(cli_rows))
    mismatches = [
        {"i": i, "app": a, "dwcli": c}
        for i, (a, c) in enumerate(zip(app_rows[:n], cli_rows[:n]))
        if a != c
    ]
    result = {
        "balance": {
            "dwcliTotalDuffs": cli_total,
            "heroShown": shown["heroAmount"],
            "heroExpected": floored(cli_total) if cli_total else None,
            "heroTooltipDuffs": full_duffs,
            "match": balance_ok,
        },
        "rows": {"compared": n, "appRows": len(app_rows), "dwcliRows": len(cli_rows), "mismatches": len(mismatches), "firstMismatches": mismatches[:5]},
        "pass": balance_ok and n == 200 and len(app_rows) >= 200 and not mismatches,
    }
    print(json.dumps(result, indent=2))
    return 0 if result["pass"] else 1


if __name__ == "__main__":
    sys.exit(main())
