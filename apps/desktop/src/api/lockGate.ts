// The renderer's lock gate: closed at start until a status says the vault is not locked, and from
// the moment the vault locks (its `vaultLockState` event, or the renderer's own lock answered)
// until a status read since says so again. While it is closed the shell shows no wallet view, the
// cache keeps only the lock screen's own reads, wallet toasts are held, and the backend refuses
// every command but the ones the locked window needs (`guardWhileLocked`), so no wallet read
// starts in between.
//
// Every report of the lock state counts against one generation, bumped by each change the engine
// announces (its events, a lock answered) and by a lost event queue: a status read, the network's
// opening status or a lock's answer from before a newer change is never applied.
//
// A lost event queue may have held a lock, so it closes the gate as a lock does, and only a status
// read that succeeds opens it again (`confirmLockState`):
// - each read has a token and a deadline: a reply after its deadline, or to a read superseded, is
//   ignored, and a failure or a deadline never opens the gate but schedules the next read, after a
//   wait that doubles up to `CONFIRM_RETRY_MAX_MS`, which nothing cuts short;
// - a status opens the gate only in the generation it was read in, checked in the same step that
//   writes it and opens the gate; a status from before a change is read again;
// - a newer loss while a read is under way is answered by a read begun after it, and the bridge
//   that started the confirmation ends it with its timers (`disposeConfirm`).

import { useSyncExternalStore } from "react";
import type { QueryClient, QueryKey } from "@tanstack/react-query";
import { holdWalletToasts } from "../components/toast";
import type { Backend } from "./backend";
import type { AppError } from "./generated/AppError";
import type { LockStatus } from "./generated/LockStatus";
import type { IpcBackend } from "./tauri";
import { keys } from "./queries";

// Read when first asked: queries.ts imports this module too.
let kept: Set<unknown> | undefined;

export const keptWhileLocked = (key: QueryKey): boolean =>
  (kept ??= new Set<unknown>([keys.network[0], keys.lockStatus[0], keys.lastLockReport[0], keys.lockReconcile[0], keys.lockDrain[0]])).has(key[0]);

/** Removes every query the lock screen does not read. A read still under way is cancelled with
 * its query, so its answer lands nowhere. */
export function forgetWalletContent(client: QueryClient) {
  client.removeQueries({ predicate: (q) => !keptWhileLocked(q.queryKey) });
}

let closed = true;
let generation = 0;
/** The generation the cached status was read in: it opens the gate only while that is current. */
let statusGeneration = -1;
holdWalletToasts(true);
const listeners = new Set<() => void>();
const notify = () => listeners.forEach((l) => l());

export const walletGateClosed = (): boolean => closed;

/** The lock state's generation: a status read, or a lock's answer, begun under another is older
 * than what the window knows. */
export const lockGeneration = (): number => generation;

/** The engine announced a lock state, or events were lost that may have: older reports go. */
export function lockStateChanged() {
  generation++;
}

/** Reads the lock status, again while the lock state changed during the read, so the status is
 * never older than the last change. */
export async function freshLockStatus(read: () => Promise<LockStatus>): Promise<LockStatus> {
  for (;;) {
    const since = generation;
    const status = await read();
    if (since === generation) {
      statusGeneration = since;
      return status;
    }
  }
}

/** The network opened with `status`, read from generation `since`: it stands for the lock status
 * unless the state changed meanwhile, when the status is read again. */
export function seedLockStatus(client: QueryClient, status: LockStatus, since: number) {
  if (since === generation) {
    statusGeneration = since;
    client.setQueryData(keys.lockStatus, status);
  } else void client.invalidateQueries({ queryKey: keys.lockStatus });
}

export function subscribeWalletGate(l: () => void): () => void {
  listeners.add(l);
  return () => void listeners.delete(l);
}

export const useWalletGateClosed = (): boolean => useSyncExternalStore(subscribeWalletGate, walletGateClosed);

/** The vault locked: the gate closes at once, before anything is invalidated or read again. A
 * status read already under way may predate the lock, so it is dropped and the cached status
 * says locked until a read begun since answers. `since`: the generation a lock call began in,
 * whose answer counts only if nothing changed since (an unlock after it, say). */
export function closeWalletGate(client: QueryClient, since?: number) {
  if (since !== undefined && since !== generation) return;
  generation++;
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
  endConfirm();
  if (!closed) return;
  closed = false;
  holdWalletToasts(false);
  notify();
}

/** The first wait before a failed confirming read is tried again, and the longest. */
export const CONFIRM_RETRY_MS = 1_000;
export const CONFIRM_RETRY_MAX_MS = 30_000;
/** How long a confirming read may take before it counts as failed. */
export const CONFIRM_DEADLINE_MS = 10_000;

/** A lost event queue's status still to be read: how, and for which cache. */
let confirmation: { client: QueryClient; read: () => Promise<LockStatus>; failures: number } | null = null;
/** The read under way: its token, and the timer of its deadline. */
let inflight: { token: number; deadline: ReturnType<typeof setTimeout> } | null = null;
let retry: ReturnType<typeof setTimeout> | undefined;
let tokens = 0;

function endConfirm() {
  confirmation = null;
  if (inflight) clearTimeout(inflight.deadline);
  inflight = null;
  clearTimeout(retry);
  retry = undefined;
}

function startRead() {
  const c = confirmation;
  if (!c || inflight || retry !== undefined) return;
  const token = ++tokens;
  const since = generation;
  inflight = { token, deadline: setTimeout(() => finish(token, since, null), CONFIRM_DEADLINE_MS) };
  let reply: Promise<LockStatus>;
  try {
    reply = c.read();
  } catch {
    reply = Promise.reject(new Error("lock status unreadable"));
  }
  reply.then(
    (status) => finish(token, since, status),
    () => finish(token, since, null),
  );
}

/** Read `token`, begun in generation `since`, answered `status`, or failed or ran out of time. */
function finish(token: number, since: number, status: LockStatus | null) {
  if (inflight?.token !== token) return;
  clearTimeout(inflight.deadline);
  inflight = null;
  const c = confirmation;
  if (!c) return;
  if (status && since === generation) {
    endConfirm();
    statusGeneration = since;
    c.client.setQueryData(keys.lockStatus, status);
    if (status.state === "locked") closeWalletGate(c.client);
    else openWalletGate();
  } else if (status) startRead();
  else {
    c.failures++;
    retry = setTimeout(() => {
      retry = undefined;
      startRead();
    }, Math.min(CONFIRM_RETRY_MAX_MS, CONFIRM_RETRY_MS * 2 ** (c.failures - 1)));
  }
}

/** Events were lost, a lock among them perhaps: the gate closes as for a lock, and opens only once
 * a status read begun since succeeds and says the vault is not locked. */
export function confirmLockState(client: QueryClient, read: () => Promise<LockStatus>) {
  closeWalletGate(client);
  // A read under way began before this loss: its answer is from an older generation, so the next
  // read starts when it ends.
  if (confirmation) Object.assign(confirmation, { client, read });
  else confirmation = { client, read, failures: 0 };
  startRead();
}

/** The bridge that confirms goes: its reads answer nothing and its timers stop. */
export function disposeConfirm() {
  endConfirm();
}

/** Whether a lost event queue's status is still to be read. */
export const lockUnconfirmed = (): boolean => confirmation !== null;

/** Keeps the gate on the cached lock status: it opens only once a status read in the current
 * generation says the vault is not locked; an older one is read again. Returns the unsubscribe. */
export function followLockStatus(client: QueryClient): () => void {
  const apply = () => {
    const status = client.getQueryData<LockStatus>(keys.lockStatus);
    if (!status) return;
    if (status.state === "locked") closeWalletGate(client);
    else if (statusGeneration === generation) openWalletGate();
    else void client.invalidateQueries({ queryKey: keys.lockStatus });
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

const UNGUARDED = Symbol("unguarded");

/** `backend`, refusing every command outside `LOCKED_COMMANDS` while the gate is closed, without
 * asking the engine. */
export function guardWhileLocked<B extends Backend>(backend: B): B {
  return new Proxy(backend, {
    get(target, prop, receiver) {
      if (prop === UNGUARDED) return target;
      const value = Reflect.get(target, prop, receiver);
      if (typeof value !== "function" || typeof prop !== "string" || NOT_COMMANDS.has(prop) || LOCKED_COMMANDS.has(prop)) return value;
      return (...args: unknown[]) => {
        return closed ? Promise.reject(LOCKED) : value.apply(target, args);
      };
    },
  });
}

/** Starts sync past the gate: a reset under way puts sync back even if the wallet locked after it
 * stopped it. */
export function startSpvPastLock(backend: Pick<Backend, "startSpv">): Promise<null> {
  const unguarded = (backend as unknown as Record<symbol, Pick<Backend, "startSpv"> | undefined>)[UNGUARDED] ?? backend;
  return unguarded.startSpv();
}

/** The wallets Forgot passphrase can name while locked: their ids and names, nothing else. */
export async function walletNamesForRecovery(backend: Backend): Promise<{ walletId: string; name: string }[]> {
  const unguarded = ((backend as unknown as Record<symbol, Backend | undefined>)[UNGUARDED] ?? backend) as Backend;
  return (await unguarded.listWallets()).map(({ walletId, name }) => ({ walletId, name }));
}

/** Tests: the gate as at start, or open (`closedNow` false). */
export function resetWalletGate(closedNow = false) {
  closed = closedNow;
  endConfirm();
  statusGeneration = generation;
  holdWalletToasts(closedNow);
  notify();
}
