#!/usr/bin/env python3
"""Summarise an E0-10a run directory (tuples-*.jsonl, lookups-*.jsonl).

usage: analyze.py RUN_DIR [--heights NET=FILE ...]

A heights file (heights.py) maps a quorum hash (RPC order) to the height of
its block; the report uses it for the quorum's age at the cited height.
Prints a JSON summary on stdout.

Definitions (per probe run; lookup files are named `lookups-<run>-<network>`):
- a tuple is *live* when the trusted service gave a key for it and the probe
  first asked for it at or after the run's first `synced` event (masternode
  sync complete). Headline rates are over live tuples;
- *hit*: dash-spv returned the quorum and its key equals the trusted key
  (a different key fails the proof, so it counts as a miss);
- *verified hit*: a hit whose entry status is `Verified`.
"""
import collections
import glob
import json
import os
import sys

from heights import read_heights, rows


def pct(values, q):
    """Nearest-rank percentile (the maximum when q * n rounds past the end)."""
    if not values:
        return None
    values = sorted(values)
    return values[min(len(values) - 1, int(q * len(values)))]


def summarize_tuples(path, heights):
    queries = list(rows(path))
    ok = [r for r in queries if r.get("ok")]
    tuples = set()
    quorums = {}
    for r in ok:
        for c in r["ctx_calls"]:
            tuples.add((c["type"], c["hash"], c["ccl"]))
            q = quorums.setdefault(c["hash"], {"type": c["type"], "first_t": r["t"], "last_t": r["t"],
                                               "ccl_min": c["ccl"], "ccl_max": c["ccl"], "lookups": 0})
            q["last_t"] = r["t"]
            q["ccl_min"] = min(q["ccl_min"], c["ccl"])
            q["ccl_max"] = max(q["ccl_max"], c["ccl"])
            q["lookups"] += 1
    lags = []
    for h, q in quorums.items():
        base = heights.get(h)
        q["base_height"] = base
        if base is not None:
            q["lag_min"] = q["ccl_min"] - base
            q["lag_max"] = q["ccl_max"] - base
            lags.extend([q["lag_min"], q["lag_max"]])
    elapsed = [r["elapsed_ms"] for r in ok]
    # Proofs from evonodes whose chain-locked height trails the round's newest
    # by more than 50 blocks (a lagging or stuck node).
    round_max = collections.defaultdict(int)
    for r in queries:
        for c in r.get("ctx_calls", []):
            round_max[r["round"]] = max(round_max[r["round"]], c["ccl"])
    lagging = [(r, c) for r in queries for c in r.get("ctx_calls", []) if c["ccl"] < round_max[r["round"]] - 50]
    return {
        "queries": len(queries),
        "ok": len(ok),
        "errors": collections.Counter(r.get("error", "")[:120] for r in queries if not r.get("ok")).most_common(5),
        "span_hours": round((queries[-1]["t"] - queries[0]["t"]) / 3.6e6, 2) if queries else 0,
        "types": sorted({t[0] for t in tuples}),
        "distinct_tuples": len(tuples),
        "distinct_quorums": len(quorums),
        "ccl_range": [min((t[2] for t in tuples), default=None), max((t[2] for t in tuples), default=None)],
        "trusted_errors": sum(1 for r in queries for c in r.get("ctx_calls", []) if "error" in c),
        "lag_ccl_minus_base": {"min": min(lags, default=None), "max": max(lags, default=None)},
        "quorums": [{"hash": h, **q} for h, q in quorums.items()],
        "nodes": len({r["node"] for r in queries}),
        "lagging_node_proofs": len(lagging),
        "lagging_nodes": sorted({(r["node"], round_max[r["round"]] - c["ccl"], r.get("ok", False)) for r, c in lagging})[:10],
        "elapsed_ms_p50": pct(elapsed, 0.5),
        "elapsed_ms_p95": pct(elapsed, 0.95),
    }


def split_runs(path):
    """Rows grouped per probe run (each run starts with a `start` event)."""
    runs = []
    for r in rows(path):
        if r.get("event") == "start" or not runs:
            runs.append([])
        runs[-1].append(r)
    return runs


def trusted_tuples(path):
    """(type, hash, ccl) tuples the trusted service gave a key for."""
    return {(c["type"], c["hash"], c["ccl"]) for r in rows(path) for c in r.get("ctx_calls", []) if "key" in c}


def summarize_lookups(path, keyed_tuples):
    runs = [summarize_run(run, keyed_tuples) for run in split_runs(path)]
    return runs[0] if len(runs) == 1 else {"runs": runs}


def level(r):
    """0 miss or wrong key, 1 hit not Verified, 2 verified hit."""
    if not r["found"] or r.get("key_matches_trusted") is False:
        return 0
    return 2 if r["status"] == "Verified" else 1


def summarize_run(run_rows, keyed_tuples):
    start = next((r for r in run_rows if r.get("event") == "start"), {})
    t0 = start.get("t")
    synced_t = next((r["t"] for r in run_rows if r.get("event") == "synced"), None)
    first_ask, best = {}, {}
    statuses, errors = collections.Counter(), collections.Counter()
    mismatches, lookup_ms, hit_t, verified_t, tips = 0, [], [], [], []
    behind_tip = {}  # first ask: header tip at the last tick minus the cited ccl
    for r in run_rows:
        ev = r.get("event")
        if ev == "tick":
            tip = ((r.get("progress") or {}).get("headers") or {}).get("tip")
            if tip is not None:
                tips.append((r["t"], tip))
        if ev != "lookup":
            continue
        key = (r["type"], r["hash"], r["ccl"])
        lookup_ms.append(r["lookup_ms"])
        if r["found"]:
            statuses[r["status"].split("(")[0]] += 1
            mismatches += r.get("key_matches_trusted") is False
        else:
            errors[r["error"].split(":")[0]] += 1
        if key not in first_ask and tips:
            behind_tip[key] = tips[-1][1] - r["ccl"]
        first_ask.setdefault(key, r)
        lv = level(r)
        best[key] = max(best.get(key, 0), lv)
        if lv >= 1:
            hit_t.append(r["t"])
        if lv == 2:
            verified_t.append(r["t"])

    # Tuples the trusted service could not key either (a lagging evonode's
    # proof, say) are reported apart, not in the rates.
    keyed = {k: r for k, r in first_ask.items() if k in keyed_tuples}
    live = {k: r for k, r in keyed.items() if synced_t is not None and r["t"] >= synced_t}
    live_behind = [behind_tip[k] for k in live if k in behind_tip]
    live_hit = [r for r in live.values() if level(r) >= 1]
    live_verified = [r for r in live.values() if level(r) == 2]

    # Longest stretch with no header-tip advance (a stalled or dead client).
    stall, last_change = 0, None
    for i, (t, tip) in enumerate(tips):
        if i == 0 or tip != tips[i - 1][1]:
            last_change = t
        stall = max(stall, t - last_change)

    def since_start(times):
        return (min(times) - t0) / 1000 if times and t0 else None

    def rate(part):
        return round(len(part) / len(live), 4) if live else None

    return {
        "label": start.get("label"),
        "start_t": t0,
        "ended": any(r.get("event") == "end" for r in run_rows),
        "runner_ended": any(r.get("event") == "runner_ended" for r in run_rows),
        "synced_after_s": (synced_t - t0) / 1000 if synced_t and t0 else None,
        "first_hit_after_s": since_start(hit_t),
        "first_verified_hit_after_s": since_start(verified_t),
        "tuples_asked": len(first_ask),
        "tuples_without_trusted_key": len(first_ask) - len(keyed),
        "without_trusted_key": [
            {"hash": k[1], "ccl": k[2], "found": r["found"]} for k, r in first_ask.items() if k not in keyed
        ][:10],
        "live_tuples": len(live),
        "live_first_asked_while_not_synced": sum(1 for r in live.values() if not r["synced"]),
        "live_first_ask_hit": len(live_hit),
        "live_first_ask_verified": len(live_verified),
        "live_eventually_verified": sum(1 for k in live if best[k] == 2),
        "miss_rate_first_ask": round(1 - len(live_hit) / len(live), 4) if live else None,
        "verified_rate_first_ask": rate(live_verified),
        "status_counts": dict(statuses),
        "error_counts": dict(errors),
        "key_mismatches": mismatches,
        "ccl_behind_tip_live": {
            "min": min(live_behind, default=None),
            "p50": pct(live_behind, 0.5),
            "max": max(live_behind, default=None),
        },
        "lookup_ms_p50": pct(lookup_ms, 0.5),
        "lookup_ms_p99": pct(lookup_ms, 0.99),
        "lookup_ms_max": max(lookup_ms, default=None),
        "header_tip_last": tips[-1][1] if tips else None,
        "header_tip_max_stall_min": round(stall / 60000, 1),
        "live_first_ask_misses": [
            {k: r[k] for k in ("hash", "ccl", "age_s", "error", "status", "synced") if k in r}
            for r in live.values() if level(r) == 0
        ][:20],
        "live_first_ask_unverified": [
            {k: r[k] for k in ("hash", "ccl", "age_s", "status") if k in r}
            for r in live_hit if level(r) == 1
        ][:20],
    }


def run_name(path):
    """`lookups-<run>-<network>.jsonl` -> (`<run>-<network>`, `<network>`)."""
    name = os.path.basename(path).removeprefix("lookups-").removesuffix(".jsonl")
    return name, name.rsplit("-", 1)[-1]


def summarize(run, heights):
    out = {"tuples": {}, "lookups": {}}
    keyed = {}
    for path in sorted(glob.glob(os.path.join(run, "tuples-*.jsonl"))):
        net = os.path.basename(path)[len("tuples-"):-len(".jsonl")]
        out["tuples"][net] = summarize_tuples(path, heights.get(net, {}))
        keyed[net] = trusted_tuples(path)
    for path in sorted(glob.glob(os.path.join(run, "lookups-*.jsonl"))):
        name, net = run_name(path)
        out["lookups"][name] = summarize_lookups(path, keyed.get(net, set()))
    return out


def main():
    run = sys.argv[1]
    heights = {}
    args = sys.argv[2:]
    while args:
        if args.pop(0) == "--heights":
            net, path = args.pop(0).split("=", 1)
            heights[net] = read_heights(path)
    json.dump(summarize(run, heights), sys.stdout, indent=1, default=str)
    print()


if __name__ == "__main__":
    main()
