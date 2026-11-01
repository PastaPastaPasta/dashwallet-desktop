import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SETTLE_MS } from "../format/sync";
import type { SyncStatus } from "./generated/SyncStatus";
import { announceable, feedSync, rawSync, readTicket, resetSync, shownSync } from "./syncDisplay";

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

describe("DEC1058_staleSyncReply", () => {
  it("a read that started before an event is dropped when it answers, and the hold's timer stays", () => {
    feedSync(owner, status(true), 0);
    const ticket = readTicket(owner); // the read starts: its snapshot says caught up
    const behind = status(false, 1);
    expect(feedSync(owner, behind, 1000)).toBe(true); // an event: one block behind
    expect(feedSync(owner, status(true), 2000, ticket)).toBe(false); // the old reply arrives
    expect(rawSync(owner)).toBe(behind);
    expect(shownSync(owner)?.caughtUp).toBe(true); // still the hold
    vi.setSystemTime(1000 + SETTLE_MS);
    vi.advanceTimersByTime(SETTLE_MS);
    expect(shownSync(owner)?.caughtUp).toBe(false);
  });

  it("of two reads, the one that started later wins whichever answers first", () => {
    const first = readTicket(owner);
    const second = readTicket(owner);
    const newer = status(false, 5);
    expect(feedSync(owner, newer, 0, second)).toBe(true);
    expect(feedSync(owner, status(true), 1, first)).toBe(false);
    expect(rawSync(owner)).toBe(newer);
  });

  it("a read nothing overtook is applied", () => {
    expect(feedSync(owner, status(true), 0, readTicket(owner))).toBe(true);
    expect(rawSync(owner)?.caughtUp).toBe(true);
  });
});

describe("DEC1058_announceable", () => {
  it("is false before any reading, and for a wallet that starts one block behind", () => {
    expect(announceable(owner, 0)).toBe(false);
    feedSync(owner, status(false, 1), 0);
    expect(announceable(owner, 1000)).toBe(false);
  });

  it("is true when caught up, and for a minute after a caught-up wallet is a block behind", () => {
    feedSync(owner, status(true), 0);
    expect(announceable(owner, 100)).toBe(true);
    feedSync(owner, status(false, 1), 1000);
    expect(announceable(owner, 1000 + SETTLE_MS - 1)).toBe(true);
    expect(announceable(owner, 1000 + SETTLE_MS)).toBe(false);
  });

  it("is false when the wallet is far behind, even after being caught up", () => {
    feedSync(owner, status(true), 0);
    feedSync(owner, status(false, 5000), 1000);
    expect(announceable(owner, 1100)).toBe(false);
  });
});
