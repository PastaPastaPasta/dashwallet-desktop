// The renderer's view of the engine. Two implementations: `TauriBackend` (IPC to dw-app,
// the real engine, through the generated `generated/commands.ts`) and `FixtureBackend` (a
// sample wallet in TypeScript, for screenshot, Playwright and `--selftest --fixture` runs).
// Every call maps 1:1 to a dw-app command; the screens use this subset, the rest of the
// facade is in `generated/commands.ts`.

import type { AppError, AppEvent, ErrorCode } from "./generated/commands";
import { ERROR_CODES } from "./generated/commands";
import type { Balances } from "./generated/Balances";
import type { HistoryRequest } from "./generated/HistoryRequest";
import type { HistoryRows } from "./generated/HistoryRows";
import type { LockStatus } from "./generated/LockStatus";
import type { OpenedNetwork } from "./generated/OpenedNetwork";
import type { SyncStatus } from "./generated/SyncStatus";
import type { TxDetails } from "./generated/TxDetails";
import type { WalletSummary } from "./generated/WalletSummary";

export interface Backend {
  readonly kind: "tauri" | "fixture";
  /** Opens the launch network (or returns it when it is already the active one). */
  openNetwork(): Promise<OpenedNetwork>;
  listWallets(): Promise<WalletSummary[]>;
  lockStatus(): Promise<LockStatus>;
  unlock(passphrase: string): Promise<LockStatus>;
  lock(): Promise<LockStatus>;
  balance(walletId: string): Promise<Balances | null>;
  syncStatus(): Promise<SyncStatus>;
  historyPage(walletId: string, request: HistoryRequest): Promise<HistoryRows>;
  txDetail(walletId: string, txid: string): Promise<TxDetails>;
  /** Subscribes to engine events; returns the unsubscribe function. */
  onEvent(listener: (event: AppEvent) => void): Promise<() => void>;
  /** The clock and locale the screens format dates with (fixed in fixtures). */
  now(): Date;
  readonly locale: string | undefined;
}

const KNOWN_CODES: ReadonlySet<string> = new Set(ERROR_CODES);

/** Every command rejects with an `AppError` (dw-app's error union). */
export function isAppError(e: unknown): e is AppError {
  return (
    typeof e === "object" &&
    e !== null &&
    typeof (e as { code?: unknown }).code === "string" &&
    KNOWN_CODES.has((e as { code: string }).code) &&
    typeof (e as { message?: unknown }).message === "string"
  );
}

/** True when `e` is an `AppError` with `code`. */
export function hasCode(e: unknown, code: ErrorCode): e is AppError {
  return isAppError(e) && e.code === code;
}

/** Seconds the vault asks to wait before the next attempt (`vault.throttled`,
 * `vault.wrong_passphrase`); 0 when it asks for none. */
export function retryAfterSecs(e: unknown): number {
  if (!isAppError(e)) return 0;
  if (e.data?.kind === "retryAfter") return e.data.secs;
  if (e.data?.kind === "wrongPassphrase") return e.data.retryAfterSecs ?? 0;
  return 0;
}

/** Display text for anything a backend call or a query may throw. */
export function errorMessage(e: unknown): string {
  return isAppError(e) || e instanceof Error ? e.message : String(e);
}
