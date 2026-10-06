#!/usr/bin/env python3
"""AT-SPI dump and drive for `dash-wallet --demo` (GtkBackend).

Reuses the G2 probe's helpers (probes/crossui-linux/scripts/atspi_smoke.py).
For each step it waits for the page's marker text, dumps the accessibility
tree to atspi-<step>.txt and saves a screenshot <step>.png. A step written
NAME=select:ITEM first selects ITEM in the sidebar list through the AT-SPI
Selection interface. NAME=flow:onboarding, flow:send, flow:tools (peers page,
unit selector, address-book QR code) and flow:overlay (sync overlay, Hide)
drive a whole flow through AT-SPI (EditableText to type into fields found by their
accessible name, Action to press buttons) and record a tree and screenshot
at each stage. Results are appended to atspi-checks.json.

Usage: atspi_demo.py --pid PID --out DIR --steps 1-overview,2-send=select:Send
"""

import argparse
import json
import os
import re
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
# A valid testnet address that is not one of the demo wallet's own.
PAY_TO = "yPgfYhP6PwdZd8xn1TKDps27nL6kLpvh98"
DEMO_PASSPHRASE = "demo"
NEW_PASSPHRASE = "correct horse battery staple 42"
# The demo's "new wallet" phrase (WalletDemo DemoEnvironment.phrase, first 12).
DEMO_WORDS = ["galaxy", "rocket", "velvet", "harbor", "tiny", "maple", "oyster", "crane", "sunset", "ribbon",
              "empty", "above"]
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
        # "Dash Wallet", or "Dash Wallet - <wallet> - [network]" (QT-011).
        if '"Dash Wallet' in line:
            window = line.split()[0]
            break
    source = f"xwd -id {window} -silent" if window else "xwd -root -silent"
    pipeline = f"{source} | xwdtopnm 2>/dev/null | pnmtopng > '{path}'"
    ok = subprocess.run(["sh", "-c", pipeline]).returncode == 0
    print(f"== screenshot {path} ({'window ' + window if window else 'root'}): {'ok' if ok else 'FAILED'}", flush=True)


def has_text_prefix(value):
    def predicate(_node, info):
        return info["name"].startswith(value) or info.get("text", "").startswith(value)
    return predicate


def record(app, out_dir, step, secret_phrase=False):
    """Tree dump and screenshot of the current state. With `secret_phrase`
    (a live wallet's recovery phrase is on screen) the words are masked in the
    dump and no screenshot is taken."""
    time.sleep(1.0)  # let layout settle before the screenshot
    lines, nodes = walk(app)
    if secret_phrase:
        lines = [re.sub(r'"(\d+)\. \S+"', r'"\1. ****"', line) for line in lines]
        lines = [re.sub(r'text="(\d+)\. \S+"', r'text="\1. ****"', line) for line in lines]
    with open(os.path.join(out_dir, f"atspi-{step}.txt"), "w") as fh:
        fh.write("\n".join(lines) + "\n")
    print(f"== {step}: dumped {len(nodes)} nodes", flush=True)
    if not secret_phrase:
        screenshot(out_dir, step)
    return nodes


def press(report, app, title, timeout=15, shown=None):
    """Presses the first sensitive push button named `title` (AT-SPI Action).
    `shown` replaces the title in the report (for recovery-phrase words)."""
    hits = wait_for(app, lambda n, i: i["role"] in ("push button", "button") and i["name"] == title
                    and "sensitive" in i["states"], timeout=timeout)
    action = safe(lambda: hits[0][0].queryAction()) if hits else None
    ok = action is not None and bool(safe(lambda: action.doAction(0), False))
    report.check("hard", f"press '{shown or title}' through AT-SPI Action", ok)
    return ok


def type_into(report, app, field, value, timeout=15):
    """Types into the editable field whose accessible name is `field` (AT-SPI EditableText)."""
    hits = wait_for(app, lambda n, i: i["role"] in ("text", "entry", "password text") and i["name"] == field
                    and "editable" in i["states"], timeout=timeout)
    editable = safe(lambda: hits[0][0].queryEditableText()) if hits else None
    ok = editable is not None and bool(safe(lambda: editable.setTextContents(value), False))
    report.check("hard", f"type into the field named '{field}' through AT-SPI EditableText", ok,
                 "" if hits else "no editable field with that name")
    return ok


def onboarding_flow(report, app, out_dir, step):
    """Onboarding on a network without wallets (`--demo onboarding`, or live
    mode on an empty data directory): create, show phrase, verify, encrypt,
    wallet ready. The words to verify are read from the phrase page."""
    report.check("hard", "onboarding: welcome page", bool(wait_for(app, has_text("Create a new wallet"), 30)))
    if not press(report, app, "Create a new wallet"):
        return
    shown = wait_for(app, has_text("I wrote it down"), 15)
    report.check("hard", "onboarding: recovery phrase page", bool(shown))
    nodes = record(app, out_dir, f"{step}-phrase", secret_phrase="live" in step)
    words = {}
    for _n, info, _d in nodes:
        match = re.fullmatch(r"(\d+)\. (\S+)", info["name"] or info.get("text", ""))
        if match:
            words[int(match.group(1))] = match.group(2)
    report.check("hard", "onboarding: the phrase page exposes 12 numbered words", len(words) == 12, str(len(words)))
    if not press(report, app, "I wrote it down"):
        return
    for _ in range(8):
        header = wait_for(app, has_text_prefix("Select word #"), 5)
        if not header:
            break
        text = header[0][1]["name"] or header[0][1].get("text", "")
        position = int(text.rsplit("#", 1)[1])
        word = words.get(position, DEMO_WORDS[position - 1])
        if not press(report, app, word, shown=f"word #{position}" if "live" in step else None):
            return
        time.sleep(0.5)
    report.check("hard", "onboarding: phrase verified, passphrase page shown",
                 bool(wait_for(app, has_text("Encrypt wallet"), 15)))
    record(app, out_dir, f"{step}-passphrase")
    if not (type_into(report, app, "New passphrase", NEW_PASSPHRASE)
            and type_into(report, app, "Repeat new passphrase", NEW_PASSPHRASE)):
        return
    if not press(report, app, "Encrypt wallet"):
        return
    # Live mode creates the vault with the engine's key derivation (debug
    # build). Without a node the wallet is not synced, so the sync overlay
    # (QT-027) covers the Overview until hidden.
    ready = wait_for(app, lambda n, i: has_text("Balances")(n, i)
                     or has_text("Recent transactions may not yet be visible")(n, i),
                     300 if "live" in step else 60)
    if ready and (ready[0][1].get("text") or ready[0][1]["name"]).startswith("Recent transactions"):
        record(app, out_dir, f"{step}-overlay")
        if not press(report, app, "Hide"):
            return
        ready = wait_for(app, has_text("Balances"), 15)
    report.check("hard", "onboarding: the new wallet's Overview is shown", bool(ready))
    record(app, out_dir, f"{step}-done")


def send_flow(report, app, out_dir, step):
    """`--demo --page send`: fill a payment, review it (authorize if asked), confirm, done."""
    report.check("hard", "send: page shown", bool(wait_for(app, has_text("Pay To:"), 30)))
    if not (type_into(report, app, "Pay To", PAY_TO) and type_into(report, app, "Amount", "0.25")):
        return
    record(app, out_dir, f"{step}-filled")
    if not press(report, app, "Send"):
        return
    stage = wait_for(app, lambda n, i: (i["role"] == "push button" and i["name"] == "Authorize")
                     or has_text("Confirm send coins")(n, i), 20)
    if stage and stage[0][1]["name"] == "Authorize":
        record(app, out_dir, f"{step}-authorize")
        if not (type_into(report, app, "Passphrase", DEMO_PASSPHRASE) and press(report, app, "Authorize")):
            return
    review = wait_for(app, has_text("Confirm send coins"), 20)
    report.check("hard", "send: review (confirm) panel shown", bool(review))
    nodes = record(app, out_dir, f"{step}-review")
    lines = [i.get("text") or i["name"] for _n, i, _d in nodes if i["role"] in ("label", "static")]
    report.check("hard", "send: review lists the recipient address", any(PAY_TO in line for line in lines))
    time.sleep(3.5)  # the confirm button counts down 3 s (QT-067)
    if not press(report, app, "Send"):
        return
    # After the broadcast the app opens the new transaction on the Transactions page.
    done = wait_for(app, lambda n, i: i["role"] == "list item" and PAY_TO in i["name"] and "-0.25" in i["name"], 20)
    report.check("hard", "send: the sent payment is listed on the Transactions page", bool(done),
                 done[0][1]["name"] if done else "")
    record(app, out_dir, f"{step}-done")


def tools_flow(report, app, out_dir, step):
    """`--demo`: the status row's unit selector, the peers page from its peer
    count (QT-147), then Show QR in the address book (QT-095)."""
    report.check("hard", "tools: overview shown", bool(wait_for(app, has_text("Balances"), 30)))
    unit = wait_for(app, lambda n, i: i["role"] == "combo box" and i["name"] == "Unit to show amounts in", 15)
    report.check("hard", "tools: status-row unit selector is a combo box named 'Unit to show amounts in'", bool(unit))
    peers_button = wait_for(app, lambda n, i: i["role"] == "push button" and re.fullmatch(r"\d+ peers", i["name"] or ""), 15)
    report.check("hard", "tools: status-row peers button", bool(peers_button),
                 peers_button[0][1]["name"] if peers_button else "")
    if not peers_button or not press(report, app, peers_button[0][1]["name"]):
        return
    shown = wait_for(app, lambda n, i: i["role"] == "push button" and i["name"] == "Change Peers", 15)
    report.check("hard", "tools: peers page with 'Change Peers'", bool(shown))
    rows = wait_for(app, lambda n, i: i["role"] == "list item" and (i["name"] or "").startswith("203.0.113."), 15)
    report.check("hard", "tools: peer rows are named after their address", bool(rows), rows[0][1]["name"] if rows else "")
    record(app, out_dir, f"{step}-peers")
    if not press(report, app, "Close"):
        return
    if not press(report, app, "Address Book"):
        return
    qr_button = wait_for(app, lambda n, i: i["role"] == "push button" and i["name"] == "Show QR", 15)
    report.check("hard", "tools: address book lists entries with 'Show QR'", bool(qr_button))
    if not qr_button or not press(report, app, "Show QR"):
        return
    uri = wait_for(app, has_text_prefix("dash:y"), 15)
    report.check("hard", "tools: the entry's QR code and dash: URI are shown", bool(uri)
                 and bool(wait_for(app, lambda n, i: i["role"] == "push button" and i["name"] == "Hide QR", 5)),
                 (uri[0][1].get("text") or uri[0][1]["name"]) if uri else "")
    _lines, nodes = walk(app)
    names = [i["name"] for _n, i, _d in nodes if i["role"] == "combo box"]
    report.check("hard", "tools: the address-list picker is named 'Address list'", "Address list" in names, str(names))
    record(app, out_dir, f"{step}-qr")


def overlay_flow(report, app, out_dir, step):
    """`--demo offline`: SPV has no peers and is three days behind, so the
    sync overlay (QT-027) shows by itself; Hide returns to the wallet."""
    shown = wait_for(app, has_text("Recent transactions may not yet be visible"), 30)
    report.check("hard", "overlay: shown by itself while the tip is old", bool(shown))
    for label in ("Number of blocks left", "Last block time", "Progress increase per hour"):
        report.check("hard", f"overlay: row '{label}'", bool(wait_for(app, has_text(label), 5)))
    record(app, out_dir, f"{step}-shown")
    if not press(report, app, "Hide"):
        return
    report.check("hard", "overlay: Hide returns to the wallet", bool(wait_for(app, has_text("Balances"), 15)))
    report.check("hard", "overlay: status row offers 'Sync details'",
                 bool(wait_for(app, lambda n, i: i["role"] == "push button" and i["name"] == "Sync details", 10)))
    record(app, out_dir, f"{step}-hidden")


# --- M2 flows -------------------------------------------------------------

def toggle(report, app, title, timeout=15):
    """Flips the switch named `title` through its AT-SPI Action (GtkSwitch 'toggle')."""
    hits = wait_for(app, lambda n, i: i["role"] in ("check box", "toggle button", "switch") and i["name"] == title
                    and "sensitive" in i["states"], timeout=timeout)
    action = safe(lambda: hits[0][0].queryAction()) if hits else None
    ok = action is not None and action.nActions > 0 and bool(safe(lambda: action.doAction(0), False))
    report.check("hard", f"toggle '{title}' through AT-SPI Action", ok,
                 "" if hits else "no sensitive switch with that name")
    return ok


def select_sidebar(report, app, item):
    listbox, texts = sidebar(app)
    ok = False
    if listbox is not None and item in texts:
        selection = safe(lambda: listbox.querySelection())
        ok = selection is not None and bool(safe(lambda: selection.selectChild(texts.index(item)), False))
    report.check("hard", f"select sidebar '{item}' through AT-SPI Selection", ok, str(texts))
    return ok


def named(role_names, title):
    def predicate(_n, info):
        return info["role"] in role_names and info["name"] == title
    return predicate


def m2_menus_flow(report, app, out_dir, step):
    """`--demo`: dash-qt's menu bar (File, Settings, Window, Help) from the
    shell model, and the window title with the network tag (QT-011)."""
    report.check("hard", "menus: overview shown", bool(wait_for(app, has_text("Balances"), 30)))
    frames = [c for c in children(app) if role(c) == "frame"]
    titles = [name(f) for f in frames]
    report.check("hard", "menus: window title is 'Dash Wallet - … - [testnet]' (QT-011)",
                 any(t.startswith("Dash Wallet - ") and t.endswith("[testnet]") for t in titles), str(titles))
    bar = wait_for(app, lambda n, i: i["role"] == "menu bar", 10)
    report.check("hard", "menus: the window has a menu bar", bool(bar))
    _lines, nodes = walk(app)
    items = [i["name"] for _n, i, _d in nodes if i["role"] in ("menu item", "menu")]
    for title in ("File", "Settings", "Window", "Help"):
        report.check("hard", f"menus: top-level menu '{title}'", title in items, str(items[:12]))
    record(app, out_dir, f"{step}-overview")


def m2_options_flow(report, app, out_dir, step):
    """`--demo --page options`: the tabs, Wallet ▸ coin control on, OK; then
    Send shows Coin Control Features, Inputs… opens Coin Selection, and
    selecting a coin fills the summary (QT-068…074, QT-135…141)."""
    report.check("hard", "options: page shown", bool(wait_for(app, has_text("Reset Options"), 30)))
    record(app, out_dir, f"{step}-main")
    if not press(report, app, "Wallet"):
        return
    report.check("hard", "options: Wallet tab shows the coin control switch",
                 bool(wait_for(app, named(("check box", "toggle button", "switch"), "Enable coin control features"), 10)))
    record(app, out_dir, f"{step}-wallet")
    if not toggle(report, app, "Enable coin control features"):
        return
    if not press(report, app, "OK"):
        return
    report.check("hard", "options: OK saves ('Options saved.')", bool(wait_for(app, has_text("Options saved."), 10)))
    if press(report, app, "Network"):
        report.check("hard", "options: Network tab says proxy needs an engine update",
                     bool(wait_for(app, has_text_prefix("Proxy support requires an engine update"), 10)))
        record(app, out_dir, f"{step}-network")
    if press(report, app, "Display"):
        report.check("hard", "options: Display tab has the third-party URL field",
                     bool(wait_for(app, named(("text", "entry"), "Third-party transaction URLs"), 10)))
        record(app, out_dir, f"{step}-display")
    if not select_sidebar(report, app, "Send"):
        return
    report.check("hard", "coin control: Send shows 'Coin Control Features'",
                 bool(wait_for(app, has_text("Coin Control Features"), 15)))
    if not press(report, app, "Inputs…"):
        return
    report.check("hard", "coin selection: page shown", bool(wait_for(app, has_text("Coin Selection"), 15)))
    report.check("hard", "coin selection: 'automatically selected' before a pick",
                 bool(wait_for(app, has_text("automatically selected"), 10)))
    record(app, out_dir, f"{step}-coin-selection")
    coin = wait_for(app, lambda n, i: i["role"] in ("check box", "toggle button", "switch")
                    and (i["name"] or "").startswith("Select ") and "sensitive" in i["states"], 10)
    report.check("hard", "coin selection: coins are switches named 'Select <amount> at <address>'", bool(coin),
                 coin[0][1]["name"] if coin else "")
    if not coin or not toggle(report, app, coin[0][1]["name"]):
        return
    report.check("hard", "coin selection: the summary shows Quantity after a pick",
                 bool(wait_for(app, has_text("Quantity:"), 10)))
    record(app, out_dir, f"{step}-coin-selected")


def m2_tools_flow(report, app, out_dir, step):
    """`--demo --page tools-console`: run getblockcount in the console, then
    the Information, Peers and Repair tabs (QT-143…148)."""
    report.check("hard", "console: welcome text", bool(wait_for(
        app, has_text_prefix("Welcome to the Dash Wallet RPC console."), 30)))
    report.check("hard", "console: anti-scam warning", bool(wait_for(app, has_text_prefix("WARNING: Scammers"), 5)))
    if not (type_into(report, app, "Console command", "getblockcount") and press(report, app, "Run")):
        return
    echoed = wait_for(app, has_text("> getblockcount"), 10)
    reply = wait_for(app, lambda n, i: i["role"] == "label" and re.fullmatch(r"\d+", i.get("text") or i["name"] or ""), 10)
    report.check("hard", "console: the command is echoed and answered with a block count", bool(echoed and reply),
                 (reply[0][1].get("text") or reply[0][1]["name"]) if reply else "")
    record(app, out_dir, f"{step}-console")
    if press(report, app, "Information"):
        report.check("hard", "information: client version row", bool(wait_for(app, has_text("Client version"), 10)))
        report.check("hard", "information: full-node rows say so",
                     bool(wait_for(app, lambda n, i: "Requires full-node data source" in (i.get("text") or i["name"] or ""), 10)))
        record(app, out_dir, f"{step}-information")
    if press(report, app, "Peers"):
        report.check("hard", "peers tab: 'Change Peers'", bool(wait_for(app, named(("push button",), "Change Peers"), 10)))
        record(app, out_dir, f"{step}-peers")
    if press(report, app, "Repair"):
        report.check("hard", "repair tab: rescan and reset buttons",
                     bool(wait_for(app, named(("push button",), "Rescan Chain (full)"), 10))
                     and bool(wait_for(app, named(("push button",), "Reset chain data and resync"), 5)))
        record(app, out_dir, f"{step}-repair")


def m2_psbt_flow(report, app, out_dir, step):
    """`--demo --page psbt`: load from the clipboard (empty) and from pasted
    base64; the demo has no PSBT parser, so it says the feature is not
    available instead of inventing an analysis (QT-076, QT-078)."""
    report.check("hard", "psbt: page shown", bool(wait_for(app, has_text("Load a partially signed transaction"), 30)))
    record(app, out_dir, f"{step}-empty")
    if press(report, app, "Load PSBT from clipboard…"):
        report.check("hard", "psbt: an empty clipboard is refused with dash-qt's text",
                     bool(wait_for(app, has_text("Unable to decode PSBT from clipboard (invalid base64)"), 10)))
    if not (type_into(report, app, "PSBT (base64)", "cHNidP8BAAoCAAAAAAAAAAAAAAAA") and press(report, app, "Load")):
        return
    report.check("hard", "psbt: the demo answers 'not available yet' (no fake analysis)",
                 bool(wait_for(app, has_text("This feature is not available yet."), 10)))
    record(app, out_dir, f"{step}-load")


def m2_pages_flow(report, app, out_dir, step):
    """`--demo`: Wallets, Security, About, Command-line options from the
    sidebar, then a transaction's details with dash-qt's actions."""
    report.check("hard", "pages: overview shown", bool(wait_for(app, has_text("Balances"), 30)))
    if press(report, app, "Wallets"):
        report.check("hard", "wallets: import, restore and watch-only controls",
                     bool(wait_for(app, named(("push button",), "Import File…"), 10))
                     and bool(wait_for(app, named(("text", "entry"), "Account xpub"), 5)))
        record(app, out_dir, f"{step}-wallets")
    if press(report, app, "Security"):
        report.check("hard", "security: auto-lock picker", bool(wait_for(app, named(("combo box",), "Auto Lock"), 10)))
        report.check("hard", "security: says there is no biometric unlock on this system",
                     bool(wait_for(app, has_text_prefix("This computer has no biometric unlock"), 10)))
        record(app, out_dir, f"{step}-security")
    if press(report, app, "About"):
        report.check("hard", "about: Export Logs", bool(wait_for(app, named(("push button",), "Export Logs"), 10)))
        record(app, out_dir, f"{step}-about")
        if press(report, app, "Command-line options"):
            report.check("hard", "command-line options: dash-qt options listed",
                         bool(wait_for(app, has_text("-choosedatadir"), 10)))
            record(app, out_dir, f"{step}-command-line")
    if not select_sidebar(report, app, "Transactions"):
        return
    rows = wait_for(app, lambda n, i: i["role"] == "list item" and ("Received" in (i["name"] or "") or "Sent" in (i["name"] or "")), 15)
    report.check("hard", "transactions: rows listed", bool(rows))
    lists = [n for n, info in find_all(app, lambda n, i: is_list_container(i)) if len(list_items(n)) > 0]
    target = max(lists, key=lambda n: len(list_items(n))) if lists else None
    selection = safe(lambda: target.querySelection()) if target is not None else None
    ok = selection is not None and bool(safe(lambda: selection.selectChild(0), False))
    report.check("hard", "transactions: select the first row through AT-SPI Selection", ok)
    if not ok:
        return
    report.check("hard", "transactions: details show dash-qt's Abandon / Resend actions",
                 bool(wait_for(app, named(("push button",), "Abandon transaction"), 15))
                 and bool(wait_for(app, named(("push button",), "Resend transaction"), 5)))
    report.check("hard", "transactions: details list 'Net amount'", bool(wait_for(app, has_text("Net amount"), 5)))
    record(app, out_dir, f"{step}-transaction-details")


def m2_chooser_flow(report, app, out_dir, step):
    """Live `-choosedatadir`: dash-qt's Intro page (QT-004), OK with the
    default directory, then the wallet opens (onboarding on a new root)."""
    report.check("hard", "chooser: welcome page", bool(wait_for(app, has_text("Welcome to Dash Wallet."), 60)))
    report.check("hard", "chooser: status line for the default directory",
                 bool(wait_for(app, lambda n, i: (i.get("text") or i["name"] or "") in (
                     "A new data directory will be created.",
                     "Directory already exists. Add /name if you intend to create a new directory here."), 15)))
    record(app, out_dir, f"{step}-chooser")
    if not press(report, app, "OK"):
        return
    report.check("hard", "chooser: OK opens the wallet (onboarding on the new root)",
                 bool(wait_for(app, has_text("Create a new wallet"), 300)))
    record(app, out_dir, f"{step}-opened")


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
        if any(name(f).startswith("Dash Wallet") for f in frames):
            break
        time.sleep(0.25)
    report.check("hard", 'window is a frame named "Dash Wallet…"', any(name(f).startswith("Dash Wallet") for f in frames),
                 str([name(f) for f in frames]))

    for step in args.steps.split(","):
        step_name, _, action = step.partition("=")
        if action.startswith("flow:"):
            flows = {"onboarding": onboarding_flow, "send": send_flow, "tools": tools_flow, "overlay": overlay_flow,
                     "m2-menus": m2_menus_flow, "m2-options": m2_options_flow, "m2-tools": m2_tools_flow,
                     "m2-psbt": m2_psbt_flow, "m2-pages": m2_pages_flow, "m2-chooser": m2_chooser_flow}
            flows[action[len("flow:"):]](report, app, args.out, step_name)
            continue
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
        nodes = record(app, args.out, step_name)
        stats = summarize(nodes)
        report.check("soft", f"{step_name}: control naming", True, json.dumps(stats))
        if step_name.endswith("overview") or step_name.endswith("send"):
            _listbox, texts = sidebar(app)
            report.check("hard", f"{step_name}: sidebar list items {SIDEBAR}", all(t in texts for t in SIDEBAR),
                         str(texts))
        if step_name.endswith("send") or step_name.endswith("overview"):
            unnamed = [i["role"] for _n, i, _d in nodes
                       if i["role"] in ("text", "password text", "check box", "combo box") and not i["name"]]
            report.check("hard", f"{step_name}: every entry, switch and picker has an accessible name",
                         not unnamed, str(unnamed))
            rows = [i["name"] for _n, i, _d in nodes if i["role"] == "list item"]
            report.check("hard", f"{step_name}: every list row has an accessible name",
                         bool(rows) and all(rows), str(rows[:6]))
        if step_name.endswith("send"):
            buttons = {info["name"] for _n, info, _d in nodes if info["role"] == "push button"}
            for title in ("Send", "Add Recipient", "Clear All", "Use available balance"):
                report.check("hard", f"send: push button named '{title}'", title in buttons)
            names = {(info["role"], info["name"]) for _n, info, _d in nodes}
            for expected in (("text", "Pay To"), ("text", "Amount"), ("check box", "Subtract fee from amount"),
                             ("combo box", "Confirmation time target")):
                report.check("hard", f"send: {expected[0]} named '{expected[1]}'", expected in names)
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
