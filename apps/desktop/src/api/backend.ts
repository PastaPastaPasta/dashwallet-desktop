// The renderer's view of the engine. Two implementations: `TauriBackend` (IPC to dw-app,
// the real engine) and `FixtureBackend` (a sample wallet in TypeScript, for screenshot,
// Playwright and `--selftest --fixture` runs). Every call maps 1:1 to a dw-app command.

import type { Balances } from "./generated/Balances";
import type { EngineSignal } from "./generated/EngineSignal";
import type { HistoryRequest } from "./generated/HistoryRequest";
import type { HistoryRows } from "./generated/HistoryRows";
import type { LaunchInfo } from "./generated/LaunchInfo";
import type { LockStatus } from "./generated/LockStatus";
import type { SyncStatus } from "./generated/SyncStatus";
import type { TxDetails } from "./generated/TxDetails";
import type { WalletSummary } from "./generated/WalletSummary";

export interface Backend {
  readonly kind: "tauri" | "fixture";
  launchInfo(): Promise<LaunchInfo>;
  openNetwork(): Promise<LockStatus>;
  listWallets(): Promise<WalletSummary[]>;
  lockStatus(): Promise<LockStatus>;
  unlock(passphrase: string): Promise<LockStatus>;
  lock(): Promise<LockStatus>;
  balance(walletId: string): Promise<Balances | null>;
  syncStatus(): Promise<SyncStatus>;
  historyPage(walletId: string, request: HistoryRequest): Promise<HistoryRows>;
  txDetail(walletId: string, txid: string): Promise<TxDetails>;
  /** Subscribes to engine signals; returns the unsubscribe function. */
  onSignal(listener: (signal: EngineSignal) => void): Promise<() => void>;
  /** The clock and locale the screens format dates with (fixed in fixtures). */
  now(): Date;
  readonly locale: string | undefined;
}

/** Error shape every command rejects with (`AppError` in dw-app). */
export interface BackendError {
  code: string;
  message: string;
  retryAfterSecs: number | null;
}

export function isBackendError(e: unknown): e is BackendError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}
