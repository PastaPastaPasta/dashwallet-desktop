#!/usr/bin/env python3
"""Gate U3(c): the box of every layout container (`data-testid`) agrees within 2 px across engines.

Inputs are self-test reports (`--selftest` from the Tauri app, `selftest.json` from Playwright),
one per engine. Every `<theme>/<screen>` and test id present in all reports is compared on
x, y, width and height against the first report.

Usage: compare_boxes.py NAME=report.json NAME=report.json [...] [--tolerance 2]
Prints a JSON summary; exit status 0 when every delta is within the tolerance.
"""

import argparse
import json
import sys


def boxes(report):
    out = {}
    for screen, rec in report["screens"].items():
        seen = {}
        for b in rec["boxes"]:
            # Repeated ids (none expected) get an ordinal so they still pair up.
            n = seen.get(b["id"], 0)
            seen[b["id"]] = n + 1
            out[(screen, b["id"] if n == 0 else f'{b["id"]}#{n}')] = b
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("reports", nargs="+", help="NAME=path")
    ap.add_argument("--tolerance", type=float, default=2.0)
    args = ap.parse_args()
    named = [r.split("=", 1) for r in args.reports]
    data = {name: boxes(json.load(open(path))) for name, path in named}
    base_name = named[0][0]
    base = data[base_name]
    common = set(base)
    for d in data.values():
        common &= set(d)
    summary = {"base": base_name, "tolerance": args.tolerance, "compared": len(common), "engines": {}}
    ok = True
    for name, d in data.items():
        if name == base_name:
            continue
        over = []
        worst = 0.0
        for key in sorted(common):
            a, b = base[key], d[key]
            delta = max(abs(a[k] - b[k]) for k in ("x", "y", "w", "h"))
            worst = max(worst, delta)
            if delta > args.tolerance:
                over.append({"screen": key[0], "id": key[1], "delta": round(delta, 2), base_name: a, name: b})
        ok &= not over
        summary["engines"][name] = {
            "maxDelta": round(worst, 2),
            "overTolerance": len(over),
            "onlyHere": len(set(d) - set(base)),
            "missingHere": len(set(base) - set(d)),
            "worst": sorted(over, key=lambda o: -o["delta"])[:10],
        }
    summary["pass"] = ok
    print(json.dumps(summary, indent=2))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
