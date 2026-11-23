#!/usr/bin/env python3
"""Interactive nodes without an accessible name, as Orca sees the running app (AT-SPI).

Walks the AT-SPI tree of the application named --app (default dash-wallet) on the accessibility
bus of the current session and checks every node that is SHOWING and VISIBLE. A node is
interactive when its role is one a user operates (INTERACTIVE below), when it is editable, or
when it is FOCUSABLE and its role is not a plain container or text (PASSIVE). An interactive node
fails when its accessible name is empty after trimming.

Prints JSON lines: one {"unnamed": {...}} per failing node (its role, states, the path of its
ancestors' roles and names, its own text or description, its parent's name, its siblings' names
and WebKit's object attributes, such as the tag), then one {"summary": {...}}. --redact drops
every name and text from the failures (screens that show a recovery phrase). Exit status: 0 when
the walk ran (failures or not), 2 when the application is not on the bus.

    python3 -I scripts/atspi-unnamed.py [--app NAME] [--wait SECONDS] [--redact]

Needs python3-gi and gir1.2-atspi-2.0, and the AT-SPI bus (at-spi-bus-launcher) of the session.
"""

import argparse
import json
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402

R = Atspi.Role

INTERACTIVE = {
    R.PUSH_BUTTON,
    R.TOGGLE_BUTTON,
    R.CHECK_BOX,
    R.RADIO_BUTTON,
    R.LINK,
    R.ENTRY,
    R.PASSWORD_TEXT,
    R.COMBO_BOX,
    R.MENU_ITEM,
    R.CHECK_MENU_ITEM,
    R.RADIO_MENU_ITEM,
    R.SLIDER,
    R.SPIN_BUTTON,
    R.PAGE_TAB,
    getattr(R, "SWITCH", R.TOGGLE_BUTTON),
}

PASSIVE = {
    R.APPLICATION,
    R.FRAME,
    R.WINDOW,
    R.PANEL,
    R.SECTION,
    R.FILLER,
    R.DOCUMENT_WEB,
    R.DOCUMENT_FRAME,
    R.SCROLL_PANE,
    R.VIEWPORT,
    R.FORM,
    R.GROUPING,
    R.LANDMARK,
    R.LIST,
    R.TABLE,
    R.TABLE_ROW,
    R.TABLE_CELL,
    R.TOOL_BAR,
    R.ARTICLE,
    R.BLOCK_QUOTE,
    R.FOOTER,
    R.HEADER,
    R.REDUNDANT_OBJECT,
    R.TEXT,
    R.STATIC,
    R.LABEL,
    R.PARAGRAPH,
    R.HEADING,
    R.CAPTION,
    R.DESCRIPTION_TERM,
    R.DESCRIPTION_VALUE,
    R.MARK,
    R.SUPERSCRIPT,
    R.SUBSCRIPT,
    # Live regions: a message that takes focus to be read (a form's error) is read from its text.
    R.NOTIFICATION,
    R.ALERT,
    R.STATUS_BAR,
    R.LOG,
    R.TIMER,
}

MAX_NODES = 50_000
MAX_CHILDREN = 2_000


def text_of(node, limit=160):
    try:
        iface = node.get_text_iface()
        if iface is None:
            return ""
        count = Atspi.Text.get_character_count(node)
        return (Atspi.Text.get_text(node, 0, min(count, limit)) or "").replace("￼", "").strip()
    except Exception:
        return ""


def states_of(node):
    st = node.get_state_set()
    return st, sorted(Atspi.StateType(s).value_nick for s in st.get_states())


def name_of(node):
    try:
        return (node.get_name() or "").strip()
    except Exception:
        return ""


def role_of(node):
    try:
        return node.get_role_name() or ""
    except Exception:
        return ""


def role_id(node):
    try:
        return node.get_role()
    except Exception:
        return R.INVALID


def attributes_of(node):
    try:
        attrs = node.get_attributes() or {}
    except Exception:
        return {}
    keep = ("tag", "id", "class", "xml-roles", "html-id")
    return {k: v for k, v in attrs.items() if k in keep}


def is_interactive(role, st):
    """`role`: an Atspi.Role."""
    if role in INTERACTIVE:
        return True
    if st.contains(Atspi.StateType.EDITABLE) and st.contains(Atspi.StateType.FOCUSABLE):
        return True
    return st.contains(Atspi.StateType.FOCUSABLE) and role not in PASSIVE


def node_key(node):
    try:
        return (node.get_process_id(), node.path)
    except Exception:
        return (id(node), "")


def siblings_of(parent, node, redact):
    out = []
    try:
        n = min(parent.get_child_count(), MAX_CHILDREN)
        index = node.get_index_in_parent()
    except Exception:
        return out
    for i in range(max(0, index - 2), min(n, index + 3)):
        if i == index:
            continue
        try:
            sib = parent.get_child_at_index(i)
        except Exception:
            continue
        if sib is None:
            continue
        label = name_of(sib) or text_of(sib, 60)
        out.append({"role": role_of(sib), "name": "…" if redact and label else label})
    return out


def walk(app, redact):
    failures = []
    stats = {"walked": 0, "showing": 0, "interactive": 0}
    seen = set()
    stack = [(app, [])]
    while stack and stats["walked"] < MAX_NODES:
        node, path = stack.pop()
        key = node_key(node)
        if key in seen:
            continue
        seen.add(key)
        stats["walked"] += 1
        try:
            st, states = states_of(node)
        except Exception:
            continue
        if st.contains(Atspi.StateType.DEFUNCT):
            continue
        role = role_of(node)
        name = name_of(node)
        if st.contains(Atspi.StateType.SHOWING) and st.contains(Atspi.StateType.VISIBLE):
            stats["showing"] += 1
            if is_interactive(role_id(node), st):
                stats["interactive"] += 1
                if not name:
                    parent = node.get_parent()
                    failures.append(
                        {
                            "role": role,
                            "states": states,
                            "path": [{"role": r, "name": "…" if redact and n else n} for r, n in path],
                            "text": "" if redact else (text_of(node) or (node.get_description() or "").strip()),
                            "parent": "" if redact or parent is None else name_of(parent),
                            "siblings": siblings_of(parent, node, redact) if parent is not None else [],
                            "attributes": attributes_of(node),
                        }
                    )
        try:
            count = min(node.get_child_count(), MAX_CHILDREN)
        except Exception:
            continue
        child_path = path + [(role, name)]
        for i in reversed(range(count)):
            try:
                child = node.get_child_at_index(i)
            except Exception:
                continue
            if child is not None:
                stack.append((child, child_path))
    stats["truncated"] = stats["walked"] >= MAX_NODES
    return failures, stats


def find_apps(want):
    desktop = Atspi.get_desktop(0)
    apps = []
    for i in range(desktop.get_child_count()):
        try:
            app = desktop.get_child_at_index(i)
        except Exception:
            continue
        if app is not None and name_of(app) == want:
            apps.append(app)
    return apps


def has_document(app):
    """True once the web view's document is in the tree (WebKit builds it on first request)."""
    stack = [app]
    walked = 0
    while stack and walked < 5_000:
        node = stack.pop()
        walked += 1
        if role_id(node) in (R.DOCUMENT_WEB, R.DOCUMENT_FRAME):
            try:
                return node.get_child_count() > 0
            except Exception:
                return False
        try:
            for i in range(min(node.get_child_count(), MAX_CHILDREN)):
                child = node.get_child_at_index(i)
                if child is not None:
                    stack.append(child)
        except Exception:
            continue
    return False


def signature(failures, stats):
    return (stats["interactive"], json.dumps(failures, sort_keys=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--app", default="dash-wallet")
    parser.add_argument("--wait", type=float, default=30.0, help="seconds to wait for the app and a settled tree")
    parser.add_argument("--redact", action="store_true", help="drop names and text from the failures")
    args = parser.parse_args()

    until = time.monotonic() + args.wait
    apps = []
    while time.monotonic() < until:
        apps = [a for a in find_apps(args.app) if has_document(a)]
        if apps:
            break
        time.sleep(0.5)
    if not apps:
        names = []
        desktop = Atspi.get_desktop(0)
        for i in range(desktop.get_child_count()):
            child = desktop.get_child_at_index(i)
            names.append(name_of(child) if child is not None else None)
        print(json.dumps({"error": f"no application {args.app!r} with a web document on the AT-SPI bus", "apps": names}))
        return 2

    # The tree follows the DOM a moment later: walk until two walks in a row agree.
    until = time.monotonic() + args.wait
    last = None
    while True:
        failures, stats = [], {"walked": 0, "showing": 0, "interactive": 0, "truncated": False}
        for app in apps:
            f, s = walk(app, args.redact)
            failures += f
            for k in ("walked", "showing", "interactive"):
                stats[k] += s[k]
            stats["truncated"] = stats["truncated"] or s["truncated"]
        sig = signature(failures, stats)
        if sig == last or time.monotonic() >= until:
            stats["settled"] = sig == last
            break
        last = sig
        time.sleep(0.4)

    for f in failures:
        print(json.dumps({"unnamed": f}, ensure_ascii=False))
    print(json.dumps({"summary": {**stats, "unnamed": len(failures)}}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
