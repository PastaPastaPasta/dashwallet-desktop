import type { SyncStatus } from "../api/generated/SyncStatus";

/** Overall sync progress, 0–100, as the mean of the phases that report a target. `null`
 * before any phase reports one. Raw: the engine does not damp, and neither does this. */
export function syncPercent(s: SyncStatus): number | null {
  if (s.caughtUp) return 100;
  const known = s.phases.filter((p) => p.targetHeight !== null && p.targetHeight > 0);
  if (known.length === 0) return null;
  const sum = known.reduce((acc, p) => acc + (p.done ? 1 : Math.min(1, (p.currentHeight ?? 0) / p.targetHeight!)), 0);
  return (sum / known.length) * 100;
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
