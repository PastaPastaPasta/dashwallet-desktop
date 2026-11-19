"""DP3-05: the contact payments suite (T2, T3).

Drives DashPay through `dwcli` only (E0-09) on the network `DW_DP_NET` names; skipped when unset.
A and B are the contacts the DP2-06 round trip made (the `contacts` fixture), each on its own data
root:

1. pay both ways: A pays B and B pays A through the contact channel; each recipient's contact
   activity shows the payment, received from the other, with its amount;
2. no peers: A pays B with SPV running but no peer connected: definite, so no payment lock is
   set, and the next payment with peers goes through. The engine reports it as a rejection rather
   than "no peers" (marked xfail: `reason_means_no_peers` misses dash-spv's wording);
3. unknown outcome: A pays B through a relay that holds the transaction back, so no node gets it:
   the outcome is unknown; the contact is locked to that txid and a second payment is refused;
   once the relay lets the bytes through, `contact resolve-lock` settles the lock `Sent` and B
   sees the payment;
4. 25 cancelled confirms (more than the 20-address DIP-15 gap) in one session, each a prepared
   payment abandoned unsent, then a payment: B still sees it.

    DW_DP_HOLD_UPSTREAM=<ip:port of a peer serving BIP157 filters> (the relay's peer)
"""

from __future__ import annotations

import os

import pytest

from dwd_regtest.dashpay import HoldingPeer, pay_contact, send_kind, synced_until, try_pay, wait_until

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
    assert no_peers["error"]["code"] == "send", no_peers["error"]
    assert send_kind(no_peers["error"]) in ("no_peers", "rejected"), no_peers["error"]
    assert no_peers["lock"] is None
    assert recipient_sees(b, a.identity, no_peers["paid"]["txid"], "dp3-b-sees-2")["amount"] == 130_000


def test_no_peers_is_reported_as_no_peers(no_peers):
    error = no_peers["error"]
    if send_kind(error) == "rejected" and "no connected peers" in error["message"]:
        pytest.xfail("engine bug: dash-spv's 'SPV broadcast not sent: no connected peers' is not matched by "
                     "reason_means_no_peers (dw-engine send/mod.rs), so a broadcast no peer got is reported "
                     "as `broadcast rejected`, not `no peers`")
    assert send_kind(error) == "no_peers", error


def test_unknown_outcome_locks_until_resolved(pair, contacts):
    a, b = pair
    upstream = os.environ.get("DW_DP_HOLD_UPSTREAM")
    if not upstream:
        pytest.skip("set DW_DP_HOLD_UPSTREAM to a peer that serves BIP157 filters")
    relay = HoldingPeer(upstream)
    try:
        with a.session("dp3-unknown", peers=[relay.address]) as sa:
            wait_until(lambda: sa.ok("dashpay", "sync-status")["startup"] == "ready", 600, "A's bring-up")
            answer = try_pay(sa, b.identity, 140_000)
            assert not answer.get("ok"), answer
            error = answer["error"]
            assert send_kind(error) == "unknown", error
            assert relay.held_count() >= 1
            lock = sa.ok("contact", "lock", b.identity)
            assert lock and lock["txid"], lock
            txid = lock["txid"]
            refused = try_pay(sa, b.identity, 1000)["error"]
            assert refused["code"] == "contact.payment_locked", refused
            assert refused["params"]["txid"] == txid
            assert sa.ok("contact", "resolve-lock", b.identity) == "unknown"
            relay.release()
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
        for _ in range(CANCELS):
            cancelled = pay_contact(sa, b.identity, 150_000, "--cancel")
            assert cancelled["cancelled"] is True
            addresses.append(cancelled["address"])
        paid = pay_contact(sa, b.identity, 150_000)
    print(f"{CANCELS} cancelled confirms used {len(set(addresses))} address(es); paid {paid.get('address')}")
    assert recipient_sees(b, a.identity, paid["txid"], "dp3-b-sees-4")["amount"] == 150_000
    assert len(set(addresses)) == 1, addresses
    assert paid.get("address", addresses[0]) == addresses[0], (paid, addresses)
