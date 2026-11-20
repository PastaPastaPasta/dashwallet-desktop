// What the renderer keeps while the vault is locked: the lock screen's own reads, and the
// network the window is on. Every other query may hold wallet content (balances, names, history,
// contacts, addresses, notifications), so a lock drops it and the pages read it again once they
// mount after the unlock.

import type { QueryClient, QueryKey } from "@tanstack/react-query";
import { keys } from "./queries";

const KEPT = new Set<unknown>([keys.network[0], keys.lockStatus[0], keys.lastLockReport[0], keys.lockReconcile[0], keys.lockDrain[0]]);

export const keptWhileLocked = (key: QueryKey): boolean => KEPT.has(key[0]);

/** Removes every query the lock screen does not read. A read still under way is cancelled with
 * its query, so its answer lands nowhere. */
export function forgetWalletContent(client: QueryClient) {
  client.removeQueries({ predicate: (q) => !keptWhileLocked(q.queryKey) });
}
