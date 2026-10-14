// The local calendar day, for Activity's date filters (U-03 D25): "Today", "This week", "This
// month" and "This year" all move at a local midnight, so their bounds are taken again then, and
// when the window comes back (a suspended machine fires no timers while it sleeps).

import { useCallback, useSyncExternalStore } from "react";

export const dayKey = (d: Date) => `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`;

/** Milliseconds from `now` to the next local midnight. */
export function msToNextMidnight(now: Date): number {
  return new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1).getTime() - now.getTime();
}

/** Calls `onChange` at every local midnight by `clock`, and when the window regains focus or
 * becomes visible. Returns the unsubscribe. */
export function watchLocalDay(onChange: () => void, clock: () => Date): () => void {
  let timer: ReturnType<typeof setTimeout>;
  const arm = () => {
    timer = setTimeout(() => {
      onChange();
      arm();
    }, msToNextMidnight(clock()));
  };
  arm();
  const back = () => {
    if (typeof document === "undefined" || document.visibilityState !== "hidden") onChange();
  };
  if (typeof window !== "undefined") window.addEventListener("focus", back);
  if (typeof document !== "undefined") document.addEventListener("visibilitychange", back);
  return () => {
    clearTimeout(timer);
    if (typeof window !== "undefined") window.removeEventListener("focus", back);
    if (typeof document !== "undefined") document.removeEventListener("visibilitychange", back);
  };
}

/** The local day by `clock`, re-read at midnight and when the window comes back. */
export function useLocalDay(clock: () => Date): string {
  const subscribe = useCallback((onChange: () => void) => watchLocalDay(onChange, clock), [clock]);
  return useSyncExternalStore(subscribe, () => dayKey(clock()));
}
