"""DP2-06: the contacts suite (T2, T3).

Drives DashPay through `dwcli` only (E0-09) on the network `DW_DP_NET` names; skipped when unset.
Two fresh members A and B, each on its own data root (its own device) with a registered identity:

1. round trip: A may ask B, A's request leaves B in A's pending section, B sees it as a request,
   accepts, and both list the other as a contact with a ready payment channel; A's session stays
   open meanwhile, and its channel must turn ready there too, not only after a restart (xfail:
   the engine drains the contact crypto only on an unlock or a bring-up);
2. the contact in a new process: still a contact, with its username once a pass names it;
3. ignore and unignore: a third member C asks A; A ignores the request (it moves to the hidden
   section as `ignored`, and stays there after a sync pass and a restart), then unignores it: the
   row goes at once, and the next requests sweep brings C's request back;
4. hidden: A hides B (the hidden section only, the relation still `contact`), then shows it again;
5. restore with contact accounts before SPV: A pays B through the contact channel; B's phrase is
   restored on a new data root; the bring-up before SPV finds B's identity and its contact A, so
   the first scan from the birth height finds the payment, attributed to A.

The one-way contact (a request sent but never answered, paid to) needs platform's receiving
account for sent-only contacts (#4740, which superseded #5256), which the pinned revision does not
have; its case is skipped with that reason.
"""

from __future__ import annotations

import pytest

from dwd_regtest.dashpay import (
    contact_page,
    find_contact,
    make_member,
    pay_contact,
    synced_until,
    wait_pass,
    wait_until,
)

PAY_DUFFS = 150_000


def test_round_trip(pair, contacts):
    a, b = pair
    assert contacts["eligibility"] == "ok"
    section, row = contacts["a_pending"]
    assert (section, row["relation"]) == ("pending", "pending_outgoing"), contacts["a_pending"]
    section, row = contacts["b_incoming"]
    assert section == "requests" and row["username"] == a.label, contacts["b_incoming"]
    section, row = contacts["b_contact"]
    assert section == "contacts" and row["username"] == a.label
    section, row = contacts["a_contact"]
    assert (section, row["channel"], row["username"]) == ("contacts", "ready", b.label), contacts["a_contact"]


def test_channel_ready_while_unlocked(contacts):
    section, row = contacts["a_live"]
    if row["channel"] == "setup_pending":
        pytest.xfail("engine bug: B's accept, seen by A's sync while A's vault is unlocked, queues A's account "
                     "build for a drain that runs only on an unlock or a bring-up, so the contact stays "
                     "`setup_pending` (\"Unlock to finish setting up\") until A's next unlock or restart")
    assert (section, row["channel"]) == ("contacts", "ready"), contacts["a_live"]


def test_contact_after_restart(pair, contacts):
    a, b = pair
    with a.session("dp2-restart") as sa:
        assert sa.ok("contact", "eligibility", b.identity) == "already_contact"
        section, row = wait_until(lambda: (r := find_contact(sa, b.identity)) and r[1]["username"] and r, 900,
                                  "B's username in a new process")
    assert (section, row["relation"], row["channel"], row["username"]) == ("contacts", "contact", "ready", b.label)


def test_ignore_and_unignore(dp, funder, pair, contacts):
    a, _ = pair
    c = make_member(dp, funder, "c", "dwdpc", 10_000_000)
    with c.session("dp2-c") as sc:
        sc.ok("contact", "request", a.identity, timeout=1200)
    with a.session("dp2-ignore") as sa:
        synced_until(sa, lambda: (r := find_contact(sa, c.identity)) and r[0] == "requests" and r,
                     1200, "A to list C's request")
        sa.ok("contact", "ignore", c.identity)
        section, row = find_contact(sa, c.identity)
        assert (section, row["relation"]) == ("hidden", "ignored"), (section, row)
        wait_pass(sa)
        section, row = find_contact(sa, c.identity)
        assert (section, row["relation"]) == ("hidden", "ignored"), (section, row)
    with a.session("dp2-ignore-restart") as sa:
        section, row = find_contact(sa, c.identity)
        assert (section, row["relation"]) == ("hidden", "ignored"), (section, row)
        sa.ok("contact", "unignore", c.identity)
        assert find_contact(sa, c.identity) is None
        section, row = synced_until(sa, lambda: find_contact(sa, c.identity), 900,
                                    "C's request to come back after unignore")
    assert (section, row["relation"]) == ("requests", "pending_incoming"), (section, row)


def test_hidden(pair, contacts):
    a, b = pair
    with a.session("dp2-hidden") as sa:
        sa.ok("contact", "details", b.identity, "--alias", "Bee", "--hidden")
        hidden = contact_page(sa, "--section", "hidden")
        contacts_only = contact_page(sa, "--section", "contacts")
        detail = sa.ok("contact", "show", b.identity)
        sa.ok("contact", "details", b.identity, "--alias", "Bee")
        shown = find_contact(sa, b.identity)
    assert b.identity in [r["contact"] for r in hidden["sections"][0]["contacts"]], hidden
    assert b.identity not in [r["contact"] for r in contacts_only["sections"][0]["contacts"]]
    assert detail["summary"]["relation"] == "contact" and detail["private_details"]["hidden"] is True
    assert detail["summary"]["alias"] == "Bee"
    section, row = shown
    assert section == "contacts" and row["hidden"] is False and row["alias"] == "Bee"


def test_restore_has_contact_accounts_before_spv(dp, pair, contacts):
    a, b = pair
    with a.session("dp2-pay") as sa:
        paid = pay_contact(sa, b.identity, PAY_DUFFS)
    print(f"A paid B {PAY_DUFFS} duffs: {paid['txid']}")
    root = dp.root("restore")
    restored = root.restore(b.wallet.mnemonic, b.birth, f"dp2r-{dp.run_id}")
    with root.session(restored.id, name="dp2-restore") as sr:
        status = sr.ok("dashpay", "sync-status")
        assert status["startup"] in ("ready", "starting"), status
        found = wait_until(lambda: find_contact(sr, a.identity), 600, "the restored wallet's contact A")
        assert found[1]["relation"] == "contact", found
        activity = wait_until(
            lambda: [p for p in sr.ok("contact", "activity", a.identity)["items"] if p.get("txid") == paid["txid"]],
            900, "the restored wallet's scan to find A's payment")
    (row,) = activity
    assert row["amount"] == PAY_DUFFS, row


@pytest.mark.skip(reason="one-way contacts need platform #4740 (supersedes #5256); the pinned revision "
                         "72d0a3e74c does not contain it")
def test_one_way_contact():
    raise AssertionError("unreachable")
