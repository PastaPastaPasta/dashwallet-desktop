// The renderer's view of the engine: dw-app's commands through the generated
// `generated/commands.ts` (`IpcBackend`, api/tauri.ts). Every call maps 1:1 to a dw-app
// command; the screens use this subset, the rest of the facade is in `generated/commands.ts`.
// `kind` says what answers the IPC: dw-app with the real engine, or the fixture backend
// (src/fixture, U-07), which mocks the IPC itself and exists only in fixture builds.

import type { AppError, AppEvent, ErrorCode } from "./generated/commands";
import { ERROR_CODES } from "./generated/commands";
import type { AuthGrant } from "./generated/AuthGrant";
import type { Balances } from "./generated/Balances";
import type { DisplayUnit } from "./generated/DisplayUnit";
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
import type { AddressClass } from "./generated/AddressClass";
import type { AddressInfo } from "./generated/AddressInfo";
import type { BroadcastOutcome } from "./generated/BroadcastOutcome";
import type { ChangePolicy } from "./generated/ChangePolicy";
import type { CoinSelectionSummary } from "./generated/CoinSelectionSummary";
import type { CoinSource } from "./generated/CoinSource";
import type { FeeMode } from "./generated/FeeMode";
import type { FeePolicy } from "./generated/FeePolicy";
import type { OutPoint } from "./generated/OutPoint";
import type { PreparedTxHandle } from "./generated/PreparedTxHandle";
import type { QrMatrix } from "./generated/QrMatrix";
import type { ReceiveRequest } from "./generated/ReceiveRequest";
import type { Recipient } from "./generated/Recipient";
import type { TxDetailExtrasInfo } from "./generated/TxDetailExtrasInfo";
import type { TxEstimate } from "./generated/TxEstimate";
import type { Utxo } from "./generated/Utxo";
import type { UtxoFilter } from "./generated/UtxoFilter";
import type { AccountXpub } from "./generated/AccountXpub";
import type { BackupFileHeader } from "./generated/BackupFileHeader";
import type { BackupPolicyInfo } from "./generated/BackupPolicyInfo";
import type { BackupSummary } from "./generated/BackupSummary";
import type { CoreExportFormatName } from "./generated/CoreExportFormatName";
import type { CoreExportReport } from "./generated/CoreExportReport";
import type { CoreImportReport } from "./generated/CoreImportReport";
import type { CoreMnemonicCheck } from "./generated/CoreMnemonicCheck";
import type { KeyMaterialInput } from "./generated/KeyMaterialInput";
import type { NodeSummary } from "./generated/NodeSummary";
import type { UiSettings } from "./generated/UiSettings";
import type { WalletFileInfo } from "./generated/WalletFileInfo";
import type { WalletLoadStatus } from "./generated/WalletLoadStatus";
import type { WatchOnlyOptions } from "./generated/WatchOnlyOptions";

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
  /** `walletId` is `null` only for `changeCredential` (vault.rs). */
  authorize(purpose: GrantPurposeSpec, walletId: string | null, credential: CredentialSpec): Promise<AuthGrant>;
  /** Drops a grant before it expires (unknown ids are ignored). */
  revokeGrant(grantId: string): Promise<null>;
  revealMnemonic(walletId: string, grantId: string): Promise<RevealedMnemonic>;
  /** Forgot passphrase (IOS-014): replaces the vault with one holding only `walletId`'s secrets. */
  recoverWithMnemonic(walletId: string, mnemonic: string, bip39Passphrase: string, newPassphrase: string): Promise<VaultRecovery>;
  dispatchStatus(walletId: string, artifact: string): Promise<DispatchStateName | null>;
  balance(walletId: string): Promise<Balances | null>;
  syncStatus(): Promise<SyncStatus>;
  historyPage(walletId: string, request: HistoryRequest): Promise<HistoryRows>;
  txDetail(walletId: string, txid: string): Promise<TxDetails>;
  /** Which actions the detail sheet offers (Abandon, Resend). */
  txDetailExtras(walletId: string, txid: string): Promise<TxDetailExtrasInfo>;
  /** Sets (or, with `null`, clears) a transaction's label. */
  setTxLabel(walletId: string, txid: string, label: string | null): Promise<null>;
  abandonTransaction(walletId: string, txid: string): Promise<null>;
  resendTransaction(walletId: string, txid: string): Promise<null>;
  // Receive (U-03 D11).
  currentReceiveAddress(walletId: string): Promise<AddressInfo>;
  nextReceiveAddress(walletId: string, label: string | null): Promise<AddressInfo>;
  createReceiveRequest(walletId: string, amount: number | null, label: string | null, message: string | null): Promise<ReceiveRequest>;
  receiveRequests(walletId: string): Promise<ReceiveRequest[]>;
  deleteReceiveRequest(walletId: string, requestId: number): Promise<null>;
  /** The wallet's DashPay username for the Receive QR (DP3-03); `null` without one. */
  receiveUsername(walletId: string): Promise<string | null>;
  /** dash-qt's payment URI, with `user=` last when `user` is set (DP3-03, IOS-055). */
  buildPaymentUri(address: string, amount: number | null, label: string | null, message: string | null, user: string | null): Promise<string>;
  qrMatrix(text: string): Promise<QrMatrix>;
  // Send (U-03 D2, E0-13 D8): a draft and its prepared transactions are handles.
  classifyAddress(network: string, text: string): Promise<AddressClass>;
  /** dw-units' parser for an amount the user typed in `unit`; `null` when it is not one. */
  parseAmount(text: string, unit: DisplayUnit): Promise<number | null>;
  newTxDraft(walletId: string): Promise<number>;
  closeDraft(draft: number): Promise<boolean>;
  draftSetRecipients(draft: number, recipients: Recipient[]): Promise<null>;
  draftSetSource(draft: number, source: CoinSource): Promise<null>;
  draftSetFee(draft: number, fee: FeeMode): Promise<null>;
  draftSetChange(draft: number, change: ChangePolicy): Promise<null>;
  draftEstimate(draft: number): Promise<TxEstimate>;
  draftPrepare(draft: number, grantId: string): Promise<PreparedTxHandle>;
  draftBroadcast(draft: number, prepared: number): Promise<BroadcastOutcome>;
  draftAbandon(draft: number, prepared: number): Promise<null>;
  maxSpendable(walletId: string, source: CoinSource, fee: FeeMode): Promise<number>;
  feePolicy(): Promise<FeePolicy>;
  utxos(walletId: string, filter: UtxoFilter): Promise<Utxo[]>;
  coinSelectionSummary(walletId: string, outpoints: OutPoint[], payAmounts: number[], fee: FeeMode): Promise<CoinSelectionSummary>;
  /** "Require authentication for every payment" (E0-13 D11). */
  paymentAuthentication(): Promise<boolean>;
  // Settings (U-04). Passphrases and key material are secret, as in onboarding.
  /** The display settings in the data root (U-04 D2); the defaults when there is no file. */
  uiSettings(): Promise<UiSettings>;
  /** Saves them; dw-app also sets the window's theme unless `--theme` was given (D3). */
  setUiSettings(settings: UiSettings): Promise<UiSettings>;
  /** Encrypts an unencrypted vault with a `changeCredential` grant; the vault ends locked. */
  encryptVault(newPassphrase: string, grantId: string): Promise<LockStatus>;
  changePassphrase(oldPassphrase: string, newPassphrase: string): Promise<LockStatus>;
  setPaymentAuthentication(on: boolean, grantId: string): Promise<null>;
  /** Deletes the vault and every secret in it (IOS-109); the wallets must be removed first. */
  destroyVault(credential: CredentialSpec): Promise<LockStatus>;
  /** Every registered wallet, open or closed (QT-101). */
  walletLoadStates(): Promise<WalletLoadStatus[]>;
  loadWallet(walletId: string): Promise<null>;
  unloadWallet(walletId: string): Promise<null>;
  setLoadOnStartup(walletId: string, loadOnStartup: boolean): Promise<null>;
  renameWallet(walletId: string, name: string): Promise<null>;
  /** Deletes a wallet and its data with a `wipe` grant (IOS-110). */
  removeWallet(walletId: string, grantId: string): Promise<null>;
  importWatchOnly(xpub: string, options: WatchOnlyOptions): Promise<string>;
  accountXpub(walletId: string, account: number): Promise<AccountXpub>;
  backupWallet(walletId: string, destPath: string, backupPassphrase: string | null): Promise<BackupSummary>;
  /** Restores the wallets of a `.dwbackup`; returns their ids. */
  restoreBackup(path: string, passphrase: string | null): Promise<string[]>;
  automaticBackups(walletId: string | null): Promise<BackupSummary[]>;
  backupDirectory(): Promise<string>;
  backupPolicy(): Promise<BackupPolicyInfo>;
  setBackupPolicy(keep: number): Promise<BackupPolicyInfo>;
  readBackupHeader(path: string): Promise<BackupFileHeader>;
  /** What a file is, from its content (QT-106, QT-107). */
  inspectWalletFile(path: string): Promise<WalletFileInfo>;
  importDumpWallet(path: string, options: ImportOptions): Promise<CoreImportReport>;
  importWalletDat(path: string, walletPassphrase: string | null, options: ImportOptions): Promise<CoreImportReport>;
  importKeyMaterial(material: KeyMaterialInput, options: ImportOptions): Promise<CoreImportReport>;
  coreMnemonicCompatibility(walletId: string): Promise<CoreMnemonicCheck>;
  /** Writes the wallet's keys for Dash Core with a `revealSecret` grant (QT-109). */
  exportForCore(walletId: string, format: CoreExportFormatName, destPath: string, grantId: string): Promise<CoreExportReport>;
  /** Hides received coins at or under this many duffs; `null`: off (QT-138). */
  dustProtection(): Promise<number | null>;
  setDustProtection(threshold: number | null): Promise<null>;
  nodeInfo(): Promise<NodeSummary>;
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
