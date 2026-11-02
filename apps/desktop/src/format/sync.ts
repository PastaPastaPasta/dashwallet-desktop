import type { SyncStatus } from "../api/generated/SyncStatus";

/** Overall sync progress, 0–100, as the mean of the phases that report a target. `null`
 * before any phase reports one. Raw: the engine does not damp, and neither does this. */
export function syncPercent(s: SyncStatus): number | null {
  if (s.caughtUp) return 100;
  const raw = phasePercent(s);
  // Never "100%" while the engine says it is not caught up: that reads as a bug.
  return raw === null ? null : Math.min(raw, MAX_SYNCING_PERCENT);
}

/** The mean of the phases that report a target, as the engine has them (up to 100). */
function phasePercent(s: SyncStatus): number | null {
  const known = s.phases.filter((p) => p.targetHeight !== null && p.targetHeight > 0);
  if (known.length === 0) return null;
  const sum = known.reduce((acc, p) => acc + (p.done ? 1 : Math.min(1, (p.currentHeight ?? 0) / p.targetHeight!)), 0);
  return (sum / known.length) * 100;
}

/** The most a not-caught-up sync shows. */
export const MAX_SYNCING_PERCENT = 99.9;

/** A synced wallet whose phases are this many blocks or fewer behind their targets has had a new block
 * arrive; more is syncing (DEC-1058 #5). Blocks, not a share of the chain: 1% of mainnet is days. */
export const SETTLE_BLOCKS = 3;

/** And a reading that is not caught up for longer than this is not a new block, whatever its gap. */
export const SETTLE_MS = 60_000;

/** What decides a payment's announcement is a longer allowance than the display's: a block that takes
 * the engine minutes to process still has its payments announced (the status bar stops holding at
 * `SETTLE_MS`, a new payment is still news for this long after the last caught-up reading). */
export const NEWS_MS = 10 * 60_000;

/** What `settleSync` remembers between readings. */
export interface Settled {
  /** The last reading was caught up, or a new block that `settleSync` held at synced. */
  synced: boolean;
  /** Since when readings were held at synced without the engine saying caught up (ms); `null` if not. */
  heldSince: number | null;
}

export const NOT_SETTLED: Settled = { synced: false, heldSince: null };

/** Whether every phase that reports a target is within `blocks` of it. */
function within(s: SyncStatus, blocks: number): boolean {
  const known = s.phases.filter((p) => p.targetHeight !== null && p.targetHeight > 0);
  return known.length > 0 && known.every((p) => p.done || p.targetHeight! - (p.currentHeight ?? 0) <= blocks);
}

/** `s` as the status bar should show it, and what to remember for the next reading. A new block makes
 * the engine report `caughtUp: false` for a moment with every phase a block from its target: while
 * the wallet was synced, it stays caught up if the phases are within `SETTLE_BLOCKS` and it has not
 * been so for `windowMs` (`SETTLE_MS`). A real catch-up (far behind, or stuck) and `running: false` show as they are. */
export function settleSync(prev: Settled, s: SyncStatus, now: number, windowMs: number = SETTLE_MS): { status: SyncStatus; settled: Settled } {
  if (s.running && s.caughtUp) return { status: s, settled: { synced: true, heldSince: null } };
  const heldSince = prev.heldSince ?? now;
  if (prev.synced && s.running && within(s, SETTLE_BLOCKS) && now - heldSince < windowMs) {
    return { status: { ...s, caughtUp: true }, settled: { synced: true, heldSince } };
  }
  return { status: s, settled: NOT_SETTLED };
}

/** SPV is running and not caught up yet. */
export const isSyncing = (s: SyncStatus) => s.running && !s.caughtUp;

/** The status bar's state text (dash-qt strings). The percentage is not part of it: a live
 * region that changed with every percent would make screen readers repeat it. */
export function syncState(s: SyncStatus | undefined): string {
  if (!s) return "Connecting…";
  if (!s.running) return "Not syncing";
  return s.caughtUp ? "Up to date" : "Synchronizing with network…";
}
