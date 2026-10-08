import { describe, expect, it } from "vitest";
import type { SyncStatus } from "../api/generated/SyncStatus";
import { syncPercent } from "./sync";

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
