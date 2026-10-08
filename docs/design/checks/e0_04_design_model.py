#!/usr/bin/env python3
"""Spec check for the E0-04 design (docs/design/E0-04-grants-leases.md).

Run: python3 -I docs/design/checks/e0_04_design_model.py   (exit 0 = pass)

It checks every safety and timing property the design claims, by exhaustive
exploration, and runs each wrong rule the design rejects to show that the
check catches it. It replaces e0_04_dispatch_model.py, e0_04_split_model.py
and e0_04_mutations.py, which stay as the design's inputs.

What is modelled, at the granularity of the design's atomic steps:

- The fence's journal entry for one registered artifact (an asset lock):
  on disk `Unsent`, `Dispatching` or `PreFence` (seeded when the journal
  is created, for rows that predate it) or none, in memory also
  `Committing`, `Ambiguous` and `Revoked`. Every
  read-and-change of the in-memory entry is one step under the lease
  table's mutex J; the durable writes are separate steps outside J.
- The original flow O: register (durable Unsent), track the Built row and
  reserve its inputs, flush the row, admit, write Dispatching, transport.
- A resume R of the same row (platform-wallet's recovery), in the same
  process and again after a crash, which goes through the same admit.
- lock_vault L: freeze (revoke every lease, snapshot the permits), drain
  (wait until each snapshotted permit is dropped or past its deadline),
  return; or its future is dropped mid-drain.
- A crash at any step, with every outcome of the write or transport call
  in progress, followed by a reload (leases are per process, so dead).

Part 1 explores six scenarios and checks that no violation is reachable:
`flow` (O, R and L interleaved), `flow+crash` (the same with one crash and
a reload at any point), `restart` (a never-dispatched row found at load),
`ambiguous` (a genuine possible dispatch, then the lock), `legacy` (a row
written before the fence existed) and `unknown` (a row with no entry,
which must be neither sent nor cleaned up). It replays review DW-E0-03 r4 M1's
counterexample under the r3 rule (a violation) and shows the design does
not admit it, replays the M-A split-step trace's design counterpart, and
replays the acceptance tests' barriers 1 and 2.

Part 2 is a tick-by-tick simulation of the lock bound with up to three
leases, each with its own admit time, record-write time and transport
time, and the freeze as an event. It checks that lock_vault returns within
max(H, vault gate wait) of its call however many leases there are, that no
First is admitted once lock_vault was called, and that a second lock_vault
during a drain returns within the same bound. The rejected timing rules
(sequential revocation, a deadline that starts at the transport call, a
host that waits for the library, revocation deferred to the end of the
drain) each fail it.

Part 3 explores the lock order between the permits, the wallet-manager
guard G and the drain: the draft's FIFO permit lock deadlocks when a task
asks for a permit while holding G; the design's non-blocking admit and
host-side deadline cannot.

This is a model of the design's decisions, not a test of the code, which
does not exist yet.
"""

from __future__ import annotations

import itertools
import sys
from dataclasses import dataclass, replace
from typing import Iterator, Optional

U, C, D, A, T = "Unsent", "Committing", "Dispatching", "Ambiguous", "Revoked"
P = "PreFence"  # seeded at the journal's creation for rows that predate it
DEFINITE = ("Cancelled", "Failed", "NotSent")  # verdicts that promise "never sent"


# ---------------------------------------------------------------------------
# Part 1: dispatch, crash and lock
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class S:
    proc: int = 0  # 0: the process that built the row; 1: after a crash
    j_disk: Optional[str] = None  # journal entry on disk: None, U, D, P
    row_disk: bool = False  # the Built row is durable in wallet.sqlite
    mir: Optional[str] = None  # journal entry in memory: None, U, C, D, A, T, P
    lease: str = "live"  # live | revoked | dead (an earlier process's)
    lock: str = "idle"  # idle | draining | returned | dropped | gone (after a crash)
    snap: frozenset = frozenset()  # permits lock_vault waits for
    permits: frozenset = frozenset()  # First permits not yet dropped/expired
    pending_write: bool = False  # mutation transport-first: write queued
    row_mem: bool = False  # the row is tracked in memory
    reserved: bool = False  # its inputs are reserved
    o: str = "start"
    r: str = "idle"
    w: str = "none"  # a record write whose admit future was dropped
    o_verdict: Optional[str] = None
    crashes: int = 0  # crashes still allowed
    # History, used only by the checks.
    committed: bool = False  # a Dispatching write was attempted
    tcalled: bool = False  # some transport call started
    r_tcalled: bool = False
    sends: tuple = ()  # bytes that actually left: (who, proc)
    cleaned: bool = False  # a cleanup untracked the row
    releases: int = 0  # owner-guarded releases of this build's reservation
    foreign: bool = False  # after a reload, another build reserved the inputs
    foreign_freed: bool = False  # a release freed that other build's hold
    flags: frozenset = frozenset()  # violations seen on a transition


FLAG_TEXT = {
    "late_commit": "a commit after lock_vault was called",
    "foreign_commit": "a commit under an origin from an earlier process",
    "no_durable_commit": "a hand-off without a durable Dispatching record",
    "row_not_durable": "a hand-off before its row was durable",
    "early_return": "lock_vault returned while a hand-off it waits for ran",
    "first_send_after_lock": "a first actual send after lock_vault returned",
}


def actor(s: S, who: str) -> str:
    return {"O": s.o, "R": s.r}[who]


def put(s: S, who: str, state: str, verdict: Optional[str] = None) -> S:
    if who == "O":
        s = replace(s, o=state)
        return replace(s, o_verdict=verdict) if verdict else s
    return replace(s, r=state)


def flag(s: S, name: str) -> S:
    return replace(s, flags=s.flags | {name})


def lease_admits(s: S, m: frozenset) -> bool:
    if "revoke-in-drain" in m:
        # The draft: admission closes while draining; revocation waits for
        # the drain's end.
        return s.lease == "live" and s.lock != "draining"
    return s.lease == "live"


def start_commit(s: S, who: str, m: frozenset) -> S:
    """Unsent -> Committing under J, with a First permit."""
    s = replace(s, mir=C, permits=s.permits | {who}, committed=True)
    if s.proc == 0 and s.lock != "idle":
        s = flag(s, "late_commit")
    if s.proc != 0:
        s = flag(s, "foreign_commit")
    if "transport-first" in m:
        # The record write is queued and the transport is called at once.
        return to_transport(replace(s, pending_write=True), who, True)
    return put(s, who, "commit")


def to_transport(s: S, who: str, permit: bool) -> S:
    if s.j_disk not in (D, P):
        s = flag(s, "no_durable_commit")
    if not s.row_disk:
        s = flag(s, "row_not_durable")
    s = replace(s, tcalled=True, r_tcalled=s.r_tcalled or who == "R")
    return put(s, who, "transport" if permit else "resend")


def send(s: S, who: str) -> S:
    before_lock = s.committed and "late_commit" not in s.flags
    if (
        not s.sends
        and s.proc == 0
        and s.lock in ("returned", "dropped")
        and s.j_disk != P
        and not before_lock
    ):
        s = flag(s, "first_send_after_lock")
    return replace(s, sends=s.sends + ((who, s.proc),))


def refuse(s: S, who: str, m: frozenset) -> S:
    """The fence refuses a First. Under J: Unsent -> Revoked, and only the
    caller whose compare-and-set made Revoked cleans up (exactly once)."""
    if "r3" in m:
        return put(s, who, "cleanup")  # the r3 rule writes no record
    if s.mir == U or (s.mir == P and "legacy-unsent" in m):
        return put(replace(s, mir=T), who, "cleanup")
    return put(s, who, "done", "Cancelled" if who == "O" else None)


def admit(s: S, who: str, m: frozenset) -> Iterator[tuple]:
    """The fence's admit: one step under J (unless a mutation splits it)."""
    if "r3" in m and who == "R":
        yield "Resend by call path, no lease", to_transport(s, who, False)
        return
    if "split-literal" in m or "split-cas" in m:
        yield f"classify {s.mir}", put(s, who, f"cls:{s.mir}")
        return
    yield from decide(s, who, s.mir, m)


def decide(s: S, who: str, seen: Optional[str], m: frozenset) -> Iterator[tuple]:
    if seen is None:
        if "no-entry-resend" in m:
            yield "no entry: Resend", to_transport(s, who, False)
        else:
            # Unknown provenance: neither sent nor cleaned up (a Notice).
            yield "no entry: Deferred, kept", put(s, who, "done", "MaybeSent")
    elif seen == P:
        if "legacy-unsent" in m:
            yield "PreFence treated as Unsent: refused", refuse(s, who, m)
        else:
            yield "PreFence: Resend", to_transport(s, who, False)
    elif seen == T:
        yield "Revoked: refused", refuse(s, who, m)
    elif seen == D:
        if "strict-resend" in m and s.lease != "live":
            yield "Dispatching, lease not live: refused", put(
                s, who, "done", "MaybeSent"
            )
        else:
            yield "Dispatching: Resend", to_transport(s, who, False)
    elif seen == C:
        yield "commit in progress elsewhere: Deferred after H", put(
            s, who, "done", "MaybeSent"
        )
    elif seen == A:
        yield "ambiguous write: retry it", put(replace(s, mir=C), who, "retry")
    elif seen == U:
        if lease_admits(s, m):
            if "check-then-act" in m:
                yield "lease checked (no permit yet)", put(s, who, "checked")
            else:
                yield "First: commit and permit", start_commit(s, who, m)
        else:
            yield "First refused", refuse(s, who, m)


def act_split(s: S, who: str, seen: str, m: frozenset) -> Iterator[tuple]:
    """The second half of a split admit, acting on an earlier read."""
    seen_v = None if seen == "None" else seen
    if seen_v != U:
        yield from decide(s, who, seen_v, m)
        return
    if "split-cas" in m and s.mir != U:
        # Compare-and-set: the entry changed since the read; decide again.
        yield from decide(s, who, s.mir, m)
        return
    if lease_admits(s, m):
        yield "act on Unsent: commit", start_commit(s, who, m)
    else:
        # Literal: write Revoked whatever the entry is now.
        yield "act on Unsent: refused, Revoked", put(replace(s, mir=T), who, "cleanup")


def commit_steps(s: S, who: str, retry: bool, m: frozenset) -> Iterator[tuple]:
    """The Dispatching write of the actor holding Committing (outside J)."""
    ok = replace(s, j_disk=D, mir=D)
    if retry:
        yield "D write ok: Resend", to_transport(ok, who, False)
    elif who in s.permits:
        yield "D write ok: hand off under the permit", to_transport(ok, who, True)
    else:
        yield "D write ok after the deadline: Deferred", put(
            replace(ok, permits=s.permits - {who}), who, "done", "MaybeSent"
        )
    drop = s.permits - {who}
    yield from write_failures(s, who, drop, m)
    if who in s.permits:
        yield "permit deadline passes during the write", replace(s, permits=drop)
        if who == "O":
            yield "admit future dropped; the write goes on", replace(
                s, o="done", o_verdict="MaybeSent", w="write", permits=drop
            )


def write_failures(s: S, who: str, drop: frozenset, m: frozenset) -> Iterator[tuple]:
    if "clean-on-write-fail" in m:
        # The draft: a failed write leaves a clean Unsent.
        bad = put(replace(s, mir=U, permits=drop), who, "done", "NotSent")
        yield "D write failed: back to Unsent", bad
        yield "D write failed but reached disk: back to Unsent", replace(bad, j_disk=D)
        return
    bad = put(replace(s, mir=A, permits=drop), who, "done", "MaybeSent")
    yield "D write failed: Ambiguous, Deferred", bad
    yield "D write failed but reached disk: Ambiguous, Deferred", replace(bad, j_disk=D)


def transport_steps(s: S, who: str, permit: bool) -> Iterator[tuple]:
    drop = s.permits - {who}
    yield "transport returns: sent", put(
        replace(send(s, who), permits=drop), who, "done", "Sent"
    )
    yield "transport rejects: not sent, kept for a resend", put(
        replace(s, permits=drop), who, "done", "MaybeSent"
    )
    if permit:
        if who in s.permits:
            yield "permit deadline passes: MaybeSent", put(
                replace(s, permits=drop), who, "late", "MaybeSent"
            )
    else:
        yield "resend times out: MaybeSent", put(s, who, "late", "MaybeSent")


def cleanup_steps(s: S, who: str, verdict: str, m: frozenset) -> Iterator[tuple]:
    other_claim = who == "O" and s.r not in ("idle", "done")
    if "no-cleanup" in m or (("no-override" in m or "r3" in m) and other_claim):
        yield "refusal leaves the row", put(
            s, who, "done", "MaybeSent" if other_claim else verdict
        )
        return
    base = replace(
        s, row_mem=False, reserved=False, cleaned=True,
        releases=s.releases + (1 if s.reserved else 0),
    )
    if "unguarded-release" in m and s.foreign:
        base = replace(base, foreign_freed=True)
    yield "row removed, inputs released; removal flushed", put(
        replace(base, row_disk=False), who, "done", verdict
    )
    yield "row removed, inputs released; row still on disk", put(
        base, who, "done", verdict
    )


def flow_steps(s: S, who: str, m: frozenset) -> Iterator[tuple]:
    st = actor(s, who)
    if who == "O" and st == "start":
        yield "register (journal Unsent, durable)", replace(
            s, o="track", j_disk=U, mir=U
        )
        yield "register fails: nothing tracked", replace(s, o="done", o_verdict="Failed")
    elif who == "O" and st == "track":
        yield "track Built, reserve inputs", replace(
            s, o="flush", row_mem=True, reserved=True
        )
    elif who == "R" and st == "idle":
        if s.row_mem:
            yield "claim the tracked row", replace(s, r="flush")
        else:
            yield "nothing to resume", replace(s, r="done")
    elif st == "flush" and who == "R" and s.row_disk:
        # A row loaded from disk, or flushed by O, is already durable.
        yield "row already durable", put(s, who, "admit")
    elif st == "flush":
        nxt = put(s, who, "admit")
        if "no-flush" in m:
            yield "(no flush)", nxt
        else:
            yield "flush ok", replace(nxt, row_disk=s.row_mem)
        if who == "R":
            yield "flush fails: give up", put(s, who, "done")
        elif "abort-direct" in m:
            gone = replace(
                s, o="done", o_verdict="Failed", row_mem=False, reserved=False,
                cleaned=True, releases=s.releases + 1,
            )
            yield "flush fails: release directly", gone
            yield "flush fails: release directly; row lands later", replace(
                gone, row_disk=True
            )
        elif s.mir == U:
            # Abandon through the fence: Unsent -> Revoked under J. A failed
            # flush keeps its buffer, so the row may still land on disk.
            ab = replace(s, mir=T, o="cleanup_failed")
            yield "flush fails: abandon (Revoked)", ab
            yield "flush fails: abandon (Revoked); row lands later", replace(
                ab, row_disk=True
            )
        elif s.mir == T:
            yield "flush fails: already Revoked", replace(
                s, o="done", o_verdict="Cancelled"
            )
        else:
            yield "flush fails: entry not Unsent, keep the row", replace(
                s, o="done", o_verdict="MaybeSent"
            )
    elif st == "admit":
        yield from admit(s, who, m)
    elif st.startswith("cls:"):
        yield from act_split(s, who, st[4:], m)
    elif st == "checked":
        yield "take the permit and commit (no re-check)", start_commit(s, who, m)
    elif st == "commit":
        yield from commit_steps(s, who, False, m)
    elif st == "retry":
        yield from commit_steps(s, who, True, m)
    elif st == "transport":
        yield from transport_steps(s, who, True)
    elif st == "resend":
        yield from transport_steps(s, who, False)
    elif st == "late":
        yield "bytes leave late", put(send(s, who), who, "done")
        yield "bytes never leave", put(s, who, "done")
    elif st == "cleanup":
        yield from cleanup_steps(s, who, "Cancelled" if who == "O" else "Refused", m)
    elif st == "cleanup_failed":
        yield from cleanup_steps(s, who, "Failed", m)


def lock_steps(s: S, m: frozenset) -> Iterator[tuple]:
    if s.lock == "idle":
        n = replace(s, lock="draining", snap=s.permits)
        if "revoke-in-drain" not in m and n.lease == "live":
            n = replace(n, lease="revoked")
        yield "lock_vault called: freeze", n
    elif s.lock == "draining":
        held = s.snap & s.permits
        if not held or "no-drain-wait" in m:
            n = replace(s, lock="returned", snap=frozenset())
            if held:
                n = flag(n, "early_return")
            if "revoke-in-drain" in m and n.lease == "live":
                n = replace(n, lease="revoked")
            yield "drain done: lock_vault returns", n
        yield "lock_vault future dropped mid-drain", replace(
            s, lock="dropped", snap=frozenset()
        )


def crash_steps(s: S, m: frozenset) -> Iterator[tuple]:
    if s.proc != 0 or s.crashes == 0:
        return
    writing = "commit" in (s.o, s.r) or "retry" in (s.o, s.r) or s.w == "write"
    j_opts = sorted({s.j_disk, D} if writing else {s.j_disk}, key=str)
    flying = [x for x in ("O", "R") if actor(s, x) in ("transport", "resend", "late")]
    send_opts = [s.sends] + [s.sends + ((x, 0),) for x in flying]
    for j, snd in itertools.product(j_opts, send_opts):
        yield f"crash and reload (journal {j}, {len(snd)} sends)", replace(
            s,
            proc=1, j_disk=j, mir=j, sends=snd,
            lease="live" if "lease-reuse" in m else "dead",
            lock="gone", snap=frozenset(), permits=frozenset(),
            pending_write=False, row_mem=s.row_disk, reserved=False,
            o="gone", r="idle", w="none", crashes=s.crashes - 1,
        )


def steps(s: S, m: frozenset, with_crash: bool = True) -> Iterator[tuple]:
    yield from (("L: " + lab, n) for lab, n in lock_steps(s, m))
    for who in ("O", "R"):
        yield from ((f"{who}: {lab}", n) for lab, n in flow_steps(s, who, m))
    if s.w == "write":
        yield "W: orphaned D write ok", replace(s, w="none", j_disk=D, mir=D)
        yield "W: orphaned D write failed", replace(s, w="none", mir=A)
        yield "W: orphaned D write failed but reached disk", replace(
            s, w="none", mir=A, j_disk=D
        )
    if s.pending_write:
        yield "W: queued D write lands", replace(
            s, pending_write=False, j_disk=D, mir=D if s.mir == C else s.mir
        )
    if s.proc == 1 and not s.foreign and not s.reserved and s.row_mem:
        # The pin does not re-reserve a loaded row's inputs, so a new build
        # may take them.
        yield "B: another build reserves the row's inputs", replace(s, foreign=True)
    if with_crash:
        yield from crash_steps(s, m)


def all_done(s: S) -> bool:
    return (
        s.o in ("done", "absent", "gone")
        and s.r == "done"
        and s.w == "none"
        and not s.pending_write
        and s.lock != "draining"
    )


def may_send(s: S) -> bool:
    """The artifact went out or can still go out (now, or after a reload)."""
    return (
        s.tcalled or s.mir in (C, D, A, P) or s.j_disk in (D, P) or s.pending_write
    )


def violations(s: S, scenario: str, m: frozenset) -> list:
    out = [FLAG_TEXT[f] for f in sorted(s.flags)]
    if s.cleaned and may_send(s):
        out.append("a possibly-sent row cleaned up (untracked, inputs released)")
    if s.releases > 1:
        out.append("the build's reservation released twice")
    if s.foreign_freed:
        out.append("another build's reservation released")
    if s.o_verdict in DEFINITE and may_send(s):
        out.append("reported not sent, but sent or still sendable")
    if s.mir == T and s.j_disk == D:
        out.append("an artifact both Revoked and Dispatching")
    if scenario == "unknown" and s.cleaned:
        out.append("a row of unknown provenance cleaned up")
    if all_done(s):
        if (
            s.j_disk not in (D, P)
            and s.mir in (U, T)
            and s.lease != "live"
            and (s.row_mem or s.reserved)
        ):
            out.append("a never-dispatched row left reserved")
        if scenario in ("ambiguous", "legacy") and not s.r_tcalled:
            out.append("a genuine possible dispatch was not resent")
    elif not any(True for _ in steps(s, m, with_crash=False)):
        out.append("deadlock")
    return out


def scenarios() -> dict:
    flow = S()
    built = S(j_disk=U, mir=U, row_disk=True, row_mem=True, reserved=True)
    return {
        "flow": flow,
        "flow+crash": replace(flow, crashes=1),
        "restart": replace(built, proc=1, lease="dead", lock="gone", o="absent"),
        "ambiguous": replace(
            built, j_disk=D, mir=D, committed=True, tcalled=True,
            lease="revoked", lock="returned", o="late", o_verdict="MaybeSent",
            crashes=1,
        ),
        # A row written before the fence existed: the journal was seeded
        # with a PreFence entry for it when it was created.
        "legacy": replace(
            built, proc=1, j_disk=P, mir=P, lease="dead", lock="gone", o="absent",
        ),
        # A row with no entry at all: an old copy of wallet.sqlite restored
        # by hand, or a library that skipped register. Neither send nor clean.
        "unknown": replace(
            built, proc=1, j_disk=None, mir=None, lease="dead", lock="gone",
            o="absent",
        ),
    }


def explore(init: S, scenario: str, m: frozenset) -> list:
    """Every reachable violation with the shortest trace to it (BFS)."""
    seen = {init: []}
    frontier = [init]
    found = {}
    while frontier:
        nxt = []
        for s in frontier:
            for v in violations(s, scenario, m):
                found.setdefault(v, seen[s])
            for label, n in steps(s, m):
                if n not in seen:
                    seen[n] = seen[s] + [label]
                    nxt.append(n)
        frontier = nxt
    return sorted(found.items()), len(seen)


def replay(init: S, trace: list, m: frozenset) -> Optional[S]:
    s = init
    for label in trace:
        s = next((n for lab, n in steps(s, m) if lab == label), None)
        if s is None:
            return None
    return s


BUILT = [
    "O: register (journal Unsent, durable)",
    "O: track Built, reserve inputs",
    "O: flush ok",
]

# Review DW-E0-03 r4 M1, steps 1-5, under the r3 rule.
R4_TRACE_R3 = BUILT + [
    "R: claim the tracked row",
    "R: row already durable",
    "L: lock_vault called: freeze",
    "L: drain done: lock_vault returns",
    "O: First refused",
    "O: refusal leaves the row",
    "R: Resend by call path, no lease",
    "R: transport returns: sent",
]
# The same interleaving under the design.
R4_TRACE_DESIGN = BUILT + [
    "R: claim the tracked row",
    "R: row already durable",
    "L: lock_vault called: freeze",
    "L: drain done: lock_vault returns",
    "O: First refused",
    "O: row removed, inputs released; removal flushed",
    "R: Revoked: refused",
]
# M-A (e0_04_split_model.py): O dispatches, the lock comes, and only then
# does the resume act. Under the design its admit reads Dispatching.
MA_TRACE_DESIGN = BUILT + [
    "R: claim the tracked row",
    "R: row already durable",
    "O: First: commit and permit",
    "O: D write ok: hand off under the permit",
    "O: transport returns: sent",
    "L: lock_vault called: freeze",
    "L: drain done: lock_vault returns",
    "R: Dispatching: Resend",
]
BARRIER_1 = BUILT + [
    "L: lock_vault called: freeze",
    "L: drain done: lock_vault returns",
    "O: First refused",
    "O: row removed, inputs released; removal flushed",
]
BARRIER_2 = BUILT + [
    "O: First: commit and permit",
    "L: lock_vault called: freeze",
    "O: D write ok: hand off under the permit",
    "O: transport returns: sent",
    "L: drain done: lock_vault returns",
]
BARRIER_2_STALL = BUILT + [
    "O: First: commit and permit",
    "L: lock_vault called: freeze",
    "O: D write ok: hand off under the permit",
    "O: permit deadline passes: MaybeSent",
    "L: drain done: lock_vault returns",
    "O: bytes leave late",
]

# Each wrong rule the design rejects, and what it stands for.
MUTATIONS = {
    "r3": "the r3 rule: kind from the call path, pin's claim exclusion",
    "split-literal": "read, fence and write as separate steps (M-A)",
    "no-override": "a refusal does not override another party's claim",
    "no-cleanup": "a refusal leaves the row and its inputs",
    "strict-resend": "a Resend is refused once the lease is not live",
    "check-then-act": "the lease check and the permit are separate steps",
    "transport-first": "the transport is called before the record is durable",
    "clean-on-write-fail": "a failed record write leaves a clean Unsent (M-B c)",
    "legacy-unsent": "a pre-fence row is treated as never sent (M-B d)",
    "no-entry-resend": "a row with no entry is resent (trusts the call site)",
    "abort-direct": "an aborted build releases its inputs without the fence",
    "no-flush": "the row is handed off before it is durable",
    "revoke-in-drain": "revocation at the end of the drain; drop reopens (M-C)",
    "no-drain-wait": "lock_vault returns without waiting for the snapshot",
    "lease-reuse": "lease ids restart per process, so an old origin matches (m-1)",
    "unguarded-release": "the cleanup releases inputs without the build's token",
}


def part1() -> bool:
    ok = True

    def check(cond: bool, msg: str) -> None:
        nonlocal ok
        print(("PASS " if cond else "FAIL ") + msg)
        ok = ok and cond

    design = frozenset()
    print("Part 1: dispatch record, commit, crash and lock")
    total = 0
    for name, init in scenarios().items():
        found, n = explore(init, name, design)
        total += n
        for v, tr in found[:3]:
            print(f"       {v}: {' -> '.join(tr)}")
        check(not found, f"design, scenario '{name}': no violation ({n} states)")

    sc = scenarios()
    end = replay(sc["flow"], R4_TRACE_R3, frozenset({"r3"}))
    check(
        end is not None
        and "a first actual send after lock_vault returned" in violations(end, "flow", frozenset({"r3"})),
        "r3 rule admits the r4 M1 counterexample",
    )
    check(
        replay(sc["flow"], R4_TRACE_R3, design) is None,
        "design does not admit the r4 M1 trace",
    )
    end = replay(sc["flow"], R4_TRACE_DESIGN, design)
    check(
        end is not None and not end.sends and end.o_verdict == "Cancelled"
        and not end.row_mem and not end.reserved and end.r == "done"
        and end.releases == 1,
        "design, r4 interleaving: the resume finds Revoked, nothing is sent, "
        "row gone, inputs released, O reports Cancelled",
    )
    end = replay(sc["flow"], MA_TRACE_DESIGN, design)
    check(
        end is not None and end.r == "resend" and end.reserved and not end.cleaned,
        "design, M-A interleaving: the late resume reads Dispatching and "
        "resends; the inputs stay reserved",
    )
    end = replay(replace(sc["flow"], r="done"), BARRIER_1, design)
    check(
        end is not None and not end.sends and not end.tcalled
        and end.o_verdict == "Cancelled" and not end.row_mem,
        "barrier 1: paused before the check, resumed after lock_vault "
        "returned: nothing handed off, Cancelled, row gone",
    )
    early = replay(replace(sc["flow"], r="done"), BARRIER_2[:5] + [
        "L: drain done: lock_vault returns"], design)
    end = replay(replace(sc["flow"], r="done"), BARRIER_2, design)
    check(
        early is None and end is not None and end.o_verdict == "Sent"
        and end.lock == "returned",
        "barrier 2: paused after the check, lock_vault cannot return until "
        "the hand-off is done; Sent",
    )
    end = replay(replace(sc["flow"], r="done"), BARRIER_2_STALL, design)
    check(
        end is not None and end.o_verdict == "MaybeSent" and not violations(end, "flow", design),
        "barrier 2, stalled transport: lock_vault returns at the deadline, "
        "MaybeSent, late bytes allowed",
    )

    print("       mutations (each must be caught):")
    for mut, what in MUTATIONS.items():
        mm = frozenset({mut})
        hits = []
        for name, init in scenarios().items():
            found, _ = explore(init, name, mm)
            hits += [(name, v, tr) for v, tr in found]
        caught = bool(hits)
        if hits:
            name, v, tr = min(hits, key=lambda h: len(h[2]))
            print(f"       - {mut}: {what}\n           [{name}] {v}: {' -> '.join(tr)}")
        check(caught, f"mutation '{mut}' is caught")
    split_cas = []
    for name, init in scenarios().items():
        found, _ = explore(init, name, frozenset({"split-cas"}))
        split_cas += found
    check(
        not split_cas,
        "a split read and act is safe once the act is a compare-and-set "
        "(what the single step under J guarantees)",
    )
    return ok


# ---------------------------------------------------------------------------
# Part 2: the lock bound, tick by tick
# ---------------------------------------------------------------------------


def run_lock(rule: str, H: int, plans: list, drop_at=None, second_at=None):
    """lock_vault is called at tick 0. Each plan is None or (a, w, x): that
    lease's flow tries a First at tick a, its record write then takes w
    ticks and its transport x ticks, all under the lease's permit.

    Returns (tick the drain ends, tick a second lock_vault called at
    `second_at` ends, whether a First was admitted at or after tick 0).

    Rules: `design` (revoke every lease at the call; permit deadline = grant
    + H, enforced by the host; the library cuts its work there too),
    `sequential` (revoke one lease at a time, waiting for each),
    `transport-deadline` (the deadline is counted from the transport call),
    `host-waits` (the host waits until the library drops the permit; the
    library cuts only the transport), `drop-reopens` (admission closes at
    the call, revocation happens when the drain ends, and dropping the
    lock_vault future reopens admission)."""
    n = len(plans)
    state = ["idle"] * n
    deadline = [0] * n
    until = [0] * n
    revoked = [False] * n
    closed = False
    late_admit = False
    snapshot: set = set()
    seq = 0
    end_tick = None
    second: Optional[set] = None
    end2 = None

    def live(i: int, t: int) -> bool:
        if state[i] not in ("writing", "transport"):
            return False
        if rule == "host-waits":
            return True
        return t < deadline[i]

    for t in range(-H - 4, 8 * H + 40):
        if t == 0:
            snapshot = {i for i in range(n) if state[i] in ("writing", "transport")}
            if rule in ("design", "transport-deadline", "host-waits"):
                revoked = [True] * n
            elif rule == "drop-reopens":
                closed = True
        if t == drop_at and end_tick is None and rule == "drop-reopens":
            closed = False  # nothing was revoked; admission is open again
        for i, plan in enumerate(plans):
            if plan is None:
                continue
            a, w, x = plan
            if state[i] == "idle" and t == a:
                if revoked[i] or closed:
                    state[i] = "refused"
                    continue
                late_admit |= t >= 0
                state[i], until[i] = "writing", t + w
                # Under transport-deadline the write has no deadline at all.
                deadline[i] = 10**9 if rule == "transport-deadline" else t + H
            if state[i] == "writing" and t >= until[i]:
                state[i], until[i] = "transport", t + x
                if rule in ("transport-deadline", "host-waits"):
                    deadline[i] = t + H
            if state[i] == "transport" and (t >= until[i] or t >= deadline[i]):
                state[i] = "done"  # returned, or cut at the deadline
        if t >= 0 and end_tick is None:
            if rule == "sequential":
                while seq < n:
                    revoked[seq] = True
                    if live(seq, t):
                        break
                    seq += 1
                if seq == n:
                    end_tick = t
            elif not any(live(i, t) for i in snapshot):
                end_tick = t
                if rule == "drop-reopens":
                    revoked, closed = [True] * n, False
        if t == second_at:
            second = {i for i in range(n) if live(i, t)}
        if second is not None and end2 is None and not any(live(i, t) for i in second):
            end2 = t
    return end_tick, end2, late_admit


def part2() -> bool:
    ok = True

    def check(cond: bool, msg: str) -> None:
        nonlocal ok
        print(("PASS " if cond else "FAIL ") + msg)
        ok = ok and cond

    H = 4
    print(f"Part 2: one bound H for every lease (tick simulation, H = {H})")
    stall = 3 * H
    durs = (0, 1, stall)
    plans = [None] + [
        (a, w, x) for a in range(-H - 2, H + 3) for w in durs for x in durs
    ]
    worst, late = {}, {}
    for rule in ("design", "sequential", "transport-deadline", "host-waits"):
        worst[rule], late[rule] = 0, False
        for k in (1, 2):
            for combo in itertools.product(plans, repeat=k):
                e, _, la = run_lock(rule, H, list(combo))
                worst[rule] = max(worst[rule], e)
                late[rule] |= la
    coarse = [None] + [
        (a, w, x) for a in (-H - 1, -1, 0, 2) for w in (0, stall) for x in (1, stall)
    ]
    for combo in itertools.product(coarse, repeat=3):
        e, _, la = run_lock("design", H, list(combo))
        worst["design"] = max(worst["design"], e)
        late["design"] |= la
    check(
        worst["design"] <= H and not late["design"],
        f"design: the drain ends within H of the call for 1-3 leases "
        f"(worst {worst['design']}), and no First is admitted after the call; "
        f"lock_vault returns within max(H, vault gate wait)",
    )
    check(
        worst["sequential"] > H and late["sequential"],
        f"sequential revocation fails: worst {worst['sequential']} > H, and a "
        f"First is admitted after the call",
    )
    check(
        worst["transport-deadline"] > H,
        f"a deadline counted from the transport call fails (worst "
        f"{worst['transport-deadline']}): the record write is outside it",
    )
    check(
        worst["host-waits"] > H,
        f"a host that waits for the library to drop the permit fails (worst "
        f"{worst['host-waits']})",
    )
    two = list(itertools.product(plans, repeat=2))
    reopened = any(run_lock("drop-reopens", H, list(c), drop_at=k)[2]
                   for c in two for k in range(0, H))
    kept = not any(run_lock("design", H, list(c), drop_at=k)[2]
                   for c in two for k in range(0, H))
    check(
        reopened and kept,
        "a lock_vault future dropped mid-drain: deferred revocation reopens "
        "admission (caught); revocation at the call does not",
    )
    w2 = max(
        (run_lock("design", H, list(c), second_at=k)[1] or 0)
        for c in two for k in range(0, H + 1)
    )
    check(w2 <= H, f"a second lock_vault during the drain ends within H of the first call (worst {w2})")
    return ok


# ---------------------------------------------------------------------------
# Part 3: lock order (permits, the wallet-manager guard G, the drain)
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class L3:
    a: int = 0  # A: holds a permit, then needs G (its transport path)
    b: int = 0  # B: holds G, then asks for a permit
    l: int = 0  # L: lock_vault's drain
    g: Optional[str] = None  # holder of G
    readers: frozenset = frozenset()  # permit holders
    writer: bool = False  # the drain holds the permit lock exclusively
    queue: tuple = ()  # FIFO waiters on the permit lock: ("r", who) / ("w", "L")
    frozen: bool = False


def l3_steps(s: L3, rule: str) -> Iterator[tuple]:
    if rule == "draft":
        def grantable(kind, who):
            if s.writer:
                return False
            ahead = s.queue[: s.queue.index((kind, who))]
            if kind == "r":
                return not any(k == "w" for k, _ in ahead)
            return not s.readers and not ahead
        # A
        if s.a == 0:
            yield "A asks for a permit", replace(s, a=1, queue=s.queue + (("r", "A"),))
        elif s.a == 1 and grantable("r", "A"):
            yield "A gets the permit", replace(s, a=2, readers=s.readers | {"A"}, queue=tuple(q for q in s.queue if q != ("r", "A")))
        elif s.a == 2 and s.g is None:
            yield "A takes G", replace(s, a=3, g="A")
        elif s.a == 3:
            yield "A done", replace(s, a=4, g=None, readers=s.readers - {"A"})
        # B
        if s.b == 0 and s.g is None:
            yield "B takes G", replace(s, b=1, g="B")
        elif s.b == 1:
            yield "B asks for a permit (holding G)", replace(s, b=2, queue=s.queue + (("r", "B"),))
        elif s.b == 2 and grantable("r", "B"):
            yield "B gets the permit", replace(s, b=3, readers=s.readers | {"B"}, queue=tuple(q for q in s.queue if q != ("r", "B")))
        elif s.b == 3:
            yield "B done", replace(s, b=4, g=None, readers=s.readers - {"B"})
        # L
        if s.l == 0:
            yield "L asks for the permits exclusively", replace(s, l=1, queue=s.queue + (("w", "L"),))
        elif s.l == 1 and grantable("w", "L"):
            yield "L revokes and returns", replace(s, l=2, writer=False, queue=tuple(q for q in s.queue if q != ("w", "L")))
        return
    # Design: admit never waits; the drain waits only for permits to end or
    # pass their deadline; the deadline drops the holder's future.
    if s.a == 0:
        if s.frozen:
            yield "A refused", replace(s, a=4)
        else:
            yield "A admitted", replace(s, a=2, readers=s.readers | {"A"})
    elif s.a == 2 and s.g is None:
        yield "A takes G", replace(s, a=3, g="A")
    elif s.a == 3:
        yield "A done", replace(s, a=4, g=None, readers=s.readers - {"A"})
    if s.a in (2, 3):
        yield "A's deadline drops its future", replace(
            s, a=4, g=None if s.g == "A" else s.g, readers=s.readers - {"A"}
        )
    if s.b == 0 and s.g is None:
        yield "B takes G", replace(s, b=1, g="B")
    elif s.b == 1:
        if s.frozen:
            yield "B refused (holding G)", replace(s, b=4, g=None)
        else:
            yield "B admitted (holding G)", replace(s, b=3, readers=s.readers | {"B"})
    elif s.b == 3:
        yield "B done", replace(s, b=4, g=None, readers=s.readers - {"B"})
    if s.l == 0:
        yield "L freezes", replace(s, l=1, frozen=True)
    elif s.l == 1 and not s.readers:
        yield "L's drain ends; lock_vault returns", replace(s, l=2)


def l3_deadlocks(rule: str):
    init = L3()
    seen = {init: []}
    frontier = [init]
    while frontier:
        nxt = []
        for s in frontier:
            succ = list(l3_steps(s, rule))
            if not succ and not (s.a == 4 and s.b == 4 and s.l == 2):
                return seen[s]
            for lab, n in succ:
                if n not in seen:
                    seen[n] = seen[s] + [lab]
                    nxt.append(n)
        frontier = nxt
    return None


def part3() -> bool:
    ok = True

    def check(cond: bool, msg: str) -> None:
        nonlocal ok
        print(("PASS " if cond else "FAIL ") + msg)
        ok = ok and cond

    print("Part 3: lock order between permits, the wallet guard and the drain")
    tr = l3_deadlocks("draft")
    check(tr is not None, "draft (FIFO permit lock, permit asked under G) can deadlock")
    if tr:
        print("       " + " -> ".join(tr))
    check(l3_deadlocks("design") is None,
          "design (admit never waits, host-side deadline) cannot deadlock")
    return ok


def main() -> int:
    ok = part1()
    ok = part2() and ok
    ok = part3() and ok
    print("ok" if ok else "FAILED")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
