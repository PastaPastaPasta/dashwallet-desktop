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
import { MAX_SYNCING_PERCENT } from "../format/sync";
import { t } from "../i18n";

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

/** The first scan's share of the pool's note tree, 0–99.9, while Shielded is not established yet;
 * `null` before the pass knows the pool's size, or once established. The tree is the whole pool's,
 * not the user's notes, so only the share is shown. */
export function shieldedScanPercent(status: ShieldedStatus | null | undefined): number | null {
  if (!status?.active || status.ready || !status.scan || status.scan.total <= 0) return null;
  return Math.min((status.scan.checked / status.scan.total) * 100, MAX_SYNCING_PERCENT);
}

/** What Receive in Shielded can show: the address (bound; the first scan may still run), a wait
 * while the key binds, or why there is none: the first bind needs an unlock (`locked`, also a
 * mixing-only unlock), or the wallet is watch-only. */
export type ShieldedReceiveGate = "address" | "binding" | "unlock" | "watchOnly";

export function shieldedReceiveGate(status: ShieldedStatus | null | undefined): ShieldedReceiveGate {
  switch (status?.account) {
    case "locked":
    case "mixingOnly":
      return "unlock";
    case "watchOnly":
      return "watchOnly";
    case "binding":
      return "binding";
    default:
      return "address";
  }
}

/** Why Send, Add and Transfer wait while the account is not established: the vault's lock or a
 * watch-only wallet when that holds the viewing key's first bind, else the sync itself. */
export type ShieldedWait = "syncing" | "locked" | "mixingOnly" | "watchOnly";

/** `null` once established, or while Shielded is not active (`shieldedSyncing`). */
export function shieldedWait(status: ShieldedStatus | null | undefined): ShieldedWait | null {
  if (!status || !shieldedSyncing(status)) return null;
  const a = status.account;
  return a === "locked" || a === "mixingOnly" || a === "watchOnly" ? a : "syncing";
}

/** The line for `shieldedWait`, or `null`. */
export const shieldedWaitText = (status: ShieldedStatus | null | undefined): string | null => {
  const wait = shieldedWait(status);
  return wait && SHIELDED_SYNC_COPY.wait(wait);
};

export const SHIELDED_SYNC_COPY = {
  get syncing() {
    return t("transfer.shielded.sync.syncing");
  },
  wait: (wait: ShieldedWait) => t(`transfer.shielded.sync.${wait}`),
  /** `percent` formatted, or `null`: "Syncing Shielded…" alone. */
  progress: (percent: string | null) => (percent === null ? t("shell.status.syncingShielded") : t("transfer.shielded.sync.progress", { percent })),
  get scanning() {
    return t("transfer.shielded.sync.scanning");
  },
  /** When the last pass ended, whatever its result (the engine stamps a failed pass too), or when an
   * idle pool was last proved unchanged and the pass skipped. */
  lastChecked: (when: string) => t("transfer.shielded.sync.lastChecked", { when }),
};

/** The sheet's sync line at `now` (Unix seconds): "Scanning…" while a pass runs, else when the last
 * pass ended or the idle pool was last proved unchanged; `null` before a scan has completed, or while
 * Shielded is off. */
export function shieldedSyncText(status: ShieldedStatus | null | undefined, now: number): string | null {
  if (!status?.active) return null;
  if (status.syncing) return SHIELDED_SYNC_COPY.scanning;
  if (status.syncedIndex === null || status.lastSyncAt === null) return null;
  return SHIELDED_SYNC_COPY.lastChecked(ago(status.lastSyncAt, now));
}
