#!/usr/bin/env python3
"""AT-SPI dump and drive for `dash-wallet --demo` (GtkBackend).

Reuses the G2 probe's helpers (probes/crossui-linux/scripts/atspi_smoke.py).
For each step it waits for the page's marker text, dumps the accessibility
tree to atspi-<step>.txt and saves a screenshot <step>.png. A step written
NAME=select:ITEM first selects ITEM in the sidebar list through the AT-SPI
Selection interface. Results are appended to atspi-checks.json.

Usage: atspi_demo.py --pid PID --out DIR --steps 1-overview,2-send=select:Send
"""

import argparse
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../../../probes/crossui-linux/scripts"))
from atspi_smoke import (  # noqa: E402
    Report, children, descendant_text, find_all, find_app, is_list_container, list_items, name, role, safe,
    wait_for, walk,
)

# Text that shows the page has rendered, by step suffix.
MARKERS = {
    "overview": "Balances",
    "send": "Pay To:",
    "transactions": "Export…",
    "receive": "Request payment",
    "onboarding": "Create a new wallet",
}
SIDEBAR = ["Overview", "Send", "Receive", "Transactions"]


def has_text(value):
    def predicate(_node, info):
        return value in (info["name"], info.get("text", ""))
    return predicate


def screenshot(out_dir, step):
    """PNG of the app window (xwd -id), or of the whole screen if no window is found."""
    path = os.path.join(out_dir, f"{step}.png")
    tree = subprocess.run(["xwininfo", "-root", "-tree"], capture_output=True, text=True).stdout
    window = None
    for line in tree.splitlines():
        if '"Dash Wallet"' in line:
            window = line.split()[0]
            break
    source = f"xwd -id {window} -silent" if window else "xwd -root -silent"
    pipeline = f"{source} | xwdtopnm 2>/dev/null | pnmtopng > '{path}'"
    ok = subprocess.run(["sh", "-c", pipeline]).returncode == 0
    print(f"== screenshot {path} ({'window ' + window if window else 'root'}): {'ok' if ok else 'FAILED'}", flush=True)


def sidebar(app):
    for node, _info in find_all(app, lambda n, i: is_list_container(i)):
        items = list_items(node)
        texts = [name(it) or " ".join(descendant_text(it, 1)) for it in items]
        if all(label in texts for label in SIDEBAR):
            return node, texts
    return None, []


def summarize(nodes):
    """Counts of controls with and without an accessible name (ADR 0002 gaps A1-A4)."""
    stats = {}
    for _node, info, _depth in nodes:
        r = info["role"]
        if r not in ("push button", "check box", "toggle button", "text", "list item", "combo box"):
            continue
        entry = stats.setdefault(r, {"total": 0, "named": 0, "examples_unnamed": []})
        entry["total"] += 1
        placeholder = [a for a in info["attributes"] if a.startswith("placeholder-text:")]
        if info["name"]:
            entry["named"] += 1
        elif len(entry["examples_unnamed"]) < 3:
            entry["examples_unnamed"].append(placeholder[0] if placeholder else "(no name)")
    return stats


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--steps", required=True)
    args = parser.parse_args()
    report = Report()

    app = find_app(args.pid, timeout=60)
    if not report.check("hard", f"app pid {args.pid} on the AT-SPI bus", app is not None):
        return finish(report, args.out)
    # The app registers on the bus before its window exists; wait for the frame.
    deadline = time.monotonic() + 30
    frames = []
    while time.monotonic() < deadline:
        frames = [c for c in children(app) if role(c) == "frame"]
        if any(name(f) == "Dash Wallet" for f in frames):
            break
        time.sleep(0.25)
    report.check("hard", 'window is a frame named "Dash Wallet"', any(name(f) == "Dash Wallet" for f in frames),
                 str([name(f) for f in frames]))

    for step in args.steps.split(","):
        step_name, _, action = step.partition("=")
        if action.startswith("select:"):
            item = action[len("select:"):]
            listbox, texts = sidebar(app)
            ok = False
            if listbox is not None and item in texts:
                selection = safe(lambda: listbox.querySelection())
                ok = selection is not None and bool(safe(lambda: selection.selectChild(texts.index(item)), False))
            report.check("hard", f"select sidebar '{item}' through AT-SPI Selection", ok, str(texts))
        marker = MARKERS[step_name.split("-", 1)[1]]
        found = wait_for(app, has_text(marker), timeout=30)
        report.check("hard", f"{step_name}: marker text '{marker}' is exposed", bool(found))
        time.sleep(1.0)  # let layout settle before the screenshot
        lines, nodes = walk(app)
        with open(os.path.join(args.out, f"atspi-{step_name}.txt"), "w") as fh:
            fh.write("\n".join(lines) + "\n")
        print(f"== {step_name}: dumped {len(nodes)} nodes", flush=True)
        screenshot(args.out, step_name)
        stats = summarize(nodes)
        report.check("soft", f"{step_name}: control naming", True, json.dumps(stats))
        if step_name.endswith("overview") or step_name.endswith("send"):
            _listbox, texts = sidebar(app)
            report.check("hard", f"{step_name}: sidebar list items {SIDEBAR}", all(t in texts for t in SIDEBAR),
                         str(texts))
        if step_name.endswith("send"):
            buttons = {info["name"] for _n, info, _d in nodes if info["role"] == "push button"}
            for title in ("Send", "Add Recipient", "Clear All", "Use available balance"):
                report.check("hard", f"send: push button named '{title}'", title in buttons)
            placeholders = {a for _n, info, _d in nodes if info["role"] == "text" for a in info["attributes"]}
            report.check("hard", "send: address entry exposes placeholder 'Pay to: Dash address'",
                         "placeholder-text:Pay to: Dash address" in placeholders)
    return finish(report, args.out)


def finish(report, out_dir):
    path = os.path.join(out_dir, "atspi-checks.json")
    existing = []
    if os.path.exists(path):
        with open(path) as fh:
            existing = json.load(fh)
    with open(path, "w") as fh:
        json.dump(existing + report.checks, fh, indent=1)
    failures = report.hard_failures
    print(f"== {len(report.checks)} checks, {len(failures)} hard failures", flush=True)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
