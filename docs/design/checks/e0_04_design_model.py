#!/usr/bin/env python3
"""Spec check for the E0-04 design, rev2 (docs/design/E0-04-grants-leases.md).

Run: python3 -I docs/design/checks/e0_04_design_model.py   (exit 0 = pass)

Within the state spaces below it checks the safety and timing properties of
the design's section 10.2, by exhaustive exploration. It runs each wrong rule
the design rejects, rev0's rules included, to show that the check catches it,
and it replays the reproducing interleavings of review DW-E0-04-design r1 (GPT)
under rev0's rule (a violation) and rev1's (none), and those of r2 (GPT, Opus)
under rev1's rule (a violation) and rev2's (none). It replaces
e0_04_dispatch_model.py, e0_04_split_model.py and e0_04_mutations.py, which
stay as the design's inputs. It is a model of the design's decisions, not a test
of the code, and it does not cover what section 10.1 lists as out of scope.

What is modelled, at the granularity of the design's atomic steps:

- The fence's journal entry for one registered artifact (an asset lock):
  on disk `Unsent`, `Dispatching` or `PreFence` (seeded for rows that predate
  the fence), with the row's recovery payload, or none; in memory also
  `Committing`, `Ambiguous` and `Revoked`. Every read-and-change of the
  in-memory entry is one step under the lease table's mutex J; the durable
  writes are separate steps outside J.
- The original flow O: register (durable Unsent and payload), track the Built
  row (reserving the inputs and pinning them in-broadcast), flush the row,
  admit, write Dispatching, transport. Its future may be dropped between track
  and admit (a drop guard abandons), and it may hand the same bytes off again.
- A resume R of the same row in the same process; after a crash or a power
  loss, R is the engine's catch-up at load (H6), which first restores a lost
  row from its payload and fences a possibly-sent row's inputs (L12, L16).
- Row-less artifacts: a state transition, a TxDraft send with inputs, a
  second holder of the same bytes (review GPT 1), and a resumable step with its
  durable write-ahead marker, across a crash, re-signed with identical or new
  bytes (GPT 3).
- K, the cleanup task the library spawns for the refusal's CAS winner; W, a
  journal write whose admit future was dropped; B, another build that may
  reserve inputs left free after a reload.
- lock_vault L: freeze (revoke every lease, snapshot the permits), drain,
  return with an outcome per flow judged from its history (GPT 7); or its
  future is dropped mid-drain.
- One crash, or one power loss (which may drop wallet.sqlite rows the FULL
  journal kept, GPT 5), at any step, with every outcome of the write or
  transport call in progress, followed by a reload.

The oracle judges every definite verdict against the immutable send history,
the attempts still running and the journal, never against the bookkeeping
under test (GPT 8).

Part 1 explores twelve scenarios and checks that no violation is reachable.
It replays review DW-E0-03 r4 M1's counterexample under the r3 rule (a
violation) and shows the design does not admit it; replays the M-A split-step
trace's design counterpart; replays the acceptance tests' barriers 1 and 2;
and replays GPT r1's reproductions of majors 1, 3, 5 and 7.

Part 2 is a tick-by-tick simulation of the lock bound with up to three leases
and the freeze as an event; five rejected timing rules fail it. It also checks
the lease-creation race (lock_gen), the window between the freeze and the
vault gate (GPT 2) and the rebind cap (GPT 6), each under rev0 and rev1; and
joined locks around an unlock (GPT r2 8), refunds after a rebind (GPT r2 9),
Repair's evidence rule (GPT r2 13) and the Touch ID combined cap (DEC-67),
each under rev1 and rev2.

Part 3 explores the lock order between the permits, the wallet-manager guard
G and the drain.

Part 4 explores an own-key registration's key hold against the ChainLock
fallback (GPT 4, Opus 9) under rev0, Mode A (surfaced fallback) and Mode B
(hidden fallback, bounded by key_until): Mode A has no finding, Mode B exactly
the documented residual.

Part 5 is Mode B (no platform PR): one funding call with power loss, a
withholding peer, the catch-up's resend, DP1-05's discovery, Repair's
self-spend and the user's choice to fund again, under rev1's "no entry means
retry" and rev2's status derivation (Opus r2 2, GPT r2 12); registration
step 2's ChainLock-height retry with its Locked classification and its Lock
copy (GPT r2 10, 11); and a retry offered after a restart (Opus r2 1a).
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
    rowless: bool = False  # the artifact has no row (a TxDraft send, an ST)
    rl_admitted: bool = False  # a row-less First was admitted in this process
    rl_out: bool = False  # a hand-off of those row-less bytes may have let them out
    rl_active: frozenset = frozenset()  # actors with a row-less attempt still running
    rl_inputs: bool = False  # the row-less artifact is a Core send with inputs
    rl_revoked: bool = False  # its inputs were released: tombstoned for this process
    resumable: bool = False  # a row-less step a later process may sign again (GPT 3)
    resigned: bool = False  # the resumed flow signs different bytes for the step
    step_marked: bool = False  # after a reload: the step's write-ahead marker exists
    lease_proc: int = 0  # the process the lease belongs to
    power: int = 0  # power losses still allowed (separate from crashes, GPT 5)
    payload: bool = False  # the journal holds the row's recovery payload (GPT 5)
    lock_outcome: Optional[str] = None  # what lock_vault's report says for O's flow
    pinned: bool = False  # the build's in-broadcast fence on the inputs
    pre_running: frozenset = frozenset()  # Firsts running at the freeze
    repeats: int = 0  # times O may hand the same bytes off again
    k: str = "none"  # a spawned cleanup task, with the verdict it settles
    expired: frozenset = frozenset()  # actors whose permit deadline passed
    # History, used only by the checks.
    committed: bool = False  # a Dispatching write was attempted
    tcalled: bool = False  # some transport call started
    r_tcalled: bool = False
    sends: tuple = ()  # bytes that actually left: (who, proc)
    cleaned: bool = False  # a cleanup untracked the row
    cleanups: int = 0  # cleanup runs in this process
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
    "twice_cleaned": "the cleanup ran twice in one process",
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


def grant(s: S, who: str) -> S:
    """A First permit, granted in a J step: the commit."""
    s = replace(s, permits=s.permits | {who}, expired=s.expired - {who}, committed=True)
    if s.lock not in ("idle", "gone"):
        s = flag(s, "late_commit")
    if s.lease_proc != s.proc:
        s = flag(s, "foreign_commit")
    return s


def start_commit(s: S, who: str, m: frozenset) -> S:
    """Unsent -> Committing under J, with a First permit."""
    s = grant(replace(s, mir=C), who)
    if "transport-first" in m:
        # The record write is queued and the transport is called at once.
        return to_transport(replace(s, pending_write=True), who, True)
    return put(s, who, "commit")


def to_transport(s: S, who: str, permit: bool, m: frozenset = frozenset()) -> S:
    if not s.rowless:
        if s.j_disk not in (D, P):
            s = flag(s, "no_durable_commit")
        if not s.row_disk:
            s = flag(s, "row_not_durable")
    if permit and "permit-ends-at-first" in m:
        s = replace(s, permits=s.permits - {who})
    s = replace(s, tcalled=True, r_tcalled=s.r_tcalled or who == "R")
    return put(s, who, "transport" if permit else "resend")


def send(s: S, who: str) -> S:
    before_lock = s.committed and "late_commit" not in s.flags
    if (
        not s.sends
        and s.lock in ("returned", "dropped")
        and s.j_disk != P
        and not before_lock
    ):
        s = flag(s, "first_send_after_lock")
    return replace(s, sends=s.sends + ((who, s.proc),))


def refuse(s: S, who: str, m: frozenset) -> S:
    """The fence refuses a First. Under J: Unsent -> Revoked, and only the
    caller whose compare-and-set made Revoked cleans up. The cleanup runs as
    a task the library spawns, so dropping the caller cannot lose it."""
    verdict = "Cancelled" if who == "O" else None
    if "r3" in m:
        return put(s, who, "cleanup")  # the r3 rule writes no record
    if s.rowless:
        # A Core send's refusal releases its inputs once, by the caller whose
        # compare-and-set tombstones the id; a state transition has nothing
        # to release. A resumed step whose marker exists may have been sent
        # by an earlier process: MaybeSent, never Cancelled (H10).
        if s.step_marked and "no-step-marker" not in m:
            verdict = "MaybeSent" if who == "O" else None
        n = put(s, who, "done", verdict)
        if s.rl_inputs and not s.rl_revoked:
            return spawn_cleanup(replace(n, rl_revoked=True), "rowless")
        return n
    if s.mir == U or (s.mir == P and "legacy-unsent" in m):
        n = replace(s, mir=T)
        if "inline-cleanup" in m:
            return put(n, who, "cleanup")
        return spawn_cleanup(put(n, who, "done", verdict), "refused")
    if "every-refuser-cleans" in m:
        return spawn_cleanup(put(s, who, "done", verdict), "refused")
    return put(s, who, "done", verdict)


def spawn_cleanup(s: S, why: str) -> S:
    return replace(s, k=why) if s.k == "none" else replace(s, k=s.k + "+" + why)


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
    if seen is None and s.rowless:
        if s.rl_revoked and "rowless-rev0" not in m:
            yield "row-less Revoked: refused", refuse(s, who, m)
        elif s.rl_admitted and "rowless-no-memory" not in m:
            yield "row-less, admitted before in this process: Resend", to_transport(
                replace(s, rl_active=s.rl_active | {who}), who, False
            )
        elif lease_admits(s, m):
            n = replace(s, rl_admitted=True, rl_active=s.rl_active | {who})
            if s.resumable and "no-step-marker" not in m:
                # The durable write-ahead marker is the commit: written, like
                # Dispatching, before any transport call (GPT 3).
                yield "row-less step First: write-ahead marker and permit", start_commit(n, who, m)
            else:
                yield "row-less First: permit", to_transport(grant(n, who), who, True, m)
        else:
            yield "row-less First refused", refuse(s, who, m)
        return
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
        yield "commit in progress elsewhere: Deferred", put(
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
        yield "D write ok: hand off under the permit", to_transport(ok, who, True, m)
    else:
        yield "D write ok after the deadline: Deferred", put(
            replace(ok, permits=s.permits - {who}), who, "done", "MaybeSent"
        )
    drop = s.permits - {who}
    yield from write_failures(s, who, drop, m)
    if who in s.permits:
        yield "permit deadline passes during the write", replace(
            s, permits=drop, expired=s.expired | {who}
        )
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


def transport_steps(s: S, who: str, permit: bool, m: frozenset) -> Iterator[tuple]:
    drop = s.permits - {who}
    if s.rowless:
        out = replace(s, rl_out=True)
        done = replace(out, rl_active=s.rl_active - {who})
        yield "transport returns: sent", put(
            replace(send(done, who), permits=drop), who, "done", "Sent"
        )
        yield from rowless_rejected(s, who, drop, m)
        if permit and who not in s.expired:
            yield "permit deadline passes: MaybeSent", put(
                replace(out, permits=drop, expired=s.expired | {who}), who, "late", "MaybeSent"
            )
        elif not permit:
            yield "resend times out: MaybeSent", put(out, who, "late", "MaybeSent")
        return
    yield "transport returns: sent", put(
        replace(send(s, who), permits=drop), who, "done", "Sent"
    )
    yield "transport rejects: not sent, kept for a resend", put(
        replace(s, permits=drop), who, "done", "MaybeSent"
    )
    if permit and who not in s.expired:
        yield "permit deadline passes: MaybeSent", put(
            replace(s, permits=drop, expired=s.expired | {who}), who, "late", "MaybeSent"
        )
    else:
        yield "resend times out: MaybeSent", put(s, who, "late", "MaybeSent")


def rowless_rejected(s: S, who: str, drop: frozenset, m: frozenset) -> Iterator[tuple]:
    """A definite rejection of one row-less attempt: its bytes did not leave
    in this call. Under J the fence settles the artifact as definitely
    unsent (forget the id, refund, and let the engine release the inputs)
    only if no hand-off of these bytes may have let them out and no other
    attempt is still running (review GPT 1). Otherwise the attempt reports
    MaybeSent and nothing is released."""
    n = replace(s, permits=drop, rl_active=s.rl_active - {who})
    label = "transport rejects: definitely not sent"
    if s.resumable and s.j_disk == D:
        # Its durable marker is a commit, as for a registered artifact.
        yield "transport rejects: not sent, marker kept", put(n, who, "done", "MaybeSent")
        return
    if "forget-rowless-history" in m:
        # GPT r1's escaping mutation: a rejected repeat forgets every earlier
        # hand-off and reports NotSent.
        n = replace(n, rl_admitted=False, rl_out=False, committed=False)
        yield label, put(n, who, "done", "NotSent")
        return
    if "rowless-rev0" in m:
        # rev0: forget on this attempt's own rejection unless an earlier one
        # finished possibly-out; concurrent attempts are not counted.
        if not s.rl_out:
            n = replace(n, committed=False, rl_admitted=False)
            yield label, put(n, who, "done", "NotSent")
        else:
            yield label, put(n, who, "done", "MaybeSent")
        return
    unsent = not s.rl_out and not (s.rl_active - {who})
    if unsent:
        n = replace(n, committed=False)
        if "rowless-keep-on-notsent" not in m:
            n = replace(n, rl_admitted=False)
        # The aggregate is definitely unsent: for a Core send the engine
        # releases the draft's inputs (owner-guarded) and its pin.
        n = put(n, who, "done", "NotSent")
        yield label, spawn_cleanup(replace(n, rl_revoked=True), "rowless") if s.rl_inputs else n
    else:
        yield label, put(n, who, "done", "MaybeSent")


def cleaned_up(s: S, m: frozenset) -> list:
    """Untrack the row and release the build's reservation (owner-guarded):
    the states with the row's removal flushed, and not yet flushed."""
    base = replace(
        s, row_mem=False, reserved=False, cleaned=True,
        # Settle the in-broadcast pin released (review Opus 5); the rev0
        # cleanup left it pending, so the inputs stayed unselectable.
        pinned=s.pinned and "pin-left-pending" in m,
        releases=s.releases + (1 if s.reserved else 0),
        cleanups=s.cleanups + 1,
    )
    if base.cleanups > 1:
        base = flag(base, "twice_cleaned")
    if "unguarded-release" in m and s.foreign:
        base = replace(base, foreign_freed=True)
    out = [("removal flushed", replace(base, row_disk=False))]
    if s.row_disk and not s.rowless:
        out.append(("row still on disk", base))
    return out


def cleanup_steps(s: S, who: str, verdict: str, m: frozenset) -> Iterator[tuple]:
    """Inline cleanup by the refused caller (the r3 rule, `inline-cleanup`)."""
    other_claim = who == "O" and s.r not in ("idle", "done")
    if "no-cleanup" in m or (("no-override" in m or "r3" in m) and other_claim):
        yield "refusal leaves the row", put(
            s, who, "done", "MaybeSent" if other_claim else verdict
        )
        return
    for lab, n in cleaned_up(s, m):
        yield f"row removed, inputs released; {lab}", put(n, who, "done", verdict)
    if "inline-cleanup" in m:
        yield "caller's future dropped before its cleanup", put(s, who, "done", verdict)


def flow_steps(s: S, who: str, m: frozenset) -> Iterator[tuple]:
    st = actor(s, who)
    if who == "O" and st == "start":
        # The entry carries the row's recovery payload (signed bytes, input
        # outpoints, funding metadata), written in the same FULL transaction.
        yield "register (journal Unsent, durable)", replace(
            s, o="track", j_disk=U, mir=U, payload="no-payload" not in m
        )
        yield "register fails: nothing tracked", replace(s, o="done", o_verdict="Failed")
    elif who == "O" and st == "track":
        # The build holds a key-wallet reservation and an in-broadcast pin.
        yield "track Built, reserve inputs", replace(
            s, o="flush", row_mem=True, reserved=True, pinned=True
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
            ab = spawn_cleanup(replace(s, mir=T, o="done", o_verdict="Failed"), "abandoned")
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
        yield from transport_steps(s, who, True, m)
    elif st == "resend":
        yield from transport_steps(s, who, False, m)
    elif st == "late":
        end = replace(s, rl_active=s.rl_active - {who})
        yield "bytes leave late", put(send(end, who), who, "done")
        yield "bytes never leave", put(end, who, "done")
    elif st == "cleanup":
        yield from cleanup_steps(s, who, "Cancelled" if who == "O" else "Refused", m)
    if who == "O" and st in ("flush", "admit") and not s.rowless:
        # The build's future is dropped after the row was tracked and
        # before admit decided: its drop guard abandons synchronously.
        if "no-drop-guard" in m:
            yield "build future dropped (no guard)", replace(s, o="done")
        elif s.mir == U:
            yield "build future dropped: guard abandons (Revoked)", spawn_cleanup(
                replace(s, mir=T, o="done"), "abandoned"
            )
        else:
            yield "build future dropped: entry not Unsent, keep", replace(s, o="done")
    if (
        who == "O" and st == "done" and s.repeats > 0 and not s.cleaned and s.k == "none"
        and s.o_verdict in ("Sent", "MaybeSent", "NotSent")
    ):
        # A new call: its verdict replaces the earlier one.
        yield "hand the same bytes off again", replace(
            s, o="admit", repeats=s.repeats - 1, o_verdict=None
        )


def lock_steps(s: S, m: frozenset) -> Iterator[tuple]:
    if s.lock == "idle":
        running = frozenset(
            x for x in ("O", "R")
            if actor(s, x) in ("commit", "transport") and x not in s.expired
        )
        n = replace(s, lock="draining", snap=s.permits, pre_running=running)
        if "revoke-in-drain" not in m and n.lease == "live":
            n = replace(n, lease="revoked")
        yield "lock_vault called: freeze", n
    elif s.lock == "draining":
        held = s.snap & s.permits
        # What the drain must wait for, judged from the hand-offs themselves:
        # a First begun before the freeze, still running, not past its
        # deadline.
        running = {
            x for x in s.pre_running
            if actor(s, x) in ("commit", "transport") and x not in s.expired
        }
        if not held or "no-drain-wait" in m:
            n = replace(
                s, lock="returned", snap=frozenset(), pre_running=frozenset(),
                lock_outcome=lock_outcome(s, m),
            )
            if running:
                n = flag(n, "early_return")
            if "revoke-in-drain" in m and n.lease == "live":
                n = replace(n, lease="revoked")
            yield "drain done: lock_vault returns", n
        yield DROP_LOCK, replace(s, lock="dropped", snap=frozenset())


DROP_LOCK = "lock_vault future dropped mid-drain"


def lock_outcome(s: S, m: frozenset) -> str:
    """What the lock report says for O's flow when the drain ends."""
    if "snapshot-outcome" in m:
        # rev0 §8.4, literally: Cancelled when the flow had nothing in flight.
        return "MaybeSent" if "O" in s.pre_running else "Cancelled"
    # rev1: from the flow's monotone dispatch history (GPT 7).
    if s.sends:
        return "Sent"
    if s.mir in (D, P) and s.row_mem:
        return "WillBeSent"
    if may_send(s) or s.committed:
        return "MaybeSent"
    return "Cancelled"


def crash_steps(s: S, m: frozenset) -> Iterator[tuple]:
    if s.proc != 0:
        return
    if s.rowless:
        if s.resumable and s.crashes:
            yield from resumable_reload(s, m)
        return
    if s.crashes:
        yield from reload_steps(s, m, power=False)
    if s.power:
        yield from reload_steps(s, m, power=True)


def resumable_reload(s: S, m: frozenset) -> Iterator[tuple]:
    """A crash during a resumable row-less step; the flow resumes in the next
    process under a new lease and signs the step again, identical bytes or
    (resigned) different ones; a lock may come there too."""
    writing = "commit" in (s.o, s.r) or "retry" in (s.o, s.r) or s.w == "write"
    j_opts = sorted({s.j_disk, D} if writing else {s.j_disk}, key=str)
    flying = [x for x in ("O", "R") if actor(s, x) in ("transport", "resend", "late")]
    send_opts = [s.sends] + [s.sends + ((x, 0),) for x in flying]
    for j, snd in itertools.product(j_opts, send_opts):
        yield f"crash; the flow resumes (marker {j}, {len(snd)} sends)", replace(
            s, proc=1, j_disk=j, mir=None if s.resigned else j,
            step_marked=s.resigned and j == D, sends=snd,
            lease="live", lease_proc=1, lock="idle", snap=frozenset(),
            permits=frozenset(), pending_write=False, o="admit", o_verdict=None,
            r="done", w="none", k="none", crashes=s.crashes - 1, cleanups=0,
            expired=frozenset(), rl_admitted=False, rl_active=frozenset(),
            # A durable marker is a commit made before anything in this
            # process, the lock included.
            rl_out=False, repeats=0, committed=j == D, lock_outcome=None,
        )


def reload_steps(s: S, m: frozenset, power: bool) -> Iterator[tuple]:
    writing = "commit" in (s.o, s.r) or "retry" in (s.o, s.r) or s.w == "write"
    j_opts = sorted({s.j_disk, D} if writing else {s.j_disk}, key=str)
    flying = [x for x in ("O", "R") if actor(s, x) in ("transport", "resend", "late")]
    send_opts = [s.sends] + [s.sends + ((x, 0),) for x in flying]
    # A power loss can lose wallet.sqlite rows (synchronous=NORMAL, Q11); the
    # FULL journal keeps what it acknowledged.
    row_opts = sorted({s.row_disk, False}) if power else [s.row_disk]
    kind = "power loss" if power else "crash"
    for j, snd, row in itertools.product(j_opts, send_opts, row_opts):
        restore = (
            not row and j in (D, P) and s.payload and "no-payload" not in m
        )
        row = row or restore
        held = row and j in (D, P) and "no-rereserve" not in m
        yield f"{kind} and reload (journal {j}, {len(snd)} sends, row {row})", replace(
            s,
            proc=1, j_disk=j, mir=j, sends=snd, row_disk=row,
            lease="live" if "lease-reuse" in m else "dead",
            lock="gone", snap=frozenset(), permits=frozenset(),
            pending_write=False, row_mem=row,
            # The catch-up at load (H6): a possibly-sent entry gets its row
            # back from the journal's payload if wallet.sqlite lost it (GPT
            # 5), and its inputs a pending-spend fence (L12).
            reserved=held, pinned=held,
            o="gone", r="idle", w="none", k="none",
            crashes=s.crashes - (0 if power else 1), power=s.power - (1 if power else 0),
            cleanups=0, expired=frozenset(),
        )


def steps(s: S, m: frozenset, with_crash: bool = True) -> Iterator[tuple]:
    yield from (("L: " + lab, n) for lab, n in lock_steps(s, m))
    if s.k != "none":
        rest = s.k.split("+", 1)[1] if "+" in s.k else "none"
        claimed = s.r not in ("idle", "done")
        if "no-cleanup" in m or ("no-override" in m and claimed):
            yield "K: cleanup task leaves the row", replace(s, k=rest)
        else:
            for lab, n in cleaned_up(replace(s, k=rest), m):
                yield f"K: cleanup task: row removed, inputs released; {lab}", n
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
    if s.proc == 1 and not s.foreign and not s.reserved and not s.pinned and s.row_mem and not s.rowless:
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
        and s.k == "none"
        and not s.pending_write
        and s.lock != "draining"
    )


def in_flight(s: S) -> bool:
    return any(actor(s, x) in ("transport", "resend", "late") for x in ("O", "R"))


def may_send(s: S) -> bool:
    """The artifact went out or can still go out (now, or after a reload).
    Judged from the immutable send history, the attempts still running and
    the journal, never from the bookkeeping under test (review GPT 8)."""
    return (
        bool(s.sends) or in_flight(s) or s.mir in (C, D, A, P)
        or s.j_disk in (D, P) or s.pending_write
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
    if s.proc == 1 and not s.rowless and s.j_disk in (D, P) and not (s.reserved and s.pinned):
        # With or without its wallet.sqlite row (GPT 5).
        out.append("inputs of a possibly-sent row left selectable after load")
    if s.lock_outcome == "Cancelled" and may_send(s):
        out.append("the lock report says Cancelled for a possibly-sent flow")
    if all_done(s):
        if s.cleaned and (s.reserved or s.pinned) and not may_send(s):
            out.append("inputs of a definitely-unsent artifact left unselectable")
        if (
            s.j_disk not in (D, P)
            and s.mir in (U, T)
            and s.lease != "live"
            and (s.row_mem or s.reserved)
        ):
            out.append("a never-dispatched row left reserved")
        if scenario in ("ambiguous", "legacy") and not s.r_tcalled:
            out.append("a genuine possible dispatch was not resent")
        if (
            scenario in ("flow+crash", "flow+power") and s.proc == 1
            and s.j_disk == D and not s.r_tcalled
        ):
            out.append("a genuine possible dispatch was not resent")
    elif not any(lab != "L: " + DROP_LOCK for lab, _ in steps(s, m, with_crash=False)):
        out.append("deadlock")
    return out


def scenarios() -> dict:
    flow = S()
    built = S(j_disk=U, mir=U, row_disk=True, row_mem=True, reserved=True, pinned=True)
    return {
        "flow": flow,
        "flow+crash": replace(flow, crashes=1),
        # A power loss instead: wallet.sqlite may lose rows the FULL journal
        # kept (GPT 5).
        "flow+power": replace(flow, power=1),
        "restart": replace(
            built, proc=1, lease="dead", lock="gone", o="absent", reserved=False, pinned=False
        ),
        "ambiguous": replace(
            built, j_disk=D, mir=D, committed=True, tcalled=True,
            lease="revoked", lock="returned", o="late", o_verdict="MaybeSent",
            crashes=1,
        ),
        # A row written before the fence existed: the journal was seeded
        # with a PreFence entry for it when it was created.
        "legacy": replace(
            built, proc=1, j_disk=P, mir=P, lease="dead", lock="gone", o="absent",
            reserved=True,
        ),
        # A row with no entry at all: an old copy of wallet.sqlite restored
        # by hand, or a library that skipped register. Neither send nor clean.
        # A row-less state transition: its First, a repeat of the same bytes
        # (an identical re-signature, F10) and the lock.
        "rowless": S(rowless=True, o="admit", r="done", repeats=1),
        # A resumable row-less step (an identity transition from an existing
        # lock): a crash, then the flow signs the identical bytes again in the
        # next process, where a lock may win (GPT 3).
        "rowless-resume": S(rowless=True, resumable=True, o="admit", r="done", crashes=1),
        # The same, re-signed into different bytes for the same step.
        "rowless-resigned": S(
            rowless=True, resumable=True, resigned=True, o="admit", r="done", crashes=1
        ),
        # A row-less Core send (TxDraft) with reserved and pinned inputs.
        "rowless-core": S(
            rowless=True, rl_inputs=True, o="admit", r="done", reserved=True,
            pinned=True, repeats=1,
        ),
        # The same, with an independent caller R handing off the identical
        # bytes concurrently (review GPT 1).
        "rowless-concurrent": S(
            rowless=True, rl_inputs=True, o="admit", r="admit", reserved=True,
            pinned=True, repeats=1,
        ),
        "unknown": replace(
            built, proc=1, j_disk=None, mir=None, lease="dead", lock="gone",
            o="absent", reserved=False, pinned=False,
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
    "R: Revoked: refused",
    "K: cleanup task: row removed, inputs released; removal flushed",
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
    "K: cleanup task: row removed, inputs released; removal flushed",
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

# Review DW-E0-04 r1 (GPT), majors 1, 3, 5 and 7: the reviewer's reproducing
# interleavings, with the rev0 rule each one breaks. Under that rule the
# trace is admitted and ends in a violation; under rev1 it is not admitted
# (and the scenario explores clean, above).
GPT_R1_REPLAYS = [
    ("GPT 1: concurrent row-less resend after Cancelled", "rowless-concurrent", "rowless-rev0", [
        "O: row-less First: permit",
        "R: row-less, admitted before in this process: Resend",
        "O: transport rejects: definitely not sent",
        "L: lock_vault called: freeze",
        "L: drain done: lock_vault returns",
        "O: hand the same bytes off again",
        "O: row-less First refused",
        "K: cleanup task: row removed, inputs released; removal flushed",
        "R: transport returns: sent",
    ]),
    ("GPT 3: a resumable row-less step without a write-ahead marker", "rowless-resume", "no-step-marker", [
        "O: row-less First: permit",
        "crash; the flow resumes (marker None, 1 sends)",
        "L: lock_vault called: freeze",
        "O: row-less First refused",
    ]),
    ("GPT 5: a Dispatching orphan after a power loss", "flow+power", "no-payload", BUILT + [
        "O: First: commit and permit",
        "O: D write ok: hand off under the permit",
        "power loss and reload (journal Dispatching, 0 sends, row False)",
    ]),
    ("GPT 7: an empty permit snapshot reported as Cancelled", "flow", "snapshot-outcome", BUILT + [
        "O: First: commit and permit",
        "O: D write ok: hand off under the permit",
        "O: transport returns: sent",
        "L: lock_vault called: freeze",
        "L: drain done: lock_vault returns",
    ]),
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
    "rowless-no-memory": "a repeat of row-less bytes under a revoked lease is refused",
    "no-rereserve": "the load catch-up leaves a possibly-sent row's inputs free",
    "no-drop-guard": "a build dropped before admit leaves its row tracked",
    "inline-cleanup": "the refused caller cleans up inline, so a drop loses it",
    "every-refuser-cleans": "every refused caller cleans up, not just the CAS winner",
    "permit-ends-at-first": "the permit is dropped when the transport call starts",
    "rowless-keep-on-notsent": "a row-less id stays admitted after a definite not-sent",
    "forget-rowless-history": "a rejected row-less repeat forgets earlier hand-offs (GPT r1)",
    "rowless-rev0": "rev0: a row-less id is forgotten on one attempt's rejection (GPT 1)",
    "pin-left-pending": "rev0: the cleanup leaves the in-broadcast pin pending (Opus 5)",
    "snapshot-outcome": "rev0: the lock report says Cancelled for a flow with nothing in flight (GPT 7)",
    "no-step-marker": "rev0: a resumable row-less step has no write-ahead marker (GPT 3)",
    "no-payload": "rev0: the journal keeps no recovery payload for a lost row (GPT 5)",
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
    sc_all = sc
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

    print("       review DW-E0-04 r1 (GPT) reproductions, rev0 rule vs rev1:")
    for name, sc, mut, tr in GPT_R1_REPLAYS:
        init = sc_all[sc]
        r0 = replay(init, tr, frozenset({mut}))
        bad = r0 is not None and bool(violations(r0, sc, frozenset({mut})))
        r1 = replay(init, tr, design)
        fixed = r1 is None or not violations(r1, sc, design)
        if r0 is not None:
            print(f"         {name}: rev0 -> {violations(r0, sc, frozenset({mut}))}")
        check(bad and fixed, f"{name}: a violation under rev0's rule, none under rev1's")

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


def creation_race(rule: str) -> Optional[tuple]:
    """begin_lease starts at tick b (it reads lock_gen, then redeems its
    grants) and inserts the lease into the table at tick b + d; its flow then
    tries a First at tick a. lock_vault is called at tick 0 and its freeze
    revokes every lease in the table. Returns the first (b, d, a) at which a
    First under a lease begun before the call is admitted at or after it."""
    def lock_gen(t: int) -> int:
        return 1 if t >= 0 else 0  # the freeze at tick 0 increments it

    for b in range(-3, 3):
        for d in (0, 1, 2):
            ins = b + d
            read = lock_gen(b)
            inserted = rule == "no-gen-check" or lock_gen(ins) == read
            for a in range(ins, 6):
                # In the table at the freeze (inserted before tick 0): revoked.
                revoked = ins < 0 <= a
                live_at_a = inserted and not revoked
                if live_at_a and b < 0 and a >= 0:
                    return (b, d, a)
    return None


def gate_window(rule: str) -> Optional[tuple]:
    """Review GPT 2. lock_vault is called at tick 0: its freeze revokes every
    lease in the table, and its vault gate, run on the blocking pool, ends
    the vault epoch (and with it every grant issued before) at tick g, which
    may be later than H. No permit is in flight, so the drain is empty and
    lock_vault returns at max(0, g). A grant G was issued before the call. A
    creator begins at tick b >= 0, after the freeze, so lock_gen does not
    stop it: it redeems G at the tick it starts, inserts, signs, and its
    flow tries a First at tick a. Under rev0 the creator waits only for
    pre-freeze permits (none); under rev1 it waits for the table's lock
    barrier, which ends when both the gate and the drain have.
    Returns the first (g, b, a) at which a First under G's authority is
    admitted after lock_vault returned."""
    H = 4
    for g in range(0, H + 3):
        ret = g
        for b in range(0, H + 3):
            start = b if rule == "rev0" else max(b, g)
            if start >= g:
                continue  # G died with the epoch the gate ended: nothing redeems it
            for a in range(start, 3 * H):
                if a >= ret:
                    return (g, b, a)
    return None


def rebind_cap(rule: str) -> Optional[tuple]:
    """Review GPT 6. A lease has a Credits cap of 1000 with `spent` charged.
    It moves to NeedsGrant and is rebound with a fresh grant capping
    credits at `fresh`. Under rev0 the lease keeps its old remaining budget
    (and the vault issues PlatformIdentity for any nonzero cap); under rev1
    the remaining budget becomes min(old remaining, fresh) and the charges
    already made stand. Returns the first (spent, fresh, cost) at which a
    newly signed transition costing more than the fresh grant is admitted."""
    for spent in (0, 400, 1000):
        for fresh in (0, 1, 500, 2000):
            for cost in (1, 100, 600, 1000):
                remaining = 1000 - spent
                if rule == "rev0":
                    allowed = remaining if fresh > 0 else 0
                else:
                    allowed = min(remaining, fresh)
                if cost <= allowed and cost > fresh:
                    return (spent, fresh, cost)
    return None


def joined_lock(rule: str) -> Optional[tuple]:
    """Review GPT r2 8. K1 is called at tick 0 while a pre-K1 permit stalls,
    so its drain ends at H - 1; K1's vault gate completes at g1. An unlock at
    u, after that gate, reopens the vault, and a grant G is issued then. K2
    is called at k, during the drain. Under rev1 K2 joins K1's coordinator
    and its completed gate; under rev2 every lock request runs its own vault
    gate (H14), so K2's gate ends G's epoch before K2 returns. A creator then
    waits for the barrier and redeems G if it is still alive. Returns the
    first (g1, u, k) at which a grant issued before K2's call is redeemed
    after K2 returned."""
    H = 4
    drain_end = H - 1
    for g1 in (0, 1):
        for u in range(g1 + 1, drain_end):
            for k in range(u + 1, drain_end + 1):
                k2_gate = None if rule == "rev1" else k
                k2_return = max(drain_end, k2_gate if k2_gate is not None else 0)
                creator = k2_return  # after the barrier clears
                alive = k2_gate is None or creator < k2_gate
                if alive:
                    return (g1, u, k)
    return None


def rebind_ceiling(rule: str) -> Optional[tuple]:
    """Review GPT r2 9. A Credits cap of 1000; an artifact S charged c_s is
    unresolved; the lease is rebound with a fresh cap F; S then settles
    definitely unsent and its charge is refunded; a new transition costing x
    is signed. Under rev1 the refund replenishes the remaining budget; under
    rev2 every charge belongs to the authority generation it was made under,
    a refund restores only that generation's accounting, and new signing is
    bounded by the current generation's ceiling min(old available, F).
    Returns the first (c_s, F, x) at which a new charge above F is admitted."""
    for c_s in (100, 500):
        for fresh in (1, 50, 600):
            for x in (1, 100, 600):
                ceiling = min(1000 - c_s, fresh)
                if rule == "rev1":
                    available = ceiling + c_s  # the refund adds the old charge back
                else:
                    available = ceiling  # the refund is generation 0's, not this one's
                if x <= available and x > fresh:
                    return (c_s, fresh, x)
    return None


def repair_resolution(rule: str) -> Optional[tuple]:
    """Review GPT r2 13. Repair looks at a tracked row with no journal entry
    (unknown provenance): its inputs are unspent and the transaction is not
    in the local chain or mempool. Under rev1 "Discard" then declares it
    definitely unsent; under rev2 only positive evidence does: a conflicting
    self-spend of one of its inputs is ChainLocked. Returns the first
    (peer_holds, conflict_final) at which the transaction is declared
    definitely unsent while it can still confirm."""
    for peer_holds in (False, True):
        for conflict_final in (False, True):
            unsent = True if rule == "rev1" else conflict_final
            can_still_confirm = peer_holds and not conflict_final
            if unsent and can_still_confirm:
                return (peer_holds, conflict_final)
    return None


def quickunlock_sum(rule: str) -> Optional[tuple]:
    """DEC-67 / review Opus r2 5e. Touch ID may issue PlatformOp grants
    capped at the spend limit L. "Accept and pay" asks for a set: a
    PlatformOp{d1, c1} and a Spend{d2}. Under rev1 the duffs and the credits
    were each checked against L separately (credits at L x 1000), so the
    set could reach about 2L in value; under rev2 one combined value
    sum(duffs) + ceil(sum(credits) / 1000) is checked against L. Returns the
    first set whose total value exceeds L and is still issued."""
    L = 1000
    for d1 in (0, 600, 1000):
        for c1 in (0, 600_000, 1_000_000):
            for d2 in (0, 600, 1000):
                duffs, credits = d1 + d2, c1
                value = duffs + -(-credits // 1000)
                if rule == "rev1":
                    issued = duffs <= L and credits <= L * 1000
                else:
                    issued = value <= L
                if issued and value > L:
                    return (d1, c1, d2)
    return None


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
    g0, g1 = gate_window("rev0"), gate_window("rev1")
    check(
        g0 is not None and g1 is None,
        f"GPT 2: a lease created after the freeze but before the vault gate ends: "
        f"under rev0 it admits a First after lock_vault returned (gate, begin, "
        f"First at {g0}); under rev1 it waits for the lock barrier, and the "
        f"pre-lock grant is dead by then",
    )
    r0, r1 = rebind_cap("rev0"), rebind_cap("rev1")
    check(
        r0 is not None and r1 is None,
        f"GPT 6: rebind under a smaller fresh grant: under rev0 a newly signed "
        f"transition above the fresh cap is admitted (spent, fresh, cost {r0}); "
        f"under rev1 the remaining budget is the minimum",
    )
    j1, j2 = joined_lock("rev1"), joined_lock("rev2")
    check(
        j1 is not None and j2 is None,
        f"GPT r2 8: lock, unlock, grant, a second lock joining the drain: under rev1 "
        f"the grant from before the second lock is redeemed after it returned "
        f"(g1, u, k {j1}); under rev2 every lock request runs its own vault gate",
    )
    c1, c2 = rebind_ceiling("rev1"), rebind_ceiling("rev2")
    check(
        c1 is not None and c2 is None,
        f"GPT r2 9: a refund after a rebind: under rev1 it lifts new signing above the "
        f"fresh cap (c_s, F, x {c1}); under rev2 charges keep their generation",
    )
    p1, p2 = repair_resolution("rev1"), repair_resolution("rev2")
    check(
        p1 is not None and p2 is None,
        f"GPT r2 13: Repair on a row of unknown provenance: under rev1 negative "
        f"observations declare it unsent while a peer holds it {p1}; under rev2 only a "
        f"ChainLocked conflicting spend does",
    )
    q1, q2 = quickunlock_sum("rev1"), quickunlock_sum("rev2")
    check(
        q1 is not None and q2 is None,
        f"DEC-67: Touch ID for \"Accept and pay\": under rev1 the set reaches about "
        f"twice the spend limit {q1}; under rev2 one combined value is capped",
    )
    bad = creation_race("no-gen-check")
    check(
        creation_race("design") is None and bad is not None,
        f"a lease begun before the call and inserted after it: refused by the "
        f"lock_gen check; without it a First is admitted after the call "
        f"(begin, insert delay, First at {bad})",
    )
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


def fifo_grantable(readers: frozenset, writer: bool, queue: tuple, req: tuple) -> bool:
    """tokio's RwLock is fair: a read waits behind a queued write."""
    if writer:
        return False
    ahead = queue[: queue.index(req)]
    if req[0] == "r":
        return not any(k == "w" for k, _ in ahead)
    return not readers and not ahead


def guard_deadlock(rule: str):
    """B holds the wallet guard G for reading and calls admit; W asks for G
    for writing meanwhile. Under `admit-takes-guard`, admit takes G for
    reading too (as a debit lookup would), and queues behind W."""
    init = (0, 0, frozenset(), False, ())
    seen = {init: []}
    frontier = [init]
    while frontier:
        nxt = []
        for st in frontier:
            b, w, readers, writer, queue = st
            succ = []
            if b == 0:
                succ.append(("B asks for G.read", (1, w, readers, writer, queue + (("r", "B"),))))
            elif b == 1 and fifo_grantable(readers, writer, queue, ("r", "B")):
                succ.append(("B holds G.read", (2, w, readers | {"B"}, writer, tuple(q for q in queue if q != ("r", "B")))))
            elif b == 2:
                if rule == "admit-takes-guard":
                    succ.append(("B calls admit; admit asks for G.read", (3, w, readers, writer, queue + (("r", "B2"),))))
                else:
                    succ.append(("B calls admit (takes no guard)", (5, w, readers, writer, queue)))
            elif b == 3 and fifo_grantable(readers, writer, queue, ("r", "B2")):
                succ.append(("admit holds G.read", (5, w, readers | {"B2"}, writer, tuple(q for q in queue if q != ("r", "B2")))))
            elif b == 5:
                succ.append(("B releases G", (6, w, readers - {"B", "B2"}, writer, queue)))
            if w == 0:
                succ.append(("W asks for G.write", (b, 1, readers, writer, queue + (("w", "W"),))))
            elif w == 1 and fifo_grantable(readers, writer, queue, ("w", "W")):
                succ.append(("W holds G.write, then releases", (b, 2, readers, False, tuple(q for q in queue if q != ("w", "W")))))
            if not succ and not (b == 6 and w == 2):
                return seen[st]
            for lab, n in succ:
                if n not in seen:
                    seen[n] = seen[st] + [lab]
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
    tr = guard_deadlock("admit-takes-guard")
    check(tr is not None and guard_deadlock("design") is None,
          "an admit that took the wallet guard would deadlock a caller holding "
          "it (L2) behind a queued writer; the design's admit takes no guard")
    if tr:
        print("       " + " -> ".join(tr))
    return ok


# ---------------------------------------------------------------------------
# Part 4: an own-key registration and the ChainLock fallback (GPT 4, Opus 9)
# ---------------------------------------------------------------------------


def keyhold_steps(st: tuple, rule: str) -> Iterator[tuple]:
    """One own-key registration after its funding: step 1
    (create_funded_asset_lock_proof) waits for an InstantSend proof; step 2
    (FromExistingAssetLock) signs and submits the identity transition with
    the lease's held key. `rule`: `rev0` (the pin: step 2 falls back to the
    ChainLock wait inside the library on an IS-proof rejection, and a
    signer `Locked` afterwards is a failed registration); `modeA` (the
    platform PR's surfaced fallback: the library returns
    ChainLockFallbackRequired, the engine parks and drops the key at once,
    and the continuation needs a new grant); `modeB` (no PR: the fallback
    stays hidden, the KeyHold timer bounds the key, and a signer `Locked`
    out of step 2 under an expired lease maps to Parked, Opus 9)."""
    phase, key, flags = st
    if key and phase not in ("done", "failed", "parked", "step2_new"):
        yield "key_until passes: KeyHold dropped", (phase, False, flags)
    if phase == "step1":
        yield "IS proof within the window", ("step2", key, flags)
        yield "step 1 times out: the engine parks", ("parked", False, flags)
    elif phase == "step2":
        if not key:
            yield "step 2 signer Locked: park", ("parked", False, flags)
            return
        yield "Platform accepts", ("done", key, flags)
        if rule == "modeA":
            yield "IS proof rejected: fallback surfaced, the engine parks", ("parked", False, flags)
        else:
            yield "IS proof rejected: hidden ChainLock wait", ("cl_hidden", key, flags)
    elif phase == "cl_hidden":
        if key:
            yield "ChainLock proof: the library re-signs with the held key", (
                "done", key, flags | {"signed_old"}
            )
        elif rule == "rev0":
            yield "ChainLock proof: signer Locked, registration Failed", (
                "failed", key, flags | {"failed_committed"}
            )
        else:
            yield "ChainLock proof: signer Locked, mapped to Parked", ("parked", key, flags)
    elif phase == "parked":
        yield "proof arrives; a new grant and a new lease", ("step2_new", True, flags)
    elif phase == "step2_new":
        yield "Platform accepts (new lease)", ("done", False, flags)


def keyhold_findings(rule: str) -> set:
    init = ("step1", True, frozenset())
    seen = {init}
    frontier = [init]
    found = set()
    while frontier:
        nxt = []
        for st in frontier:
            phase, key, flags = st
            if phase == "cl_hidden" and key:
                found.add("own key held during a hidden ChainLock wait")
            if "signed_old" in flags:
                found.add("signed under the old authority after a ChainLock fallback")
            if "failed_committed" in flags:
                found.add("registration reported Failed with its funds committed")
            for _, n in keyhold_steps(st, rule):
                if n not in seen:
                    seen.add(n)
                    nxt.append(n)
        frontier = nxt
    return found


def part4() -> bool:
    ok = True

    def check(cond: bool, msg: str) -> None:
        nonlocal ok
        print(("PASS " if cond else "FAIL ") + msg)
        ok = ok and cond

    print("Part 4: own-key registration and the ChainLock fallback (GPT 4, Opus 9)")
    r0, ra, rb = (keyhold_findings(r) for r in ("rev0", "modeA", "modeB"))
    print(f"       rev0: {sorted(r0)}")
    print(f"       Mode B residual: {sorted(rb)}")
    check(
        "own key held during a hidden ChainLock wait" in r0
        and "registration reported Failed with its funds committed" in r0 and not ra,
        "GPT 4: under rev0 the key stays usable through a hidden ChainLock wait and "
        "a later signer Locked fails a funded registration; Mode A (surfaced "
        "fallback) parks at the fallback itself",
    )
    check(
        rb == {"own key held during a hidden ChainLock wait",
               "signed under the old authority after a ChainLock fallback"},
        "Mode B: no failed funded registration; the documented residual is a key "
        "held (and usable) into a hidden ChainLock wait, bounded by key_until",
    )
    return ok


# ---------------------------------------------------------------------------
# Part 5: Mode B (no platform PR): funding status, no double pay, step 2
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class F:
    """One Mode B funding call (create_funded_asset_lock_proof) of a top-up
    or registration, the engine's view of it, and the user's later choice
    to fund again."""

    call: str = "idle"  # idle | pre | signed | tracked | sent | ok | err | gone
    sigs: int = 0  # signatures the engine's signer adapter released in the call
    marker: bool = False  # the funding step's durable write-ahead marker (GPT r2 12)
    resolved: Optional[str] = None  # a definite resolution, recorded with the marker
    outcome: Optional[str] = None  # what the call itself reported
    row: Optional[str] = None  # the library's tracked row: Built | Broadcast | Proof
    out: bool = False  # T1 may be on the network (a peer may hold it)
    onchain: bool = False
    dead: bool = False  # a conflicting self-spend of its inputs is ChainLocked
    locked: bool = False
    power: int = 1  # power losses still allowed
    t2: bool = False  # a second funding lock was built


def f_status(s: F, rule: str) -> Optional[str]:
    """dispatch_status of the funding, Mode B (review Opus r2 2). rev1 had no
    Mode B source and read "no entry" as permission; rev2 derives the status
    from the tracked row (list_tracked_locks), the marker and how the call
    ended, and is never None."""
    if rule == "rev1":
        if s.row in ("Built", "Broadcast"):
            return "WillBeSent"
        if s.row == "Proof":
            return "Sent"
        return None if s.outcome is None else s.outcome
    if s.dead:
        return "NotSent"
    if s.row == "Proof":
        return "Sent"
    if s.row in ("Built", "Broadcast"):
        return "WillBeSent"
    if s.resolved == "NotSent":
        return "NotSent"
    if not s.marker and s.call == "idle":
        return "NotSent"  # never started
    return "MaybeSent"


def f_may_fund_again(s: F, rule: str) -> bool:
    st = f_status(s, rule)
    if rule == "rev1":
        return st in (None, "NotSent", "Cancelled")
    return st == "NotSent"


def f_steps(s: F, rule: str) -> Iterator[tuple]:
    from dataclasses import replace as rp

    if not s.locked:
        yield "Lock", rp(s, locked=True)
    if s.call == "idle" and not s.t2:
        yield "start the funding call", rp(s, call="pre", marker=rule == "rev2")
    elif s.call == "pre":
        if s.locked:
            # The first signature fails: nothing was signed in this call.
            yield "signer Locked before any signature", rp(
                s, call="err", outcome="NotSent",
                resolved="NotSent" if rule == "rev2" else None,
            )
        else:
            yield "sign the asset lock", rp(s, call="signed", sigs=1)
    elif s.call == "signed":
        yield "track Built", rp(s, call="tracked", row="Built")
    elif s.call == "tracked":
        yield "broadcast", rp(s, call="sent", out=True)
        yield "transport not ready: the pin untracks and releases", rp(
            s, call="err", row=None, outcome="NotSent",
            resolved="NotSent" if rule == "rev2" else None,
        )
    elif s.call == "sent":
        yield "accepted; proof", rp(s, call="ok", row="Proof", outcome="Sent")
        yield "peer lost after dispatch", rp(s, call="err", row="Broadcast", outcome="MaybeSent")
    if s.power and s.call not in ("idle",):
        # wallet.sqlite (NORMAL) may lose the row; the FULL marker survives.
        lost_row = None if s.row in ("Built", "Broadcast") else s.row
        yield "power loss: the row is lost", rp(s, call="gone", row=lost_row, outcome=None, power=0)
    if s.row in ("Built", "Broadcast") and s.call in ("err", "gone", "ok"):
        yield "catch-up resends the row (pin semantics)", rp(s, out=True)
    if s.out and not s.onchain and not s.dead:
        yield "a peer releases T1: it confirms", rp(s, onchain=True)
    if s.onchain and s.row != "Proof":
        yield "DP1-05 finds the lock on chain (RecoveredFromChain)", rp(s, row="Proof")
    if (
        not s.onchain and not s.dead and s.call in ("err", "gone", "ok")
        and (s.marker or s.row in ("Built", "Broadcast"))
    ):
        yield "Repair: a self-spend of every coin is ChainLocked", rp(s, dead=True)
    if not s.t2 and s.call in ("err", "gone") and f_may_fund_again(s, rule):
        yield "the user funds again (retry, discard and register again)", rp(s, t2=True)


def f_findings(rule: str) -> dict:
    init = F()
    seen = {init: []}
    frontier = [init]
    found = {}
    while frontier:
        nxt = []
        for st in frontier:
            if st.t2 and not st.dead and (st.out or st.row in ("Built", "Broadcast")):
                found.setdefault("two funding locks can both confirm (double pay)", seen[st])
            if f_status(st, rule) is None and st.call in ("err", "gone"):
                found.setdefault("dispatch_status has no answer for a started funding", seen[st])
            for lab, n in f_steps(st, rule):
                if n not in seen:
                    seen[n] = seen[st] + [lab]
                    nxt.append(n)
        frontier = nxt
    return found


def f_can_fund_again_after(rule: str) -> set:
    """Liveness of the rev2 gate: the last step before each reachable
    "the user funds again", so a definitely-unsent funding never leaves the
    user stuck."""
    init = F()
    seen = {init: []}
    frontier = [init]
    after = set()
    while frontier:
        nxt = []
        for st in frontier:
            for lab, n in f_steps(st, rule):
                if lab.startswith("the user funds again"):
                    after.add(seen[st][-1] if seen[st] else "")
                if n not in seen:
                    seen[n] = seen[st] + [lab]
                    nxt.append(n)
        frontier = nxt
    return after


def step2_findings(rule: str) -> dict:
    """Mode B registration step 2 (review GPT r2 10, 11): the call signs S1
    and submits it; a lagging node answers "ChainLock height too low"; the
    library backs off, signs S2 and submits again. Lock may land anywhere.
    rev1 called a final signer Locked Cancelled (step 2 as a sole-artifact
    call) and showed "Funds locked ... Lock stops it here; you'll finish
    after you unlock" during the call. rev2 calls a signer Locked Cancelled
    only when the call released no signature, and shows DEC-67's line while
    a library call runs."""
    init = ("pre", 0, False, False, False, None, None)
    # (phase, sigs, s1_out, executed, locked, outcome, copy)
    seen = {init: []}
    frontier = [init]
    found = {}

    def classify(sigs):
        if rule == "rev1":
            return "Cancelled"
        return "Cancelled" if sigs == 0 else "MaybeSent"

    while frontier:
        nxt = []
        for st in frontier:
            phase, sigs, s1_out, executed, locked, outcome, copy = st
            if outcome == "Cancelled" and s1_out:
                found.setdefault("step 2 reported Cancelled after S1 was handed off", seen[st])
            if copy == "stop" and executed == "after_lock":
                found.setdefault("the Lock copy promised a stop that did not happen", seen[st])
            succ = []
            if not locked:
                running = phase not in ("ok", "err")
                if rule == "rev1":
                    shown = "stop"  # funded: "Lock stops it here; you'll finish after you unlock"
                else:
                    shown = "may_still_send" if running else "stop"
                succ.append(("Lock", (phase, sigs, s1_out, executed, True, outcome, shown)))
            if phase == "pre":
                if locked:
                    succ.append(("sign S1: signer Locked", ("err", sigs, s1_out, executed, locked, classify(sigs), copy)))
                else:
                    succ.append(("sign S1", ("s1", 1, s1_out, executed, locked, outcome, copy)))
            elif phase == "s1":
                succ.append(("submit S1", ("submitted", sigs, True, executed, locked, outcome, copy)))
            elif phase == "submitted":
                succ.append(("Platform executes S1", ("ok", sigs, s1_out, "after_lock" if locked else True, locked, "Sent", copy)))
                succ.append(("a lagging node: ChainLock height too low; back off", ("backoff", sigs, s1_out, executed, locked, outcome, copy)))
            elif phase == "backoff":
                if locked:
                    succ.append(("sign S2: signer Locked", ("err", sigs, s1_out, executed, locked, classify(sigs), copy)))
                else:
                    succ.append(("sign and submit S2; accepted", ("ok", 2, s1_out, "after_lock" if locked else True, locked, "Sent", copy)))
            if s1_out and not executed:
                succ.append(("S1 executes late (another node had it)", (phase, sigs, s1_out, "after_lock" if locked else True, locked, outcome, copy)))
            for lab, n in succ:
                if n not in seen:
                    seen[n] = seen[st] + [lab]
                    nxt.append(n)
        frontier = nxt
    return found


def restart_retry(rule: str) -> Optional[tuple]:
    """Review Opus r2 1a, DW-E0-08 r2 N-1. A row-less state transition (a
    withdrawal) ends broadcast_unknown, the app restarts, and the host asks
    dispatch_status before offering a retry. A withdrawal is not resumable,
    so after the restart the engine has no record of it and answers None
    (no entry) whatever the identity's nonce now says (H16). rev1's contract
    read None as "retry allowed"; rev2 reads None as unknown for a transition
    or a TxDraft (only an asset lock's None means never registered). Returns
    the first (nonce_state, original_executes) at which a retry is offered
    while the original can still execute."""
    for nonce in ("unconsumed", "consumed_by_this", "consumed_by_other"):
        for original_executes in (False, True):
            if nonce == "consumed_by_other" and original_executes:
                continue  # its nonce is gone: it cannot execute
            if nonce == "consumed_by_this" and not original_executes:
                continue
            status = None  # no record after the restart
            if rule == "rev1":
                retry = status is None or status == "NotSent"
            else:
                retry = status == "NotSent"  # None is unknown for a transition
            can_execute = nonce == "unconsumed"
            if retry and can_execute:
                return (nonce, original_executes)
    return None


def rowless_tombstone(rule: str) -> set:
    """DW-E0-08 r2 N-1, manager correction to rev2 ruling 4. A row-less state
    transition h settles definitely unsent (every attempt definitely
    rejected, none possibly out). The host then asks dispatch_status(h),
    possibly after a restart, and reads the answer as a transition's: only
    Some(NotSent) allows a retry, and None means unknown. Optionally the
    identical bytes are admitted again under a live lease (F10) and sent;
    the process may stop at each point. Rules:
    `forget` (rev2 as first committed): the id is forgotten at settlement;
    `no-override`: a per-process tombstone that a later First of the same
    bytes does not replace;
    `rev2`: a per-process tombstone that the J step admitting a later First of
    the same bytes replaces with its attempt, before any transport.
    Returns the findings."""
    found = set()
    for readmit in (False, True):
        for stop in ("none", "before_send", "after_send"):
            if not readmit and stop != "none":
                continue
            for restart in (False, True):
                status = None if rule == "forget" else "NotSent"
                if status is None:
                    found.add("a definitely-unsent transition reads unknown in its process")
                out = False
                if readmit:
                    if rule == "rev2":
                        status = "MaybeSent"
                    if stop != "before_send":
                        out = True
                        if rule == "rev2" and stop == "none":
                            status = "Sent"
                if restart:
                    status = None  # the per-process set is gone
                retry = status == "NotSent"
                if retry and out:
                    found.add("dispatch_status says not sent while the transition may be out")
    return found


def asset_lock_absent(rule: str) -> set:
    """Manager correction to rev2 ruling 4: an asset lock's None (no entry)
    passes the engine's funding gate (discard_registration), the only kind
    for which None allows anything. It is safe because
    register precedes tracking and every transport (I1), entries are never
    deleted while their wallet tracks a row (6.5), and a tracked row with no
    entry counts as evidence. Each scenario is (entry, row, possibly out)
    as the host's dispatch_status call finds it. Rules:
    `rev2`: the row is consulted first, then the entry, and None is answered
    only with neither;
    `entry-only`: no entry is None whatever the rows say;
    `skip-consumed`: a Consumed row is ignored, as the seeding ignores it.
    Returns the findings."""
    scenarios = [
        ("never registered", None, None, False),
        ("registered, stopped before tracking", "unsent_dead", None, False),
        ("registered and tracked, Lock won", "revoked", "built", False),
        ("a live flow before its commit", "unsent_live", "built", False),
        ("committing", "committing", "built", False),
        ("committed and sent", "dispatching", "built", True),
        ("committed, sent and seen", "dispatching", "seen", True),
        ("journal deleted after the lock was consumed", None, "consumed", True),
        ("journal deleted, the row resent", "prefence", "built", True),
        ("journal rolled back below a registered and sent row", None, "built", True),
    ]
    found = set()
    for name, entry, row, out in scenarios:
        seen_row = row in ("seen", "consumed") and not (rule == "skip-consumed" and row == "consumed")
        has_row = row is not None and not (rule == "skip-consumed" and row == "consumed")
        if rule != "entry-only" and seen_row:
            st = "Sent"
        elif entry in ("committing", "dispatching", "prefence", "ambiguous"):
            st = "WillBeSent" if has_row else "MaybeSent"
        elif entry == "unsent_live":
            st = "MaybeSent"
        elif entry in ("unsent_dead", "revoked"):
            st = "NotSent"
        elif rule != "entry-only" and has_row:
            st = "MaybeSent"
        else:
            st = None
        retry = st is None or st == "NotSent"  # the asset-lock exception
        if retry and out:
            found.add(f"a second funding while the lock may be out: {name}")
        if retry and entry == "unsent_live":
            found.add(f"a retry offered while the flow can still commit: {name}")
    return found


def part5() -> bool:
    ok = True

    def check(cond: bool, msg: str) -> None:
        nonlocal ok
        print(("PASS " if cond else "FAIL ") + msg)
        ok = ok and cond

    print("Part 5: Mode B (no platform PR)")
    f1, f2 = f_findings("rev1"), f_findings("rev2")
    for v, tr in sorted(f1.items()):
        print(f"       rev1: {v}: {' -> '.join(tr)}")
    check(
        bool(f1) and not f2,
        "Opus r2 2, GPT r2 12: Mode B funding: rev1 has no status source and lets a "
        "second funding lock be built after a peer-held lock lost its row; rev2 "
        "derives the status from the tracked row, the marker and the call's end, "
        "so no double pay is reachable (with power loss and a withholding peer)",
    )
    live = f_can_fund_again_after("rev2")
    check(
        {"signer Locked before any signature",
         "transport not ready: the pin untracks and releases",
         "Repair: a self-spend of every coin is ChainLocked"} <= live,
        f"rev2 never strands a definitely-unsent funding: funding again is allowed "
        f"after {sorted(live)}",
    )
    s1, s2 = step2_findings("rev1"), step2_findings("rev2")
    for v, tr in sorted(s1.items()):
        print(f"       rev1: {v}: {' -> '.join(tr)}")
    check(
        "step 2 reported Cancelled after S1 was handed off" in s1
        and "the Lock copy promised a stop that did not happen" in s1 and not s2,
        "GPT r2 10, 11: Mode B step 2 with a ChainLock-height retry: rev1 reports "
        "Cancelled after S1 went out and its funded copy promises a stop; rev2 "
        "classifies by released signatures and shows DEC-67's line while a call runs",
    )
    r1, r2 = restart_retry("rev1"), restart_retry("rev2")
    check(
        r1 is not None and r2 is None,
        f"Opus r2 1a, E0-08 N-1: a restart after broadcast_unknown on a withdrawal: rev1 "
        f"offers a retry on None while the original can execute {r1}; rev2 reads None as "
        f"unknown for a transition",
    )
    tombs = {r: rowless_tombstone(r) for r in ("forget", "no-override", "rev2")}
    for r in ("forget", "no-override"):
        print(f"       {r}: {sorted(tombs[r])}")
    check(
        "a definitely-unsent transition reads unknown in its process" in tombs["forget"]
        and "dispatch_status says not sent while the transition may be out" in tombs["no-override"]
        and not tombs["rev2"],
        "E0-08 N-1: a settled transition keeps a per-process tombstone that answers NotSent, "
        "replaced under J by a later First of the same bytes; forgetting it, or not "
        "replacing it, fails; after a restart it reads None, which is unknown",
    )
    absent = {r: asset_lock_absent(r) for r in ("entry-only", "skip-consumed", "rev2")}
    for r in ("entry-only", "skip-consumed"):
        print(f"       {r}: {sorted(absent[r])}")
    check(
        any("rolled back" in f for f in absent["entry-only"])
        and any("consumed" in f for f in absent["skip-consumed"])
        and not absent["rev2"],
        "asset-lock exception: None allows a second funding only with neither an entry nor "
        "a tracked row of any status; reading the entry alone, or skipping Consumed rows, "
        "funds twice",
    )
    return ok


def main() -> int:
    ok = part1()
    ok = part2() and ok
    ok = part3() and ok
    ok = part4() and ok
    ok = part5() and ok
    print("ok" if ok else "FAILED")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
