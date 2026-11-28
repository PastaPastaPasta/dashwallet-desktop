#!/usr/bin/env python3
"""scripts/atspi-unnamed.py's rules on synthetic trees (no AT-SPI bus, no app).

    python3 -I scripts/test_atspi_unnamed.py

Needs python3-gi and gir1.2-atspi-2.0 for the role and state enums.
"""

import importlib.util
import itertools
import pathlib
import sys
import unittest

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("atspi_unnamed", pathlib.Path(__file__).with_name("atspi-unnamed.py"))
walker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(walker)

Atspi = walker.Atspi
R = walker.R
S = Atspi.StateType
SHOWN = {S.SHOWING, S.VISIBLE}
paths = itertools.count()


class Broken(Exception):
    pass


class States:
    def __init__(self, states):
        self.states = states

    def get_states(self):
        return list(self.states)


class Node:
    """An AT-SPI accessible as atspi-unnamed.py reads it. `fail` names reads that raise (each
    call, or only the first `fail_once` ones)."""

    def __init__(self, role, name="", states=(), actions=(), children=(), text="", child_count=None, fail=(), fail_once=0, shown=SHOWN):
        self.role = role
        self.name = name
        self.states = set(states) | shown
        self.actions = list(actions)
        self.children = list(children)
        self.text = text
        self.child_count = child_count
        self.fail = set(fail)
        self.fail_once = fail_once
        self.parent = None
        self.path = f"/node/{next(paths)}"
        for c in self.children:
            c.parent = self

    def _check(self, what):
        if what in self.fail:
            if self.fail_once == 0:
                raise Broken(what)
            self.fail_once -= 1
            if self.fail_once == 0:
                self.fail.discard(what)
            raise Broken(what)

    def get_state_set(self):
        self._check("states")
        return States(self.states)

    def get_role(self):
        return self.role

    def get_role_name(self):
        return R(self.role).value_nick.replace("-", " ")

    def get_name(self):
        return self.name

    def get_description(self):
        return ""

    def get_attributes(self):
        return {}

    def get_process_id(self):
        return 1

    def get_parent(self):
        return self.parent

    def get_index_in_parent(self):
        return self.parent.children.index(self) if self.parent else -1

    def get_child_count(self):
        self._check("children")
        return len(self.children) if self.child_count is None else self.child_count

    def get_child_at_index(self, i):
        self._check("child")
        return self.children[i] if i < len(self.children) else None

    def get_text_iface(self):
        return self if self.text else None

    def get_character_count(self):
        return len(self.text)

    def get_text(self, start, end):
        return self.text[start:end]

    def get_action_iface(self):
        self._check("actions")
        return self if self.actions else None

    def get_n_actions(self):
        return len(self.actions)

    def get_action_name(self, i):
        return self.actions[i]


def app(*children):
    return Node(R.APPLICATION, "dash-wallet", children=[Node(R.FRAME, "Dash Wallet", children=[Node(R.DOCUMENT_WEB, children=list(children))])])


def unnamed_roles(tree):
    failures, stats, _ = walker.walk(tree, redact=False)
    return [f["role"] for f in failures], stats


class Rules(unittest.TestCase):
    def test_a_focusable_alert_without_an_action_passes(self):
        roles, stats = unnamed_roles(app(Node(R.ALERT, states={S.FOCUSABLE}, text="The passphrases do not match.")))
        self.assertEqual(roles, [])
        self.assertEqual(stats["interactive"], 0)

    def test_an_unnamed_alert_with_a_press_action_fails(self):
        roles, stats = unnamed_roles(app(Node(R.ALERT, actions=["press"])))
        self.assertEqual(roles, ["alert"])
        self.assertEqual(stats["interactive"], 1)

    def test_a_named_alert_with_a_press_action_passes(self):
        roles, stats = unnamed_roles(app(Node(R.ALERT, "Retry", actions=["press"])))
        self.assertEqual(roles, [])
        self.assertEqual(stats["interactive"], 1)

    def test_an_unnamed_section_with_a_click_action_fails(self):
        roles, _ = unnamed_roles(app(Node(R.SECTION, actions=["click"])))
        self.assertEqual(roles, ["section"])

    def test_an_action_a_user_does_not_operate_keeps_the_exemption(self):
        roles, _ = unnamed_roles(app(Node(R.PARAGRAPH, states={S.FOCUSABLE}, actions=["show-context-menu"])))
        self.assertEqual(roles, [])

    def test_an_unnamed_button_fails_and_a_named_one_passes(self):
        roles, stats = unnamed_roles(app(Node(R.PUSH_BUTTON), Node(R.PUSH_BUTTON, "Close")))
        self.assertEqual(roles, [R(R.PUSH_BUTTON).value_nick.replace("-", " ")])
        self.assertEqual(stats["interactive"], 2)

    def test_a_focusable_unknown_role_is_interactive(self):
        roles, _ = unnamed_roles(app(Node(R.IMAGE, states={S.FOCUSABLE})))
        self.assertEqual(roles, ["image"])

    def test_an_editable_focusable_section_is_interactive(self):
        roles, _ = unnamed_roles(app(Node(R.SECTION, states={S.EDITABLE, S.FOCUSABLE})))
        self.assertEqual(roles, ["section"])

    def test_a_hidden_node_is_not_checked(self):
        hidden = Node(R.PUSH_BUTTON)
        hidden.states = set()
        roles, stats = unnamed_roles(app(hidden))
        self.assertEqual((roles, stats["interactive"]), ([], 0))


    def test_a_control_scrolled_out_of_view_is_checked(self):
        roles, _ = unnamed_roles(app(Node(R.PUSH_BUTTON, shown={S.VISIBLE})))
        self.assertEqual(roles, [R(R.PUSH_BUTTON).value_nick.replace("-", " ")])

    def test_a_menu_item_outside_the_document_that_is_not_showing_is_not_checked(self):
        menu = Node(R.MENU_ITEM, shown={S.VISIBLE})
        tree = Node(R.APPLICATION, "dash-wallet", children=[Node(R.FRAME, "Dash Wallet", children=[Node(R.MENU_BAR, children=[menu])])])
        roles, stats = unnamed_roles(tree)
        self.assertEqual((roles, stats["interactive"]), ([], 0))


class Reads(unittest.TestCase):
    def test_an_unreadable_subtree_is_counted_not_passed(self):
        broken = Node(R.SECTION, fail={"states"}, children=[Node(R.PUSH_BUTTON)])
        roles, stats = unnamed_roles(app(broken))
        self.assertEqual(roles, [])
        self.assertEqual(stats["read_failures"], 1)
        self.assertEqual(stats["interactive"], 0)

    def test_a_read_that_fails_once_is_retried(self):
        flaky = Node(R.SECTION, fail={"states"}, fail_once=1, children=[Node(R.PUSH_BUTTON)])
        roles, stats = unnamed_roles(app(flaky))
        self.assertEqual(roles, [R(R.PUSH_BUTTON).value_nick.replace("-", " ")])
        self.assertEqual(stats["read_failures"], 0)

    def test_unreadable_children_are_counted(self):
        _, stats = unnamed_roles(app(Node(R.SECTION, fail={"children"}, children=[Node(R.PUSH_BUTTON)])))
        self.assertEqual(stats["read_failures"], 1)

    def test_a_missing_child_is_counted(self):
        _, stats = unnamed_roles(app(Node(R.SECTION, child_count=2, children=[Node(R.PUSH_BUTTON, "OK")])))
        self.assertEqual(stats["read_failures"], 1)

    def test_unreadable_actions_are_counted(self):
        _, stats = unnamed_roles(app(Node(R.SECTION, fail={"actions"})))
        self.assertEqual(stats["read_failures"], 1)

    def test_a_defunct_node_is_not_a_read_failure(self):
        gone = Node(R.PUSH_BUTTON, states={S.DEFUNCT})
        roles, stats = unnamed_roles(app(gone))
        self.assertEqual((roles, stats["read_failures"]), ([], 0))

    def test_more_children_than_it_reads_is_counted(self):
        many = Node(R.LIST, child_count=walker.MAX_CHILDREN + 1, children=[Node(R.PUSH_BUTTON, "OK")])
        _, stats = unnamed_roles(app(many))
        self.assertEqual(stats["truncated"], 1)


class Expect(unittest.TestCase):
    def test_parse(self):
        self.assertEqual(walker.parse_expect("heading=Import  keys"), (R.HEADING, "Import keys"))
        for bad in ("Import keys", "NOT_A_ROLE=x", "HEADING=  "):
            with self.assertRaises(Exception):
                walker.parse_expect(bad)

    def test_found_by_name_or_text_and_role(self):
        tree = app(
            Node(R.DIALOG, "Import keys"),
            Node(R.HEADING, text="Backups"),
            Node(R.PUSH_BUTTON, "Import file"),
            Node(R.PARAGRAPH, text="You can join DashPay once the wallet has synced."),
        )
        want = {
            (R.DIALOG, "Import keys"),
            (R.HEADING, "Backups"),
            (R.DIALOG, "Import file"),
            (R.PARAGRAPH, "You can join DashPay once the wallet has synced."),
        }
        _, _, found = walker.walk(tree, redact=False, expect=want)
        self.assertEqual(
            found,
            {
                (R.DIALOG, "Import keys"),
                (R.HEADING, "Backups"),
                (R.PARAGRAPH, "You can join DashPay once the wallet has synced."),
            },
        )


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False).result.wasSuccessful() else 1)
