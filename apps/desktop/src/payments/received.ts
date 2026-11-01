// Payments that arrived while the app was open (DEC-1058 #6). The engine says so with
// `newTransactions` (live, not a sync catch-up) and `tx_notices` names the rows; this keeps those
// rows for the bell (an entry and its unread badge), the app-wide "Received X DASH" toast and
// Receive's received state. Nothing here is persisted: the history is Activity's, and what was
// missed while the app was closed is not announced again.

import { useSyncExternalStore } from "react";
import type { Backend } from "../api/backend";
import type { TxNoticeRow } from "../api/generated/TxNoticeRow";

export interface ReceivedPayment {
  /** `txid:recordIndex`. */
  id: string;
  walletId: string;
  txid: string;
  duffs: number;
  /** The address it paid, when the engine names one. */
  address: string | null;
  /** When the app learned of it (ms). */
  at: number;
  read: boolean;
}

/** Most kept per app run: an old payment is in Activity. */
const MAX_KEPT = 50;

/** Record types that are a payment to the wallet: dash-qt's "Incoming transaction" popups. Mixing,
 * dust and rewards are not announced. */
const INCOMING_TYPES: ReadonlySet<string> = new Set(["recvWithAddress", "recvFromOther"]);

/** Whether a notice row is money somebody paid the wallet. */
export const isReceivedPayment = (row: TxNoticeRow): boolean => row.amount > 0 && !row.coinjoinInternal && INCOMING_TYPES.has(row.txType);

let list: readonly ReceivedPayment[] = [];
const listeners = new Set<() => void>();
const announced = new Set<(batch: ReceivedPayment[]) => void>();

const changed = () => listeners.forEach((l) => l());

/** Keeps the payments among `rows` that are new, newest first, and tells `onReceived` listeners. */
export function recordReceived(walletId: string, rows: readonly TxNoticeRow[], now: number = Date.now()): ReceivedPayment[] {
  const fresh = rows
    .filter(isReceivedPayment)
    .map((r): ReceivedPayment => ({ id: `${r.txid}:${r.recordIndex}`, walletId, txid: r.txid, duffs: r.amount, address: r.address, at: now, read: false }))
    .filter((p) => !list.some((q) => q.walletId === walletId && q.id === p.id));
  if (fresh.length === 0) return [];
  list = [...fresh, ...list].slice(0, MAX_KEPT);
  changed();
  for (const f of announced) f(fresh);
  return fresh;
}

/** How long a notice lookup holds Receive's rotation back before the page stops waiting for it. */
export const LOOKUP_TIMEOUT_MS = 10_000;

const lookups = new Map<string, number>();
const lookupListeners = new Set<() => void>();

const setLookups = (walletId: string, by: 1 | -1) => {
  const n = (lookups.get(walletId) ?? 0) + by;
  if (n > 0) lookups.set(walletId, n);
  else lookups.delete(walletId);
  lookupListeners.forEach((l) => l());
};

/** Whether a notice lookup of `walletId` is outstanding (and not past `LOOKUP_TIMEOUT_MS`). */
export const lookupPending = (walletId: string): boolean => lookups.has(walletId);

/** Asks the engine what `txids` are and records the payments among them. While it asks, `lookupPending`
 * says so, so a screen does not take the lack of a payment for proof (Receive's rotation). A failed read
 * says nothing: Activity still shows the payment. A lookup slower than `timeoutMs` stops being waited
 * for, and is recorded all the same if it answers later. */
export async function announceReceived(backend: Pick<Backend, "txNotices">, walletId: string, txids: string[], timeoutMs: number = LOOKUP_TIMEOUT_MS): Promise<void> {
  let open = true;
  const end = () => {
    if (!open) return;
    open = false;
    setLookups(walletId, -1);
  };
  setLookups(walletId, 1);
  const timer = setTimeout(end, timeoutMs);
  try {
    recordReceived(walletId, await backend.txNotices(walletId, txids));
  } catch (e) {
    console.warn("could not read the notices of new transactions", e);
  } finally {
    clearTimeout(timer);
    end();
  }
}

/** Calls `f` with each batch of payments as it is recorded. Returns the unsubscribe. */
export function onReceived(f: (batch: ReceivedPayment[]) => void): () => void {
  announced.add(f);
  return () => announced.delete(f);
}

/** Marks the payments `ids` of `walletId` as seen: the rows a view showed, not the ones it left out. */
export function markReceivedRead(walletId: string, ids: readonly string[]) {
  const mark = new Set(ids);
  if (!list.some((p) => p.walletId === walletId && !p.read && mark.has(p.id))) return;
  list = list.map((p) => (p.walletId === walletId && mark.has(p.id) ? { ...p, read: true } : p));
  changed();
}

/** Tests: forget everything (a fresh app). */
export function resetReceived() {
  list = [];
  lookups.clear();
  changed();
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

const EMPTY: readonly ReceivedPayment[] = [];
const byWallet = new Map<string, { source: readonly ReceivedPayment[]; value: readonly ReceivedPayment[] }>();

/** `walletId`'s payments, newest first; the same array until they change. */
export function useReceived(walletId: string | undefined): readonly ReceivedPayment[] {
  return useSyncExternalStore(subscribe, () => {
    if (!walletId) return EMPTY;
    const hit = byWallet.get(walletId);
    if (hit?.source === list) return hit.value;
    const value = list.filter((p) => p.walletId === walletId);
    byWallet.set(walletId, { source: list, value });
    return value;
  });
}

/** Whether a notice lookup of `walletId` is outstanding; a screen re-renders when that changes. */
export function useLookupPending(walletId: string | undefined): boolean {
  return useSyncExternalStore(
    (l) => {
      lookupListeners.add(l);
      return () => void lookupListeners.delete(l);
    },
    () => !!walletId && lookupPending(walletId),
  );
}

/** How many of `walletId`'s payments the bell has not shown yet. */
export const unreadReceived = (payments: readonly ReceivedPayment[]): number => payments.filter((p) => !p.read).length;
