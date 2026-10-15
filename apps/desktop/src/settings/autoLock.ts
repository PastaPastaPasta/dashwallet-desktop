// Auto lock (IOS-015, U-04 D12): after the chosen time without keyboard, pointer or wheel input
// in the window, the vault locks as the Lock button locks it. Armed only while an encrypted vault
// holds its key (E0-04 §4.8: auto lock is `lock_vault`, armed only while the key is held); a
// screen showing a flow's progress is not activity, so it does not hold the lock off.

import type { LockStateName } from "../api/generated/LockStateName";

/** The input that counts as activity. */
export const ACTIVITY_EVENTS = ["keydown", "pointerdown", "pointermove", "wheel"] as const;

export interface AutoLockClock {
  now(): number;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(id: unknown): void;
}

const SYSTEM: AutoLockClock = {
  now: () => Date.now(),
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
  clearTimeout: (id) => globalThis.clearTimeout(id as ReturnType<typeof setTimeout>),
};

/** Whether auto lock runs for a vault in `state` (encrypted, its key held). */
export const armedFor = (state: LockStateName | undefined, encrypted: boolean | undefined) =>
  encrypted === true && (state === "unlocked" || state === "unlockedMixingOnly");

/** The timer itself: `arm` with the wait and the lock call, `activity` on input, `disarm` when the
 * vault locks or the setting goes to Never. The timer checks the last input when it fires rather
 * than restarting on every event, so a moving pointer costs no timer churn. */
export class AutoLockTimer {
  #secs: number | null = null;
  #onExpire: (() => void) | null = null;
  #last = 0;
  #timer: unknown = null;

  constructor(private readonly clock: AutoLockClock = SYSTEM) {}

  get armed(): boolean {
    return this.#secs !== null;
  }

  arm(secs: number, onExpire: () => void) {
    this.disarm();
    this.#secs = secs;
    this.#onExpire = onExpire;
    this.#last = this.clock.now();
    this.#schedule(secs * 1000);
  }

  disarm() {
    if (this.#timer !== null) this.clock.clearTimeout(this.#timer);
    this.#timer = null;
    this.#secs = null;
    this.#onExpire = null;
  }

  activity() {
    if (this.#secs !== null) this.#last = this.clock.now();
  }

  #schedule(ms: number) {
    this.#timer = this.clock.setTimeout(() => this.#fire(), ms);
  }

  #fire() {
    this.#timer = null;
    if (this.#secs === null) return;
    const left = this.#last + this.#secs * 1000 - this.clock.now();
    if (left > 0) return this.#schedule(left);
    const expire = this.#onExpire;
    this.disarm();
    expire?.();
  }
}
