"""Split-step variant of e0_04_dispatch_model.py: the record is read under
the guard in one step (classification) and the fence verdict plus the
record write happen in a later step, as the DASHPAY §2.6 text allows
("reads the record under the guard ... takes the kind from it"; the fence
call takes the permit afterwards). Also models the m1 freeze: admission can
close (refusing a First) while another party still holds the shared permit.

Variants of the refused-First write:
  literal: write Revoked, remove row, release inputs (the spec text)
  cas:     only if the record is still Unsent; otherwise no cleanup and
           continue as Resend
Run: python3 -I docs/design/checks/e0_04_split_model.py docs/design/checks/e0_04_dispatch_model.py

E0-04 design input (DEC-57), from the fix4 review of DW-E0-03. It prints the
violations each variant admits; it is a probe, not a pass/fail check.
"""
import importlib.util
import sys
from dataclasses import replace

spec = importlib.util.spec_from_file_location("m", sys.argv[1])
m = importlib.util.module_from_spec(spec)
sys.modules["m"] = m
spec.loader.exec_module(m)
UNSENT, DISPATCHING, REVOKED = m.UNSENT, m.DISPATCHING, m.REVOKED

VARIANT = "literal"
FREEZE = False


def act(s, seen, who):
    """Fence + write for an actor that classified the record as `seen`.
    Returns (label, state, admitted, permit)."""
    if seen == REVOKED or not s.row and seen != DISPATCHING:
        return "no dispatch", s, False, False
    if seen == DISPATCHING:
        return "Resend", s, True, False
    # seen Unsent -> First: permit taken, lease/admission checked.
    # Under FREEZE, admission can be closed while other holders exist;
    # the shared permit is still obtainable (drain not yet queued).
    refused = s.lease_revoked or (FREEZE and s.rule == "frozen")
    if s.lease_revoked and s.holders and not FREEZE:
        # cannot happen without the freeze: revocation needed exclusivity
        pass
    if refused:
        if VARIANT == "cas" and s.record != UNSENT:
            if s.record == DISPATCHING:
                return "refused, record now Dispatching: Resend", s, True, False
            return "refused, record Revoked: no dispatch", s, False, False
        return "First refused: Revoked, released", replace(
            s, record=REVOKED, row=False, reserved=False
        ), False, False
    if VARIANT == "cas" and s.record == REVOKED:
        return "admitted but Revoked: no dispatch", s, False, False
    return "First admitted", replace(
        s, record=DISPATCHING, dispatched_live=True
    ), True, True


def steps(s):
    if not s.lease_revoked and not s.holders:
        yield "L: revoke", replace(s, lease_revoked=True)
    if FREEZE and s.rule != "frozen" and not s.lease_revoked:
        # freeze closes admission without waiting for holders
        yield "L: freeze", replace(s, rule="frozen")
    # O
    if s.o == "pause":
        yield "O: classify", replace(s, o="cls:" + s.record)
    elif s.o.startswith("cls:"):
        label, n, adm, p = act(s, s.o[4:], "O")
        if adm:
            n = replace(n, o="transport",
                        holders=n.holders | {"O"} if p else n.holders)
        else:
            n = replace(n, o="done", o_verdict="Cancelled")
        yield "O: " + label, n
    elif s.o == "transport":
        yield "O: sent", replace(m.send(s, "O"), o="done", o_verdict="Sent",
                                 holders=s.holders - {"O"})
        yield "O: H expires", replace(s, o="late", o_verdict="MaybeSent",
                                      holders=s.holders - {"O"})
    elif s.o == "late":
        yield "O: late send", replace(m.send(s, "O"), o="done")
    # R
    if s.r == "idle":
        if not s.row:
            yield "R: nothing", replace(s, r="done")
        else:
            yield "R: claim+classify", replace(s, r="cls:" + s.record,
                                               claims=s.claims + 1)
    elif s.r.startswith("cls:"):
        label, n, adm, p = act(s, s.r[4:], "R")
        if adm:
            n = replace(n, r="transport",
                        holders=n.holders | {"R"} if p else n.holders)
        else:
            n = replace(n, r="done", claims=n.claims - 1)
        yield "R: " + label, n
    elif s.r == "transport":
        yield "R: sent", replace(m.send(s, "R"), r="done",
                                 claims=s.claims - 1, holders=s.holders - {"R"})
        yield "R: H expires", replace(s, r="late", claims=s.claims - 1,
                                      holders=s.holders - {"R"})
    elif s.r == "late":
        yield "R: late send", replace(m.send(s, "R"), r="done")


m.steps = steps
for freeze in (False, True):
    for variant in ("literal", "cas"):
        VARIANT, FREEZE = variant, freeze
        for scen, init in m.scenarios("e0-04").items():
            found = m.explore(init, scen)
            vs = {}
            for v, tr in found:
                vs.setdefault(v, tr)
            print(f"freeze={freeze} {variant:7} {scen:12}",
                  {v: " -> ".join(t) for v, t in vs.items()} or "none")
