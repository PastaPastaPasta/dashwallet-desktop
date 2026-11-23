#!/usr/bin/env python3
"""AT-SPI smoke test for the CrossUI Linux probe.

Finds the running probe on the AT-SPI bus, dumps its accessibility tree, drives it
through AT-SPI only (Selection to switch sidebar pages, EditableText to type an
address, Action to press Send), and checks what each widget exposes.

Hard checks decide the exit code. Soft checks record accessibility gaps that the
G2 ADR reports but that do not fail the run.

Usage: atspi_smoke.py --pid PID --out DIR
"""

import argparse
import json
import os
import subprocess
import sys
import time

import pyatspi

VALID_ADDRESS = "XpESxaUmonkq8RaLLp46Brx2K39ggQe226"
NOT_IMPLEMENTED_STATUS = "Address format OK. Sending is not implemented in this probe."
SIDEBAR = ["Overview", "Send", "Receive", "Transactions"]
INTERESTING_STATES = [
    "active", "checked", "editable", "enabled", "focusable", "focused", "pressed",
    "selectable", "selected", "sensitive", "showing", "visible",
]


class Report:
    def __init__(self):
        self.checks = []

    def check(self, kind, name, ok, detail=""):
        self.checks.append({"kind": kind, "name": name, "ok": bool(ok), "detail": detail})
        tag = "PASS" if ok else ("FAIL" if kind == "hard" else "GAP ")
        print(f"[{tag}] {kind}: {name}" + (f" -- {detail}" if detail else ""), flush=True)
        return ok

    @property
    def hard_failures(self):
        return [c for c in self.checks if c["kind"] == "hard" and not c["ok"]]


def safe(fn, default=None):
    try:
        return fn()
    except Exception:  # AT-SPI calls raise on defunct or partially built nodes
        return default


def children(node):
    count = safe(lambda: node.childCount, 0) or 0
    result = []
    for i in range(count):
        child = safe(lambda: node.getChildAtIndex(i))
        if child is not None:
            result.append(child)
    return result


def role(node):
    return safe(lambda: node.getRoleName(), "?")


def name(node):
    return safe(lambda: node.name, "") or ""


def describe(node):
    """Return a dict of what AT-SPI exposes for one node."""
    states = safe(lambda: node.getState().getStates(), []) or []
    state_names = sorted(
        s for s in (pyatspi.stateToString(st) for st in states) if s in INTERESTING_STATES
    )
    info = {
        "role": role(node),
        "name": name(node),
        "description": safe(lambda: node.description, "") or "",
        "states": state_names,
        "interfaces": sorted(safe(lambda: node.get_interfaces(), []) or []),
        "attributes": sorted(safe(lambda: node.getAttributes(), []) or []),
    }
    extents = safe(lambda: node.queryComponent().getExtents(pyatspi.WINDOW_COORDS))
    if extents is not None:
        info["extents"] = [extents.x, extents.y, extents.width, extents.height]
    text = safe(lambda: node.queryText())
    if text is not None:
        # Explicit end offset: GTK 4.14's GtkText returns "" for getText(0, -1).
        info["text"] = safe(lambda: text.getText(0, text.characterCount), "")
    action = safe(lambda: node.queryAction())
    if action is not None:
        info["actions"] = [safe(lambda i=i: action.getName(i), "?") for i in range(action.nActions)]
    return info


# SwiftCrossUI puts every modifier in its own GtkFixed (ADR 0002 gap A6), so
# a button inside a card on a scrolled page sits ~80 levels below the
# application; the M2 run of 2026-10-06 lost its deepest controls at 80.
def walk(node, depth=0, lines=None, nodes=None, max_depth=200):
    if lines is None:
        lines, nodes = [], []
    info = describe(node)
    nodes.append((node, info, depth))
    extra = []
    if info["description"]:
        extra.append(f'desc="{info["description"]}"')
    if info.get("text"):
        extra.append(f'text="{info["text"]}"')
    # GtkLabel always advertises the same 8 clipboard/link/menu actions; elide them.
    if info.get("actions") and info["role"] != "label":
        extra.append("actions=" + ",".join(info["actions"]))
    attributes = [a for a in info["attributes"] if not a.startswith("toolkit:")]
    if attributes:
        extra.append("attrs=" + ",".join(attributes))
    extra.append("{" + ",".join(info["states"]) + "}")
    if info.get("extents") and info["role"] in ("frame", "list", "list item", "push button",
                                                "text", "check box", "scroll pane"):
        extra.append("@({},{} {}x{})".format(*info["extents"]))
    lines.append(f'{"  " * depth}[{info["role"]}] "{info["name"]}" ' + " ".join(extra))
    if depth < max_depth:
        for child in children(node):
            walk(child, depth + 1, lines, nodes, max_depth)
    return lines, nodes


def descendant_text(node, limit=6):
    """Names/texts of labels below `node` (what a screen reader could fall back to)."""
    found = []
    for _, info, _ in walk(node)[1][1:]:
        value = info["name"] or info.get("text", "")
        if value and info["role"] in ("label", "static", "text"):
            found.append(value)
        if len(found) >= limit:
            break
    return found


def find_app(pid, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        desktop = pyatspi.Registry.getDesktop(0)
        for app in children(desktop):
            if safe(lambda: app.get_process_id()) == pid:
                if children(app):
                    return app
        time.sleep(0.5)
    return None


def find_all(root, predicate):
    return [(n, info) for n, info, _ in walk(root)[1] if predicate(n, info)]


def wait_for(root, predicate, timeout=10.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        hits = find_all(root, predicate)
        if hits:
            return hits
        time.sleep(0.25)
    return []


def window_geometry():
    """Top-level X window sizes, to correlate layout growth with X errors."""
    result = subprocess.run(["xwininfo", "-root", "-children"], capture_output=True, text=True)
    return [l.strip() for l in result.stdout.splitlines() if "CrossUILinuxProbe" in l
            or "Dash Wallet Probe" in l]


def screenshot(out_dir, step):
    """Save the Xvfb root window as PNG (xwd | xwdtopnm | pnmtopng)."""
    path = os.path.join(out_dir, f"screen-{step}.png")
    pipeline = f"xwd -root -silent | xwdtopnm 2>/dev/null | pnmtopng > '{path}'"
    if subprocess.run(["sh", "-c", pipeline]).returncode != 0:
        print(f"== screenshot {step} failed", flush=True)


def dump(out_dir, step, root):
    lines, nodes = walk(root)
    path = os.path.join(out_dir, f"atspi-{step}.txt")
    with open(path, "w") as fh:
        fh.write("\n".join(lines) + "\n")
    screenshot(out_dir, step)
    print(f"== dumped {len(nodes)} nodes to {path}; X windows: {window_geometry()}", flush=True)
    return nodes


def is_list_container(info):
    return info["role"] in ("list", "list box", "tree", "table")


def list_items(node):
    return [c for c in children(node) if role(c) in ("list item", "table row", "tree item")]


def sidebar_list(app):
    for node, info in find_all(app, lambda n, i: is_list_container(i)):
        items = list_items(node)
        texts = [name(it) or " ".join(descendant_text(it, 1)) for it in items]
        if texts[: len(SIDEBAR)] == SIDEBAR:
            return node, items
    return None, []


def select_index(report, listbox, index, label):
    selection = safe(lambda: listbox.querySelection())
    ok = selection is not None and bool(safe(lambda: selection.selectChild(index), False))
    report.check("hard", f"select sidebar '{label}' through the AT-SPI Selection interface", ok)
    return ok


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--timeout", type=float, default=30.0)
    args = parser.parse_args()
    os.makedirs(args.out, exist_ok=True)
    report = Report()

    start = time.monotonic()
    app = find_app(args.pid, args.timeout)
    if not report.check("hard", "probe registered on the AT-SPI bus", app is not None,
                        f"pid {args.pid}"):
        return finish(report, args.out)
    report.check("info", "time until the app appeared on the bus", True,
                 f"{time.monotonic() - start:.1f} s, app name '{name(app)}'")

    # --- Overview page -------------------------------------------------------
    frames = wait_for(app, lambda n, i: i["role"] == "frame" and i["name"] == "Dash Wallet Probe")
    report.check("hard", "window exposed as role=frame named 'Dash Wallet Probe'", bool(frames))
    wait_for(app, lambda n, i: i["name"].startswith("Balance:") or i.get("text", "").startswith("Balance:"))
    dump(args.out, "1-overview", app)

    listbox, items = sidebar_list(app)
    report.check("hard", "sidebar list exposes 4 list items Overview/Send/Receive/Transactions",
                 listbox is not None, f"{len(items)} items")
    if listbox is not None:
        own_names = [name(it) for it in items]
        report.check("soft", "sidebar list items carry their own accessible name",
                     own_names[: len(SIDEBAR)] == SIDEBAR, f"item names = {own_names}")

    # GTK 4.14 reports GtkSwitch with the AT-SPI "check box" role (no switch role on the wire).
    switches = find_all(app, lambda n, i: i["role"] in ("switch", "toggle button", "check box"))
    report.check("hard", "toggle (switch style) exposed", bool(switches),
                 ", ".join(f'{i["role"]} "{i["name"]}" actions={i.get("actions")}' for _, i in switches))
    report.check("soft", "switch named 'Hide balance' (label associated with the control)",
                 any(i["name"] == "Hide balance" for _, i in switches))
    if switches:
        action = safe(lambda: switches[0][0].queryAction())
        toggled = action is not None and action.nActions > 0 and bool(
            safe(lambda: action.doAction(0), False))
        hidden = toggled and bool(wait_for(app, lambda n, i: i["name"] == "Balance: hidden"))
        report.check("hard", "toggle the switch through AT-SPI Action and see the balance hidden",
                     hidden)
        if toggled:
            safe(lambda: action.doAction(0))  # restore the visible balance

    # --- Send page -----------------------------------------------------------
    if listbox is None or not select_index(report, listbox, 1, "Send"):
        return finish(report, args.out)
    buttons = wait_for(app, lambda n, i: i["role"] in ("push button", "button") and i["name"] == "Send")
    report.check("hard", "button exposed as role=push button named 'Send'", bool(buttons))
    entries = find_all(app, lambda n, i: i["role"] in ("entry", "text", "password text")
                       and "editable" in i["states"])
    report.check("hard", "text field exposed as an editable entry", bool(entries),
                 ", ".join(f'{i["role"]} "{i["name"]}" desc="{i["description"]}"' for _, i in entries))
    report.check("soft", "text field has an accessible name (a label, e.g. 'Pay to')",
                 any(i["name"] for _, i in entries))
    report.check("soft", "text field exposes its placeholder 'Dash address' (attribute)",
                 any("Dash address" in a for _, i in entries for a in i["attributes"]),
                 f'attributes = {[i["attributes"] for _, i in entries]}')
    checks = find_all(app, lambda n, i: i["role"] == "check box")
    report.check("hard", "checkbox-style toggle exposed as role=check box", bool(checks))
    report.check("soft", "checkbox named 'Use only mixed funds'",
                 any(i["name"] == "Use only mixed funds" for _, i in checks))
    report.check("soft", "checkbox exposes a toggle action",
                 any(i.get("actions") for _, i in checks),
                 f'actions = {[i.get("actions") for _, i in checks]}')
    dump(args.out, "2-send", app)

    if buttons and entries:
        editable = safe(lambda: entries[0][0].queryEditableText())
        typed = editable is not None and bool(
            safe(lambda: editable.setTextContents(VALID_ADDRESS), False))
        report.check("hard", "type an address through AT-SPI EditableText", typed)
        value = describe(entries[0][0]).get("text", "")
        report.check("hard", "read the typed address back through AT-SPI Text",
                     value == VALID_ADDRESS, f"text = '{value}'")
        action = safe(lambda: buttons[0][0].queryAction())
        clicked = action is not None and bool(safe(lambda: action.doAction(0), False))
        report.check("hard", "press Send through the AT-SPI Action interface", clicked)
        status = wait_for(app, lambda n, i: NOT_IMPLEMENTED_STATUS in (i["name"], i.get("text", "")))
        report.check("hard", "view model received the typed address and updated the status label",
                     bool(status), NOT_IMPLEMENTED_STATUS if status else "status label not found")
        dump(args.out, "3-send-pressed", app)

    # --- Transactions page ---------------------------------------------------
    listbox, _ = sidebar_list(app)
    if listbox is None or not select_index(report, listbox, 3, "Transactions"):
        return finish(report, args.out)

    def tx_list(n, i):
        if not is_list_container(i):
            return False
        rows = list_items(n)
        return len(rows) >= 50 and "sample tx" in (name(rows[0]) or " ".join(descendant_text(rows[0], 1)))

    lists = wait_for(app, tx_list)
    rows = list_items(lists[0][0]) if lists else []
    report.check("hard", "transaction list exposes >= 50 list items with their text", bool(lists),
                 f"{len(rows)} rows")
    if rows:
        report.check("soft", "transaction list items carry their own accessible name",
                     all(name(r) for r in rows), f"first row name = '{name(rows[0])}', "
                     f"first row label = {descendant_text(rows[0], 1)}")
        selection = safe(lambda: lists[0][0].querySelection())
        picked = selection is not None and bool(safe(lambda: selection.selectChild(4), False))
        time.sleep(0.5)
        selected = "selected" in describe(rows[4])["states"]
        report.check("hard", "select transaction row 5 through AT-SPI Selection", picked and selected)
    dump(args.out, "4-transactions", app)
    return finish(report, args.out)


def finish(report, out_dir):
    with open(os.path.join(out_dir, "atspi-checks.json"), "w") as fh:
        json.dump(report.checks, fh, indent=2)
    failures = report.hard_failures
    print(f"== {len(report.checks)} checks, {len(failures)} hard failures, "
          f"{sum(1 for c in report.checks if c['kind'] == 'soft' and not c['ok'])} gaps", flush=True)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
