#!/usr/bin/env python3
"""Executable spec check for the E0-04 commit points (DASHPAY §2.6).

Run: python3 docs/design/checks/e0_04_dispatch_model.py  (exit 0 = pass)

DRAFT: an input to the E0-04 design (DEC-57), not an acceptance check yet.
Part 1 assumes that reading the record, the fence verdict and the record
write are one atomic step; e0_04_split_model.py drops that assumption and
finds violations (DASHPAY §2.6 open issues M-A, m-2, m-3).

Part 1 explores every interleaving of an asset lock's original first
hand-off, an asset-lock recovery (resume) of the same row, and
`lock_vault`, under two rules:

- "r3": the rule this check replaced. The hook's kind comes from the call
  path (a resume is a `Resend`, admitted without a lease) and a refused
  first hand-off leaves the row while a resume claim exists (the pin's
  `untrack_asset_lock`).
- "e0-04": the rule DASHPAY §2.6 specifies. Each signed artifact carries a
  dispatch record (Unsent, Dispatching, Revoked) bound to its flow's lease.
  The kind comes from that record, never from the call path: only a
  recorded possible dispatch (Dispatching) is a `Resend`. A refused `First`
  records Revoked and, in the same step, removes the row and releases its
  inputs, whatever resume claims exist; a resume that finds the row gone or
  Revoked sends nothing.

The check passes when the r3 rule admits the review's counterexample (a
first actual send after `lock_vault` returned, through a lease-free
`Resend`; review DW-E0-03 r4 M1) and the e0-04 rule admits no violation in
any of the three scenarios E0-04 tests: that interleaving, a never-
dispatched `Built` row found at load, and a genuine ambiguous prior
dispatch. Only the last may send under a revoked lease, and it must.

Part 2 is a discrete-time check of the single-H lock bound with two leases
(review DW-E0-03 r4 m1): revoking leases one after another can take about
2H; freezing first-dispatch admission across the session and then draining
every lease together takes at most H.

This is a model of the specified decisions, not a test of the hook, which
does not exist yet. Pinned code it abstracts (`bc41f1bc23`,
`PW` = packages/rs-platform-wallet/src): PW/wallet/asset_lock/build.rs
1100-1240, sync/recovery.rs 949-1025 and 1170-1185, sync/tracking.rs
172-199. Derived from the reviewer's resend_model.py.
"""

from __future__ import annotations

import sys
from dataclasses import dataclass, replace
from typing import Iterator, Optional

UNSENT, DISPATCHING, REVOKED = "Unsent", "Dispatching", "Revoked"


@dataclass(frozen=True)
class S:
    """One state of the original flow (o), the recovery (r) and the lock."""

    rule: str
    lease_revoked: bool
    # The artifact's dispatch record. Under r3 it is never consulted: the
    # field then only tracks whether a dispatch happened.
    record: str
    row: bool  # the `Built` row is tracked
    reserved: bool  # its inputs are reserved
    claims: int  # active resume claims on the row
    holders: frozenset  # actors holding the lease's shared permit
    o: str  # pause | transport | late | cleanup | done | absent
    r: str  # idle | claimed | transport | late | done
    o_verdict: Optional[str]
    # Sends, each tagged with whether a dispatch of these bytes was
    # recorded while the lease was live (ok) or not (a first send after
    # the lock: a violation).
    sends: tuple
    dispatched_live: bool


def all_done(s: S) -> bool:
    return s.o in ("done", "absent") and s.r == "done" and not s.holders


def send(s: S, who: str) -> S:
    return replace(s, sends=s.sends + ((who, s.dispatched_live),))


def handoff(s: S, kind_from_path: str) -> Iterator[tuple]:
    """One atomic step: take the permit (if needed), classify, record.

    Yields (label, next state, admitted, needs_permit).
    """
    if s.rule == "r3":
        if kind_from_path == "Resend":
            yield "Resend by call path, no lease", s, True, False
        elif s.lease_revoked:
            yield "First refused", s, False, False
        else:
            yield "First admitted", replace(
                s, record=DISPATCHING, dispatched_live=not s.lease_revoked
            ), True, True
        return
    if s.record == REVOKED:
        yield "record Revoked: no dispatch", s, False, False
    elif s.record == DISPATCHING:
        yield "record Dispatching: Resend, no lease", s, True, False
    elif s.lease_revoked:
        # Revoked proves the bytes never left and never will, so it
        # overrides every resume claim: the row and its inputs go now.
        yield "First refused: record Revoked, row and inputs released", replace(
            s, record=REVOKED, row=False, reserved=False
        ), False, False
    else:
        yield "First admitted: record Dispatching", replace(
            s, record=DISPATCHING, dispatched_live=not s.lease_revoked
        ), True, True


def steps(s: S) -> Iterator[tuple]:
    # lock_vault: takes the permit exclusively, so only when none is held.
    if not s.lease_revoked and not s.holders:
        yield "L: lock_vault revokes the lease and returns", replace(
            s, lease_revoked=True
        )

    # The original flow holds a released signature and its Built row.
    if s.o == "pause":
        for label, n, admitted, permit in handoff(s, "First"):
            if admitted:
                n = replace(
                    n,
                    o="transport",
                    holders=n.holders | {"O"} if permit else n.holders,
                )
            else:
                n = replace(n, o="cleanup")
            yield f"O: {label}", n
    elif s.o == "transport":
        yield "O: transport call returns; permit dropped", replace(
            send(s, "O"), o="done", o_verdict="Sent", holders=s.holders - {"O"}
        )
        yield "O: H expires; permit dropped, outcome unknown", replace(
            s, o="late", o_verdict="MaybeSent", holders=s.holders - {"O"}
        )
    elif s.o == "late":
        yield "O: timed-out bytes leave late", replace(send(s, "O"), o="done")
    elif s.o == "cleanup":
        if s.rule == "r3":
            if s.claims == 0 and s.row:
                n = replace(
                    s, row=False, reserved=False, o="done", o_verdict="Cancelled"
                )
            else:
                n = replace(s, o="done", o_verdict="MaybeSent")
            yield "O: cleanup after refusal", n
        else:
            yield "O: report Cancelled", replace(
                s, o="done", o_verdict="Cancelled"
            )

    # Recovery: snapshot and claim, wait for the transport, then hand off.
    if s.r == "idle":
        if not s.row:
            yield "R: nothing to resume", replace(s, r="done")
        else:
            yield "R: snapshot Built row, claim it", replace(
                s, r="claimed", claims=s.claims + 1
            )
    elif s.r == "claimed":
        for label, n, admitted, permit in handoff(s, "Resend"):
            if admitted:
                n = replace(
                    n,
                    r="transport",
                    holders=n.holders | {"R"} if permit else n.holders,
                )
            else:
                n = replace(n, r="done", claims=n.claims - 1)
            yield f"R: {label}", n
    elif s.r == "transport":
        yield "R: transport call returns", replace(
            send(s, "R"), r="done", claims=s.claims - 1, holders=s.holders - {"R"}
        )
        yield "R: H expires; permit dropped", replace(
            s, r="late", claims=s.claims - 1, holders=s.holders - {"R"}
        )
    elif s.r == "late":
        yield "R: timed-out bytes leave late", replace(send(s, "R"), r="done")


def violations(s: S, scenario: str) -> list:
    out = []
    if any(not ok for _, ok in s.sends):
        out.append("a first actual send after lock_vault returned")
    if not s.reserved and (s.dispatched_live or s.sends):
        out.append("inputs released for bytes that may be on the wire")
    if all_done(s):
        if not s.dispatched_live and (s.row or s.reserved or s.claims):
            out.append("a never-dispatched row left reserved after the lock")
        if s.o_verdict == "Cancelled" and s.sends:
            out.append("reported Cancelled but sent")
        if scenario == "ambiguous" and not any(w == "R" for w, _ in s.sends):
            out.append("a genuine ambiguous dispatch was not resent")
    return out


def replay(init: S, trace: list) -> Optional[S]:
    """The state a trace of step labels leads to, or None if the rule does
    not admit it."""
    s = init
    for label in trace:
        s = next((n for lab, n in steps(s) if lab == label), None)
        if s is None:
            return None
    return s


def explore(init: S, scenario: str) -> list:
    """All reachable violations, each with the shortest trace found (BFS)."""
    seen = {init: []}
    frontier = [init]
    found = []
    while frontier:
        nxt = []
        for s in frontier:
            for v in violations(s, scenario):
                found.append((v, seen[s]))
            for label, n in steps(s):
                if n not in seen:
                    seen[n] = seen[s] + [label]
                    nxt.append(n)
        frontier = nxt
    return found


def scenarios(rule: str) -> dict:
    base = S(
        rule=rule,
        lease_revoked=False,
        record=UNSENT,
        row=True,
        reserved=True,
        claims=0,
        holders=frozenset(),
        o="pause",
        r="idle",
        o_verdict=None,
        sends=(),
        dispatched_live=False,
    )
    return {
        # The review's interleaving, with every other ordering of the three
        # actors: a signed asset lock tracked as Built before its first
        # hand-off, a recovery of the same row, and lock_vault.
        "interleaving": base,
        # A Built row never handed off, found at load: the process ended
        # before the record was persisted, so it is Unsent, and its lease
        # did not survive the process.
        "restart": replace(base, lease_revoked=True, o="absent"),
        # A first hand-off recorded Dispatching under its live lease, then
        # timed out (MaybeSent); the lock came after. The same state stands
        # for a crash after the record and before the transport call.
        "ambiguous": replace(
            base,
            lease_revoked=True,
            record=DISPATCHING,
            dispatched_live=True,
            o="done",
            o_verdict="MaybeSent",
        ),
    }


def drain_bound(h: int) -> tuple:
    """Worst lock_vault drain, from the moment the vault gate is passed (time
    0), over two leases whose hand-offs each stall for the full H and start
    at any integer time in [-h, 2h]. Returns (sequential, frozen)."""
    starts = [None] + list(range(-h, 2 * h + 1))
    worst_seq = worst_frozen = 0
    for a in starts:
        for b in starts:
            # Sequential: revoke A (wait for its in-flight hand-off), then
            # B. B admits hand-offs until its own revocation begins.
            t = 0
            if a is not None and a < t:
                t = max(t, a + h)
            if b is not None and b < t:
                t = max(t, b + h)
            worst_seq = max(worst_seq, t)
            # Frozen: admission closes at 0 for every lease; then every
            # lease drains together. Only hand-offs begun before 0 remain.
            t = max([0] + [x + h for x in (a, b) if x is not None and x < 0])
            worst_frozen = max(worst_frozen, t)
    return worst_seq, worst_frozen


def main() -> int:
    ok = True

    def check(cond: bool, msg: str) -> None:
        nonlocal ok
        print(("PASS " if cond else "FAIL ") + msg)
        ok = ok and cond

    print("Part 1: dispatch classification and cleanup")
    # The review's trace (r4 M1 steps 1-5), replayed under each rule.
    trace = [
        "R: snapshot Built row, claim it",
        "L: lock_vault revokes the lease and returns",
        "O: First refused",
        "O: cleanup after refusal",
        "R: Resend by call path, no lease",
        "R: transport call returns",
    ]
    old = scenarios("r3")
    end = replay(old["interleaving"], trace)
    check(
        end is not None
        and "a first actual send after lock_vault returned"
        in violations(end, "interleaving"),
        "r3 rule admits the r4 M1 counterexample (the check can see it)",
    )
    for i, label in enumerate(trace, 1):
        print(f"       {i}. {label}")
    check(
        replay(scenarios("e0-04")["interleaving"], trace) is None,
        "e0-04 rule does not admit that trace (a refused First records "
        "Revoked; the resume finds it and sends nothing)",
    )
    check(
        any(v.startswith("a first actual send") for v, _ in explore(
            old["restart"], "restart")),
        "r3 rule also resends a never-dispatched row at load",
    )

    for name, init in scenarios("e0-04").items():
        found = explore(init, name)
        for v, trace in found[:3]:
            print(f"       {v}: {' -> '.join(trace)}")
        check(not found, f"e0-04 rule, scenario '{name}': no violation")

    print("Part 2: one lock bound H for every lease")
    seq, frozen = drain_bound(10)
    check(seq > 10, f"sequential revocation can wait {seq} > H = 10")
    check(frozen <= 10, f"freeze, then drain together: at most {frozen} <= H")

    print("ok" if ok else "FAILED")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
