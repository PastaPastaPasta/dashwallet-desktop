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

/** A synced wallet that falls this far behind (percent of the chain) is syncing again; less is a
 * new block arriving (DEC-1058 #5). */
export const RESYNC_BELOW_PERCENT = 99;

/** `s` as the status bar should show it, given whether the last reading was synced. A new block makes
 * the engine report `caughtUp: false` for a moment with every phase at or near 100%: while the wallet
 * was synced and the phases are still within `RESYNC_BELOW_PERCENT`, it stays caught up. A real
 * catch-up (offline for a while: far behind) and `running: false` still show. */
export function settleSync(wasSynced: boolean, s: SyncStatus): SyncStatus {
  if (!wasSynced || !s.running || s.caughtUp) return s;
  const percent = phasePercent(s);
  return percent !== null && percent >= RESYNC_BELOW_PERCENT ? { ...s, caughtUp: true } : s;
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
