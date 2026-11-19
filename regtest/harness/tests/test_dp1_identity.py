"""DP1-07: the identity suite (T2) and the nightly testnet registration (T3).

Drives DashPay through `dwcli` only (E0-09) on the network `DW_DP_NET` names; skipped when unset.
A fresh wallet is funded from the funder, registers an identity with a username from its Core
balance (DP1-02), and the suite then checks what a user relies on:

1. a fresh non-contested label is available, a contested-shaped one is available but contested,
   and the offline rules normalize homographs (o → 0) before the contest rule (DP1-03);
2. the registration's quote, its grant and the funding it charged (the funding cap holds), the
   flow ending `done`, and the identity listed with its name as the main one, its DashPay keys
   and credits;
3. the name is taken once registered, resolves to the identity and finds it as the wallet's own;
4. after a DashPay sync pass and in a new process the main name is still the same (#4978);
5. the phrase restored on another data root finds the identity and its name (DP1-05).

    DW_DP_NET=testnet DW_DP_STATE=/work/scratch/dw-suites/t3 DW_DP_FUNDER=… DW_DP_FUNDER_BIRTH=… \\
        DWCLI=<target>/debug/dwcli .venv/bin/python -m pytest -v tests/test_dp1_identity.py
"""

from __future__ import annotations

import secrets

import pytest

from dwd_regtest.dashpay import ids, register, wait_pass, wait_until

FUND_DUFFS = 5_000_000


@pytest.fixture(scope="module")
def registered(dp, funder):
    root = dp.root("dp1")
    wallet = root.create(f"dp1-{dp.run_id}")
    txid, tip = funder.pay(root, wallet.id, FUND_DUFFS)
    print(f"funded {wallet.id} with {FUND_DUFFS} duffs: {txid}")
    label = dp.label("dwdp1n")
    with root.session(wallet.id, name="dp1") as s:
        before = s.ok("identity", "list")
        availability = s.ok("name", "availability", label)
        contested = s.ok("name", "availability", "dwcontest" + "".join(secrets.choice("abcdefgh") for _ in range(6)))
        quote, row, steps = register(s, wallet.id, label)
        print(f"registration {label}: {steps}")
        identities = s.ok("identity", "list")
        balance = s.ok("identity", "balance")
        taken = s.ok("name", "availability", label)
        resolved = s.ok("name", "resolve", label)
        found = s.ok("identity", "find", label)
        wait_pass(s, ("dash_pay_sync", "dpns_sync"), timeout=1500)
        after_sync = s.ok("identity", "list")
    return {
        "root": root, "wallet": wallet, "label": label, "before": before, "availability": availability,
        "contested": contested, "quote": quote, "row": row, "steps": steps, "identities": identities,
        "balance": balance, "taken": taken, "resolved": resolved, "found": found, "after_sync": after_sync,
        "funding": txid, "birth": tip - 12,
    }


def test_names_before_registration(registered):
    assert registered["before"] == []
    assert registered["availability"] == {"kind": "available", "contested": False}
    assert registered["contested"]["kind"] in ("available", "contest_open", "taken", "locked")
    if registered["contested"]["kind"] == "available":
        assert registered["contested"]["contested"] is True


def test_offline_rules(dp, funder):
    root = dp.root("dp1")
    homograph = root.json("name", "check", "dwdp1-o0o")
    assert homograph["valid"] is True and homograph["normalized"] == "dwdp1-000"
    assert homograph["contested"] is True
    with_digit = root.json("name", "check", "dwdp2-o0o")
    assert with_digit["normalized"] == "dwdp2-000" and with_digit["contested"] is False
    too_short = root.json("name", "check", "ab")
    assert too_short["valid"] is False
    assert {r["rule"]: r["passed"] for r in too_short["rules"]}["min_length"] is False


def test_registration_ends_done_within_its_grant(registered):
    quote, row = registered["quote"], registered["row"]
    assert quote["contested"] is False
    assert quote["grant"]["max_duffs"] >= quote["total_duffs"] > quote["lock_duffs"] > 0
    assert row["phase"] == "done", row
    assert row["failure"] is None and row["name_failure"] is None, row
    assert row["funds_committed"] is True and row["identity"]
    phases = [step["phase"] for step in registered["steps"]]
    assert phases[-1] == "done" and "failed" not in phases, registered["steps"]


def test_identity_is_listed_with_its_name(registered):
    (identity,) = registered["identities"]
    assert identity["identity"] == registered["row"]["identity"]
    assert identity["index"] == 0
    assert identity["is_main"] is True
    assert identity["main_name"] == registered["label"], identity
    assert registered["label"] in identity["names"]
    assert identity["has_dashpay_keys"] is True
    assert (registered["balance"] or 0) > 0


def test_name_is_taken_and_resolves(registered):
    identity = registered["row"]["identity"]
    assert registered["taken"]["kind"] == "taken", registered["taken"]
    assert registered["taken"]["owner"] in (identity, None)
    assert registered["resolved"] is not None and identity in str(registered["resolved"])
    assert registered["found"]["identity"] == identity
    assert registered["found"]["own"] is True
    assert registered["label"] in registered["found"]["names"]


def test_main_name_survives_sync_and_restart(registered):
    (after,) = registered["after_sync"]
    assert after["main_name"] == registered["label"]
    root, wallet = registered["root"], registered["wallet"]
    (again,) = root.json("identity", "list", "--wallet", wallet.id)
    assert again["identity"] == registered["row"]["identity"]
    assert again["main_name"] == registered["label"]


def test_restore_finds_the_identity(dp, registered):
    root = dp.root("dp1-restore")
    restored = root.restore(registered["wallet"].mnemonic, registered["birth"], f"dp1r-{dp.run_id}")
    with root.session(restored.id, name="dp1-restore") as s:
        rows = wait_until(lambda: s.ok("identity", "list"), 900, "the restored wallet's identity")
        assert ids(rows) == [registered["row"]["identity"]]
        named = wait_until(lambda: (r := s.ok("identity", "list")) and r[0]["main_name"] and r, 600,
                           "the restored identity's name")
    assert named[0]["main_name"] == registered["label"]
    assert named[0]["is_main"] is True
