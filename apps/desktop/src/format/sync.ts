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
