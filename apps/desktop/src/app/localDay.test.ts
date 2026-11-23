import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dateRange } from "../format/tx";
import { dayKey, localKey, msToNextMidnight, watchLocalDay } from "./localDay";

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

  it("notices a day that changed while the machine slept", () => {
    vi.setSystemTime(new Date(2026, 9, 14, 10, 0, 0));
    const calls = vi.fn();
    const stop = watchLocalDay(calls, () => new Date());
    // Asleep for a day: the clock moved, the midnight timer (due in 14 h) did not run.
    vi.setSystemTime(new Date(2026, 9, 15, 10, 0, 0));
    vi.advanceTimersByTime(60_000);
    expect(calls).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(10 * 60_000);
    expect(calls).toHaveBeenCalledTimes(1);
    stop();
  });

  it("a new timezone on the same date changes the key and the bounds", () => {
    const tz = process.env.TZ;
    try {
      process.env.TZ = "Europe/Berlin";
      vi.setSystemTime(new Date(2026, 9, 14, 12, 0, 0));
      const clock = () => new Date();
      const win = new EventTarget();
      vi.stubGlobal("window", win);
      const before = { key: localKey(clock()), range: dateRange("today", clock()) };
      const calls = vi.fn();
      const stop = watchLocalDay(calls, clock);
      // Flown to New York while away: still 14 October there (06:00).
      process.env.TZ = "America/New_York";
      expect(dayKey(clock())).toBe("2026-10-14");
      win.dispatchEvent(new Event("focus"));
      expect(calls).toHaveBeenCalledTimes(1);
      expect(localKey(clock())).not.toBe(before.key);
      expect(dateRange("today", clock()).dateFrom! - before.range.dateFrom!).toBe(6 * 3600);
      // Without the focus, the minute's check sees it too.
      process.env.TZ = "Asia/Tokyo";
      vi.advanceTimersByTime(60_000);
      expect(calls).toHaveBeenCalledTimes(2);
      stop();
    } finally {
      process.env.TZ = tz;
      if (tz === undefined) delete process.env.TZ;
    }
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
