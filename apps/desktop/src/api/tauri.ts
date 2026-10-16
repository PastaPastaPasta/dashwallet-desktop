import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Backend } from "./backend";
import * as cmd from "./generated/commands";
import type { AppEvent, ShellEvent } from "./generated/commands";
import type { AddressPurposeName } from "./generated/AddressPurposeName";
import type { DisplayUnit } from "./generated/DisplayUnit";
import type { FeeMode } from "./generated/FeeMode";
import type { OutPoint } from "./generated/OutPoint";
import type { JsonValue } from "./generated/serde_json/JsonValue";

// Read once, when the module loads: the fixture backend's mock IPC installs its own
// `__TAURI_INTERNALS__` later, and must not make Tauri-only tools look reachable.
const IN_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** True inside the Tauri webview (not under the fixture backend's mock IPC). */
export const inTauri = () => IN_TAURI;

/** The one `Backend`: dw-app's commands over IPC. In a fixture build outside Tauri the IPC is
 * the fixture backend's mock (src/fixture), so both modes run this same code. */
export class IpcBackend implements Backend {
  constructor(
    readonly kind: Backend["kind"],
    /** The fixture's fixed clock; the system's otherwise. */
    private readonly clock: () => Date = () => new Date(),
    readonly locale: string | undefined = undefined,
  ) {}

  launchInfo = () => cmd.launchInfo();
  openNetwork = () => cmd.openNetwork(null, null);
  listWallets = () => cmd.listWallets();
  lockStatus = () => cmd.lockStatus();
  unlock = (passphrase: string) => cmd.unlock(passphrase);
  unlockMixingOnly = (passphrase: string) => cmd.unlockMixingOnly(passphrase);
  lock = () => cmd.lock();
  lastLockReport = () => cmd.lastLockReport();
  leases = () => cmd.leases();
  createVault = cmd.createVault;
  generateMnemonic = (wordCount: number) => cmd.generateMnemonic(wordCount, "english");
  checkMnemonic = cmd.checkMnemonic;
  importWallet = cmd.importWallet;
  authorize = cmd.authorize;
  authorizeSet = cmd.authorizeSet;
  beginFlow = cmd.beginFlow;
  endFlow = cmd.endFlow;
  revokeGrant = cmd.revokeGrant;
  revealMnemonic = cmd.revealMnemonic;
  recoverWithMnemonic = cmd.recoverWithMnemonic;
  dispatchStatus = cmd.dispatchStatus;
  balance = (walletId: string) => cmd.balance(walletId);
  syncStatus = () => cmd.syncStatus();
  historyPage = cmd.historyPage;
  txDetail = cmd.txDetail;
  txDetailExtras = cmd.txDetailExtras;
  setTxLabel = cmd.setTxLabel;
  abandonTransaction = cmd.abandonTransaction;
  resendTransaction = cmd.resendTransaction;
  currentReceiveAddress = cmd.currentReceiveAddress;
  nextReceiveAddress = cmd.nextReceiveAddress;
  createReceiveRequest = cmd.createReceiveRequest;
  receiveRequests = cmd.receiveRequests;
  deleteReceiveRequest = cmd.deleteReceiveRequest;
  receiveUsername = cmd.receiveUsername;
  buildPaymentUri = cmd.buildPaymentUri;
  qrMatrix = cmd.qrMatrix;
  classifyAddress = cmd.classifyAddress;
  parseAmount = (text: string, unit: DisplayUnit) => cmd.parseAmount(text, unit);
  newTxDraft = cmd.newTxDraft;
  closeDraft = cmd.closeDraft;
  draftSetRecipients = cmd.draftSetRecipients;
  draftSetContactRecipient = cmd.draftSetContactRecipient;
  draftSetSource = cmd.draftSetSource;
  draftSetFee = cmd.draftSetFee;
  draftSetChange = cmd.draftSetChange;
  draftEstimate = cmd.draftEstimate;
  draftPrepare = cmd.draftPrepare;
  draftBroadcast = cmd.draftBroadcast;
  draftAbandon = cmd.draftAbandon;
  maxSpendable = cmd.maxSpendable;
  feePolicy = () => cmd.feePolicy();
  utxos = cmd.utxos;
  coinSelectionSummary = (walletId: string, outpoints: OutPoint[], payAmounts: number[], fee: FeeMode) =>
    cmd.coinSelectionSummary(walletId, outpoints, payAmounts, fee, false);
  paymentAuthentication = () => cmd.paymentAuthentication();
  uiSettings = () => cmd.uiSettings();
  setUiSettings = cmd.setUiSettings;
  encryptVault = cmd.encryptVault;
  changePassphrase = cmd.changePassphrase;
  setPaymentAuthentication = cmd.setPaymentAuthentication;
  destroyVault = cmd.destroyVault;
  walletLoadStates = () => cmd.walletLoadStates();
  loadWallet = cmd.loadWallet;
  unloadWallet = cmd.unloadWallet;
  setLoadOnStartup = cmd.setLoadOnStartup;
  renameWallet = cmd.renameWallet;
  removeWallet = cmd.removeWallet;
  importWatchOnly = cmd.importWatchOnly;
  accountXpub = cmd.accountXpub;
  backupWallet = cmd.backupWallet;
  restoreBackup = cmd.restoreBackup;
  automaticBackups = cmd.automaticBackups;
  backupDirectory = () => cmd.backupDirectory();
  backupPolicy = () => cmd.backupPolicy();
  setBackupPolicy = cmd.setBackupPolicy;
  readBackupHeader = cmd.readBackupHeader;
  inspectWalletFile = cmd.inspectWalletFile;
  importDumpWallet = cmd.importDumpWallet;
  importWalletDat = cmd.importWalletDat;
  importKeyMaterial = cmd.importKeyMaterial;
  coreMnemonicCompatibility = cmd.coreMnemonicCompatibility;
  exportForCore = cmd.exportForCore;
  dustProtection = () => cmd.dustProtection();
  setDustProtection = cmd.setDustProtection;
  nodeInfo = () => cmd.nodeInfo();
  dashpayStatus = cmd.dashpayStatus;
  identities = cmd.identities;
  dashpayUnreadCount = cmd.dashpayUnreadCount;
  dashpayContacts = cmd.dashpayContacts;
  dashpayPendingSetupCount = cmd.dashpayPendingSetupCount;
  dashpaySyncSoon = () => cmd.dashpaySyncSoon();
  searchUsers = cmd.searchUsers;
  dashpayContact = cmd.dashpayContact;
  contactActivity = cmd.contactActivity;
  frequentContacts = cmd.frequentContacts;
  resolveContactPaymentLock = cmd.resolveContactPaymentLock;
  grantRequest = cmd.grantRequest;
  creditsCostTable = cmd.creditsCostTable;
  dashpayEligibility = cmd.dashpayEligibility;
  dashpaySendRequest = cmd.dashpaySendRequest;
  dashpayMyUserLink = cmd.dashpayMyUserLink;
  dashpayVerifyScanned = cmd.dashpayVerifyScanned;
  screenScanMode = cmd.screenScanMode;
  scanQrFile = cmd.scanQrFile;
  scanQrImage = cmd.scanQrImage;
  scanQrScreen = cmd.scanQrScreen;
  cancelScreenScan = cmd.cancelScreenScan;
  dashpayAcceptRequest = cmd.dashpayAcceptRequest;
  dashpayIgnoreContact = cmd.dashpayIgnoreContact;
  dashpayUnignoreContact = cmd.dashpayUnignoreContact;
  dashpaySetPrivateDetails = cmd.dashpaySetPrivateDetails;
  dashpayEnableKeys = cmd.dashpayEnableKeys;
  dashpayEvents = cmd.dashpayEvents;
  dashpayMarkRead = cmd.dashpayMarkRead;
  takeLinks = cmd.takeLinks;
  routeLink = cmd.routeLink;
  shellSync = cmd.shellSync;
  warnings = () => cmd.warnings();
  consoleCommands = () => cmd.consoleCommands();
  consoleRedact = cmd.consoleRedact;
  consoleExecute = cmd.consoleExecute;
  peers = () => cmd.peers();
  rotatePeers = () => cmd.rotatePeers();
  rescan = cmd.rescan;
  rescanProgress = () => cmd.rescanProgress();
  cancelRescan = () => cmd.cancelRescan();
  stopSpv = () => cmd.stopSpv();
  startSpv = () => cmd.startSpv();
  resetChainData = () => cmd.resetChainData();
  setBirthHeight = cmd.setBirthHeight;
  dropUnconfirmed = cmd.dropUnconfirmed;
  registrations = cmd.registrations;
  registrationQuote = cmd.registrationQuote;
  resolveStuckFunding = cmd.resolveStuckFunding;
  // The search is the renderer's (U-05 D13): dash-qt's wildcards over label and address.
  addressBook = (walletId: string, purpose: AddressPurposeName | null) => cmd.addressBook(walletId, purpose, null);
  saveAddressBookEntry = cmd.saveAddressBookEntry;
  deleteAddressBookEntry = cmd.deleteAddressBookEntry;
  signMessage = cmd.signMessage;
  verifyMessage = cmd.verifyMessage;
  analyzePsbt = cmd.analyzePsbt;
  signPsbt = cmd.signPsbt;
  broadcastPsbt = cmd.broadcastPsbt;
  draftCreateUnsigned = cmd.draftCreateUnsigned;
  lockOutpoints = cmd.lockOutpoints;
  unlockOutpoints = cmd.unlockOutpoints;
  openFile = cmd.openFile;
  saveFileAs = cmd.saveFileAs;
  chooseFilePath = cmd.chooseFilePath;
  onEvent = (listener: (e: AppEvent) => void) => listen<AppEvent>(cmd.ENGINE_EVENT, (e) => listener(e.payload));
  onShellEvent = (listener: (e: ShellEvent) => void) => listen<ShellEvent>(cmd.SHELL_EVENT, (e) => listener(e.payload));
  now = () => this.clock();
}

/** Measurement and self-test commands; not part of `Backend`. */
export const tauriTools = {
  startupMark: cmd.startupMark,
  selftestReport: (report: unknown) => cmd.selftestReport(report as JsonValue),
  selftestCheckpoint: cmd.selftestCheckpoint,
  selftestUnlock: () => cmd.selftestUnlock(),
  ipcBenchRows: cmd.ipcBenchRows,
  historyPageTimed: cmd.historyPageTimed,
  selftestLinkRouted: cmd.selftestLinkRouted,
  probeUnlisted: cmd.probeUnlisted,
  /** A plugin command that is not installed (U6: the renderer cannot reach a shell). */
  invokeUnknownPlugin: () => invoke<unknown>("plugin:shell|execute", { program: "sh", args: ["-c", "id"] }),
  /** The dialog plugin's own open command: the plugin is installed for dw-app's file chooser, but no
   * capability grants its commands (U-05 D2), so the page cannot show a dialog or name a path. */
  invokeDialogPlugin: () => invoke<unknown>("plugin:dialog|open", { options: {} }),
};
