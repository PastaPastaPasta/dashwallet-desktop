import { describe, expect, it } from "vitest";
import type { SyncStatus } from "../api/generated/SyncStatus";
import { isSyncing, settleSync, syncPercent, syncState } from "./sync";

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
  /** The status bar's states over a sequence of engine readings, as useSyncStatus feeds them. */
  const run = (readings: SyncStatus[]) => {
    let was = false;
    return readings.map((r) => {
      const s = settleSync(was, r);
      was = s.running && s.caughtUp;
      return { text: syncState(s), percent: isSyncing(s) ? syncPercent(s) : null };
    });
  };
  const synced = { ...base, caughtUp: true, phases: phases(1000, 1000, true) };

  it("a new block while synced stays Up to date, never Syncing 100%", () => {
    const newBlock = { ...base, caughtUp: false, phases: phases(1000, 1001) };
    const out = run([synced, newBlock, { ...newBlock, phases: phases(1001, 1001, true) }, synced, newBlock, synced]);
    expect(out.map((o) => o.text)).toEqual(Array(6).fill("Up to date"));
    expect(out.every((o) => o.percent === null)).toBe(true);
  });

  it("syncing from the start shows its progress, and never 100.0% before it is caught up", () => {
    const out = run([
      { ...base, phases: phases(0) },
      { ...base, phases: phases(500) },
      { ...base, phases: phases(1000, 1000, true) },
      synced,
    ]);
    expect(out.map((o) => o.text)).toEqual(["Synchronizing with network…", "Synchronizing with network…", "Synchronizing with network…", "Up to date"]);
    expect(out.map((o) => o.percent)).toEqual([0, 50, 99.9, null]);
  });

  it("a synced wallet that fell far behind syncs again, and offline shows", () => {
    const out = run([synced, { ...base, caughtUp: false, phases: phases(300) }, { ...base, caughtUp: false, phases: phases(900) }, { ...synced, running: false }]);
    expect(out.map((o) => o.text)).toEqual(["Up to date", "Synchronizing with network…", "Synchronizing with network…", "Not syncing"]);
    expect(out[1].percent).toBe(30);
  });
});
