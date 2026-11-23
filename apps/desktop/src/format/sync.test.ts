import { describe, expect, it } from "vitest";
import type { SyncStatus } from "../api/generated/SyncStatus";
import { isSyncing, NOT_SETTLED, SETTLE_BLOCKS, SETTLE_MS, settleSync, syncPercent, syncState, type Settled } from "./sync";

const base: SyncStatus = {
  running: true,
  phases: [],
  activePhase: null,
  tipHeight: null,
  tipTime: null,
  chainlockHeight: null,
  connectedPeers: 0,
  caughtUp: false,
  secondsSinceProgress: null,
};

describe("syncPercent", () => {
  it("is unknown before any phase reports a target", () => {
    expect(syncPercent(base)).toBeNull();
  });
  it("averages the reporting phases", () => {
    const s: SyncStatus = {
      ...base,
      phases: [
        { phase: "headers", currentHeight: 100, targetHeight: 100, done: true },
        { phase: "filters", currentHeight: 25, targetHeight: 100, done: false },
        { phase: "masternodes", currentHeight: null, targetHeight: null, done: false },
      ],
    };
    expect(syncPercent(s)).toBe(62.5);
    expect(syncPercent({ ...s, caughtUp: true })).toBe(100);
  });
});

describe("syncState", () => {
  it("names the state without the percentage", () => {
    expect(syncState(undefined)).toBe("Connecting…");
    expect(syncState({ ...base, running: false })).toBe("Not syncing");
    expect(syncState(base)).toBe("Synchronizing with network…");
    expect(syncState({ ...base, caughtUp: true })).toBe("Up to date");
    expect(isSyncing(base)).toBe(true);
    expect(isSyncing({ ...base, caughtUp: true })).toBe(false);
  });
});

describe("DEC1058_statusDoesNotFlap", () => {
  const phases = (current: number, target = 1000, done = false): SyncStatus["phases"] => [
    { phase: "headers", currentHeight: current, targetHeight: target, done },
    { phase: "filters", currentHeight: current, targetHeight: target, done },
  ];
  /** The status bar's states over a sequence of [engine reading, ms] pairs, as useSyncStatus feeds them. */
  const run = (readings: [SyncStatus, number][]) => {
    let settled: Settled = NOT_SETTLED;
    return readings.map(([r, at]) => {
      const out = settleSync(settled, r, at);
      settled = out.settled;
      return { text: syncState(out.status), percent: isSyncing(out.status) ? syncPercent(out.status) : null };
    });
  };
  const synced = { ...base, caughtUp: true, phases: phases(1000, 1000, true) };
  const syncing = { ...base, caughtUp: false };

  it("a new block while synced stays Up to date, never Syncing 100%", () => {
    const newBlock = { ...syncing, phases: phases(1000, 1001) };
    const out = run([[synced, 0], [newBlock, 1000], [{ ...newBlock, phases: phases(1001, 1001, true) }, 2000], [synced, 3000], [newBlock, 4000], [synced, 5000]]);
    expect(out.map((o) => o.text)).toEqual(Array(6).fill("Up to date"));
    expect(out.every((o) => o.percent === null)).toBe(true);
  });

  it("syncing from the start shows its progress, and never 100.0% before it is caught up", () => {
    const out = run([[{ ...syncing, phases: phases(0) }, 0], [{ ...syncing, phases: phases(500) }, 1000], [{ ...syncing, phases: phases(1000, 1000, true) }, 2000], [synced, 3000]]);
    expect(out.map((o) => o.text)).toEqual(["Synchronizing with network…", "Synchronizing with network…", "Synchronizing with network…", "Up to date"]);
    expect(out.map((o) => o.percent)).toEqual([0, 50, 99.9, null]);
  });

  it("a synced wallet that fell behind syncs again, and offline shows", () => {
    const out = run([[synced, 0], [{ ...syncing, phases: phases(300) }, 1000], [{ ...syncing, phases: phases(900) }, 2000], [{ ...synced, running: false }, 3000]]);
    expect(out.map((o) => o.text)).toEqual(["Up to date", "Synchronizing with network…", "Synchronizing with network…", "Not syncing"]);
    expect(out[1].percent).toBe(30);
  });

  it("is a gap in blocks, not in percent: mainnet-scale heights a few days behind are syncing", () => {
    const TIP = 2_380_000;
    const syncedMainnet = { ...base, caughtUp: true, phases: phases(TIP, TIP, true) };
    // One block behind: a new block. 1,700 blocks behind (about 3 days) is 99.93%, and is syncing.
    const behind = (blocks: number) => ({ ...syncing, phases: phases(TIP - blocks, TIP) });
    const out = run([[syncedMainnet, 0], [behind(1), 1000], [behind(SETTLE_BLOCKS), 2000], [behind(SETTLE_BLOCKS + 1), 3000], [behind(1700), 4000]]);
    expect(out.map((o) => o.text)).toEqual(["Up to date", "Up to date", "Up to date", "Synchronizing with network…", "Synchronizing with network…"]);
    expect(out[4].percent).toBeCloseTo(99.9, 1);
  });

  it("a stuck reading one block short stops being held after a minute, and does not hold itself up", () => {
    const stuck = { ...syncing, phases: phases(999, 1000) };
    const out = run([[synced, 0], [stuck, 1000], [stuck, 1000 + SETTLE_MS - 1], [stuck, 1000 + SETTLE_MS], [stuck, 1000 + SETTLE_MS + 5000]]);
    expect(out.map((o) => o.text)).toEqual(["Up to date", "Up to date", "Up to date", "Synchronizing with network…", "Synchronizing with network…"]);
    // Never synced: a reading within the gap is not held.
    expect(run([[stuck, 0], [stuck, 1000]]).map((o) => o.text)).toEqual(["Synchronizing with network…", "Synchronizing with network…"]);
  });
});
