// What the status bar shows of the sync (DEC-1058 #5), kept apart from the sync itself. The raw
// status is the engine's, in the query cache (`keys.sync`) and in `sync` events; `settleSync` smooths
// the one-block flap of a new block for display only, and its hold ends on a timer of its own, not
// on whichever reading comes next. A reading is ordered by when it was asked for (`readTicket`) or,
// for an engine event, by when it came: an older reply never replaces a newer reading, and never
// clears the timer of the hold the newer one started. `announceable` is the same machine, kept for
// notifications: it decides whether a payment found "while catching up" is a new block's.

import { useSyncExternalStore } from "react";
import { NEWS_MS, NOT_SETTLED, SETTLE_MS, settleSync, type Settled } from "../format/sync";
import type { SyncStatus } from "./generated/SyncStatus";

interface Entry {
  raw: SyncStatus | null;
  /** What `settleSync` remembers for the display. */
  settled: Settled;
  /** The same machine for notifications (`announceable`): it never feeds what is shown. */
  news: Settled;
  /** Tickets handed out (requests started, events arrived) and the newest one applied. */
  issued: number;
  applied: number;
  shown: SyncStatus | null;
  timer: ReturnType<typeof setTimeout> | undefined;
  listeners: Set<() => void>;
}

/** One entry per backend. */
const entries = new WeakMap<object, Entry>();

function entry(owner: object): Entry {
  let e = entries.get(owner);
  if (!e) {
    e = { raw: null, settled: NOT_SETTLED, news: NOT_SETTLED, issued: 0, applied: 0, shown: null, timer: undefined, listeners: new Set() };
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

/** Starts a read of `sync_status`: its reply is a reading as of now, and is dropped if an event or a
 * newer read was applied meanwhile. */
export function readTicket(owner: object): number {
  return ++entry(owner).issued;
}

/** A raw reading from the engine. A `sync` event (no `ticket`) is the newest thing there is when it
 * arrives. A read's reply passes the `ticket` it started with and is refused when something newer was
 * applied since (the reply is as old as the request); returns whether it was applied. */
export function feedSync(owner: object, raw: SyncStatus, now: number = Date.now(), ticket?: number): boolean {
  const e = entry(owner);
  const mine = ticket ?? ++e.issued;
  if (mine <= e.applied) return false;
  e.applied = mine;
  e.raw = raw;
  e.news = settleSync(e.news, raw, now, NEWS_MS).settled;
  recompute(e, now);
  return true;
}

/** Whether a payment the engine calls catch-up is a new block's, to announce: the raw reading is
 * caught up, or has been at most a few blocks short of a caught-up one for less than `NEWS_MS` (longer
 * than the display's hold: a slow block's payment is still news after the bar stopped holding).
 * Without a caught-up reading first (a wallet that starts near the tip), it is history. */
export function announceable(owner: object, now: number = Date.now()): boolean {
  const e = entries.get(owner);
  if (!e?.raw) return false;
  if (e.raw.running && e.raw.caughtUp) return true;
  return e.news.synced && e.news.heldSince !== null && now - e.news.heldSince < NEWS_MS;
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
