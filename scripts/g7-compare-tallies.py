#!/usr/bin/env python3
"""G7 (docs/contracts/m3-engine.md §9): compares the tallies `dwcli gov sync`
printed with a reference taken at the same time. Developer tool, never
shipped.

Reference formats:
  * a dashd `gobject list valid proposals` result (JSON object keyed by
    hash, `YesCount` / `NoCount` / `AbstainCount`);
  * the DashCentral budget API (`{"proposals": [{"hash", "yes", "no",
    "abstain"}, ...]}`), which reads a mainnet full node.

Usage: g7-compare-tallies.py <dwcli gov sync output> <reference.json>
Exit status 1 when a proposal's yes or no count is off by more than 1 %.
"""
import json
import re
import sys


def ours(path):
    out = {}
    for line in open(path, encoding="utf-8"):
        m = re.match(r"tally hash=(\w+) yes=(\d+) no=(\d+) abstain=(\d+) status=(\w+)", line)
        if m:
            out[m.group(1)] = tuple(int(m.group(i)) for i in (2, 3, 4))
    return out


def reference(path):
    doc = json.load(open(path, encoding="utf-8"))
    if isinstance(doc, dict) and "proposals" in doc:
        return {p["hash"]: (int(p["yes"]), int(p["no"]), int(p["abstain"])) for p in doc["proposals"]}
    return {h: (o["YesCount"], o["NoCount"], o["AbstainCount"]) for h, o in doc.items()}


def main():
    mine, ref = ours(sys.argv[1]), reference(sys.argv[2])
    worst = 0.0
    for h, (y, n, a) in sorted(mine.items()):
        if h not in ref:
            print(f"{h} ours {y}/{n}/{a} not in the reference")
            continue
        ry, rn, ra = ref[h]
        dev = max(abs(y - ry) / max(ry, 1), abs(n - rn) / max(rn, 1))
        worst = max(worst, dev)
        verdict = "equal" if (y, n, a) == (ry, rn, ra) else f"off {dev:.2%}"
        print(f"{h} ours {y}/{n}/{a} reference {ry}/{rn}/{ra} {verdict}")
    for h in sorted(set(ref) - set(mine)):
        print(f"{h} only in the reference {ref[h]}")
    print(f"proposals ours={len(mine)} reference={len(ref)} worst={worst:.2%}")
    return 1 if worst > 0.01 else 0


if __name__ == "__main__":
    sys.exit(main())
