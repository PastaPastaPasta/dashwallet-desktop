// The network switch (row 74): `switch_network` moves the window to the other network, then the
// page boots again, so no query, store, model or listener of the network it left survives. A
// refusal changes nothing and is said under the picker; a switch that closed the network (it went
// through, or it failed after the close) always boots again, with a note for the next page.

import type { Backend } from "../api/backend";
import { errorMessage, isAppError } from "../api/backend";
import type { ChosenNetwork } from "../api/generated/ChosenNetwork";
import { FLOW_NAME } from "../api/lockReport";
import { releaseListeners } from "../api/tauri";
import { prefsWritten } from "../settings/prefs";
import { NetworkText } from "./choice";

/** Where a failed switch leaves its note for the page that boots after it. */
const NOTE_KEY = "dw-network-switch-note";

/** The note a failed switch left for this page, once. */
export function takeSwitchNote(): string | null {
  try {
    const note = sessionStorage.getItem(NOTE_KEY);
    if (note !== null) sessionStorage.removeItem(NOTE_KEY);
    return note;
  } catch {
    return null;
  }
}

function leaveSwitchNote(note: string) {
  try {
    sessionStorage.setItem(NOTE_KEY, note);
  } catch {
    // No storage: the page boots without the note.
  }
}

/** What a switch to `target` that failed with `e` means: a refusal said in place (nothing changed),
 * or a failure after the close, which boots the page again with its note. */
export function switchOutcome(e: unknown, target: ChosenNetwork, from: ChosenNetwork): { problem: string } | { reboot: string } {
  if (isAppError(e)) {
    const data = e.data;
    if (e.code === "app.network_busy" && data?.kind === "networkBusy") {
      const waits = [...(data.broadcast ? ["the payment"] : []), ...data.flows.map((f) => FLOW_NAME[f])];
      return { problem: NetworkText.busy(waits, data.mixing) };
    }
    if (e.code === "wallet") return { problem: NetworkText.walletOpening };
    if (e.code === "shielded_spend_unresolved") return { problem: NetworkText.shieldedUnresolved };
    if (e.code === "app.network_switch_failed" && data?.kind === "networkSwitch")
      return { reboot: data.rolledBack ? NetworkText.backOn(target, from) : NetworkText.noneOpen(target, from) };
  }
  return { problem: `${NetworkText.notSwitched} ${errorMessage(e)}` };
}

/** Boots the page again on the window's network: after the settings write in flight, with every
 * event subscription ended (Tauri keeps a listener's entry across a reload otherwise). */
export async function bootAgain(note: string | null = null, reload: () => void = () => location.reload()) {
  if (note !== null) leaveSwitchNote(note);
  await prefsWritten();
  await releaseListeners();
  reload();
}

/** Whether a wallet of this network has a transfer or a lock that is not finished: Core →
 * Shielded locks, "Add from Dash wallet" locks not applied, top-ups pending or stuck. A read that
 * fails counts as none (the line is a note, not a gate). */
export async function hasPendingLocks(backend: Backend, walletIds: string[]): Promise<boolean> {
  const some = <T>(read: Promise<T[] | null>) => read.then((rows) => (rows?.length ?? 0) > 0).catch(() => false);
  const checks = walletIds.flatMap((id) => [
    backend
      .unfinishedTransfers(id)
      .then((t) => (t?.locks.length ?? 0) > 0)
      .catch(() => false),
    some(backend.pendingPlatformFunds(id)),
    some(backend.creditsPendingTopUps(id)),
    some(backend.creditsStuckTopUpFundings(id)),
  ]);
  return (await Promise.all(checks)).some(Boolean);
}
