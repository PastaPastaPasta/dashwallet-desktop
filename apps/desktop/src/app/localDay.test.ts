import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dateRange } from "../format/tx";
import { dayKey, msToNextMidnight, watchLocalDay } from "./localDay";

// U-03 D25: Activity's date bounds follow the local day.

describe("the local day", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 9, 14, 23, 59, 58));
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("measures the time to the next local midnight", () => {
    expect(msToNextMidnight(new Date())).toBe(2000);
    expect(dayKey(new Date())).toBe("2026-10-14");
  });

  it("changes at midnight, and the date bounds with it", () => {
    const clock = () => new Date();
    const before = dateRange("today", clock());
    const seen: string[] = [];
    const stop = watchLocalDay(() => seen.push(dayKey(clock())), clock);
    vi.advanceTimersByTime(1999);
    expect(seen).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(seen).toEqual(["2026-10-15"]);
    const after = dateRange("today", clock());
    expect(after.dateFrom! - before.dateFrom!).toBe(24 * 3600);
    // The next one is armed: a day later, again.
    vi.advanceTimersByTime(24 * 3600 * 1000);
    expect(seen).toEqual(["2026-10-15", "2026-10-16"]);
    stop();
    vi.advanceTimersByTime(48 * 3600 * 1000);
    expect(seen).toHaveLength(2);
  });

  it("reads the day again when the window comes back", () => {
    const win = new EventTarget();
    const doc = Object.assign(new EventTarget(), { visibilityState: "visible" });
    vi.stubGlobal("window", win);
    vi.stubGlobal("document", doc);
    const calls = vi.fn();
    const stop = watchLocalDay(calls, () => new Date());
    win.dispatchEvent(new Event("focus"));
    doc.dispatchEvent(new Event("visibilitychange"));
    expect(calls).toHaveBeenCalledTimes(2);
    doc.visibilityState = "hidden";
    doc.dispatchEvent(new Event("visibilitychange"));
    expect(calls).toHaveBeenCalledTimes(2);
    stop();
    win.dispatchEvent(new Event("focus"));
    expect(calls).toHaveBeenCalledTimes(2);
  });
});
