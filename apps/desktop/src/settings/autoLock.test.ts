// Auto lock (IOS-015, U-04 D12): armed only while an encrypted vault holds its key, restarted by
// input, and firing once.

import { describe, expect, it } from "vitest";
import { armedFor, AutoLockTimer, type AutoLockClock } from "./autoLock";

function clock() {
  let now = 0;
  const timers = new Map<number, { at: number; fn: () => void }>();
  let next = 1;
  const c: AutoLockClock & { advance(ms: number): void; pending(): number } = {
    now: () => now,
    setTimeout: (fn, ms) => {
      timers.set(next, { at: now + ms, fn });
      return next++;
    },
    clearTimeout: (id) => void timers.delete(id as number),
    advance(ms) {
      const end = now + ms;
      for (;;) {
        const due = [...timers.entries()].filter(([, t]) => t.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
        if (!due) break;
        timers.delete(due[0]);
        now = due[1].at;
        due[1].fn();
      }
      now = end;
    },
    pending: () => timers.size,
  };
  return c;
}

describe("auto lock", () => {
  it("IOS015_armedOnlyWhileAnEncryptedVaultHoldsItsKey", () => {
    expect(armedFor("unlocked", true)).toBe(true);
    expect(armedFor("unlockedMixingOnly", true)).toBe(true);
    expect(armedFor("locked", true)).toBe(false);
    expect(armedFor("unencrypted", false)).toBe(false);
    expect(armedFor("noVault", false)).toBe(false);
    expect(armedFor(undefined, undefined)).toBe(false);
  });

  it("IOS015_locksAfterTheWaitWithoutInput", () => {
    const c = clock();
    const t = new AutoLockTimer(c);
    let locks = 0;
    t.arm(60, () => locks++);
    c.advance(59_000);
    expect(locks).toBe(0);
    c.advance(1_000);
    expect(locks).toBe(1);
    expect(t.armed).toBe(false);
    c.advance(120_000);
    expect(locks).toBe(1);
  });

  it("IOS015_inputRestartsTheWait", () => {
    const c = clock();
    const t = new AutoLockTimer(c);
    let locks = 0;
    t.arm(60, () => locks++);
    c.advance(50_000);
    t.activity();
    c.advance(50_000);
    expect(locks).toBe(0);
    // Many events cost no new timers: the one timer checks the last input when it fires.
    for (let i = 0; i < 100; i++) t.activity();
    expect(c.pending()).toBe(1);
    c.advance(60_000);
    expect(locks).toBe(1);
  });

  it("IOS015_disarmingStopsIt", () => {
    const c = clock();
    const t = new AutoLockTimer(c);
    let locks = 0;
    t.arm(60, () => locks++);
    t.disarm();
    t.activity();
    c.advance(600_000);
    expect(locks).toBe(0);
    expect(c.pending()).toBe(0);
  });
});
