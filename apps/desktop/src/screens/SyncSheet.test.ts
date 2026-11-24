// Sync progress (IOS-023): one phase's line, and when the sheet says the sync is stuck.

import { describe, expect, it } from "vitest";
import type { SyncStatus } from "../api/generated/SyncStatus";
import { t } from "../i18n";
import { phaseText, stalledMinutes } from "./SyncSheet";

const phase = (over: object) => ({ phase: "filters" as const, currentHeight: 600, targetHeight: 1000, done: false, ...over });

describe("sync phases", () => {
  it("reads done, waiting, or current of target", () => {
    expect(phaseText(t, phase({ done: true }), "en-US")).toBe("Done");
    expect(phaseText(t, phase({ targetHeight: null }), "en-US")).toBe("Waiting");
    expect(phaseText(t, phase({}), "en-US")).toBe("600 of 1,000 (60.0%)");
    // Never "100%" for a phase that isn't done.
    expect(phaseText(t, phase({ currentHeight: 1000 }), "en-US")).toBe("1,000 of 1,000 (99.9%)");
  });

  it("calls a sync stuck after a minute without progress, only while it runs behind", () => {
    const s = { running: true, caughtUp: false, secondsSinceProgress: 59 } as SyncStatus;
    expect(stalledMinutes(s)).toBeNull();
    expect(stalledMinutes({ ...s, secondsSinceProgress: 125 })).toBe(2);
    expect(stalledMinutes({ ...s, secondsSinceProgress: 125, caughtUp: true })).toBeNull();
    expect(stalledMinutes({ ...s, secondsSinceProgress: 125, running: false })).toBeNull();
    // The engine's one-time stall reading at 45 s, then no event: the sheet's own clock carries it on.
    expect(stalledMinutes({ ...s, secondsSinceProgress: 45 }, 10)).toBeNull();
    expect(stalledMinutes({ ...s, secondsSinceProgress: 45 }, 20)).toBe(1);
  });
});
