import DashWalletCore
import Foundation

/// The engine surface the runtime layer uses (docs/contracts/m1-engine.md §2).
///
/// `EngineClient` is the production conformance over the UniFFI bindings.
/// Tests supply fakes, so WalletRuntime never depends on the concrete class.
/// Session-scoped calls name their network; a network that is not open fails
/// with `network_not_open`. Calls whose engine side has not landed fail with
/// `not_implemented`.
public protocol EngineProtocol: AnyObject, Sendable {
    /// Engine signals; consumers re-query when one arrives.
    var events: EventBus { get }
    /// Data directory of `network` ("Open data folder").
    func directory(for network: DashNetwork) -> URL

    // MARK: Sessions

    /// Opens `network` (idempotent). `options` apply only when this call opens it.
    func open(_ network: DashNetwork, options: SessionOptions) async throws(DashKitError)
    /// Closes `network`; `false` if it was not open.
    @discardableResult
    func close(_ network: DashNetwork) async throws(DashKitError) -> Bool
    /// Closes every network, ends the event streams and releases the engine.
    func shutdown() async throws(DashKitError)
    func isOpen(_ network: DashNetwork) async -> Bool
    func startSPV(on network: DashNetwork) async throws(DashKitError)
    func stopSPV(on network: DashNetwork) async throws(DashKitError)
    func isSPVRunning(on network: DashNetwork) async throws(DashKitError) -> Bool

    // MARK: Wallets

    func importWallet(
        on network: DashNetwork, mnemonic: SecretBytes, bip39Passphrase: SecretBytes, options: ImportOptions
    ) async throws(DashKitError) -> WalletID
    func walletInfos(on network: DashNetwork) async throws(DashKitError) -> [WalletInfo]
    /// `nil` until the wallet's scan has passed its birth height (unknown, not zero).
    func balances(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> WalletBalances?
    func renameWallet(on network: DashNetwork, wallet: WalletID, name: String) async throws(DashKitError)
    func removeWallet(on network: DashNetwork, wallet: WalletID, grantID: String) async throws(DashKitError)
    func generateMnemonic(wordCount: Int, language: MnemonicLanguage) throws(DashKitError) -> SecretBytes
    func checkMnemonic(_ phrase: SecretBytes) throws(DashKitError) -> MnemonicCheck

    // MARK: Vault

    func vaultStatus(on network: DashNetwork) async throws(DashKitError) -> VaultStatus
    func createVault(on network: DashNetwork, passphrase: SecretBytes?) async throws(DashKitError) -> VaultStatus
    func encryptVault(on network: DashNetwork, newPassphrase: SecretBytes, grantID: String) async throws(DashKitError)
        -> VaultStatus
    func changeVaultPassphrase(on network: DashNetwork, old: SecretBytes, new: SecretBytes) async throws(DashKitError)
        -> VaultStatus
    func unlockVault(on network: DashNetwork, passphrase: SecretBytes, scope: UnlockScope) async throws(DashKitError)
        -> VaultStatus
    func lockVault(on network: DashNetwork) async throws(DashKitError) -> VaultStatus
    /// `wallet` is the wallet the grant is for: required for every purpose
    /// except `.changeCredential`, which takes `nil` (else `invalid_argument`).
    func authorize(on network: DashNetwork, purpose: GrantPurpose, wallet: WalletID?, credential: VaultCredential)
        async throws(DashKitError) -> AuthGrant
    func revokeGrant(on network: DashNetwork, grantID: String) async throws(DashKitError)
    func revealMnemonic(on network: DashNetwork, wallet: WalletID, grantID: String) async throws(DashKitError)
        -> RevealedMnemonic

    // MARK: Vault (M2: slot B, recovery, destroy)

    /// In-memory read; works while locked.
    func quickUnlockPolicy(on network: DashNetwork) async throws(DashKitError) -> QuickUnlockPolicy
    /// Adds slot B and returns its wrap key for the OS biometric store.
    /// Needs a `.changeCredential` grant; macOS only.
    func enrollQuickUnlock(on network: DashNetwork, grantID: String) async throws(DashKitError) -> SecretBytes
    func removeQuickUnlock(on network: DashNetwork) async throws(DashKitError) -> VaultStatus
    /// One of the iOS options; `.changeCredential` grant.
    func setQuickUnlockSpendLimit(on network: DashNetwork, grantID: String, limit: Amount)
        async throws(DashKitError) -> QuickUnlockPolicy
    /// Forgot passphrase (IOS-014).
    func recoverVault(
        on network: DashNetwork, wallet: WalletID, mnemonic: SecretBytes, bip39Passphrase: SecretBytes,
        newPassphrase: SecretBytes
    ) async throws(DashKitError) -> VaultRecovery
    /// Deletes an empty vault (IOS-009/109).
    func destroyVault(on network: DashNetwork, credential: VaultCredential) async throws(DashKitError) -> VaultStatus

    // MARK: Notifications and logs (M2)

    /// Rows of a `newTransactions` event; unknown txids are skipped.
    func txNotices(on network: DashNetwork, wallet: WalletID, txids: [String]) async throws(DashKitError)
        -> [TxNotice]
    /// Zips the log files plus `extraFiles` into `file` (IOS-112).
    func exportLogs(to file: URL, extraFiles: [URL]) async throws(DashKitError) -> LogExport

    // MARK: Sync

    func syncSnapshot(on network: DashNetwork) async throws(DashKitError) -> SyncSnapshot
    func peers(on network: DashNetwork) async throws(DashKitError) -> [PeerInfo]
    func rotatePeers(on network: DashNetwork) async throws(DashKitError)
    func rescan(on network: DashNetwork, from start: RescanStart) async throws(DashKitError)

    // MARK: History

    func historyPage(on network: DashNetwork, wallet: WalletID, query: HistoryQuery) async throws(DashKitError)
        -> HistoryPage
    func txDetail(on network: DashNetwork, wallet: WalletID, txid: String) async throws(DashKitError) -> TxDetail
    func setTxLabel(on network: DashNetwork, wallet: WalletID, txid: String, label: String?) async throws(DashKitError)

    // MARK: Receive

    func currentReceiveAddress(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> AddressInfo
    func nextReceiveAddress(on network: DashNetwork, wallet: WalletID, label: String?) async throws(DashKitError)
        -> AddressInfo
    func addresses(on network: DashNetwork, wallet: WalletID, filter: AddressFilter) async throws(DashKitError)
        -> [AddressInfo]
    func createReceiveRequest(
        on network: DashNetwork, wallet: WalletID, amount: Amount?, label: String?, message: String?
    ) async throws(DashKitError) -> ReceiveRequest
    func receiveRequests(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> [ReceiveRequest]
    func deleteReceiveRequest(on network: DashNetwork, wallet: WalletID, id: UInt64) async throws(DashKitError)

    // MARK: Send

    /// An empty draft: source any, recommended fee, automatic change.
    func newTxDraft(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> any TxDraftHandle
    func maxSpendable(on network: DashNetwork, wallet: WalletID, source: CoinSource, fee: FeeMode)
        async throws(DashKitError) -> Amount

    // MARK: Coins and labels

    func utxos(on network: DashNetwork, wallet: WalletID, filter: UtxoFilter) async throws(DashKitError) -> [Utxo]
    func lockOutpoints(on network: DashNetwork, wallet: WalletID, outpoints: [OutPoint]) async throws(DashKitError)
    func unlockOutpoints(on network: DashNetwork, wallet: WalletID, outpoints: [OutPoint]) async throws(DashKitError)
    func lockedOutpoints(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> [OutPoint]
    func addressBook(on network: DashNetwork, wallet: WalletID, purpose: AddressPurpose?, search: String?)
        async throws(DashKitError) -> [AddressBookEntry]
    func saveAddressBookEntry(
        on network: DashNetwork, wallet: WalletID, address: String, label: String, purpose: AddressPurpose,
        replace: Bool
    ) async throws(DashKitError) -> AddressBookEntry
    func deleteAddressBookEntry(on network: DashNetwork, wallet: WalletID, address: String) async throws(DashKitError)

    // MARK: Messages

    /// Base64 compact signature; needs a `signMessage` grant.
    func signMessage(on network: DashNetwork, wallet: WalletID, address: String, message: String, grantID: String)
        async throws(DashKitError) -> String
}

/// One editable payment (engine `TxDraft`). `prepare` builds, signs and
/// reserves inputs; it never broadcasts. Only `broadcast` sends.
public protocol TxDraftHandle: AnyObject, Sendable {
    var walletID: WalletID { get }
    func setRecipients(_ recipients: [Recipient]) async throws(DashKitError)
    func setSource(_ source: CoinSource) async throws(DashKitError)
    func setFee(_ fee: FeeMode) async throws(DashKitError)
    func setChange(_ change: ChangePolicy) async throws(DashKitError)
    func estimate() async throws(DashKitError) -> TxEstimate
    func prepare(grantID: String) async throws(DashKitError) -> PreparedTxHandle
    func broadcast(_ prepared: PreparedTxHandle) async throws(DashKitError) -> BroadcastOutcome
    /// Releases the reserved inputs. Idempotent.
    func abandon(_ prepared: PreparedTxHandle) async throws(DashKitError)
}

/// A signed, not yet broadcast transaction (engine `PreparedTx`).
public final class PreparedTxHandle: Sendable {
    public let summary: PreparedTxSummary
    /// The engine object; `nil` for handles a test fake created.
    let engineObject: DashWalletCore.PreparedTx?

    /// For engine fakes. A handle made here cannot be broadcast by `EngineClient`.
    public init(summary: PreparedTxSummary) {
        self.summary = summary
        engineObject = nil
    }

    init(_ engineObject: DashWalletCore.PreparedTx) throws(DashKitError) {
        summary = try PreparedTxSummary(engineObject.summary())
        self.engineObject = engineObject
    }
}
