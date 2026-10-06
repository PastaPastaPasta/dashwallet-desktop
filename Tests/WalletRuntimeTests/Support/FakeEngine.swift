// An in-memory `EngineProtocol` for the runtime tests. Only DashKit is
// imported here, so every type below is DashKit's.
//
// Session calls fail with `network_not_open` unless the network was opened.
// Calls a test has not configured fail with `not_implemented`, like engine
// stubs, so a test never sees invented success.
import DashKit
import Foundation

final class FakeEngine: EngineProtocol, @unchecked Sendable {
    struct State {
        var open: Set<DashNetwork> = []
        var spvRunning: Set<DashNetwork> = []
        var calls: [String] = []
        var openError: DashKitError?
        var walletInfos: Result<[WalletInfo], DashKitError> = .failure(.notImplemented(detail: "wallet_infos"))
        var balances: [WalletID: WalletBalances] = [:]
        var vault: VaultStatus?
        var issuedGrants: [String] = []
        var revokedGrants: [String] = []
        var snapshot: Result<SyncSnapshot, DashKitError> = .failure(.notImplemented(detail: "sync_snapshot"))
        var imported: [WalletID] = []
        var drafts: [FakeTxDraft] = []
        var nextDraft: FakeTxDraft?
        var historyPage: Result<HistoryPage, DashKitError> = .failure(.notImplemented(detail: "history_page"))
        var signature: Result<String, DashKitError> = .failure(.notImplemented(detail: "sign_message"))
    }

    let events = EventBus()
    let root = URL(fileURLWithPath: "/tmp/fake-engine", isDirectory: true)
    private let lock = NSLock()
    private var state = State()
    /// Replaces `authorize` (e.g. to make it slow); `nil` issues a grant at once.
    var authorizeHandler: (@Sendable (GrantPurpose) async -> Result<AuthGrant, DashKitError>)? {
        get { lock.withLock { _authorizeHandler } }
        set { lock.withLock { _authorizeHandler = newValue } }
    }
    private var _authorizeHandler: (@Sendable (GrantPurpose) async -> Result<AuthGrant, DashKitError>)?

    func with<T>(_ body: (inout State) -> T) -> T {
        lock.withLock { body(&state) }
    }

    var calls: [String] { with { $0.calls } }

    private func record(_ name: String) {
        with { $0.calls.append(name) }
    }

    private func requireOpen(_ network: DashNetwork, _ name: String) throws(DashKitError) {
        record(name)
        guard with({ $0.open.contains(network) }) else { throw .networkNotOpen(detail: "\(network)") }
    }

    private func notImplemented(_ name: String) throws(DashKitError) -> Never {
        throw .notImplemented(detail: "FakeEngine.\(name)")
    }

    func directory(for network: DashNetwork) -> URL {
        root.appendingPathComponent(network.description, isDirectory: true)
    }

    // MARK: Sessions

    func open(_ network: DashNetwork, options: SessionOptions) async throws(DashKitError) {
        record("open \(network)")
        if let error = with({ $0.openError }) { throw error }
        let opened = with { $0.open.insert(network).inserted }
        if opened { events.publish(.sessionOpened(network)) }
    }

    func close(_ network: DashNetwork) async throws(DashKitError) -> Bool {
        record("close \(network)")
        let closed = with { s in
            s.spvRunning.remove(network)
            return s.open.remove(network) != nil
        }
        if closed { events.publish(.sessionClosed(network)) }
        return closed
    }

    func shutdown() async throws(DashKitError) {
        record("shutdown")
        with { $0.open.removeAll() }
        events.finish()
    }

    func isOpen(_ network: DashNetwork) async -> Bool {
        with { $0.open.contains(network) }
    }

    func startSPV(on network: DashNetwork) async throws(DashKitError) {
        try requireOpen(network, "startSPV \(network)")
        with { _ = $0.spvRunning.insert(network) }
        events.publish(.spvStateChanged(network, running: true))
    }

    func stopSPV(on network: DashNetwork) async throws(DashKitError) {
        try requireOpen(network, "stopSPV \(network)")
        with { _ = $0.spvRunning.remove(network) }
        events.publish(.spvStateChanged(network, running: false))
    }

    func isSPVRunning(on network: DashNetwork) async throws(DashKitError) -> Bool {
        try requireOpen(network, "isSPVRunning")
        return with { $0.spvRunning.contains(network) }
    }

    // MARK: Wallets

    func importWallet(
        on network: DashNetwork, mnemonic: SecretBytes, bip39Passphrase: SecretBytes, options: ImportOptions
    ) async throws(DashKitError) -> WalletID {
        try requireOpen(network, "importWallet")
        let hex = String(repeating: String(format: "%02x", with { $0.imported.count + 1 }), count: 32)
        let id = WalletID(hex: hex)!
        with { $0.imported.append(id) }
        events.publish(.walletCreated(network, id))
        return id
    }

    func walletInfos(on network: DashNetwork) async throws(DashKitError) -> [WalletInfo] {
        try requireOpen(network, "walletInfos")
        return try with { $0.walletInfos }.get()
    }

    func balances(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> WalletBalances {
        try requireOpen(network, "balances")
        guard let balances = with({ $0.balances[wallet] }) else { throw .walletNotFound(detail: wallet.hex) }
        return balances
    }

    func renameWallet(on network: DashNetwork, wallet: WalletID, name: String) async throws(DashKitError) {
        try requireOpen(network, "renameWallet")
        try notImplemented("renameWallet")
    }

    func removeWallet(on network: DashNetwork, wallet: WalletID, grantID: String) async throws(DashKitError) {
        try requireOpen(network, "removeWallet \(grantID)")
        try notImplemented("removeWallet")
    }

    func generateMnemonic(wordCount: Int, language: MnemonicLanguage) throws(DashKitError) -> SecretBytes {
        record("generateMnemonic \(wordCount)")
        return try CoreFunctions.generateMnemonic(wordCount: wordCount, language: language)
    }

    func checkMnemonic(_ phrase: SecretBytes) throws(DashKitError) -> MnemonicCheck {
        try CoreFunctions.checkMnemonic(phrase)
    }

    // MARK: Vault

    private func vaultOrNoVault() -> VaultStatus {
        with { $0.vault } ?? VaultStatus(
            state: .noVault, encrypted: false, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSeconds: nil,
            walletsWithSecrets: [])
    }

    private func setVault(_ state: VaultLockState, encrypted: Bool, on network: DashNetwork) -> VaultStatus {
        let status = VaultStatus(
            state: state, encrypted: encrypted, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSeconds: nil,
            walletsWithSecrets: [])
        with { $0.vault = status }
        events.publish(.lockStateChanged(network))
        return status
    }

    func vaultStatus(on network: DashNetwork) async throws(DashKitError) -> VaultStatus {
        try requireOpen(network, "vaultStatus")
        return vaultOrNoVault()
    }

    func createVault(on network: DashNetwork, passphrase: SecretBytes?) async throws(DashKitError) -> VaultStatus {
        try requireOpen(network, "createVault")
        guard with({ $0.vault }) == nil else { throw .domain(code: "vault.already_exists", detail: "") }
        return setVault(passphrase == nil ? .unencrypted : .unlocked, encrypted: passphrase != nil, on: network)
    }

    func encryptVault(on network: DashNetwork, newPassphrase: SecretBytes, grantID: String) async throws(DashKitError)
        -> VaultStatus
    {
        try requireOpen(network, "encryptVault")
        return setVault(.unlocked, encrypted: true, on: network)
    }

    func changeVaultPassphrase(on network: DashNetwork, old: SecretBytes, new: SecretBytes) async throws(DashKitError)
        -> VaultStatus
    {
        try requireOpen(network, "changeVaultPassphrase")
        return vaultOrNoVault()
    }

    func unlockVault(on network: DashNetwork, passphrase: SecretBytes, scope: UnlockScope) async throws(DashKitError)
        -> VaultStatus
    {
        try requireOpen(network, "unlockVault")
        guard passphrase.utf8String() == "correct" else {
            throw .vaultAttempt(code: "vault.wrong_passphrase", failedAttempts: 1, retryAfterSeconds: nil)
        }
        return setVault(scope == .full ? .unlocked : .unlockedMixingOnly, encrypted: true, on: network)
    }

    func lockVault(on network: DashNetwork) async throws(DashKitError) -> VaultStatus {
        try requireOpen(network, "lockVault")
        return setVault(.locked, encrypted: true, on: network)
    }

    func authorize(on network: DashNetwork, purpose: GrantPurpose, credential: VaultCredential)
        async throws(DashKitError) -> AuthGrant
    {
        try requireOpen(network, "authorize")
        let result: Result<AuthGrant, DashKitError>
        if let handler = authorizeHandler {
            result = await handler(purpose)
        } else {
            result = .success(
                AuthGrant(
                    id: "grant-\(with { $0.issuedGrants.count + 1 })", purpose: purpose,
                    expiresAt: Date().addingTimeInterval(60), singleUse: true))
        }
        let grant = try result.get()
        with { $0.issuedGrants.append(grant.id) }
        return grant
    }

    func revokeGrant(on network: DashNetwork, grantID: String) async throws(DashKitError) {
        try requireOpen(network, "revokeGrant")
        with { $0.revokedGrants.append(grantID) }
    }

    func revealMnemonic(on network: DashNetwork, wallet: WalletID, grantID: String) async throws(DashKitError)
        -> RevealedMnemonic
    {
        try requireOpen(network, "revealMnemonic")
        return RevealedMnemonic(phrase: SecretBytes(utf8: "phrase"), bip39Passphrase: SecretBytes([]))
    }

    // MARK: Sync

    func syncSnapshot(on network: DashNetwork) async throws(DashKitError) -> SyncSnapshot {
        try requireOpen(network, "syncSnapshot")
        return try with { $0.snapshot }.get()
    }

    func peers(on network: DashNetwork) async throws(DashKitError) -> [PeerInfo] {
        try requireOpen(network, "peers")
        try notImplemented("peers")
    }

    func rotatePeers(on network: DashNetwork) async throws(DashKitError) {
        try requireOpen(network, "rotatePeers")
        try notImplemented("rotatePeers")
    }

    func rescan(on network: DashNetwork, from start: RescanStart) async throws(DashKitError) {
        try requireOpen(network, "rescan")
        try notImplemented("rescan")
    }

    // MARK: History

    func historyPage(on network: DashNetwork, wallet: WalletID, query: HistoryQuery) async throws(DashKitError)
        -> HistoryPage
    {
        try requireOpen(network, "historyPage")
        return try with { $0.historyPage }.get()
    }

    func txDetail(on network: DashNetwork, wallet: WalletID, txid: String) async throws(DashKitError) -> TxDetail {
        try requireOpen(network, "txDetail")
        try notImplemented("txDetail")
    }

    func setTxLabel(on network: DashNetwork, wallet: WalletID, txid: String, label: String?) async throws(DashKitError) {
        try requireOpen(network, "setTxLabel")
        try notImplemented("setTxLabel")
    }

    // MARK: Receive

    func currentReceiveAddress(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> AddressInfo {
        try requireOpen(network, "currentReceiveAddress")
        try notImplemented("currentReceiveAddress")
    }

    func nextReceiveAddress(on network: DashNetwork, wallet: WalletID, label: String?) async throws(DashKitError)
        -> AddressInfo
    {
        try requireOpen(network, "nextReceiveAddress")
        try notImplemented("nextReceiveAddress")
    }

    func addresses(on network: DashNetwork, wallet: WalletID, filter: AddressFilter) async throws(DashKitError)
        -> [AddressInfo]
    {
        try requireOpen(network, "addresses")
        try notImplemented("addresses")
    }

    func createReceiveRequest(
        on network: DashNetwork, wallet: WalletID, amount: Amount?, label: String?, message: String?
    ) async throws(DashKitError) -> ReceiveRequest {
        try requireOpen(network, "createReceiveRequest")
        try notImplemented("createReceiveRequest")
    }

    func receiveRequests(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> [ReceiveRequest] {
        try requireOpen(network, "receiveRequests")
        try notImplemented("receiveRequests")
    }

    func deleteReceiveRequest(on network: DashNetwork, wallet: WalletID, id: UInt64) async throws(DashKitError) {
        try requireOpen(network, "deleteReceiveRequest")
        try notImplemented("deleteReceiveRequest")
    }

    // MARK: Send

    func newTxDraft(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> any TxDraftHandle {
        try requireOpen(network, "newTxDraft")
        let draft = with { s in
            let d = s.nextDraft ?? FakeTxDraft(walletID: wallet)
            s.nextDraft = nil
            s.drafts.append(d)
            return d
        }
        return draft
    }

    func maxSpendable(on network: DashNetwork, wallet: WalletID, source: CoinSource, fee: FeeMode)
        async throws(DashKitError) -> Amount
    {
        try requireOpen(network, "maxSpendable")
        try notImplemented("maxSpendable")
    }

    // MARK: Coins and labels

    func utxos(on network: DashNetwork, wallet: WalletID, filter: UtxoFilter) async throws(DashKitError) -> [Utxo] {
        try requireOpen(network, "utxos")
        try notImplemented("utxos")
    }

    func lockOutpoints(on network: DashNetwork, wallet: WalletID, outpoints: [OutPoint]) async throws(DashKitError) {
        try requireOpen(network, "lockOutpoints")
        try notImplemented("lockOutpoints")
    }

    func unlockOutpoints(on network: DashNetwork, wallet: WalletID, outpoints: [OutPoint]) async throws(DashKitError) {
        try requireOpen(network, "unlockOutpoints")
        try notImplemented("unlockOutpoints")
    }

    func lockedOutpoints(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> [OutPoint] {
        try requireOpen(network, "lockedOutpoints")
        try notImplemented("lockedOutpoints")
    }

    func addressBook(on network: DashNetwork, wallet: WalletID, purpose: AddressPurpose?, search: String?)
        async throws(DashKitError) -> [AddressBookEntry]
    {
        try requireOpen(network, "addressBook")
        try notImplemented("addressBook")
    }

    func saveAddressBookEntry(
        on network: DashNetwork, wallet: WalletID, address: String, label: String, purpose: AddressPurpose,
        replace: Bool
    ) async throws(DashKitError) -> AddressBookEntry {
        try requireOpen(network, "saveAddressBookEntry")
        try notImplemented("saveAddressBookEntry")
    }

    func deleteAddressBookEntry(on network: DashNetwork, wallet: WalletID, address: String) async throws(DashKitError) {
        try requireOpen(network, "deleteAddressBookEntry")
        try notImplemented("deleteAddressBookEntry")
    }

    // MARK: Messages

    func signMessage(on network: DashNetwork, wallet: WalletID, address: String, message: String, grantID: String)
        async throws(DashKitError) -> String
    {
        try requireOpen(network, "signMessage \(grantID)")
        return try with { $0.signature }.get()
    }
}

/// A scriptable `TxDraftHandle`.
final class FakeTxDraft: TxDraftHandle, @unchecked Sendable {
    struct State {
        var calls: [String] = []
        var recipients: [Recipient] = []
        var broadcastErrors: [DashKitError] = []
        var abandoned: [String] = []
        /// Awaited inside `broadcast` before it answers (to hold it in flight).
        var broadcastHold: (@Sendable () async -> Void)?
    }

    let walletID: WalletID
    private let lock = NSLock()
    private var state = State()

    init(walletID: WalletID) {
        self.walletID = walletID
    }

    func with<T>(_ body: (inout State) -> T) -> T {
        lock.withLock { body(&state) }
    }

    var calls: [String] { with { $0.calls } }

    func setRecipients(_ recipients: [Recipient]) async throws(DashKitError) {
        with {
            $0.calls.append("setRecipients")
            $0.recipients = recipients
        }
    }

    func setSource(_ source: CoinSource) async throws(DashKitError) {
        with { $0.calls.append("setSource") }
    }

    func setFee(_ fee: FeeMode) async throws(DashKitError) {
        with { $0.calls.append("setFee") }
    }

    func setChange(_ change: ChangePolicy) async throws(DashKitError) {
        with { $0.calls.append("setChange") }
    }

    func estimate() async throws(DashKitError) -> TxEstimate {
        with { $0.calls.append("estimate") }
        return TxEstimate(fee: Amount(duffs: 226), sizeBytes: 226, inputCount: 1, change: nil, totalSent: Amount(duffs: 1000))
    }

    func prepare(grantID: String) async throws(DashKitError) -> PreparedTxHandle {
        let n = with { s in
            s.calls.append("prepare \(grantID)")
            return s.calls.count
        }
        let summary = PreparedTxSummary(
            txid: String(repeating: String(n % 10), count: 64), fee: Amount(duffs: 226),
            feeRatePerKilobyte: Amount(duffs: 1000), sizeBytes: 226, inputs: [], outputs: [],
            totalSent: Amount(duffs: 1000), totalDebit: Amount(duffs: 1226))
        return PreparedTxHandle(summary: summary)
    }

    func broadcast(_ prepared: PreparedTxHandle) async throws(DashKitError) -> BroadcastOutcome {
        let (error, hold): (DashKitError?, (@Sendable () async -> Void)?) = with { s in
            s.calls.append("broadcast \(prepared.summary.txid)")
            return (s.broadcastErrors.isEmpty ? nil : s.broadcastErrors.removeFirst(), s.broadcastHold)
        }
        await hold?()
        if let error { throw error }
        return BroadcastOutcome(txid: prepared.summary.txid, peersAnnounced: 3)
    }

    func abandon(_ prepared: PreparedTxHandle) async throws(DashKitError) {
        with {
            $0.calls.append("abandon \(prepared.summary.txid)")
            $0.abandoned.append(prepared.summary.txid)
        }
    }
}

/// Values tests build often.
enum Fixtures {
    static let walletA = WalletID(hex: String(repeating: "a", count: 64))!
    static let walletB = WalletID(hex: String(repeating: "b", count: 64))!

    static func balances(_ confirmed: Int64) -> WalletBalances {
        WalletBalances(
            confirmed: Amount(duffs: confirmed), unconfirmed: .zero, immature: .zero, locked: .zero,
            total: Amount(duffs: confirmed))
    }

    static func info(_ id: WalletID, name: String, confirmed: Int64) -> WalletInfo {
        WalletInfo(
            walletID: id, name: name, watchOnly: false, hasMnemonic: true, hd: true, birthHeight: nil, createdAt: nil,
            balances: balances(confirmed))
    }

    static func vault(_ state: VaultLockState, encrypted: Bool = true) -> VaultStatus {
        VaultStatus(
            state: state, encrypted: encrypted, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSeconds: nil,
            walletsWithSecrets: [])
    }

    static func snapshot(
        running: Bool = true, headers: (UInt32, UInt32) = (0, 100), caughtUp: Bool = false,
        secondsSinceProgress: UInt64? = 0
    ) -> SyncSnapshot {
        SyncSnapshot(
            running: running,
            phases: [
                SyncPhaseProgress(phase: .headers, currentHeight: headers.0, targetHeight: headers.1, done: caughtUp)
            ],
            activePhase: caughtUp ? nil : .headers, tipHeight: headers.0, tipDate: nil, chainLockHeight: nil,
            connectedPeers: 3, caughtUp: caughtUp, secondsSinceProgress: secondsSinceProgress)
    }
}
