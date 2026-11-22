// Shielded while it is syncing (DEC-1058 #8, UI half). The engine reports `ShieldedStatus.ready`:
// the keys are bound and the notes restored or scanned. Until then a balance is no reading, a spend
// is refused (`shielded.not_synced`), and a shield or the address read can come back as the generic
// Platform refusal. Everything the screens ask about this goes through this file, so the engine half
// (a field or a code E1 names) is one change here.
//
// STUB, to wire at integration: nothing in the contract separates "still syncing" from "Platform
// really did not answer" for `shield_from_core` / `shield_quote` (`platform.unavailable`). Until E1
// maps that case to `shielded.not_synced`, a platform refusal while the status says not ready is
// shown as syncing (`syncingRefusal`).

import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import { SHIELDED_CODES } from "../api/pending";
import { ago } from "../governance/model";

export const SHIELDED_SYNCING_TEXT = "Shielded is still syncing; try again when it finishes.";

/** Shielded is on this network and its account is not established yet: Receive, Add, Send and Move
 * wait. `null` (no command, or not read yet) is not syncing: the card is not shown then. */
export function shieldedSyncing(status: ShieldedStatus | null | undefined): boolean {
  return !!status && status.active && !status.ready;
}

/** The error codes a not-yet-synced account makes the engine answer with, besides `not_synced`. */
const PLATFORM_REFUSALS = ["platform.unavailable", "platform.timeout", "platform.context_unavailable"];

/** Whether the refusal `code` is Shielded syncing: its own code, or a Platform refusal while the
 * status says the account is not established. */
export function syncingRefusal(code: string, syncing: boolean): boolean {
  return code === SHIELDED_CODES.notSynced || (syncing && PLATFORM_REFUSALS.includes(code));
}

export const SHIELDED_SYNC_COPY = {
  scanning: "Scanning…",
  /** When the last pass ended, whatever its result (the engine stamps a failed pass too), or when an
   * idle pool was last proved unchanged and the pass skipped. */
  lastChecked: (when: string) => `Last checked ${when}`,
} as const;

/** The sheet's sync line at `now` (Unix seconds): "Scanning…" while a pass runs, else when the last
 * pass ended or the idle pool was last proved unchanged; `null` before a scan has completed, or while
 * Shielded is off. */
export function shieldedSyncText(status: ShieldedStatus | null | undefined, now: number): string | null {
  if (!status?.active) return null;
  if (status.syncing) return SHIELDED_SYNC_COPY.scanning;
  if (status.syncedIndex === null || status.lastSyncAt === null) return null;
  return SHIELDED_SYNC_COPY.lastChecked(ago(status.lastSyncAt, now));
}
