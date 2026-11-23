#!/usr/bin/env python3
"""Interactive nodes without an accessible name, as Orca sees the running app (AT-SPI).

Walks the AT-SPI tree of the application named --app (default dash-wallet) on the accessibility
bus of the current session and checks every node that is VISIBLE and either SHOWING or inside
the web view's document: a control scrolled out of view is checked too (WebKit drops SHOWING for
it), while the window's own menu items, VISIBLE but not SHOWING, are not. A node is
interactive when its role is one a user operates (INTERACTIVE below), when it offers an action a
user operates (click, press, activate, jump), when it is editable, or when it is FOCUSABLE and its
role is not a plain container, text or live region (PASSIVE). An interactive node fails when its
accessible name is empty after trimming.

The tree follows the DOM a moment later, so the walk repeats until the tree has caught up with the
state: every --expect node is showing, there are at least --min-interactive interactive nodes,
every read succeeded (each read is retried once), and two walks in a row agree. Otherwise it gives
up after --wait seconds with "settled": false.

Prints JSON lines: one {"unnamed": {...}} per failing node (its role, states, actions, the path of
its ancestors' roles and names, its own text or description, its parent's name, its siblings'
names and WebKit's object attributes, such as the tag), then one {"summary": {...}} with
"settled", "interactive", "missing" (the --expect nodes not found), "truncated" (parents with more
children than it reads, or a walk stopped at its node limit) and "read_failures" (nodes or subtrees
left unread). --redact drops every name and text from the failures (screens that show a recovery
phrase). Exit status: 0 when the walk ran (failures or not), 2 when the application is not on the
bus.

    python3 -I scripts/atspi-unnamed.py [--app NAME] [--wait SECONDS] [--redact]
        [--expect ROLE=NAME]... [--min-interactive N]

Needs python3-gi and gir1.2-atspi-2.0, and the AT-SPI bus (at-spi-bus-launcher) of the session.
scripts/test_atspi_unnamed.py checks the rules on synthetic trees.
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
# Action names that make a node something a user operates, whatever its role.
OPERATE = {"click", "press", "activate", "jump"}


class Reader:
    """Reads from the tree, retrying a read once; counts the reads that still fail."""

    def __init__(self):
        self.failures = 0

    def __call__(self, read):
        for attempt in (0, 1):
            try:
                value = read()
            except Exception:
                value = None
            if value is not None:
                return value
        self.failures += 1
        return None


def text_of(node, limit=160):
    try:
        iface = node.get_text_iface()
        if iface is None:
            return ""
        count = iface.get_character_count()
        return (iface.get_text(0, min(count, limit)) or "").replace("\ufffc", "").strip()
    except Exception:
        return ""


def states_of(node):
    return set(node.get_state_set().get_states())


def state_nicks(states):
    return sorted(Atspi.StateType(s).value_nick for s in states)


def actions_of(node):
    iface = node.get_action_iface()
    if iface is None:
        return []
    return [(iface.get_action_name(i) or "").lower() for i in range(iface.get_n_actions())]


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


def is_interactive(role, states, actions):
    """`role`: an Atspi.Role; `states`: a set of Atspi.StateType; `actions`: the action names.
    A plain container, text or live region that takes focus only to be read is not interactive,
    unless it offers an action a user operates (click, press, activate, jump)."""
    if role in INTERACTIVE:
        return True
    if any(a in OPERATE for a in actions):
        return True
    if Atspi.StateType.EDITABLE in states and Atspi.StateType.FOCUSABLE in states:
        return True
    return Atspi.StateType.FOCUSABLE in states and role not in PASSIVE


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


def squash(text):
    return " ".join(text.split())


def parse_expect(spec):
    """`ROLE=NAME`: ROLE an Atspi.Role name (HEADING, DIALOG, PUSH_BUTTON…), NAME the node's
    accessible name or, failing that, its text."""
    role, sep, name = spec.partition("=")
    if not sep or not hasattr(R, role.strip().upper()) or not squash(name):
        raise argparse.ArgumentTypeError(f"--expect {spec!r}: want ROLE=NAME with ROLE an Atspi.Role name")
    return (getattr(R, role.strip().upper()), squash(name))


DOCUMENTS = {R.DOCUMENT_WEB, R.DOCUMENT_FRAME}


def checked(states, in_document):
    """Whether a node is one a user can reach: VISIBLE, and SHOWING or inside the web document."""
    return Atspi.StateType.VISIBLE in states and (in_document or Atspi.StateType.SHOWING in states)


def walk(app, redact, expect=()):
    """Every node below `app` that `checked` accepts. Returns (failures, stats, found): `found` is the
    set of `expect` entries seen. stats["truncated"] counts parents with more than MAX_CHILDREN
    children (and a walk stopped at MAX_NODES); stats["read_failures"] counts reads that failed
    twice, each a node or subtree left unchecked."""
    read = Reader()
    failures = []
    found = set()
    stats = {"walked": 0, "checked": 0, "interactive": 0, "truncated": 0}
    seen = set()
    stack = [(app, [], False)]
    while stack:
        if stats["walked"] >= MAX_NODES:
            stats["truncated"] += 1
            break
        node, path, in_document = stack.pop()
        key = node_key(node)
        if key in seen:
            continue
        seen.add(key)
        stats["walked"] += 1
        states = read(lambda: states_of(node))
        if states is None:
            continue
        if Atspi.StateType.DEFUNCT in states:
            continue
        role = role_of(node)
        name = name_of(node)
        rid = role_id(node)
        if checked(states, in_document):
            stats["checked"] += 1
            for want in expect:
                if rid == want[0] and want[1] in (squash(name), squash(text_of(node, 400))):
                    found.add(want)
            actions = [] if rid in INTERACTIVE else read(lambda: actions_of(node))
            if actions is not None and is_interactive(rid, states, actions):
                stats["interactive"] += 1
                if not name:
                    parent = node.get_parent()
                    failures.append(
                        {
                            "role": role,
                            "states": state_nicks(states),
                            "actions": actions,
                            "path": [{"role": r, "name": "…" if redact and n else n} for r, n in path],
                            "text": "" if redact else (text_of(node) or (node.get_description() or "").strip()),
                            "parent": "" if redact or parent is None else name_of(parent),
                            "siblings": siblings_of(parent, node, redact) if parent is not None else [],
                            "attributes": attributes_of(node),
                        }
                    )
        count = read(lambda: node.get_child_count())
        if count is None:
            continue
        if count > MAX_CHILDREN:
            stats["truncated"] += 1
            count = MAX_CHILDREN
        child_path = path + [(role, name)]
        child_in_document = in_document or rid in DOCUMENTS
        for i in reversed(range(count)):
            child = read(lambda: node.get_child_at_index(i))
            if child is not None:
                stack.append((child, child_path, child_in_document))
    stats["read_failures"] = read.failures
    return failures, stats, found


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
    return (stats["interactive"], stats["truncated"], stats["read_failures"], json.dumps(failures, sort_keys=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--app", default="dash-wallet")
    parser.add_argument("--wait", type=float, default=30.0, help="seconds to wait for the app and a settled tree")
    parser.add_argument("--redact", action="store_true", help="drop names and text from the failures")
    parser.add_argument(
        "--expect",
        action="append",
        type=parse_expect,
        default=[],
        metavar="ROLE=NAME",
        help="a node the state shows (repeatable): the tree has not caught up until every one is there",
    )
    parser.add_argument("--min-interactive", type=int, default=1, help="fewer interactive nodes than this: not caught up")
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

    # The tree follows the DOM a moment later: walk until it holds every expected node, has
    # enough interactive nodes, every read succeeded, and two walks in a row agree.
    expect = set(args.expect)
    until = time.monotonic() + args.wait
    last = None
    while True:
        failures, found = [], set()
        stats = {"walked": 0, "checked": 0, "interactive": 0, "truncated": 0, "read_failures": 0}
        for app in apps:
            f, s, seen = walk(app, args.redact, expect)
            failures += f
            found |= seen
            for k in stats:
                stats[k] += s[k]
        sig = signature(failures, stats)
        ready = found == expect and stats["interactive"] >= args.min_interactive and stats["read_failures"] == 0
        if (ready and sig == last) or time.monotonic() >= until:
            stats["settled"] = ready and sig == last
            break
        last = sig if ready else None
        time.sleep(0.4)

    missing = sorted(f"{R(r).value_nick.upper().replace('-', '_')}={n}" for r, n in expect - found)
    for f in failures:
        print(json.dumps({"unnamed": f}, ensure_ascii=False))
    print(json.dumps({"summary": {**stats, "missing": missing, "unnamed": len(failures)}}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
