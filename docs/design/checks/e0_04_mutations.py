"""Mutation probes of docs/design/checks/e0_04_dispatch_model.py.

Run: python3 -I docs/design/checks/e0_04_mutations.py docs/design/checks/e0_04_dispatch_model.py

E0-04 design input (DEC-57), from the fix4 review of DW-E0-03. Each wrong
rule should show a violation; "baseline" should show none.
"""
import importlib.util
import sys
from dataclasses import replace

spec = importlib.util.spec_from_file_location("m", sys.argv[1])
m = importlib.util.module_from_spec(spec)
sys.modules["m"] = m
spec.loader.exec_module(m)

orig_handoff = m.handoff


def run(name):
    res = {}
    for scen, init in m.scenarios("e0-04").items():
        found = m.explore(init, scen)
        res[scen] = sorted({v for v, _ in found})
    print(name, res)


# M-a: Revoked does not override claims (pin's exclusion kept).
def handoff_a(s, k):
    for label, n, adm, p in orig_handoff(s, k):
        if label.startswith("First refused") and s.claims:
            n = replace(n, row=s.row, reserved=s.reserved)
        yield label, n, adm, p


# M-b: resume of an Unsent row is a lease-free Resend (call path).
def handoff_b(s, k):
    if k == "Resend" and s.record == m.UNSENT:
        yield "Resend by call path", s, True, False
        return
    yield from orig_handoff(s, k)


# M-c: refused First leaves the row (no cleanup at all).
def handoff_c(s, k):
    for label, n, adm, p in orig_handoff(s, k):
        if label.startswith("First refused"):
            n = replace(n, row=s.row, reserved=s.reserved)
        yield label, n, adm, p


# M-d: Resend refused under a revoked lease (too strict).
def handoff_d(s, k):
    if s.record == m.DISPATCHING and s.lease_revoked:
        yield "Resend refused", s, False, False
        return
    yield from orig_handoff(s, k)


for name, h in [("a-no-override", handoff_a), ("b-callpath", handoff_b),
                ("c-no-cleanup", handoff_c), ("d-strict-resend", handoff_d)]:
    m.handoff = h
    run(name)
m.handoff = orig_handoff
run("baseline")
