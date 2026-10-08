import { describe, expect, it } from "vitest";
import type { SyncStatus } from "../api/generated/SyncStatus";
import { isSyncing, syncPercent, syncState } from "./sync";

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
