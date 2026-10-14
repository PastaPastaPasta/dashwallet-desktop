// The local calendar day, for Activity's date filters (U-03 D25): "Today", "This week", "This
// month" and "This year" all move at a local midnight, so their bounds are taken again then, and
// when the window comes back or a minute's check sees the day or the timezone changed (a
// suspended machine fires no timers while it sleeps).

import { useCallback, useSyncExternalStore } from "react";

export function dayKey(d: Date): string {
  return `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`;
}

/** The local day and the timezone it is in: a new timezone moves every bound too, even on the
 * same date (Sol r2). */
export function localKey(d: Date): string {
  return `${dayKey(d)}|${Intl.DateTimeFormat().resolvedOptions().timeZone}`;
}

/** Milliseconds from `now` to the next local midnight. */
export function msToNextMidnight(now: Date): number {
  return new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1).getTime() - now.getTime();
}

/** Calls `onChange` at every local midnight by `clock`, and when the window regains focus or
 * becomes visible. Returns the unsubscribe. */
export function watchLocalDay(onChange: () => void, clock: () => Date): () => void {
  let day = localKey(clock());
  let timer: ReturnType<typeof setTimeout>;
  const arm = () => {
    timer = setTimeout(() => {
      day = localKey(clock());
      onChange();
      arm();
    }, msToNextMidnight(clock()));
  };
  arm();
  // A suspended machine's timers stop with it, and a resume need not move the focus: a check
  // every minute catches the day changing meanwhile.
  const poll = setInterval(() => {
    if (localKey(clock()) === day) return;
    day = localKey(clock());
    onChange();
  }, 60_000);
  // Absent outside a browser (unit tests).
  const win = typeof window === "undefined" ? undefined : window;
  const doc = typeof document === "undefined" ? undefined : document;
  const back = () => {
    if (doc?.visibilityState !== "hidden") onChange();
  };
  win?.addEventListener("focus", back);
  doc?.addEventListener("visibilitychange", back);
  return () => {
    clearTimeout(timer);
    clearInterval(poll);
    win?.removeEventListener("focus", back);
    doc?.removeEventListener("visibilitychange", back);
  };
}

/** The local day and timezone by `clock` (`localKey`), re-read at midnight and when the window
 * comes back. */
export function useLocalDay(clock: () => Date): string {
  const subscribe = useCallback((onChange: () => void) => watchLocalDay(onChange, clock), [clock]);
  return useSyncExternalStore(subscribe, () => localKey(clock()));
}
