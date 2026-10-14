// The renderer's view of the engine: dw-app's commands through the generated
// `generated/commands.ts` (`IpcBackend`, api/tauri.ts). Every call maps 1:1 to a dw-app
// command; the screens use this subset, the rest of the facade is in `generated/commands.ts`.
// `kind` says what answers the IPC: dw-app with the real engine, or the fixture backend
// (src/fixture, U-07), which mocks the IPC itself and exists only in fixture builds.

import type { AppError, AppEvent, ErrorCode } from "./generated/commands";
import { ERROR_CODES } from "./generated/commands";
import type { AuthGrant } from "./generated/AuthGrant";
import type { Balances } from "./generated/Balances";
import type { ContactList } from "./generated/ContactList";
import type { ContactQueryArgs } from "./generated/ContactQueryArgs";
import type { CredentialSpec } from "./generated/CredentialSpec";
import type { DashPayStatus } from "./generated/DashPayStatus";
import type { IdentitySummary } from "./generated/IdentitySummary";
import type { LinkRoute } from "./generated/LinkRoute";
import type { ShellEvent } from "./generated/ShellEvent";
import type { ShellSync } from "./generated/ShellSync";
import type { DispatchStateName } from "./generated/DispatchStateName";
import type { GrantPurposeSpec } from "./generated/GrantPurposeSpec";
import type { HistoryRequest } from "./generated/HistoryRequest";
import type { HistoryRows } from "./generated/HistoryRows";
import type { ImportOptions } from "./generated/ImportOptions";
import type { Lease } from "./generated/Lease";
import type { LockOutcome } from "./generated/LockOutcome";
import type { LockStatus } from "./generated/LockStatus";
import type { MnemonicCheck } from "./generated/MnemonicCheck";
import type { OpenedNetwork } from "./generated/OpenedNetwork";
import type { RevealedMnemonic } from "./generated/RevealedMnemonic";
import type { SyncStatus } from "./generated/SyncStatus";
import type { TxDetails } from "./generated/TxDetails";
import type { VaultRecovery } from "./generated/VaultRecovery";
import type { WalletSummary } from "./generated/WalletSummary";

export interface Backend {
  readonly kind: "tauri" | "fixture";
  /** Opens the launch network (or returns it when it is already the active one). */
  openNetwork(): Promise<OpenedNetwork>;
  listWallets(): Promise<WalletSummary[]>;
  lockStatus(): Promise<LockStatus>;
  unlock(passphrase: string): Promise<LockStatus>;
  /** dash-qt "Unlock for mixing only" (QT-112). */
  unlockMixingOnly(passphrase: string): Promise<LockStatus>;
  /** Locks the vault; the report says what the lock did to running flows (E0-13 D3). */
  lock(): Promise<LockOutcome>;
  /** The network's last lock report since its session opened, whatever locked it. */
  lastLockReport(): Promise<LockOutcome | null>;
  leases(): Promise<Lease[]>;
  // Onboarding, recovery and the recovery phrase (U-02). Every argument and result that holds a
  // phrase or a passphrase is secret: never cached, logged or kept past its step.
  /** Creates the vault, unlocked; `null` keeps its key in the OS secret store (unencrypted). */
  createVault(passphrase: string | null): Promise<LockStatus>;
  /** A fresh English phrase; nothing is stored. */
  generateMnemonic(wordCount: number): Promise<string>;
  checkMnemonic(phrase: string): Promise<MnemonicCheck>;
  /** Adds the wallet of a phrase; returns its id. */
  importWallet(mnemonic: string, bip39Passphrase: string | null, options: ImportOptions): Promise<string>;
  authorize(purpose: GrantPurposeSpec, walletId: string | null, credential: CredentialSpec): Promise<AuthGrant>;
  revokeGrant(grantId: string): Promise<null>;
  revealMnemonic(walletId: string, grantId: string): Promise<RevealedMnemonic>;
  /** Forgot passphrase (IOS-014): replaces the vault with one holding only `walletId`'s secrets. */
  recoverWithMnemonic(walletId: string, mnemonic: string, bip39Passphrase: string, newPassphrase: string): Promise<VaultRecovery>;
  dispatchStatus(walletId: string, artifact: string): Promise<DispatchStateName | null>;
  balance(walletId: string): Promise<Balances | null>;
  syncStatus(): Promise<SyncStatus>;
  historyPage(walletId: string, request: HistoryRequest): Promise<HistoryRows>;
  txDetail(walletId: string, txid: string): Promise<TxDetails>;
  dashpayStatus(walletId: string): Promise<DashPayStatus>;
  identities(walletId: string): Promise<IdentitySummary[]>;
  dashpayUnreadCount(walletId: string, identity: string): Promise<number>;
  dashpayContacts(walletId: string, identity: string, query: ContactQueryArgs): Promise<ContactList>;
  /** The queued deep links, routed for `network` (U-01). */
  takeLinks(network: string): Promise<LinkRoute[]>;
  /** Where `text` leads (File ▸ Open URI…). */
  routeLink(network: string, text: string): Promise<LinkRoute>;
  /** What the native menus and the tray mirror. */
  shellSync(sync: ShellSync): Promise<null>;
  /** Subscribes to engine events; returns the unsubscribe function. */
  onEvent(listener: (event: AppEvent) => void): Promise<() => void>;
  /** Subscribes to the shell's events (menus, tray, deep links). */
  onShellEvent(listener: (event: ShellEvent) => void): Promise<() => void>;
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
