import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SETTLE_MS } from "../format/sync";
import type { SyncStatus } from "./generated/SyncStatus";
import { feedSync, rawSync, resetSync, shownSync } from "./syncDisplay";

const owner = {};
const status = (caughtUp: boolean, behind = 0): SyncStatus => ({
  running: true,
  phases: [{ phase: "headers", currentHeight: 1000 - behind, targetHeight: 1000, done: caughtUp }],
  activePhase: null,
  tipHeight: 1000,
  tipTime: null,
  chainlockHeight: null,
  connectedPeers: 8,
  caughtUp,
  secondsSinceProgress: null,
});

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  resetSync(owner);
  vi.useRealTimers();
});

describe("DEC1058_syncHoldExpires", () => {
  it("a hold ends on its own timer, with no further reading, and the raw status is never touched", () => {
    feedSync(owner, status(true), 0);
    expect(shownSync(owner)?.caughtUp).toBe(true);
    const newBlock = status(false, 1);
    feedSync(owner, newBlock, 1000);
    expect(shownSync(owner)?.caughtUp).toBe(true);
    // The engine's reading is what it said.
    expect(rawSync(owner)).toBe(newBlock);
    expect(rawSync(owner)?.caughtUp).toBe(false);
    // Nothing more arrives; the deadline passes.
    vi.setSystemTime(1000 + SETTLE_MS);
    vi.advanceTimersByTime(SETTLE_MS);
    expect(shownSync(owner)?.caughtUp).toBe(false);
    expect(shownSync(owner)).toBe(newBlock);
  });

  it("a reading that is caught up again clears the hold, and its timer does nothing later", () => {
    feedSync(owner, status(true), 0);
    feedSync(owner, status(false, 1), 1000);
    feedSync(owner, status(true), 2000);
    vi.setSystemTime(1000 + SETTLE_MS + 5000);
    vi.advanceTimersByTime(SETTLE_MS + 5000);
    expect(shownSync(owner)?.caughtUp).toBe(true);
  });
});
