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
import type { DashPaySyncStatus } from "./generated/DashPaySyncStatus";
import type { QuorumSource } from "./generated/QuorumSource";
import type { EligibilityName } from "./generated/EligibilityName";
import type { ScannedContactInfo } from "./generated/ScannedContactInfo";
import type { ScreenScanMode } from "./generated/ScreenScanMode";
import type { IdentitySummary } from "./generated/IdentitySummary";
import type { LinkRoute } from "./generated/LinkRoute";
import type { StashedLinkInfo } from "./generated/StashedLinkInfo";
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
import type { CoinJoinLimits } from "./generated/CoinJoinLimits";
import type { CoinJoinSettings } from "./generated/CoinJoinSettings";
import type { CoinJoinStatus } from "./generated/CoinJoinStatus";
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
import type { FilePurpose } from "./generated/FilePurpose";
import type { OpenedFile } from "./generated/OpenedFile";
import type { AddressBookEntry } from "./generated/AddressBookEntry";
import type { AddressPurposeName } from "./generated/AddressPurposeName";
import type { ConsoleCommand } from "./generated/ConsoleCommand";
import type { ConsoleOutput } from "./generated/ConsoleOutput";
import type { NodeSummary } from "./generated/NodeSummary";
import type { NodeWarning } from "./generated/NodeWarning";
import type { PeerSummary } from "./generated/PeerSummary";
import type { PsbtAnalysis } from "./generated/PsbtAnalysis";
import type { RegistrationQuote } from "./generated/RegistrationQuote";
import type { RegistrationRequest } from "./generated/RegistrationRequest";
import type { RegistrationStatus } from "./generated/RegistrationStatus";
import type { UsernameCheck } from "./generated/UsernameCheck";
import type { NameAvailability } from "./generated/NameAvailability";
import type { FinishReport } from "./generated/FinishReport";
import type { NameOutcome } from "./generated/NameOutcome";
import type { GovernanceClock } from "./generated/GovernanceClock";
import type { GovernanceInfo } from "./generated/GovernanceInfo";
import type { GovernanceSyncState } from "./generated/GovernanceSyncState";
import type { MyProposal } from "./generated/MyProposal";
import type { ProposalDetail } from "./generated/ProposalDetail";
import type { ProposalDraft } from "./generated/ProposalDraft";
import type { ProposalPreview } from "./generated/ProposalPreview";
import type { ProposalQuery } from "./generated/ProposalQuery";
import type { ProposalRow } from "./generated/ProposalRow";
import type { SuperblockDate } from "./generated/SuperblockDate";
import type { VoteCast } from "./generated/VoteCast";
import type { ContestedNames } from "./generated/ContestedNames";
import type { NameChoice } from "./generated/NameChoice";
import type { NameVoteCast } from "./generated/NameVoteCast";
import type { VoteOutcomeName } from "./generated/VoteOutcomeName";
import type { VotingMasternode } from "./generated/VotingMasternode";
import type { RescanStart } from "./generated/RescanStart";
import type { RescanStatus } from "./generated/RescanStatus";
import type { VerifyMessageResult } from "./generated/VerifyMessageResult";
import type { AccountXpub } from "./generated/AccountXpub";
import type { BackupFileHeader } from "./generated/BackupFileHeader";
import type { BackupPolicyInfo } from "./generated/BackupPolicyInfo";
import type { BackupSummary } from "./generated/BackupSummary";
import type { CoreExportFormatName } from "./generated/CoreExportFormatName";
import type { CoreExportReport } from "./generated/CoreExportReport";
import type { CoreImportReport } from "./generated/CoreImportReport";
import type { CoreMnemonicCheck } from "./generated/CoreMnemonicCheck";
import type { KeyMaterialInput } from "./generated/KeyMaterialInput";
import type { UiSettings } from "./generated/UiSettings";
import type { WalletFileInfo } from "./generated/WalletFileInfo";
import type { WalletLoadStatus } from "./generated/WalletLoadStatus";
import type { WatchOnlyOptions } from "./generated/WatchOnlyOptions";
import type { UserHit } from "./generated/UserHit";
import type { ActivityFilterName } from "./generated/ActivityFilterName";
import type { ContactActivityPage } from "./generated/ContactActivityPage";
import type { ContactRecipient } from "./generated/ContactRecipient";
import type { ContactSummaryInfo } from "./generated/ContactSummaryInfo";
import type { FlowKindName } from "./generated/FlowKindName";
import type { LockResolutionName } from "./generated/LockResolutionName";
import type { ContactDetailInfo } from "./generated/ContactDetailInfo";
import type { ContactRequestOutcome } from "./generated/ContactRequestOutcome";
import type { CostTable } from "./generated/CostTable";
import type { ContestStatus } from "./generated/ContestStatus";
import type { AvatarSizeName } from "./generated/AvatarSizeName";
import type { AvatarThumbnail } from "./generated/AvatarThumbnail";
import type { AvatarCandidateInfo } from "./generated/AvatarCandidateInfo";
import type { AvatarSourceArgs } from "./generated/AvatarSourceArgs";
import type { DashPayProfileLimits } from "./generated/DashPayProfileLimits";
import type { ProfileEditArgs } from "./generated/ProfileEditArgs";
import type { PendingTopUp } from "./generated/PendingTopUp";
import type { StuckTopUpFunding } from "./generated/StuckTopUpFunding";
import type { TopUpOutcome } from "./generated/TopUpOutcome";
import type { TopUpQuote } from "./generated/TopUpQuote";
import type { DashPayEventPage } from "./generated/DashPayEventPage";
import type { GrantAction } from "./generated/GrantAction";
import type { GrantRequest } from "./generated/GrantRequest";
import type { PrivateContactDetails } from "./generated/PrivateContactDetails";
import type { PublishStateName } from "./generated/PublishStateName";

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
  /** One credential check for several grants, all or none (E0-04: "Accept and pay"). */
  authorizeSet(purposes: GrantPurposeSpec[], walletId: string | null, credential: CredentialSpec): Promise<AuthGrant[]>;
  /** Redeems grants into a flow's lease; returns the lease id a write or a prepare takes. */
  beginFlow(walletId: string, flow: FlowKindName, grants: string[]): Promise<string>;
  /** Ends a lease (idempotent). */
  endFlow(lease: string): Promise<null>;
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
  /** "Include my username in payment requests" (DP3-03b); off, the username above is `null`
   * and requests carry none. */
  usernameInRequests(walletId: string): Promise<boolean>;
  setUsernameInRequests(walletId: string, on: boolean): Promise<null>;
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
  /** A DashPay contact as the only recipient (DP3-01): its address is reserved at prepare. */
  draftSetContactRecipient(draft: number, recipient: ContactRecipient): Promise<null>;
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
  // CoinJoin (U-06): the network's options, and mixing per wallet.
  coinjoinLimits(): Promise<CoinJoinLimits>;
  coinjoinSettings(): Promise<CoinJoinSettings>;
  /** Applies them live and stores them for the network; `enabled: false` stops every wallet. */
  setCoinjoinSettings(settings: CoinJoinSettings): Promise<null>;
  coinjoinStatus(walletId: string): Promise<CoinJoinStatus>;
  startMixing(walletId: string): Promise<null>;
  stopMixing(walletId: string): Promise<null>;
  dashpayStatus(walletId: string): Promise<DashPayStatus>;
  identities(walletId: string): Promise<IdentitySummary[]>;
  dashpayUnreadCount(walletId: string, identity: string): Promise<number>;
  dashpayContacts(walletId: string, identity: string, query: ContactQueryArgs): Promise<ContactList>;
  /** Contacts waiting for an unlock to finish their crypto (DP2-07 D8). */
  dashpayPendingSetupCount(walletId: string): Promise<number>;
  /** Asks for a DashPay pass now, in the background (Contacts opened). */
  dashpaySyncSoon(): Promise<null>;
  /** Tools ▸ Information's DashPay card (DP6-03): the bring-up, the last pass, the loops, the quorum source. */
  dashpaySyncStatus(walletId: string): Promise<DashPaySyncStatus>;
  /** Where Platform proofs get their quorum keys, without a wallet (DP6-03). */
  quorumSource(): Promise<QuorumSource>;
  /** The developer toggle: quorum keys from the trusted service only (DP6-03 D5). */
  trustedQuorumsOnly(): Promise<boolean>;
  /** DPNS usernames starting with `prefix` (DP2-03). */
  searchUsers(walletId: string, prefix: string, limit: number): Promise<UserHit[]>;
  // The contact pane and the bell (DP2-09).
  dashpayContact(walletId: string, identity: string, contact: string): Promise<ContactDetailInfo | null>;
  contactActivity(walletId: string, identity: string, contact: string, cursor: string | null, filter: ActivityFilterName): Promise<ContactActivityPage>;
  // Pay ▸ To a contact (DP3-06).
  /** The frequent strip (DP3-02): most paid, then most paying, in the last 90 days. */
  frequentContacts(walletId: string, identity: string, limit: number): Promise<ContactSummaryInfo[]>;
  /** Looks for the locked payment's outcome; `sent` and `notSent` clear the lock (DP3-01). */
  resolveContactPaymentLock(walletId: string, identity: string, contact: string): Promise<LockResolutionName>;
  /** The caps of the `platformOp` grant a DashPay write needs. */
  grantRequest(walletId: string, identity: string, action: GrantAction): Promise<GrantRequest>;
  creditsCostTable(walletId: string): Promise<CostTable>;
  // Add Contact (DP2-08).
  dashpayEligibility(walletId: string, identity: string, contact: string): Promise<EligibilityName>;
  /** `scan` is `dashpayVerifyScanned`'s claim id when the request answers an auto-accept code. */
  dashpaySendRequest(walletId: string, identity: string, to: string, scan: string | null, grant: string): Promise<ContactRequestOutcome>;
  dashpayMyUserLink(walletId: string, identity: string): Promise<string>;
  /** `text` may carry a bearer key: never in a query key, the store or a log. */
  dashpayVerifyScanned(walletId: string, text: string): Promise<ScannedContactInfo>;
  screenScanMode(): Promise<ScreenScanMode>;
  /** The QR texts of an image the open dialog chose; `null` when cancelled. */
  scanQrFile(): Promise<string[] | null>;
  /** The QR texts of an image (base64). */
  scanQrImage(data: string): Promise<string[]>;
  /** The QR texts of a screen region the user picks; `null` when cancelled. */
  scanQrScreen(): Promise<string[] | null>;
  cancelScreenScan(): Promise<null>;
  dashpayAcceptRequest(walletId: string, identity: string, from: string, grant: string): Promise<ContactRequestOutcome>;
  dashpayIgnoreContact(walletId: string, identity: string, contact: string): Promise<null>;
  dashpayUnignoreContact(walletId: string, identity: string, contact: string): Promise<null>;
  dashpaySetPrivateDetails(walletId: string, identity: string, contact: string, details: PrivateContactDetails, grant: string | null): Promise<PublishStateName>;
  dashpayEnableKeys(walletId: string, identity: string, grant: string): Promise<null>;
  dashpayEvents(walletId: string, identity: string, cursor: number | null, limit: number): Promise<DashPayEventPage>;
  dashpayMarkRead(walletId: string, identity: string, upTo: number): Promise<null>;
  /** The queued deep links, routed for `network` (U-01). */
  takeLinks(network: string): Promise<LinkRoute[]>;
  /** Where `text` leads (File ▸ Open URI…). */
  routeLink(network: string, text: string): Promise<LinkRoute>;
  /** The link stash (DP5-01): what waits, without its text; open one (a payment or contact
   * link once, `null` when gone); dismiss one. */
  stashedLinks(): Promise<StashedLinkInfo[]>;
  openStashedLink(linkId: string): Promise<LinkRoute | null>;
  dismissStashedLink(linkId: string): Promise<null>;
  /** What the native menus and the tray mirror. */
  shellSync(sync: ShellSync): Promise<null>;
  // Tools (U-05); `nodeInfo` above.
  warnings(): Promise<NodeWarning[]>;
  consoleCommands(): Promise<ConsoleCommand[]>;
  /** The line with sensitive arguments replaced by `(…)`; `console.parse_error` when it does not parse. */
  consoleRedact(line: string): Promise<string>;
  consoleExecute(walletId: string | null, line: string, grantId: string | null): Promise<ConsoleOutput>;
  peers(): Promise<PeerSummary[]>;
  /** Drops every connection and dials again. */
  rotatePeers(): Promise<null>;
  rescan(from: RescanStart): Promise<null>;
  rescanProgress(): Promise<RescanStatus | null>;
  cancelRescan(): Promise<boolean>;
  stopSpv(): Promise<null>;
  startSpv(): Promise<null>;
  /** Deletes the headers and filters; SPV must be stopped (`spv_running`). */
  resetChainData(): Promise<null>;
  setBirthHeight(walletId: string, height: number): Promise<null>;
  /** Abandons the unconfirmed transactions the network does not have (`null`: every wallet) and
   * schedules a rescan; returns how many. */
  dropUnconfirmed(walletId: string | null): Promise<number>;
  registrations(walletId: string): Promise<RegistrationStatus[]>;
  /** The costs of registering `request` (DP1-02); local for a non-contested name. */
  registrationQuote(walletId: string, request: RegistrationRequest): Promise<RegistrationQuote>;
  resolveStuckFunding(walletId: string, step: string, grant: string): Promise<BroadcastOutcome>;
  // The registration wizard (DP1-09).
  /** The label's rule checklist; offline, needs no wallet. */
  checkUsername(label: string): Promise<UsernameCheck>;
  /** The label's checklist verdict, then the network's. */
  nameAvailability(walletId: string, label: string): Promise<NameAvailability>;
  /** Starts a registration under `grant`; the draft id. Progress is `registrations`. */
  startRegistration(walletId: string, request: RegistrationRequest, grant: string): Promise<string>;
  /** `startRegistration` past the earlier registrations `past` whose funding is unknown, once the
   * user confirmed the wedge copy (m4 §2.2 "In progress"). */
  startRegistrationPastUnknownFunding(walletId: string, request: RegistrationRequest, grant: string, past: string[]): Promise<string>;
  /** `grant` is needed exactly when the row waits `unlock` or `authorize`. */
  resumeRegistration(walletId: string, draft: string, grant: string | null): Promise<null>;
  discardRegistration(walletId: string, draft: string): Promise<null>;
  /** Tools ▸ Repair "Finish transfers". */
  finishAssetLocks(walletId: string, grant: string): Promise<FinishReport>;
  /** A name for `identity`, paid from its credits. */
  registerName(walletId: string, identity: string, label: string, grant: string): Promise<NameOutcome>;
  // My Profile and the top-up (DP1-10).
  /** Asks Platform for the identity's balance; the answer arrives as `Platform{Identities}`. */
  identityRefreshBalance(walletId: string, identity: string): Promise<string | null>;
  contestStatus(walletId: string, identity: string, label: string): Promise<ContestStatus>;
  /** Any identity's avatar thumbnail, verified against its profile (DP4-02); `null` without one or while
   * "Load contact pictures" is off; `avatar.hash_mismatch` and the fetch errors show initials. */
  dashpayAvatar(walletId: string, identity: string, size: AvatarSizeName): Promise<AvatarThumbnail | null>;
  // Edit Profile and the pictures toggle (DP4-03).
  dashpayProfileLimits(walletId: string): Promise<DashPayProfileLimits>;
  /** Publishes the whole profile under a `platformOp` grant; nothing to change uses no grant. */
  dashpayUpdateProfile(walletId: string, identity: string, edit: ProfileEditArgs, grant: string): Promise<null>;
  /** A file, URL or Gravatar as an engine-re-encoded candidate (DP4-02); a URL is fetched now. */
  dashpayPrepareAvatar(walletId: string, source: AvatarSourceArgs): Promise<AvatarCandidateInfo>;
  dashpayAvatarUploadAvailable(walletId: string): Promise<boolean>;
  /** Uploads a `file` candidate; the URL it is published under. */
  dashpayUploadAvatar(walletId: string, candidate: string): Promise<string>;
  /** "Load contact pictures" for the window's network (DP4-02 D17). */
  loadContactPictures(): Promise<boolean>;
  setLoadContactPictures(on: boolean): Promise<null>;
  creditsTopUpQuote(walletId: string, identity: string, duffs: number): Promise<TopUpQuote>;
  creditsTopUp(walletId: string, identity: string, duffs: number, grant: string): Promise<TopUpOutcome>;
  creditsPendingTopUps(walletId: string): Promise<PendingTopUp[]>;
  /** Top-up fundings whose outcome is unknown: Tools ▸ Repair resolves them. */
  creditsStuckTopUpFundings(walletId: string): Promise<StuckTopUpFunding[]>;
  creditsResumeTopUp(walletId: string, txid: string, grant: string): Promise<TopUpOutcome>;
  creditsResolveTopUp(walletId: string, txid: string): Promise<null>;
  addressBook(walletId: string, purpose: AddressPurposeName | null): Promise<AddressBookEntry[]>;
  saveAddressBookEntry(walletId: string, address: string, label: string, purpose: AddressPurposeName, replace: boolean): Promise<AddressBookEntry>;
  deleteAddressBookEntry(walletId: string, address: string): Promise<null>;
  signMessage(walletId: string, address: string, message: string, grantId: string): Promise<string>;
  verifyMessage(address: string, signature: string, message: string): Promise<VerifyMessageResult>;
  analyzePsbt(walletId: string | null, psbt: string): Promise<PsbtAnalysis>;
  signPsbt(walletId: string, psbt: string, grantId: string): Promise<string>;
  broadcastPsbt(psbt: string): Promise<string>;
  /** dash-qt "Create Unsigned": the draft as a base64 PSBT; signs and reserves nothing. */
  draftCreateUnsigned(draft: number): Promise<string>;
  lockOutpoints(walletId: string, outpoints: OutPoint[]): Promise<null>;
  unlockOutpoints(walletId: string, outpoints: OutPoint[]): Promise<null>;
  // Files from the OS's dialogs (U-05 D2; screens go through app/files.ts).
  openFile(purpose: FilePurpose): Promise<OpenedFile | null>;
  saveFileAs(purpose: FilePurpose, suggestedName: string, data: string): Promise<string | null>;
  chooseFilePath(purpose: FilePurpose, save: boolean, suggestedName: string | null): Promise<string | null>;
  // Governance (MG-09 over BL-232's binding; mg-engine §2). Reads answer `governance.not_synced`
  // until a run reached `synced`; the clock, the info and the sync state need only the tip.
  governanceSyncEnabled(): Promise<boolean>;
  governanceSetSyncEnabled(enabled: boolean): Promise<null>;
  governanceSyncState(): Promise<GovernanceSyncState>;
  governanceResync(): Promise<null>;
  governanceClock(): Promise<GovernanceClock>;
  governanceInfo(): Promise<GovernanceInfo>;
  /** The next `count` superblocks (at most 24), for the proposal form. */
  governanceUpcomingSuperblocks(count: number): Promise<SuperblockDate[]>;
  governanceProposals(query: ProposalQuery): Promise<ProposalRow[]>;
  governanceProposal(hash: string): Promise<ProposalDetail>;
  /** The masternodes whose voting key the wallet's keychain derives, with their votes seen on `proposal`. */
  governanceVotingMasternodes(walletId: string, proposal: string): Promise<VotingMasternode[]>;
  /** One vote per masternode under one `governance` grant; one result each, in order. */
  governanceVote(walletId: string, proposal: string, outcome: VoteOutcomeName, masternodes: string[], grant: string): Promise<VoteCast[]>;
  governancePreviewProposal(walletId: string, draft: ProposalDraft): Promise<ProposalPreview>;
  /** Pays the 1 DASH fee under a `spend` grant and saves the proposal; it submits on its own. */
  governanceCreateProposal(walletId: string, draft: ProposalDraft, grant: string): Promise<MyProposal>;
  governanceMyProposals(walletId: string): Promise<MyProposal[]>;
  /** [Submit] (MG-06 D24). */
  governanceSubmitProposal(walletId: string, hash: string): Promise<MyProposal>;
  // Contested DPNS names (MG-11 over MG-07): need the SPV masternode list and Platform, not
  // governance sync.
  /** The open contests, the wallet's masternodes that can vote on names, and the vote limit. */
  governanceContestedNames(walletId: string): Promise<ContestedNames>;
  /** One name vote per masternode under one `governance` grant; one result each, in order. */
  governanceVoteName(walletId: string, label: string, choice: NameChoice, masternodes: string[], grant: string): Promise<NameVoteCast[]>;
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
