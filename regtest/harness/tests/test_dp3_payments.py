"""DP3-05: the contact payments suite (T2, T3).

Drives DashPay through `dwcli` only (E0-09) on the network `DW_DP_NET` names; skipped when unset.
A and B are the contacts the DP2-06 round trip made (the `contacts` fixture), each on its own data
root:

1. pay both ways: A pays B and B pays A through the contact channel; each recipient's contact
   activity shows the payment, received from the other, with its amount;
2. no peers: A pays B with SPV running but no peer connected: reported as `send.no_peers` and
   definitely not sent, so no payment lock is set, and the next payment with peers goes through
   (#85, fixed in #123: dash-spv's "no connected peers" used to read as a rejection);
3. unknown outcome: A pays B through a relay that holds the transaction back, so no node gets it:
   the outcome is unknown; the contact is locked to that txid, the relay holds exactly that
   transaction (its hash is the txid), and a second payment is refused; once the relay has
   written those bytes on to its peer, `contact resolve-lock` settles the lock `Sent` and B sees
   the payment;
4. 25 cancelled confirms (more than the 20-address DIP-15 gap) in one session, each a prepared
   payment abandoned unsent (`paid: false`) that names its contact address, every one the same,
   then a payment to that same address: B sees it.

    DW_DP_HOLD_UPSTREAM=<ip:port of a peer serving BIP157 filters> (the relay's peer; with
    DW_DP_STRICT=1, as the nightly sets, the unknown-outcome case fails rather than skips without it)
"""

from __future__ import annotations

import os

import pytest

from dwd_regtest.dashpay import (
    HoldingPeer,
    pay_contact,
    send_kind,
    synced_until,
    try_pay,
    wait_until,
)

CANCELS = 25


def received(session, contact: str, txid: str) -> dict | None:
    items = session.ok("contact", "activity", contact, "--filter", "received")["items"]
    return next((p for p in items if p.get("txid") == txid), None)


def recipient_sees(member, contact: str, txid: str, name: str, timeout: float = 900) -> dict:
    with member.session(name) as s:
        return synced_until(s, lambda: received(s, contact, txid), timeout, f"{member.label} to see {txid}")


def test_pay_both_ways(pair, contacts):
    a, b = pair
    with a.session("dp3-a-pays") as sa:
        ab = pay_contact(sa, b.identity, 110_000, "--note", "a to b")
    with b.session("dp3-b-pays") as sb:
        ba = pay_contact(sb, a.identity, 120_000)
    seen_by_b = recipient_sees(b, a.identity, ab["txid"], "dp3-b-sees")
    seen_by_a = recipient_sees(a, b.identity, ba["txid"], "dp3-a-sees")
    assert seen_by_b["amount"] == 110_000, seen_by_b
    assert seen_by_a["amount"] == 120_000, seen_by_a


@pytest.fixture(scope="module")
def no_peers(pair, contacts):
    """A pays B with SPV running and no peer connected, then pays again with peers."""
    a, b = pair
    with a.session("dp3-no-peers", peers=["127.0.0.1:9"]) as sa:
        error = sa.err("pay-contact", b.identity, "--amount", "130000", timeout=600)
        lock = sa.ok("contact", "lock", b.identity)
    with a.session("dp3-after-no-peers") as sa:
        paid = pay_contact(sa, b.identity, 130_000)
    return {"error": error, "lock": lock, "paid": paid}


def test_no_peers_sets_no_lock(pair, no_peers):
    a, b = pair
    error = no_peers["error"]
    assert send_kind(error) is not None and error["params"]["outcome"] == "not_sent", error
    assert no_peers["lock"] is None
    assert recipient_sees(b, a.identity, no_peers["paid"]["txid"], "dp3-b-sees-2")["amount"] == 130_000


def test_no_peers_is_reported_as_no_peers(no_peers):
    error = no_peers["error"]
    assert (error["code"], send_kind(error)) == ("send.no_peers", "no_peers"), error
    assert "reason" not in error["params"], error


def test_unknown_outcome_locks_until_resolved(dp, pair, contacts):
    a, b = pair
    upstream = os.environ.get("DW_DP_HOLD_UPSTREAM")
    if not upstream:
        if dp.strict:
            pytest.fail("DW_DP_STRICT=1 and DW_DP_HOLD_UPSTREAM is unset: the unknown-outcome case cannot run")
        pytest.skip("set DW_DP_HOLD_UPSTREAM to a peer that serves BIP157 filters")
    relay = HoldingPeer(upstream)
    try:
        with a.session("dp3-unknown", peers=[relay.address]) as sa:
            wait_until(lambda: sa.ok("dashpay", "sync-status")["startup"] == "ready", 600, "A's bring-up")
            answer = try_pay(sa, b.identity, 140_000)
            assert not answer.get("ok"), answer
            error = answer["error"]
            assert send_kind(error) == "broadcast_unknown", error
            lock = sa.ok("contact", "lock", b.identity)
            assert lock and lock["txid"], lock
            txid = lock["txid"]
            assert error["params"]["prepared_txid"] == txid, (error, lock)
            assert txid in relay.held_txids(), (txid, relay.held_txids())
            refused = try_pay(sa, b.identity, 1000)["error"]
            assert refused["code"] == "contact.payment_locked", refused
            assert refused["params"]["txid"] == txid
            assert sa.ok("contact", "resolve-lock", b.identity) == "unknown"
            released = relay.release()
            assert released.get(txid) is True, f"the relay could not pass {txid} on: {released}"
            wait_until(lambda: sa.ok("contact", "resolve-lock", b.identity) == "sent", 900,
                       "the lock to resolve Sent", interval=15)
            assert sa.ok("contact", "lock", b.identity) is None
    finally:
        relay.release()
        relay.close()
    assert recipient_sees(b, a.identity, txid, "dp3-b-sees-3")["amount"] == 140_000


def test_cancelled_confirms_past_the_gap(pair, contacts):
    a, b = pair
    with a.session("dp3-cancels") as sa:
        lock = sa.ok("contact", "lock", b.identity)
        assert lock is None, f"an earlier case left the contact locked: {lock}"
        addresses = []
        for i in range(CANCELS):
            cancelled = pay_contact(sa, b.identity, 150_000, "--cancel")
            assert (cancelled["cancelled"], cancelled["paid"]) == (True, False), cancelled
            assert "txid" not in cancelled and cancelled["prepared_txid"], cancelled
            assert isinstance(cancelled["address"], str) and cancelled["address"], (i, cancelled)
            addresses.append(cancelled["address"])
        assert sa.ok("contact", "lock", b.identity) is None
        paid = pay_contact(sa, b.identity, 150_000)
    print(f"{CANCELS} cancelled confirms used {len(set(addresses))} address(es); paid {paid.get('address')}")
    assert paid.get("address"), f"the payment's outcome was settled through its lock, so its output is unknown: {paid}"
    assert recipient_sees(b, a.identity, paid["txid"], "dp3-b-sees-4")["amount"] == 150_000
    assert len(set(addresses)) == 1, addresses
    assert paid["address"] == addresses[0], (paid, addresses)
