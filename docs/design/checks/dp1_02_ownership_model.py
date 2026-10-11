#!/usr/bin/env python3
"""Spec check for DP1-02's funding ownership model (docs/design/DP1-02-ownership-model.md),
rev2 with the manager's rulings of section 13.1.

Run: python3 -I -B docs/design/checks/dp1_02_ownership_model.py   (exit 0 = pass)

It explores, exhaustively and breadth first, the registration funding of a
wallet with a few rows under rev2's decision functions (section 5: standing,
owner, may_take, reconcile, committed, pending, and section 6's index_owner
and next_index). It is a model of the design's decisions, not a test of the
code.

Part 1 runs rev2 in every scenario and requires, with no expected violation:
safety S1-S6 (section 11.1) and the invariants I3, I4, I6, I10, I11 and I12;
liveness: from every reachable state the system alone (the call, the run,
reconcile, the tap, Finish, Repair, proofs, reconstruction, discovery; no
user decision) reaches "nothing outstanding" (no call, no outstanding step,
every live lock consumed, no row holding unconsumed or voided value, every
Cancelled row closed with its code), and every state that cannot is one of section
8.1's (an Unknown step on an unflagged marker); and a few reachability
witnesses for rev2's new paths. Part 2 runs each rule the design rejects and
requires the check to fail. A scenario that outgrows MAX_STATES stops the
run with "BOUND HIT" and exit 2, never a silent pass.

What is modelled:

- Rows (appdb): user rows A (index 0) and B (index 1), C made by `start`,
  restored rows with new ids (F6, F11), Finish's recovery rows. A row has an
  index, an outpoint, an identity, its closure code (funded_elsewhere,
  funding_cancelled) and its `calling` flag. By F10 a funding key is the
  index, so there is no separate key; `key` is the ground-truth intent
  (lineage) that only the checks read: a restored copy keeps its original's.
- Steps (PA-1's view, OD-06): marker status in memory and on disk (the
  standing statuses are one class `M`; `N` is NotSent), the index, the
  signed txids in memory and the durable record, the tombstones (`dead`, PA-3:
  they survive reopen and `erase_wallet`, Q9) and `durable_record` (OD-02,
  set when the marker is written under PA-2). The view lists standing steps
  and resolved steps that kept tombstones; a resolved step without
  tombstones drops out (H12: the same step may retry).
- Claims (3.1): Exact, Unknown, None; C4 (a flagged step with tombstones
  and no signed lock reads None whatever P2b re-reads; P2b re-reads a
  resolved step MaybeSent while an unattributed lock stands, Q13) and C5 (a
  flagged marker with no record and no running call reads None and stands).
- Locks: each step's own lock T<draft><n> (T1 = TA1, T3 = TB1) and the
  foreign T2 (rebuilt from the chain at index 0, so untrusted and
  unattributed; Q6 may give it the other amount): stored (may be on the
  wire), tracked, trusted index, proved, consumed (truth), tracked
  `Consumed` by this library, identity discovered, dead (can never land).
- Events: enter_funding (sets `calling`), P2b marks (the pre-marker window
  lies between), sign (record durable, or the write fails: with PA-2 the
  process stops before any broadcast), store and the tap (write lands or
  fails), the returns (Ok or a timeout; Refused before the broadcast;
  NotSent after a rejection; an error before signing or before the store),
  P2b's NotSent write (lands, or fails and lives in memory), the post-call
  writes (still calling, then the flag clears), rejection, proof,
  reconstruction at index 0, discovery, a signed lock consumed elsewhere
  while no row holds it, the run (reconcile: Attach and Close; the identity;
  the own lock; the index restore and moves), Finish (reconcile first, then
  allocation; the held path is a pending report), Repair (self-spend over
  recorded inputs: NotSent only for an Exact step whose lock hasn't landed,
  NoKnownLock otherwise), restart, discard, start, the foreign lock
  appearing, wallet removal and re-add (copies of the rows, an older backup
  taken before the funding, or no rows), and a backup whose B' holds A's
  lock while A' is open at A's index (section 5.2c, Q8).

The checks judge with rev2's functions run without the rule under test (the
reference ledger) or with the ground truth, never with the rule under test.

Out of scope: Unreadable rows and `Parked` (DEC-173), identities no row and
no lock names (index_owner's identity branch), the dispatch/fence internals
(E0-04's check), the claim/CAS mechanics (kept), timing, display fields and
waits (`funds_committed`, `RegistrationWait`, Q7's entry), the backup re-arm
(MN-2), discovery outside its gap (Q12's accepted residual), legacy steps
without recorded inputs, and identity indexes (Attach's index equals the
row's here).
"""

from __future__ import annotations

import sys
import time
from collections import deque
from typing import Iterator, NamedTuple, Optional

AMT = 1  # the fixed registration amount; Q6's T2 may carry the other one (2)
MAX_STATES = 600_000  # per exploration: about 2 GB at the measured ~3 KB a state
RUNNING = ("marked", "signed", "stored")  # P2b's "running call"


class BoundHit(Exception):
    pass


# ---------------------------------------------------------------------------
# State
# ---------------------------------------------------------------------------


class Row(NamedTuple):
    rid: str
    seq: int  # age: lower is older
    kind: str = "user"  # user | rec (Finish's recovery row: no step, never funds)
    index: Optional[int] = None
    key: str = ""  # truth: the intent; a restored copy keeps its original's
    out: Optional[str] = None  # the recorded outpoint (possibly voided)
    ident: bool = False
    code: Optional[str] = None  # closure: "fe" funded_elsewhere, "fc" funding_cancelled
    late: bool = False  # truth: made by start while a lost row's step was out


class Lock(NamedTuple):
    name: str
    signer: Optional[str]  # truth: the draft whose call built it; None = foreign
    lkey: str  # truth: the intent it was built for
    duffs: int = AMT
    stored: bool = False  # in the library's store: may be on the wire
    tracked: bool = False
    kidx: Optional[int] = None  # known (trusted) index
    proved: bool = False
    consumed: bool = False  # truth: an identity was made from it
    lib_consumed: bool = False  # the library tracks it Consumed
    discovered: bool = False  # discovery reports identity_for(lock)
    dead: bool = False  # can never land


class Step(NamedTuple):
    draft: str
    idx: int  # the marker's FundingKey (F10: the index)
    lin: str  # truth: the marking row's intent
    duffs: int = AMT
    gen: int = 1
    st: str = "M"  # P2b status in memory: M standing, N NotSent
    dst: str = "M"  # on disk
    signed: frozenset = frozenset()  # the active attempt's txids, in memory
    dsigned: frozenset = frozenset()  # the durable signed record
    deadv: frozenset = frozenset()  # tombstones (PA-3, OD-06)
    durable_record: bool = False  # OD-02: the marker was written under PA-2


class St(NamedTuple):
    rows: tuple = ()
    locks: tuple = ()
    steps: tuple = ()
    calling: tuple = ()  # (rid, phase): entered | marked | signed | stored | post
    marks: int = 0  # funding calls the scenario allows
    restarts: int = 0
    restores: int = 0
    foreign: int = 0
    elsewhere: int = 0
    starts: int = 0
    rejects: int = 0
    funders: frozenset = frozenset({"A", "A'", "C"})  # rows whose run may fund
    restarted: bool = False
    nseq: int = 10


class Cfg(NamedTuple):
    pa2: bool = True  # OD-01/02: the durable-record gate; markers carry durable_record
    m: frozenset = frozenset()  # the rejected rule under test
    t2_amt: int = AMT


def rp(x, **kw):
    return x._replace(**kw)


def ref(cfg: Cfg) -> Cfg:
    """The judge: rev2 without the rule under test."""
    return rp(cfg, m=frozenset())


def _put(items: tuple, item, key) -> tuple:
    return tuple(sorted([x for x in items if key(x) != key(item)] + [item], key=key))


def set_row(s: St, r: Row) -> St:
    return rp(s, rows=_put(s.rows, r, lambda x: x.rid))


def set_lock(s: St, lk: Lock) -> St:
    return rp(s, locks=_put(s.locks, lk, lambda x: x.name))


def set_step(s: St, st: Step) -> St:
    return rp(s, steps=_put(s.steps, st, lambda x: x.draft))


def set_call(s: St, rid: str, ph: Optional[str]) -> St:
    rest = [c for c in s.calling if c[0] != rid]
    return rp(s, calling=tuple(sorted(rest + ([(rid, ph)] if ph else []))))


def rows_by_age(s: St) -> list:
    return sorted(s.rows, key=lambda r: (r.seq, r.rid))


def lock_of(s: St, name: str) -> Optional[Lock]:
    return next((x for x in s.locks if x.name == name), None)


def step_of(s: St, d: str) -> Optional[Step]:
    return next((x for x in s.steps if x.draft == d), None)


def row_of_id(s: St, rid: str) -> Optional[Row]:
    return next((x for x in s.rows if x.rid == rid), None)


def phase_of(s: St, rid: str) -> Optional[str]:
    return next((ph for d, ph in s.calling if d == rid), None)


# ---------------------------------------------------------------------------
# rev2's decision functions (sections 3 and 5), implemented once
# ---------------------------------------------------------------------------


class Ledger:
    """One read of rows, tracked locks, identities, steps and callers."""

    def __init__(self, s: St, cfg: Cfg):
        self.s, self.cfg, self.m = s, cfg, cfg.m
        self.rows = rows_by_age(s)
        self.row_by = {r.rid: r for r in s.rows}
        self.locks = {x.name: x for x in s.locks}
        self.calling = {d for d, _ in s.calling}
        self.running = {d for d, ph in s.calling if ph in RUNNING}
        steps = [S for S in s.steps if S.st == "M" or S.deadv]  # PA-1's view (OD-06)
        if "by-row" in self.m:  # R8-02: evidence read only for rows that exist
            steps = [S for S in steps if S.draft in self.row_by]
        self.steps = steps
        self.step_by = {S.draft: S for S in steps}
        self.drafts = set(self.step_by)
        self.dead = frozenset().union(*[S.deadv for S in steps])
        self.holder_of = {r.out: r for r in s.rows if r.out is not None and r.out not in self.dead}
        self.unattributed = any(
            x.signer is None and x.tracked and not self.consumed(x.name) for x in s.locks
        )
        self._c: dict = {}

    def _memo(self, key, fn):
        if key not in self._c:
            self._c[key] = fn()
        return self._c[key]

    # 3.2 -------------------------------------------------------------------
    def consumed(self, name: str) -> bool:
        lk = self.locks.get(name)
        if lk is not None and lk.consumed and (lk.lib_consumed or lk.discovered):
            return True
        return any(r.out == name and r.ident for r in self.s.rows)

    def live(self, r: Row) -> Optional[str]:
        return r.out if r.out is not None and r.out not in self.dead else None

    def open(self, r: Row) -> bool:
        return r.kind == "user" and r.index is not None and self.live(r) is None and not r.ident

    # 3.1 -------------------------------------------------------------------
    def claim(self, S: Step) -> Optional[str]:
        if S.deadv and not S.signed and S.durable_record and "c4-unknown" not in self.m:
            return None  # C4, derived by P2b (Q13)
        if S.st != "M":
            if S.deadv and not S.signed and self.unattributed:
                return "Unknown"  # P2b re-reads a resolved step MaybeSent (C4)
            return None
        if S.signed:
            return "Exact"
        if S.durable_record and S.draft not in self.running:
            return None  # OD-02: a flagged marker with no record and no running call (C5)
        if "unknown-unclaimed" in self.m:  # R8-01
            return None
        return "Unknown"

    def signer(self, S: Step) -> Optional[Row]:
        def f():
            r = self.row_by.get(S.draft)
            if r is not None or {"no-heir", "by-row"} & self.m:
                return r
            for c in self.rows:
                if c.index == S.idx and self.open(c) and c.rid not in self.drafts:
                    return c
            return None
        return self._memo(("signer", S.draft), f)

    def own(self, S: Step) -> frozenset:
        if S.signed:
            return S.signed
        r = self.signer(S)
        lv = self.live(r) if r is not None else None
        return frozenset([lv]) if lv is not None else frozenset()

    def holder(self, S: Step) -> Optional[Row]:
        return next((self.holder_of[t] for t in sorted(self.own(S)) if t in self.holder_of), None)

    def row_of(self, S: Step) -> Optional[Row]:
        h = self.holder(S)
        return h if h is not None else self.signer(S)

    def settled(self, S: Step) -> bool:
        own = self.own(S)
        if not own:
            return False
        ro = self.row_of(S)
        return all(
            self.consumed(t) and (t in self.holder_of or ro is None or not self.open(ro))
            for t in own
        )

    def outstanding(self, S: Step) -> bool:
        return self._memo(("out", S.draft),
                          lambda: self.claim(S) is not None and not self.settled(S))

    def cancelled(self, S: Step) -> bool:
        return self.claim(S) is None

    def steps_of(self, r: Row) -> list:
        return [S for S in self.steps
                if self.outstanding(S) and (ro := self.row_of(S)) is not None and ro.rid == r.rid]

    # 3.3 -------------------------------------------------------------------
    def standing(self, r: Row) -> str:
        return self._memo(("standing", r.rid), lambda: self._standing(r))

    def _standing(self, r: Row) -> str:
        lv = self.live(r)
        if lv is not None and not self.consumed(lv):
            return "Funded"
        mine = self.steps_of(r)
        if mine and self.open(r):
            return "Awaiting"
        if mine:
            return "Closed"
        if r.ident or (lv is not None and self.consumed(lv)):
            return "Settled"
        if r.rid not in self.calling:
            if r.code == "fe" or any(
                (sg := self.signer(S)) is not None and sg.rid == r.rid
                and (h := self.holder(S)) is not None and h.rid != r.rid
                for S in self.steps
            ):
                return "Cancelled-fe"
            if r.code == "fc" or (r.out is not None and r.out in self.dead) or any(
                (sg := self.signer(S)) is not None and sg.rid == r.rid and self.cancelled(S)
                for S in self.steps
            ):
                return "Cancelled-fc"
        return "Unfunded"

    def committed(self, r: Row) -> bool:
        return self.standing(r) in ("Funded", "Awaiting", "Closed") or r.rid in self.calling

    # 3.4 -------------------------------------------------------------------
    def owner(self, lk: Lock) -> Optional[tuple]:
        if not lk.tracked or lk.name in self.holder_of or self.consumed(lk.name):
            return None
        for S in self.steps:
            if lk.name in S.deadv:
                return ("Dead", S.draft)
            if lk.name in S.signed:
                return ("Step", S.draft) if self.outstanding(S) else ("Dead", S.draft)
        for S in self.steps:
            if (
                self.claim(S) == "Unknown" and self.outstanding(S) and not self.own(S)
                and lk.kidx in (None, S.idx) and lk.duffs == S.duffs
            ):
                return ("Contested", S.draft)
        return ("Free",)

    def kind(self, lk: Lock) -> Optional[str]:
        o = self.owner(lk)
        if o is None:
            return None
        if o[0] == "Contested" and "take-contested" in self.m:
            return "Free"
        return o[0]

    # 5.1 -------------------------------------------------------------------
    def may_take(self, taker: tuple, lk: Lock) -> bool:
        k = self.kind(lk)
        if k == "Step":
            ro = self.row_of(self.step_by[self.owner(lk)[1]])
            if taker[0] == "Row":
                return ro is not None and ro.rid == taker[1].rid and self.open(taker[1])
            if "rule4" in self.m:  # Evidence::orphaned: pending for ever
                return False
            return ro is None or not self.open(ro)
        if k == "Free":
            if taker[0] == "Row":
                r = taker[1]
                if "calling-free" not in self.m and r.rid in self.calling:
                    return False  # OD-05 / I10
                if "r7-01" in self.m:
                    return self.open(r)
                return self.open(r) and self.standing(r) == "Unfunded"
            return True
        return False  # Parked, Contested, Dead

    # 5.2 -------------------------------------------------------------------
    def reconcile(self, r: Row) -> list:
        """The row's Reconcile items: ("attach", t), ("close", clear, code), and
        the rejected rule's ("move", holder, t)."""
        if r.rid in self.calling:
            return []
        out = []
        if "no-attach" not in self.m:
            for S in self.steps:
                for t in sorted(S.signed):
                    lk = self.locks.get(t)
                    ro = self.row_of(S)
                    if (
                        lk is not None and self.consumed(t) and t not in self.holder_of
                        and ro is not None and ro.rid == r.rid and self.open(r) and lk.discovered
                    ):
                        out.append(("attach", t))
        if "signer-contrary" in self.m and self.open(r):
            for S in self.steps:
                sg, h = self.signer(S), self.holder(S)
                if sg is not None and sg.rid == r.rid and h is not None and h.rid != r.rid:
                    out.append(("move", h.rid, h.out))
        if "no-closure" not in self.m:
            clear = r.out is not None and r.out in self.dead
            if "clear-inexact" in self.m and r.out is not None:
                clear = clear or any(
                    S.deadv for S in self.steps
                    if (sg := self.signer(S)) is not None and sg.rid == r.rid
                )
            st = self.standing(r)
            code = {"Cancelled-fe": "fe", "Cancelled-fc": "fc"}.get(st)
            if code == r.code:
                code = None
            if clear or code:
                out.append(("close", clear, code))
        return out

    # 5.3 -------------------------------------------------------------------
    def pending(self, held: bool) -> int:
        n = 0
        if not (held and "held-no-step" in self.m):  # R8-03
            for S in self.steps:
                if self.outstanding(S) and not any(t in self.holder_of for t in self.own(S)):
                    n += 1
        for lk in self.locks.values():
            o = self.owner(lk)
            if o is not None and o[0] in ("Contested", "Free"):
                n += 1
        return n

    # paths -----------------------------------------------------------------
    def own_lock(self, r: Row) -> Optional[Lock]:
        for S in self.steps_of(r):
            for t in sorted(S.signed):
                lk = self.locks.get(t)
                if lk and self.owner(lk) == ("Step", S.draft) and self.may_take(("Row", r), lk):
                    return lk
        return None

    def index_owner(self, r: Row) -> tuple:
        for lk in self.locks.values():
            if lk.kidx is not None and lk.kidx == r.index and self.kind(lk) == "Free":
                if self.may_take(("Row", r), lk):
                    return ("Lock", lk)
        for S in self.steps:  # rev2: another draft's step at the index
            if S.draft != r.rid and S.idx == r.index:
                return ("Unknown",) if self.outstanding(S) else ("Taken",)
        for c in self.rows:
            if c.rid != r.rid and c.kind == "user" and c.index == r.index and (
                c.out is not None or c.ident or self.committed(c) or c.seq < r.seq
            ):
                return ("Taken",)
        for lk in self.locks.values():
            o = self.owner(lk)
            if o is not None and o[0] in ("Free", "Contested") and lk.kidx is None:
                return ("Unknown",)
        return ("Free",)

    def next_index(self) -> int:
        used = {r.index for r in self.rows if r.kind == "user"}
        if "next-index-outstanding" in self.m:  # rev1: only outstanding steps
            used |= {S.idx for S in self.steps if self.outstanding(S)}
        else:
            used |= {S.idx for S in self.steps}
        i = 0
        while i in used:
            i += 1
        return i

    def start_allowed(self) -> bool:
        if any(self.committed(r) and not r.ident for r in self.rows):
            return False
        return not any(self.outstanding(S) and self.row_of(S) is None for S in self.steps)

    def discard_allowed(self, r: Row) -> bool:
        if "settled-exemption" in self.m and r.ident and r.rid not in self.calling:
            return True  # the old settled(): a row with an identity may go
        return not self.committed(r)


_LEDGERS: dict = {}


def ledger(s: St, cfg: Cfg) -> Ledger:
    """A read of (s, cfg); states are immutable, so recent reads are reused."""
    led = _LEDGERS.get((s, cfg))
    if led is None:
        if len(_LEDGERS) > 4096:
            _LEDGERS.clear()
        led = _LEDGERS[(s, cfg)] = Ledger(s, cfg)
    return led


# ---------------------------------------------------------------------------
# Ground truth, read only by the checks
# ---------------------------------------------------------------------------


def true_out(s: St, S: Step) -> bool:
    """The step's money may be out: its call runs, or its lock may land or
    has landed and no identity was made from it."""
    if any(d == S.draft and ph in RUNNING for d, ph in s.calling):
        return True
    return any(x.signer == S.draft and x.stored and not x.dead and not x.consumed for x in s.locks)


def lost_out(s: St) -> list:
    ids = {r.rid for r in s.rows}
    return [S for S in s.steps if S.draft not in ids and true_out(s, S)]


# ---------------------------------------------------------------------------
# Events
# ---------------------------------------------------------------------------


def put_out(s: St, cfg: Cfg, taker, lk: Lock, label: str, ident: bool = False) -> tuple:
    """Every outpoint write (the tap, the run, the post-call path, Finish,
    Attach): a row, or a new recovery row, takes lk. The checks read the
    reference ledger before the write."""
    rl = ledger(s, ref(cfg))
    tv = []
    o = rl.owner(lk)
    if o is not None and o[0] == "Contested":
        tv.append("S4: a Contested lock is held")
    h = rl.holder_of.get(lk.name)
    if h is not None and (taker == "rec" or h.rid != taker.rid):
        tv.append("I11: value moves between rows")
    if taker == "rec":
        S = rl.step_by.get(lk.signer) if lk.signer else None
        ro = rl.row_of(S) if S is not None else None
        if S is not None and lk.name in S.signed and ro is not None and rl.open(ro):
            tv.append("I3: unspent signed value goes to Recovery while its row_of(S) is open")
        n = set_row(s, Row(rid=f"R{lk.name}", seq=1000, kind="rec", key="rec", out=lk.name))
        return (f"{label}: recovery row takes {lk.name}", n, True, tv)
    if lk.signer is not None and lk.lkey != taker.key:
        tv.append("S2: a row is given a lock signed by another intent's step")
    mine = rl.steps_of(taker)
    if mine and not any(lk.name in S.signed for S in mine):
        tv.append("I4: a row with an outstanding step takes a lock not its own")
    if taker.rid in rl.calling and lk.signer != taker.rid:
        tv.append("I10: a calling row takes a lock not its own")
    # Attach (ident) also clears the row's failure (5.2a).
    n = set_row(s, rp(taker, out=lk.name, ident=taker.ident or ident,
                      code=None if ident else taker.code))
    return (f"{label}: {taker.rid} takes {lk.name}" + (" with its identity" if ident else ""),
            n, True, tv)


def apply_reconcile(s: St, cfg: Cfg, r: Row, item: tuple, label: str) -> tuple:
    if item[0] == "attach":
        lk = lock_of(s, item[1])
        return put_out(s, cfg, r, lk, f"{label}: Attach", ident=True)
    if item[0] == "move":
        h = row_of_id(s, item[1])
        lk = lock_of(s, item[2])
        lab, n, sysf, tv = put_out(s, cfg, r, lk, f"{label}: move")
        return (lab + f" from {h.rid}", set_row(n, rp(h, out=None)), sysf, tv)
    _, clear, code = item
    tv = []
    n = r
    if clear:
        lk = lock_of(s, r.out)
        if lk is None or not lk.dead:
            tv.append("I11: a closure clears an outpoint that can still land")
        n = rp(n, out=None)
    if code:
        n = rp(n, code=code)
    what = ", ".join(x for x in ("clear" if clear else "", f"code {code}" if code else "") if x)
    return (f"{label}: Close {r.rid} ({what})", set_row(s, n), True, tv)


def notsent(s: St, d: str, lands: bool) -> St:
    S = step_of(s, d)
    if lands:  # F3; PA-3 keeps the txids as tombstones
        S = rp(S, st="N", dst="N", signed=frozenset(), dsigned=frozenset(),
               deadv=S.deadv | S.signed | S.dsigned)
    else:  # in memory only
        S = rp(S, st="N", signed=frozenset(), deadv=S.deadv | S.signed)
    return set_step(s, S)


def end_with_notsent(s: St, d: str, label: str) -> Iterator[tuple]:
    s = set_call(s, d, "post")
    yield (label + "; NotSent lands", notsent(s, d, True), True, [])
    yield (label + "; NotSent write fails", notsent(s, d, False), True, [])


def restart(s: St) -> St:
    running = {d for d, ph in s.calling if ph in RUNNING}
    locks = tuple(x for x in s.locks if x.stored or x.signer not in running)
    steps = tuple(rp(S, st=S.dst, signed=S.dsigned, deadv=S.deadv - S.dsigned) for S in s.steps)
    return rp(s, calling=(), locks=locks, steps=steps, restarted=True)


def tap(s: St, cfg: Cfg, lk: Lock, label: str) -> Iterator[tuple]:
    """note_lock_tracked: candidates at the known index with no live outpoint,
    filtered by may_take; the in-call row first, else the oldest."""
    led = ledger(s, cfg)
    cands = [
        r for r in led.rows
        if r.kind == "user" and lk.kidx is not None and r.index == lk.kidx
        and led.live(r) is None and led.may_take(("Row", r), lk)
    ]
    cands.sort(key=lambda r: (0 if r.rid == lk.signer and r.rid in led.calling else 1, r.seq))
    if cands:
        yield put_out(s, cfg, cands[0], lk, label + "; tap write lands")
        yield (label + "; tap write fails", s, True, [])
    else:
        yield (label + "; tap finds no row", s, True, [])


def mark(s: St, cfg: Cfg, r: Row) -> tuple:
    """P2b marks the call: the step's durable marker (F1)."""
    tv = []
    for S in lost_out(s):
        if S.draft != r.rid and (S.idx == r.index or S.lin == r.key or r.late):
            tv.append("S1: a funding call starts while a lost row's step is out")
    if any(x.lkey == r.key and x.stored and not x.dead for x in s.locks):
        tv.append("I6: an intent funds again while its value exists")
    rl = ledger(s, ref(cfg))
    if any(S.draft != r.rid and S.idx == r.index for S in rl.steps):
        tv.append("I12: a row funds at another draft's step index")
    old = step_of(s, r.rid)
    if old is not None and old.deadv:
        tv.append("I12: a cancelled step is called again")
    S = Step(draft=r.rid, idx=r.index, lin=r.key, gen=(old.gen + 1) if old else 1,
             deadv=old.deadv if old else frozenset(), durable_record=cfg.pa2)
    n = set_call(set_step(rp(s, marks=s.marks - 1), S), r.rid, "marked")
    return (f"call {r.rid}: P2b marks at index {r.index}", n, True, tv)


def gate_ok(s: St, rid: str) -> bool:
    """P2b's funding permit: F7 vetoes a standing step, StepCancelled one with
    tombstones; a resolved step without tombstones may retry (H12)."""
    S = step_of(s, rid)
    return S is None or (S.st == "N" and S.dst == "N" and not S.deadv)


def call_steps(s: St, cfg: Cfg, d: str, ph: str) -> Iterator[tuple]:
    S = step_of(s, d)
    if ph == "entered":
        r = row_of_id(s, d)
        if gate_ok(s, d) and s.marks > 0:
            yield mark(s, cfg, r)
        else:
            yield (f"call {d}: permit refused", set_call(s, d, None), True, [])
    elif ph == "marked":
        name = f"T{d}{S.gen}"
        s1 = set_lock(s, Lock(name, signer=d, lkey=S.lin, duffs=S.duffs))
        one = frozenset([name])
        yield (f"call {d}: signs {name}, record durable",
               set_call(set_step(s1, rp(S, signed=one, dsigned=one)), d, "signed"), True, [])
        if cfg.pa2:
            yield (f"call {d}: signs {name}, record write fails: process stops (PA-2)",
                   restart(set_step(s1, rp(S, signed=one))), True, [])
        else:
            yield (f"call {d}: signs {name}, record write fails (logged)",
                   set_call(set_step(s1, rp(S, signed=one)), d, "signed"), True, [])
        yield from end_with_notsent(s, d, f"call {d}: refused before signing")
    elif ph == "signed":
        name = next(iter(S.signed | S.dsigned), f"T{d}{S.gen}")
        lk = rp(lock_of(s, name), stored=True, tracked=True, kidx=S.idx)
        s1 = set_call(set_lock(s, lk), d, "stored")
        yield from tap(s1, cfg, lk, f"call {d}: library stores {name} (Built, index {S.idx})")
        s2 = rp(s, locks=tuple(x for x in s.locks if x.name != name))
        yield from end_with_notsent(s2, d, f"call {d}: error before the store")
    elif ph == "stored":
        lk = lock_of(s, f"T{d}{S.gen}")
        if lk is not None and lk.dead:
            yield from end_with_notsent(s, d, f"call {d}: returns NotSent (rejected)")
            return
        yield (f"call {d}: returns Ok or a timeout", set_call(s, d, "post"), True, [])
        if lk is not None and not lk.proved:
            s1 = set_lock(s, rp(lk, dead=True))
            yield from end_with_notsent(s1, d, f"call {d}: returns Refused, never broadcast")
    elif ph == "post":
        led = ledger(s, cfg)
        r = led.row_by.get(d)
        own = led.own_lock(r) if r is not None else None
        s1 = set_call(s, d, None)
        if own is not None:
            lab, n, sysf, tv = put_out(s, cfg, r, own, f"call {d}: post-call own lock")
            yield (lab + "; calling clears", set_call(n, d, None), sysf, tv)
        else:
            yield (f"call {d}: post-call, calling clears", s1, True, [])


def restore(s: St, variant: str) -> St:
    """remove_wallet (rows deleted; steps whose claim isn't None and resolved
    steps' tombstones kept, Q9) and the same seed added again, with the
    backup's rows as new ids (F11)."""
    s = restart(s)
    steps = tuple(S for S in s.steps if S.dst == "M" or S.deadv)
    locks = tuple(rp(x, tracked=False, kidx=None, lib_consumed=False, discovered=False)
                  for x in s.locks)
    rows = []
    if variant != "none":
        for i, r in enumerate(rows_by_age(s)):
            c = rp(r, rid=r.rid + "'", seq=s.nseq + i, late=False)
            if variant == "old":  # a backup taken before the funding
                c = rp(c, out=None, ident=False, code=None)
            rows.append(c)
    return rp(s, rows=tuple(sorted(rows, key=lambda r: r.rid)), steps=steps, locks=locks,
              nseq=s.nseq + len(rows), restores=s.restores - 1)


def finish(s: St, cfg: Cfg) -> tuple:
    """import_stranded: reconcile() writes first, then each unheld lock by
    owner, each on a fresh ledger."""
    tv, labels = [], []
    for rid in [r.rid for r in rows_by_age(s)]:
        for _ in range(3):
            led = ledger(s, cfg)
            r = led.row_by.get(rid)
            items = led.reconcile(r) if r is not None else []
            if not items:
                break
            lab, s, _, t = apply_reconcile(s, cfg, r, items[0], "")
            labels.append(lab.lstrip(": "))
            tv += t
    for name in [x.name for x in s.locks]:
        led = ledger(s, cfg)
        lk = led.locks[name]
        k = led.kind(lk)
        target = None
        if k == "Step":
            ro = led.row_of(led.step_by[led.owner(lk)[1]])
            if "rec-caller" in cfg.m and ro is not None and led.open(ro) and ro.rid in led.calling:
                target = "rec"  # rev1's Finish: an open caller's own lock to Recovery
            elif ro is not None and led.may_take(("Row", ro), lk):
                target = ro
            elif led.may_take(("Rec",), lk):
                target = "rec"
        elif k == "Free":
            at = [r for r in led.rows if r.kind == "user" and lk.kidx is not None
                  and r.index == lk.kidx and led.may_take(("Row", r), lk)]
            waiting = [r for r in led.rows if r.kind == "user" and led.may_take(("Row", r), lk)
                       and AMT <= lk.duffs]
            target = (at or waiting or ["rec"])[0]
        if target is not None:
            lab, s, _, t = put_out(s, cfg, target, lk, "")
            labels.append(lab.lstrip(": "))
            tv += t
    return s, labels, tv


def steps(s: St, cfg: Cfg) -> Iterator[tuple]:
    """(label, next state, system event?, transition violations)."""
    led = ledger(s, cfg)
    for d, ph in s.calling:
        yield from call_steps(s, cfg, d, ph)
    # The run, per row that isn't calling: reconcile first (5.2), then 5.4.
    for r in led.rows:
        if r.rid in led.calling:
            continue
        for item in led.reconcile(r):
            yield apply_reconcile(s, cfg, r, item, f"run {r.rid}")
        st = led.standing(r)
        if st == "Funded":
            lk = led.locks.get(led.live(r))
            if lk is not None and lk.proved and lk.tracked:
                n = set_lock(s, rp(lk, consumed=True, lib_consumed=True))
                yield (f"run {r.rid}: identity from {lk.name}", set_row(n, rp(r, ident=True)),
                       True, [])
        own = led.own_lock(r)
        if own is not None:
            yield put_out(s, cfg, r, own, f"run {r.rid}: own lock")
        if st == "Unfunded" and led.open(r):
            io = led.index_owner(r)
            if io[0] == "Lock":
                yield put_out(s, cfg, r, io[1], f"run {r.rid}: index restore")
            elif io[0] == "Taken":
                rl = ledger(s, ref(cfg))
                tv = []
                for S in s.steps:
                    ro = rl.row_of(S) if S.draft in rl.step_by else None
                    if true_out(s, S) and (S.lin == r.key or (ro is not None and ro.rid == r.rid)):
                        tv.append("S6: an outstanding step's row changes index")
                ni = led.next_index()
                yield (f"run {r.rid}: index taken, moves to {ni}", set_row(s, rp(r, index=ni)),
                       True, tv)
            elif io[0] == "Free" and s.marks > 0 and r.rid in s.funders and gate_ok(s, r.rid):
                yield (f"run {r.rid}: enter_funding (calling)", set_call(s, r.rid, "entered"),
                       False, [])
    # The chain, the library and discovery.
    for lk in s.locks:
        if lk.stored and not lk.dead and not lk.proved:
            yield (f"{lk.name} lands (proof)",
                   set_lock(s, rp(lk, proved=True, kidx=None if lk.kidx == 0 else lk.kidx)),
                   True, [])
            if s.rejects > 0:
                yield (f"{lk.name} rejected and untracked",
                       rp(set_lock(s, rp(lk, dead=True, tracked=False)), rejects=s.rejects - 1),
                       False, [])
        if lk.proved and not lk.tracked and not lk.dead:
            yield (f"{lk.name} reconstructed at index 0",
                   set_lock(s, rp(lk, tracked=True, kidx=None)), True, [])
        if lk.consumed and not lk.discovered:
            yield (f"discovery reports identity_for({lk.name})",
                   set_lock(s, rp(lk, discovered=True)), True, [])
        if (
            s.elsewhere > 0 and lk.signer is not None and lk.proved and not lk.consumed
            and not any(r.out == lk.name for r in s.rows)
        ):
            yield (f"{lk.name} consumed elsewhere (no row holds it)",
                   rp(set_lock(s, rp(lk, consumed=True)), elsewhere=s.elsewhere - 1), False, [])
    # Finish, unheld (the held path writes nothing: its report is checked as S5).
    n, labels, tv = finish(s, cfg)
    if labels:
        yield ("Finish: " + ", ".join(labels), n, True, tv)
    # Repair (P2b r4).
    for S in s.steps:
        if S.draft in led.running or S.st != "M" or not S.signed:
            continue  # NoKnownLock without a signed lock
        lks = [lock_of(s, t) for t in S.signed]
        if all(x is None or not x.proved for x in lks):
            n = s
            for x in lks:
                if x is not None:
                    n = set_lock(n, rp(x, dead=True))
            yield (f"Repair {S.draft}: self-spend over the recorded inputs -> NotSent",
                   notsent(n, S.draft, True), True, [])
    # User decisions and the environment.
    rl = ledger(s, ref(cfg))
    for r in led.rows:
        if r.kind == "user" and led.discard_allowed(r):
            tv = ["S3: a committed row is deleted"] if rl.committed(r) else []
            yield (f"discard {r.rid}", rp(s, rows=tuple(x for x in s.rows if x.rid != r.rid)),
                   False, tv)
    if s.starts > 0 and led.start_allowed():
        c = Row(rid="C", seq=s.nseq, index=led.next_index(), key="kC", late=bool(lost_out(s)))
        yield (f"start C at index {c.index}",
               set_row(rp(s, starts=s.starts - 1, nseq=s.nseq + 1), c), False, [])
    if s.restarts > 0:
        yield ("restart", rp(restart(s), restarts=s.restarts - 1), False, [])
    if s.restores > 0:
        for v, what in (("copies", "the rows' backup"), ("old", "a backup from before funding"),
                        ("none", "no rows")):
            yield (f"remove wallet, re-add ({what})", restore(s, v), False, [])
    if s.foreign > 0:
        t2 = Lock("T2", signer=None, lkey="kF", duffs=cfg.t2_amt, stored=True, tracked=True,
                  proved=True)
        yield ("foreign T2 appears (rebuilt at index 0, unattributed)",
               rp(set_lock(s, t2), foreign=s.foreign - 1), False, [])


# ---------------------------------------------------------------------------
# Properties
# ---------------------------------------------------------------------------


def violations(s: St, cfg: Cfg) -> list:
    out = []
    locks = {x.name: x for x in s.locks}
    lins = [S.lin for S in s.steps if true_out(s, S)]
    if len(lins) != len(set(lins)):
        out.append("I6: two fundings of one intent may both be out")
    need = sum(
        1 for S in s.steps
        if true_out(s, S) and not any(
            r.out is not None and (lk := locks.get(r.out)) is not None
            and lk.signer == S.draft
            and (not lk.dead or r.out in S.signed | S.dsigned)  # the attempt's own, even rejected
            for r in s.rows)
    )
    led = ledger(s, cfg)
    if led.pending(held=False) < need:
        out.append("S5: Finish's pending is below the outstanding steps no row holds")
    if led.pending(held=True) < need:
        out.append("S5: the held path's pending is below the outstanding steps no row holds")
    return out


def goal(s: St, cfg: Cfg) -> bool:
    """Nothing outstanding: no call, no outstanding step, every live lock
    consumed, no row holding unconsumed or voided value, every Cancelled row
    closed with its code."""
    if s.calling:
        return False
    led = ledger(s, ref(cfg))
    if any(led.outstanding(S) for S in led.steps):
        return False
    if any(x.stored and not x.dead and not x.consumed for x in s.locks):
        return False
    for r in s.rows:
        lv = led.live(r)
        if lv is not None and not led.consumed(lv):
            return False
        if r.out is not None and r.out in led.dead:
            return False
        st = led.standing(r)
        if st.startswith("Cancelled") and r.code != st[-2:]:
            return False
    return True


def unflagged_unknown(s: St, cfg: Cfg) -> bool:
    """Section 8.1: an Unknown step on an unflagged marker, no call running,
    whose money no row holds as a lock that can still land."""
    led = ledger(s, ref(cfg))
    return any(
        led.outstanding(S) and led.claim(S) == "Unknown" and not S.durable_record
        and S.draft not in led.running
        and not any(t in led.holder_of and not led.locks[t].dead
                    for t in led.own(S) if t in led.locks)
        for S in led.steps
    )


def stuck_reason(s: St, cfg: Cfg) -> str:
    led = ledger(s, ref(cfg))
    for S in led.steps:
        if led.outstanding(S) and any(led.consumed(t) or (t in led.locks and led.locks[t].consumed)
                                      for t in S.signed):
            return "an outstanding step whose consumed lock never settles"
    for r in s.rows:
        if r.out is not None and r.out in led.dead:
            return "a row keeps a voided outpoint"
        st = led.standing(r)
        if st.startswith("Cancelled") and r.code != st[-2:]:
            return "a Cancelled row is never closed"
        lk = led.locks.get(r.out) if r.out else None
        if lk is not None and lk.dead and not lk.consumed and r.out not in led.dead:
            return "a row holds a rejected lock with no tombstone"
    for S in led.steps:
        if led.outstanding(S) and led.claim(S) == "Unknown":
            return "a flagged Unknown step with no exit"
    for k in ("Step", "Contested", "Free", "Dead"):
        for x in s.locks:
            o = led.owner(x)
            if o is not None and o[0] == k:
                return f"an unheld {k} lock that nothing takes"
    return "other"


def explore(init: St, cfg: Cfg, stop_early: bool = False) -> dict:
    """BFS over every interleaving: the shortest trace to each violation, and
    liveness by backward reachability of the goal over system events only.
    The stuck states must all be section 8.1's. With stop_early it
    returns at the first safety violation."""
    ids = {init: 0}
    states = [init]
    parent = [(-1, "")]
    found: dict = {}
    sys_succ: list = []
    labels: set = set()
    q = deque([0])
    while q:
        i = q.popleft()
        s = states[i]
        for v in violations(s, cfg):
            found.setdefault(v, (i, None))
        succ = []
        for label, n, system, tv in steps(s, cfg):
            labels.add(label)
            j = ids.get(n)
            if j is None:
                j = len(states)
                if j >= MAX_STATES:
                    raise BoundHit(f"more than {MAX_STATES} states")
                ids[n] = j
                states.append(n)
                parent.append((i, label))
                q.append(j)
            for v in tv:
                found.setdefault(v, (i, label))
            if system:
                succ.append(j)
        sys_succ.append(succ)
        if stop_early and found:
            break
    del ids

    def trace(i: int, last: Optional[str] = None) -> list:
        out = [last] if last else []
        while i > 0:
            i, lab = parent[i]
            out.append(lab)
        return list(reversed(out))

    res = {"n": len(states), "states": states, "labels": labels, "stuck": set(), "pred": set(),
           "trace": trace}
    if not (stop_early and found):
        pred_g: list = [[] for _ in states]
        for i, succ in enumerate(sys_succ):
            for j in succ:
                pred_g[j].append(i)
        del sys_succ
        can = [False] * len(states)
        q = deque(i for i, s in enumerate(states) if goal(s, cfg))
        for i in q:
            can[i] = True
        while q:
            j = q.popleft()
            for i in pred_g[j]:
                if not can[i]:
                    can[i] = True
                    q.append(i)
        del pred_g
        stuck = {i for i in range(len(states)) if not can[i]}
        pred = {i for i, s in enumerate(states) if unflagged_unknown(s, cfg)}
        for i in sorted(stuck - pred):
            found.setdefault(f"L: {stuck_reason(states[i], cfg)}", (i, None))
        res.update(stuck=stuck, pred=pred)
    res["found"] = {v: trace(i, lab) for v, (i, lab) in found.items()}
    return res


# ---------------------------------------------------------------------------
# Scenarios
# ---------------------------------------------------------------------------

A = Row(rid="A", seq=0, index=0, key="kA")
B = Row(rid="B", seq=1, index=1, key="kB")


def scenarios() -> dict:
    # A funds (then C, or A's copy); B is a bystander that may take locks.
    fund = St(rows=(A, B), marks=2, restarts=1, foreign=1, starts=1)
    # A and B both fund: T1 and T3 in flight together.
    two = St(rows=(A, B), marks=2, restarts=1, rejects=1, funders=frozenset({"A", "B"}))
    rest = St(rows=(A, B), marks=2, restores=1, starts=1)
    # A consumed signed lock no row holds; C1 (a discarded Done row).
    cons = St(rows=(A, B), marks=1, elsewhere=1, starts=1, restarts=1)
    # Sol's legacy substitute: A holds the foreign T2, still live; its own
    # signed TA1 is unheld.
    subst = St(
        rows=(A._replace(out="T2"), B),
        locks=(
            Lock("T2", signer=None, lkey="kF", stored=True, tracked=True, proved=True),
            Lock("TA1", signer="A", lkey="kA", stored=True, tracked=True),
        ),
        steps=(Step("A", 0, "kA", signed=frozenset({"TA1"}), dsigned=frozenset({"TA1"}),
                    durable_record=True),),
        restores=1, starts=1,
    )
    # Section 5.2c / Q8: a restored backup in which B' holds A's TA1 while A'
    # is open at S_A's index.
    wrong = St(
        rows=(Row(rid="A'", seq=10, index=0, key="kA"),
              Row(rid="B'", seq=11, index=1, key="kB", out="TA1")),
        locks=(Lock("TA1", signer="A", lkey="kA", stored=True, proved=True),),
        steps=(Step("A", 0, "kA", signed=frozenset({"TA1"}), dsigned=frozenset({"TA1"}),
                    durable_record=True),),
        marks=1, starts=1, nseq=20,
    )
    q6 = St(rows=(A, B), marks=1, restarts=1, foreign=1)
    return {
        "fund (flagged)": (fund, Cfg(pa2=True)),
        "fund (unflagged)": (fund, Cfg(pa2=False)),
        "two calls (flagged)": (two, Cfg(pa2=True)),
        "two calls (unflagged)": (two, Cfg(pa2=False)),
        "restore (flagged)": (rest, Cfg(pa2=True)),
        "restore (unflagged)": (rest, Cfg(pa2=False)),
        "consumed elsewhere (flagged)": (cons, Cfg(pa2=True)),
        "legacy substitute (flagged)": (subst, Cfg(pa2=True)),
        "wrong holder restored (flagged)": (wrong, Cfg(pa2=True)),
        "Q6 other amount (unflagged)": (q6, Cfg(pa2=False, t2_amt=2)),
    }


MUTATIONS = {
    # rev2, section 11.1
    "calling-free": "a calling row takes a Free lock (OD-05)",
    "rec-caller": "Recovery takes an open caller's Step lock (rev1's Finish)",
    "no-attach": "no attach: consumed signed value never reaches its open row (OD-03)",
    "no-closure": "no closure: Close never writes (OD-04)",
    "clear-inexact": "the closure clears an outpoint that isn't dead (any tombstone of its step)",
    "signer-contrary": "the signer takes contrary value from the holder (Q8 reversed)",
    "next-index-outstanding": "next_index skips only outstanding steps (ignores resolved ones)",
    "c4-unknown": "C4 reads Unknown: a resolved flagged step re-read MaybeSent",
    # rev1's, kept
    "by-row": "provenance read by row: deleting a row drops its step (R8-02)",
    "unknown-unclaimed": "Unknown read as Unclaimed (R8-01)",
    "settled-exemption": "the settled() exemption: a row with an identity may be discarded",
    "held-no-step": "the held Finish path omits pending's step term (R8-03)",
    "rule4": "rule 4: a signed lock its signer can't take stays pending for ever (Q1)",
    "no-heir": "no heir: an orphan step has no row",
    "take-contested": "a row takes a Contested lock (I5)",
    "r7-01": "R7-01: an Awaiting row takes a Free lock",
}


def witnesses(name: str, r: dict, cfg: Cfg) -> list:
    """Reachability witnesses for rev2's new paths: (description, reached)."""
    st, lab = r["states"], r["labels"]
    out = []

    def standing(s, rid):
        row = row_of_id(s, rid)
        return ledger(s, cfg).standing(row) if row is not None else None

    if name == "consumed elsewhere (flagged)":
        out.append(("OD-03: Attach gives an open signer its consumed lock and identity",
                    any("Attach" in x for x in lab)))
        out.append(("C1: a discarded Done row's step settles in place and start is allowed",
                    any(row_of_id(s, "A") is None and row_of_id(s, "C") is not None
                        and any(S.draft == "A" and x.consumed for S in s.steps for x in s.locks
                                if x.signer == "A") for s in st)))
    if name == "fund (flagged)":
        out.append(("C5: a flagged recordless marker after a restart closes its row "
                    "(Cancelled funding_cancelled, code written)",
                    any(s.restarted and row_of_id(s, "A") is not None
                        and row_of_id(s, "A").code == "fc"
                        and (S := step_of(s, "A")) is not None and S.st == "M" and not S.signed
                        for s in st)))
        out.append(("C4: a resolved flagged step stays None while an unattributed T2 stands, "
                    "and T2 is imported",
                    any((S := step_of(s, "A")) is not None and S.deadv and S.st == "N"
                        and (t2 := lock_of(s, "T2")) is not None
                        and any(x.out == "T2" for x in s.rows) for s in st)))
        out.append(("OD-04: a voided outpoint is cleared and its row closed",
                    any("Close A (clear" in x for x in lab)))
    if name == "wrong holder restored (flagged)":
        out.append(("5.2c/Q8: A' reads Cancelled(funded_elsewhere) and B' keeps TA1 to its "
                    "identity",
                    any(standing(s, "A'") == "Cancelled-fe" and row_of_id(s, "B'") is not None
                        and row_of_id(s, "B'").ident for s in st)))
    if name == "legacy substitute (flagged)":
        out.append(("OD-04 exact: Repair voids TA1 and A keeps its live T2",
                    any((S := step_of(s, "A")) is not None and "TA1" in S.deadv
                        and row_of_id(s, "A") is not None and row_of_id(s, "A").out == "T2"
                        for s in st)))
    if name == "Q6 other amount (unflagged)":
        out.append(("Q6: a T2 of the other amount isn't Contested: B takes it while S_A is "
                    "Unknown",
                    any(any(x.out == "T2" and x.rid == "B" for x in s.rows)
                        and unflagged_unknown(s, cfg) for s in st)))
    return out


def notes(r: dict, cfg: Cfg) -> Iterator[str]:
    """Observations that are not failures: states where a row with a written
    closure code reads other than Cancelled (5.2b says the code alone keeps it
    Cancelled; section 3.3's rows 4-5 come first)."""
    seen = set()
    for i, s in enumerate(r["states"]):
        if not any(row.code for row in s.rows):
            continue
        led = ledger(s, cfg)
        for row in s.rows:
            if row.code and (st := led.standing(row)) not in ("Cancelled-fe", "Cancelled-fc"):
                if (row.code, st) not in seen:
                    seen.add((row.code, st))
                    yield (f"a row closed with code {row.code} reads {st} "
                           f"(state {i}, {len(r['trace'](i))} steps): "
                           + " -> ".join(r["trace"](i)))


# Smallest first: a rejected rule is judged on the first scenario that catches it.
def order(sizes: dict) -> list:
    return sorted(sizes, key=lambda k: sizes[k])


def main() -> int:
    t0 = time.time()
    ok = True

    def check(cond: bool, msg: str) -> None:
        nonlocal ok
        print(("PASS " if cond else "FAIL ") + msg, flush=True)
        ok = ok and cond

    sc = scenarios()
    sizes = {}
    try:
        print("Part 1: rev2 (section 13.1 applied)", flush=True)
        for name, (init, cfg) in sc.items():
            r = explore(init, cfg)
            sizes[name] = r["n"]
            for v in sorted(r["found"]):
                tr = r["found"][v]
                print(f"         {v} ({len(tr)} steps): {' -> '.join(tr) or '(the initial state)'}")
            safety = [v for v in r["found"] if not v.startswith("L: ")]
            check(not safety, f"[{name}] S1-S6, I3, I4, I6, I10-I12 hold ({r['n']} states)")
            live = [v for v in r["found"] if v.startswith("L: ")]
            what = ("every state reaches nothing outstanding" if not r["stuck"] else
                    f"the {len(r['stuck'])} stuck states are all section 8.1's "
                    f"unflagged-Unknown states (of {len(r['pred'])} such states)")
            check(not live, f"[{name}] liveness: {what}")
            for desc, reached in witnesses(name, r, cfg):
                check(reached, f"[{name}] reached: {desc}")
            for note in notes(r, cfg):
                print(f"NOTE [{name}] {note}")
            del r

        if not ok:
            print("Part 2 skipped: rev2 must pass before a rejected rule can be judged")
            print(f"{time.time() - t0:.1f} s\nFAILED")
            return 1
        print("Part 2: rejected rules (each must FAIL the check)", flush=True)
        for mut, what in MUTATIONS.items():
            caught = None
            for name in order(sizes):
                init, cfg = sc[name]
                res = explore(init, rp(cfg, m=frozenset({mut})), stop_early=True)
                if res["found"]:
                    caught = name
                    break
            print(f"       - {mut}: {what}")
            if caught:
                init, cfg = sc[caught]
                full = explore(init, rp(cfg, m=frozenset({mut})))["found"]
                for p, ptr in sorted(full.items(), key=lambda kv: (len(kv[1]), kv[0])):
                    print(f"           FAILS (as expected): {p} [{caught}, {len(ptr)} steps]")
                v, tr = min(full.items(), key=lambda kv: (len(kv[1]), kv[0]))
                print(f"           shortest: {' -> '.join(tr) or '(the initial state)'}")
            check(caught is not None, f"rule '{mut}' is caught")
    except BoundHit as e:
        print(f"BOUND HIT: {e}; the check did not complete", flush=True)
        return 2

    print(f"{time.time() - t0:.1f} s")
    print("ok" if ok else "FAILED")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
