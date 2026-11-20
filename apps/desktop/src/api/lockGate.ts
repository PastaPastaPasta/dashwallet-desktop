// The renderer's lock gate: closed from the moment the vault locks (its `vaultLockState` event, or
// the renderer's own lock answered) until the lock status says it is not locked. While it is
// closed the shell shows the lock screen in place of the wallet's views, the cache keeps only the
// lock screen's own reads, wallet toasts are held, and the backend refuses every command but the
// ones the locked window needs (`guardWhileLocked`), so no wallet read starts in between.

import { useSyncExternalStore } from "react";
import type { QueryClient, QueryKey } from "@tanstack/react-query";
import { holdWalletToasts } from "../components/toast";
import type { Backend } from "./backend";
import type { AppError } from "./generated/AppError";
import type { LockStatus } from "./generated/LockStatus";
import type { IpcBackend } from "./tauri";
import { keys } from "./queries";

const KEPT = new Set<unknown>([keys.network[0], keys.lockStatus[0], keys.lastLockReport[0], keys.lockReconcile[0], keys.lockDrain[0]]);

export const keptWhileLocked = (key: QueryKey): boolean => KEPT.has(key[0]);

/** Removes every query the lock screen does not read. A read still under way is cancelled with
 * its query, so its answer lands nowhere. */
export function forgetWalletContent(client: QueryClient) {
  client.removeQueries({ predicate: (q) => !keptWhileLocked(q.queryKey) });
}

let closed = false;
const listeners = new Set<() => void>();
const notify = () => listeners.forEach((l) => l());

export const walletGateClosed = (): boolean => closed;

export function subscribeWalletGate(l: () => void): () => void {
  listeners.add(l);
  return () => void listeners.delete(l);
}

export const useWalletGateClosed = (): boolean => useSyncExternalStore(subscribeWalletGate, walletGateClosed);

/** The vault locked: the gate closes at once, before anything is invalidated or read again. A
 * status read already under way may predate the lock, so it is dropped and the cached status
 * says locked until a read begun since answers. */
export function closeWalletGate(client: QueryClient) {
  const status = client.getQueryData<LockStatus>(keys.lockStatus);
  if (status && status.state !== "locked") {
    void client.cancelQueries({ queryKey: keys.lockStatus });
    client.setQueryData<LockStatus>(keys.lockStatus, { ...status, state: "locked" });
  }
  forgetWalletContent(client);
  if (closed) return;
  closed = true;
  holdWalletToasts(true);
  notify();
}

function openWalletGate() {
  if (!closed) return;
  closed = false;
  holdWalletToasts(false);
  notify();
}

/** Keeps the gate on the cached lock status: it opens only once a status read says the vault is
 * not locked. Returns the unsubscribe. */
export function followLockStatus(client: QueryClient): () => void {
  const apply = () => {
    const status = client.getQueryData<LockStatus>(keys.lockStatus);
    if (!status) return;
    if (status.state === "locked") closeWalletGate(client);
    else openWalletGate();
  };
  apply();
  return client.getQueryCache().subscribe((e) => {
    if (e.type === "updated" && e.action.type === "success" && e.query.queryKey[0] === keys.lockStatus[0]) apply();
  });
}

/** What the locked window may ask: the window and its settings, the lock, the unlock and the lock
 * report, Forgot passphrase, links and the menus, and the calls that end what a lock cut short. */
export const LOCKED_COMMANDS: ReadonlySet<string> = new Set<keyof IpcBackend>([
  "launchInfo",
  "openNetwork",
  "uiSettings",
  "setUiSettings",
  "lockStatus",
  "lock",
  "unlock",
  "unlockMixingOnly",
  "lastLockReport",
  "leases",
  "dispatchStatus",
  "listWallets",
  "checkMnemonic",
  "recoverWithMnemonic",
  "takeLinks",
  "shellSync",
  "openReleasesPage",
  "endFlow",
  "revokeGrant",
  "draftAbandon",
  "closeDraft",
]);

/** The backend's own members that ask the engine nothing. */
const NOT_COMMANDS: ReadonlySet<string> = new Set<keyof IpcBackend>(["onEvent", "onShellEvent", "now"]);

const LOCKED: AppError = { code: "vault.locked", message: "The wallet is locked.", data: null };

/** A command `guardWhileLocked` refused: the engine was never asked. */
export const refusedWhileLocked = (e: unknown): boolean => e === LOCKED;

/** `backend`, refusing every command outside `LOCKED_COMMANDS` while the gate is closed, without
 * asking the engine. `listWallets` is Forgot passphrase's, which names the wallets it can recover. */
export function guardWhileLocked<B extends Backend>(backend: B): B {
  return new Proxy(backend, {
    get(target, prop, receiver) {
      const value = Reflect.get(target, prop, receiver);
      if (typeof value !== "function" || typeof prop !== "string" || NOT_COMMANDS.has(prop) || LOCKED_COMMANDS.has(prop)) return value;
      return (...args: unknown[]) => (closed ? Promise.reject(LOCKED) : value.apply(target, args));
    },
  });
}

/** Tests: the gate as at start. */
export function resetWalletGate() {
  closed = false;
  holdWalletToasts(false);
  notify();
}
