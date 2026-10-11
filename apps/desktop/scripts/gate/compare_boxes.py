#!/usr/bin/env python3
"""Gate U3(c): the box of every layout container (`data-testid`) agrees within 2 px across engines.

Inputs are self-test reports (`--selftest` from the Tauri app, `selftest.json` from Playwright),
one per engine. Every `<theme>/<screen>` and test id present in all reports is compared on
x, y, width and height against the first report.

Usage: compare_boxes.py NAME=report.json NAME=report.json [...] [--tolerance 2] [--text-runs ID,ID]
Prints a JSON summary; exit status 0 when every delta is within the tolerance.

`--text-runs` names test ids whose box is a text run's (sized by its glyphs: the hero amount, the
network capsule's label). U3(c) covers layout containers, "not text runs", so those boxes are
compared and reported apart (`textRuns`) and do not decide `pass`. `passAllBoxes` keeps the
verdict over every box.
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
    ap.add_argument("--text-runs", default="", help="comma-separated test ids of text-run boxes")
    args = ap.parse_args()
    text_runs = {t for t in args.text_runs.split(",") if t}
    named = [r.split("=", 1) for r in args.reports]
    # utf-8-sig: reports copied through Windows tools may carry a byte-order mark.
    reports = {name: json.load(open(path, encoding="utf-8-sig")) for name, path in named}
    # Boxes compare only at one viewport (`--window-size`, DW_VIEWPORT).
    viewports = {name: (r["viewport"]["width"], r["viewport"]["height"]) for name, r in reports.items()}
    if len(set(viewports.values())) > 1:
        sys.exit(f"compare_boxes: the reports' viewports differ: {viewports}")
    data = {name: boxes(r) for name, r in reports.items()}
    base_name = named[0][0]
    base = data[base_name]
    common = set(base)
    for d in data.values():
        common &= set(d)
    summary = {"base": base_name, "tolerance": args.tolerance, "compared": len(common),
               "textRunIds": sorted(text_runs), "engines": {}}
    ok = ok_all = True
    for name, d in data.items():
        if name == base_name:
            continue
        over = []
        runs_over = []
        worst = 0.0
        for key in sorted(common):
            a, b = base[key], d[key]
            delta = max(abs(a[k] - b[k]) for k in ("x", "y", "w", "h"))
            worst = max(worst, delta)
            if delta > args.tolerance:
                entry = {"screen": key[0], "id": key[1], "delta": round(delta, 2), base_name: a, name: b}
                if key[1].split("#")[0] in text_runs:
                    runs_over.append(entry)
                else:
                    over.append(entry)
        missing = set(base) - set(d)
        extra = set(d) - set(base)
        # A container that exists in one engine only is a layout difference too.
        containers_ok = not over and not missing and not extra
        ok &= containers_ok
        ok_all &= containers_ok and not runs_over
        summary["engines"][name] = {
            "maxDelta": round(worst, 2),
            "overTolerance": len(over),
            "onlyHere": sorted(f"{k[0]}:{k[1]}" for k in extra)[:10],
            "missingHere": sorted(f"{k[0]}:{k[1]}" for k in missing)[:10],
            "worst": sorted(over, key=lambda o: -o["delta"])[:10],
            "textRuns": sorted(runs_over, key=lambda o: -o["delta"])[:10],
        }
    summary["pass"] = ok
    summary["passAllBoxes"] = ok_all
    print(json.dumps(summary, indent=2))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
