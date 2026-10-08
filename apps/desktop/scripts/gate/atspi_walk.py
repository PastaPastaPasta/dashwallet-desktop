#!/usr/bin/env python3
"""Gate U4 on Linux: AT-SPI walk of the Tauri app (WebKitGTK).

Finds the running `dash-wallet` on the AT-SPI bus, then for each gate screen dumps the
accessibility tree and counts interactive elements without an accessible name. It drives
the app through AT-SPI only: `Action` presses the sidebar item, a transaction row, Close and
Lock; `EditableText` types the passphrase. Reuses ADR 0002's helpers
(probes/crossui-linux/scripts/atspi_smoke.py) so both candidates are walked the same way.

Usage: atspi_walk.py --pid PID --out DIR [--passphrase demo]
Writes DIR/atspi-<screen>.txt (+ screen-<screen>.png) and DIR/atspi-summary.json.
"""

import argparse
import json
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.dont_write_bytecode = True  # no __pycache__ inside probes/
sys.path.insert(0, os.path.join(HERE, "../../../../probes/crossui-linux/scripts"))

from atspi_smoke import Report, children, dump, find_app, find_all, name, safe, wait_for  # noqa: E402

# GTK names a button "push button"; WebKitGTK's own AT-SPI implementation names it "button".
BUTTONS = {"push button", "button", "toggle button", "radio button"}
INTERACTIVE = BUTTONS | {
    "check box", "entry", "password text", "link", "menu item", "combo box", "spin button", "slider",
}


def interactive(info):
    if info["role"] in INTERACTIVE:
        return True
    return info["role"] == "text" and "editable" in info["states"]


def screen_summary(nodes):
    items = [(n, i) for n, i, _ in nodes if interactive(i) and "showing" in i["states"]]
    unnamed = [i for _, i in items if not i["name"].strip()]
    return {
        "nodes": len(nodes),
        "interactive": len(items),
        "unnamed": len(unnamed),
        "unnamedDetail": [{"role": u["role"], "extents": u.get("extents")} for u in unnamed],
        "interactiveNames": sorted({f'{i["role"]}: {i["name"]}' for _, i in items})[:80],
    }


def press(report, app, predicate, label):
    hits = wait_for(app, lambda n, i: i["role"] in BUTTONS and "showing" in i["states"] and predicate(i))
    if not hits:
        return report.check("hard", f"find '{label}'", False)
    node, info = hits[0]
    action = safe(lambda: node.queryAction())
    ok = action is not None and bool(safe(lambda: action.doAction(0), False))
    return report.check("hard", f"press '{info['name'][:60]}' through AT-SPI Action", ok)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pid", type=int, required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--passphrase", default="demo")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    report = Report()
    summary = {}

    app = find_app(args.pid, timeout=60)
    if not report.check("hard", "dash-wallet is on the AT-SPI bus", app is not None):
        return finish(report, args.out, summary)
    summary["application"] = name(app)
    report.check("soft", "application name is 'Dash Wallet'", name(app) == "Dash Wallet", name(app))

    # Wait for the web content to expose the Overview (the hero amount button).
    ok = wait_for(app, lambda n, i: i["role"] in BUTTONS and "Click to" in (i["description"] + i["name"]), timeout=60)
    ok = ok or wait_for(app, lambda n, i: i["role"] in BUTTONS and i["name"] == "Transactions", timeout=30)
    report.check("hard", "web content is exposed (Overview controls found)", bool(ok))
    time.sleep(1)
    summary["overview"] = screen_summary(dump(args.out, "overview", app))

    press(report, app, lambda i: i["name"] == "Transactions", "Transactions")
    wait_for(app, lambda n, i: i["role"] in ("entry", "text") and "Search" in i["name"], timeout=15)
    time.sleep(1)
    summary["transactions"] = screen_summary(dump(args.out, "transactions", app))

    # Rows are buttons named by their content ("Received 1:40 AM Pending +0.20 tDASH").
    press(report, app, lambda i: "DASH" in i["name"] and any(c.isdigit() for c in i["name"]), "first transaction row")
    hits = wait_for(app, lambda n, i: i["role"] == "dialog", timeout=15)
    report.check("hard", "the detail sheet is a dialog", bool(hits))
    if hits:
        report.check("soft", "the dialog is named", bool(hits[0][1]["name"]), hits[0][1]["name"])
    time.sleep(1)
    summary["txDetail"] = screen_summary(dump(args.out, "txDetail", app))
    press(report, app, lambda i: i["name"] == "Close", "Close")
    time.sleep(1)

    press(report, app, lambda i: i["name"] == "Lock wallet", "Lock wallet")
    fields = wait_for(app, lambda n, i: i["role"] in ("password text", "entry", "text") and i["name"] == "Passphrase", timeout=15)
    report.check("hard", "the passphrase field is named", bool(fields))
    time.sleep(1)
    summary["unlock"] = screen_summary(dump(args.out, "unlock", app))
    if fields:
        node = fields[0][0]
        editable = safe(lambda: node.queryEditableText())
        typed = editable is not None and bool(safe(lambda: editable.setTextContents(args.passphrase), False))
        typed = typed or (editable is not None and bool(safe(lambda: editable.insertText(0, args.passphrase, len(args.passphrase)), False)))
        report.check("soft", "type the passphrase through AT-SPI EditableText", typed,
                     "WebKitGTK refuses programmatic text on a password field" if not typed else "")
        if not typed:
            # The field has focus (autofocus): type real keystrokes, as a user or Orca would.
            safe(lambda: node.queryComponent().grabFocus())
            typed = subprocess.run(["xdotool", "type", "--delay", "20", args.passphrase]).returncode == 0
            report.check("hard", "type the passphrase with the keyboard (xdotool)", typed)
        time.sleep(0.5)
        press(report, app, lambda i: i["name"] == "Unlock", "Unlock")
        gone = False
        for _ in range(40):
            if not find_all(app, lambda n, i: i["role"] in ("password text", "entry") and i["name"] == "Passphrase"):
                gone = True
                break
            time.sleep(0.25)
        report.check("hard", "unlocking through AT-SPI removes the lock screen", gone)

    for screen in ("overview", "transactions", "txDetail", "unlock"):
        s = summary.get(screen)
        if s:
            report.check("hard", f"{screen}: 0 unnamed interactive elements ({s['interactive']} interactive)", s["unnamed"] == 0,
                         json.dumps(s["unnamedDetail"])[:300])
    return finish(report, args.out, summary)


def finish(report, out_dir, summary):
    with open(os.path.join(out_dir, "atspi-summary.json"), "w") as fh:
        json.dump({"summary": summary, "checks": report.checks}, fh, indent=2)
    return 1 if report.hard_failures else 0


if __name__ == "__main__":
    sys.exit(main())
