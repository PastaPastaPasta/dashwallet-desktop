#!/usr/bin/env python3
"""Build the summary committed beside the write-up: analyze.py's output plus
post-day tuples split by whether the cited quorum is still active, the
Platform statuses of the newest list over each 25 h run, and RSS per window.

usage: summary.py RUN_DIR OUT_JSON
"""
import collections
import glob
import json
import sys

from analyze import level, run_name, split_runs, summarize, trusted_tuples
from heights import read_heights, rows

R, out_path = sys.argv[1:3]
NETS = ("testnet", "mainnet")
LEVELS = ("miss", "found_not_verified", "verified")

keyed = {n: trusted_tuples(f"{R}/tuples-{n}.jsonl") for n in NETS}
heights = {n: read_heights(f"{R}/heights-{n}.txt") for n in NETS}
summary = summarize(R, heights)

def reason(status):
    """`Skipped(UnknownBlock(0x..))` -> `Skipped(UnknownBlock)`."""
    parts = status.split("(")
    return f"{parts[0]}({parts[1].rstrip(')')})" if len(parts) > 1 else status


def engine_ticks(run):
    """Ticks that carry the newest list's Platform quorums."""
    return [r for r in run if r.get("event") == "tick" and (r.get("engine") or {}).get("platform_quorums_newest")]


# Post-day: every keyed tuple of the day and the best level it reached, split
# by whether the cited quorum is in the run's last newest list (active).
postday = {}
for p in sorted(glob.glob(f"{R}/lookups-postday-*.jsonl")):
    name, net = run_name(p)
    run = split_runs(p)[0]
    ticks = engine_ticks(run)
    if not ticks:
        sys.exit(f"{name}: no engine ticks, cannot tell active quorums")
    active = {h for h, _ in ticks[-1]["engine"]["platform_quorums_newest"]}
    best, best_status = {}, {}
    for r in run:
        if r.get("event") != "lookup":
            continue
        k = (r["type"], r["hash"], r["ccl"])
        if k in keyed[net] and level(r) >= best.get(k, 0):
            best[k], best_status[k] = level(r), reason(r["status"]) if r["found"] else "miss"
    buckets, reasons = {}, collections.Counter()
    for k, lv in best.items():
        b = "active quorum" if k[1] in active else "rotated-out quorum"
        buckets.setdefault(b, dict.fromkeys(LEVELS, 0))[LEVELS[lv]] += 1
        if lv == 1:
            reasons[f"{b}: {best_status[k]}"] += 1
    postday[name] = {"tuples": len(best), "by_quorum": buckets, "non_verified_tuples_by_reason": dict(reasons)}
summary["postday"] = postday


# The newest list's Platform statuses over each 25 h run: counts after sync and
# at the end, the range over all ticks, and every quorum that was not
# `Verified` at some tick (hours since start, and how proofs first found it
# after sync). A span runs from the first to the last such tick and keeps the
# first reason; it does not split a quorum that recovered and lapsed again.
engine = {}
for n in ("pin-mainnet", "pin-testnet", "dev-mainnet", "dev-testnet"):
    net = n.rsplit("-", 1)[-1]
    run = list(rows(f"{R}/lookups-{n}.jsonl"))
    t0 = run[0]["t"]
    ticks = [r for r in engine_ticks(run) if r["synced"]]
    if not ticks:
        continue

    def counts(tick):
        return dict(collections.Counter(s.split("(")[0] for _, s in tick["engine"]["platform_quorums_newest"]))

    verified = [counts(r).get("Verified", 0) for r in ticks]
    spans = {}
    for r in ticks:
        for h, st in r["engine"]["platform_quorums_newest"]:
            if st != "Verified":
                sp = spans.setdefault(h, {"base": heights[net].get(h), "reason": reason(st), "from_h": None})
                sp["from_h"] = sp["from_h"] or round((r["t"] - t0) / 3.6e6, 2)
                sp["to_h"] = round((r["t"] - t0) / 3.6e6, 2)
    for h, sp in spans.items():
        cited = next((r for r in run if r.get("event") == "lookup" and r["hash"] == h and r["synced"]), None)
        if cited:
            sp["first_cited_h"] = round((cited["t"] - t0) / 3.6e6, 2)
            sp["status_when_first_cited"] = reason(cited["status"]) if cited["found"] else "miss"
    first = ticks[0]["engine"]
    engine[n] = {
        "after_sync": {"lists": first.get("lists"), "newest": first.get("newest"), "platform_status": counts(ticks[0])},
        "end": {"lists": ticks[-1]["engine"].get("lists"), "newest": ticks[-1]["engine"].get("newest"),
                "platform_status": counts(ticks[-1])},
        "verified_min": min(verified),
        "verified_max": max(verified),
        "verified_after_sync_bases": sorted(heights[net][h] for h, st in first["platform_quorums_newest"]
                                            if st == "Verified" and h in heights[net]),
        "verified_after_sync_without_height": sum(1 for h, st in first["platform_quorums_newest"]
                                                  if st == "Verified" and h not in heights[net]),
        "not_verified_at_some_tick": spans,
    }
summary["engine_newest_list"] = engine

# RSS (MiB) min and max per window of hours since the probe started, and the
# peak. The sampler names each series after the probe's systemd unit.
series = collections.defaultdict(list)
with open(f"{R}/rss.txt") as f:
    for line in f:
        t, unit, kib = line.split()
        series[unit.removeprefix("e010a-")].append((int(t), int(kib) // 1024))
rss = {}
for unit, v in sorted(series.items()):
    t0 = next(r["t"] // 1000 for r in rows(f"{R}/lookups-{unit}.jsonl") if r.get("event") == "start")
    windows = {}
    for lo, hi in ((0, 1), (1, 6), (6, 12), (12, 18), (18, 26)):
        w = [m for t, m in v if lo * 3600 <= t - t0 < hi * 3600]
        if w:
            windows[f"{lo}-{hi}h"] = [min(w), max(w)]
    peak_t, peak = max(v, key=lambda x: x[1])
    rss[unit] = {"first_sample_s": v[0][0] - t0, "windows": windows, "peak": peak,
                 "peak_h": round((peak_t - t0) / 3600, 2)}
summary["rss_mib"] = rss

with open(out_path, "w") as f:
    json.dump(summary, f, indent=1, default=str)
    f.write("\n")
