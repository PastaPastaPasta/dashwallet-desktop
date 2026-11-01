// What the status bar shows of the sync (DEC-1058 #5), kept apart from the sync itself. The raw
// status is the engine's, in the query cache (`keys.sync`) and in `sync` events; `settleSync` smooths
// the one-block flap of a new block for display only, and its hold ends on a timer of its own, not
// on whichever reading comes next. Nothing that decides anything (notifications, gates) reads this.

import { useSyncExternalStore } from "react";
import { NOT_SETTLED, SETTLE_MS, settleSync, type Settled } from "../format/sync";
import type { SyncStatus } from "./generated/SyncStatus";

interface Entry {
  raw: SyncStatus | null;
  settled: Settled;
  shown: SyncStatus | null;
  timer: ReturnType<typeof setTimeout> | undefined;
  listeners: Set<() => void>;
}

/** One entry per backend. */
const entries = new WeakMap<object, Entry>();

function entry(owner: object): Entry {
  let e = entries.get(owner);
  if (!e) {
    e = { raw: null, settled: NOT_SETTLED, shown: null, timer: undefined, listeners: new Set() };
    entries.set(owner, e);
  }
  return e;
}

/** Works the shown status out of the last raw one, and arms the end of a hold. */
function recompute(e: Entry, now: number) {
  clearTimeout(e.timer);
  e.timer = undefined;
  if (!e.raw) return;
  const { status, settled } = settleSync(e.settled, e.raw, now);
  e.settled = settled;
  const changed = status !== e.shown;
  e.shown = status;
  // A hold ends at its deadline whatever the engine says or does not say.
  if (settled.heldSince !== null) {
    e.timer = setTimeout(() => {
      recompute(e, Date.now());
      e.listeners.forEach((l) => l());
    }, Math.max(0, settled.heldSince + SETTLE_MS - now));
  }
  if (changed) e.listeners.forEach((l) => l());
}

/** A raw reading from the engine: a read of `sync_status` or a `sync` event. */
export function feedSync(owner: object, raw: SyncStatus, now: number = Date.now()) {
  const e = entry(owner);
  e.raw = raw;
  recompute(e, now);
}

/** The status to show: the last reading, smoothed; `undefined` before the first. */
export function shownSync(owner: object): SyncStatus | undefined {
  return entries.get(owner)?.shown ?? undefined;
}

/** The raw status, for what decides rather than shows. */
export function rawSync(owner: object): SyncStatus | undefined {
  return entries.get(owner)?.raw ?? undefined;
}

/** Tests: forget the readings of `owner`. */
export function resetSync(owner: object) {
  const e = entries.get(owner);
  if (e) clearTimeout(e.timer);
  entries.delete(owner);
}

export function useShownSync(owner: object): SyncStatus | undefined {
  const e = entry(owner);
  return useSyncExternalStore(
    (l) => {
      e.listeners.add(l);
      return () => void e.listeners.delete(l);
    },
    () => e.shown ?? undefined,
  );
}
