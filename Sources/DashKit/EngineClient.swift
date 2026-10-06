import DashWalletCore
import Foundation

/// The Swift entry point to the Rust engine. One per process.
///
/// All calls are serialised through the actor; long operations run on the
/// engine's own tokio runtime, so awaiting them never blocks a Swift executor
/// thread. Events arrive on `events`.
///
/// Call `shutdown()` before releasing the client. It closes every network
/// and drops the engine on the actor's executor, so the engine's blocking
/// teardown never runs on the main thread (review L2). Debug builds assert
/// when a client is released without it.
public actor EngineClient: EngineProtocol {
    public nonisolated let events: EventBus
    public nonisolated let dataRoot: URL
    nonisolated let core: EngineCell
    /// Cached session objects. An entry is dropped as soon as the engine
    /// reports it closed (review M2).
    private var sessions: [DashNetwork: DashWalletCore.NetworkSession] = [:]
    /// Bumped by every close of a network. An `open` that was in flight
    /// across a close discards the session it got.
    private var generations: [DashNetwork: UInt64] = [:]
    private var didShutdown = false

    /// Version of the linked Rust core.
    public static var coreVersion: String { DashWalletCore.coreVersion() }

    /// - Parameters:
    ///   - dataRoot: directory holding one sub-directory per network.
    ///   - workerThreads: tokio worker threads; `nil` = one per core.
    public init(dataRoot: URL, workerThreads: UInt32? = nil, events: EventBus = EventBus()) throws(DashKitError) {
        self.events = events
        self.dataRoot = dataRoot
        let config = DashWalletCore.EngineConfig(dataRoot: dataRoot.path, workerThreads: workerThreads)
        let observer = EngineObserverAdapter(bus: events)
        core = EngineCell(try mapped { try DashWalletCore.Engine(config: config, observer: observer) })
    }

    deinit {
        let shutDown = didShutdown
        assert(shutDown, "EngineClient released without shutdown(); the engine tears down on this thread")
    }

    // MARK: Sessions

    public func open(_ network: DashNetwork, options: SessionOptions = SessionOptions()) async throws(DashKitError) {
        if let cached = sessions[network] {
            if cached.isOpen() { return }
            sessions[network] = nil
        }
        let generation = generations[network, default: 0]
        let engine = try core.get()
        let session = try await mapped { try await engine.openNetwork(network: network.ffi, options: options.ffi) }
        // The actor was free while the engine opened the network; a close in
        // that window means the returned session is already closed.
        guard generations[network, default: 0] == generation, !didShutdown else {
            throw .networkNotOpen(detail: "\(network) was closed while it was opening")
        }
        sessions[network] = session
    }

    @discardableResult
    public func close(_ network: DashNetwork) async throws(DashKitError) -> Bool {
        sessions[network] = nil
        generations[network, default: 0] &+= 1
        let engine = try core.get()
        return try await mapped { try await engine.closeNetwork(network: network.ffi) }
    }

    /// Closes every network, ends all event streams and releases the engine.
    /// Later calls fail with `network_not_open`.
    public func shutdown() async throws(DashKitError) {
        guard !didShutdown else { return }
        didShutdown = true
        sessions.removeAll()
        for network in generations.keys {
            generations[network]! &+= 1
        }
        defer { events.finish() }
        guard let engine = core.take() else { return }
        try await mapped { try await engine.shutdown() }
        // `engine` is released here, on the actor's executor (not the main thread).
    }

    public func isOpen(_ network: DashNetwork) -> Bool {
        sessions[network]?.isOpen() ?? false
    }

    public nonisolated func directory(for network: DashNetwork) -> URL {
        if let path = core.peek()?.networkDir(network: network.ffi) {
            return URL(fileURLWithPath: path, isDirectory: true)
        }
        // After shutdown: the engine's layout is `<dataRoot>/<network>`.
        return dataRoot.appendingPathComponent(network.description, isDirectory: true)
    }

    public func startSPV(on network: DashNetwork) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.startSpv() }
    }

    public func stopSPV(on network: DashNetwork) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.stopSpv() }
    }

    public func isSPVRunning(on network: DashNetwork) throws(DashKitError) -> Bool {
        let session = try session(network)
        return try mapped { try session.spvRunning() }
    }

    // MARK: Wallets

    /// Adds a wallet from a phrase (and optional BIP39 passphrase). The engine
    /// stores the seed in the network's vault before it registers the wallet,
    /// so this fails with `wallet.no_vault` / `wallet.vault_locked` rather
    /// than creating a wallet without keys. New wallets come from
    /// `generateMnemonic` followed by this call.
    public func importWallet(
        on network: DashNetwork, mnemonic: SecretBytes, bip39Passphrase: SecretBytes, options: ImportOptions
    ) async throws(DashKitError) -> WalletID {
        let session = try session(network)
        // The binding takes `Data`; both copies are zeroed once the call returns.
        let id = try await mapped {
            try await mnemonic.withTemporaryData { phrase in
                try await bip39Passphrase.withTemporaryData { passphrase in
                    try await session.importWallet(mnemonic: phrase, bip39Passphrase: passphrase, options: options.ffi)
                }
            }
        }
        return try .engine(id)
    }

    /// Restores a wallet with default options. `birthHeight` 0 scans from
    /// genesis; `nil` lets the engine choose.
    public func importWallet(
        on network: DashNetwork,
        mnemonic: SecretBytes,
        bip39Passphrase: SecretBytes = SecretBytes([]),
        birthHeight: UInt32? = nil
    ) async throws(DashKitError) -> WalletID {
        try await importWallet(
            on: network, mnemonic: mnemonic, bip39Passphrase: bip39Passphrase,
            options: ImportOptions(birthHeight: birthHeight))
    }

    public func walletInfos(on network: DashNetwork) throws(DashKitError) -> [WalletInfo] {
        let session = try session(network)
        let rows = try mapped { try session.walletInfos() }
        var result: [WalletInfo] = []
        for row in rows {
            result.append(try WalletInfo(row))
        }
        return result
    }

    public func balances(on network: DashNetwork, wallet: WalletID) throws(DashKitError) -> WalletBalances? {
        let session = try session(network)
        guard let raw = try mapped({ try session.balances(walletId: wallet.hex) }) else { return nil }
        return try WalletBalances(raw)
    }

    public func renameWallet(on network: DashNetwork, wallet: WalletID, name: String) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.renameWallet(walletId: wallet.hex, name: name) }
    }

    public func removeWallet(on network: DashNetwork, wallet: WalletID, grantID: String) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.removeWallet(walletId: wallet.hex, grantId: grantID) }
    }

    public nonisolated func generateMnemonic(wordCount: Int, language: MnemonicLanguage) throws(DashKitError) -> SecretBytes {
        try CoreFunctions.generateMnemonic(wordCount: wordCount, language: language)
    }

    public nonisolated func checkMnemonic(_ phrase: SecretBytes) throws(DashKitError) -> MnemonicCheck {
        try CoreFunctions.checkMnemonic(phrase)
    }

    // MARK: Vault

    public func vaultStatus(on network: DashNetwork) throws(DashKitError) -> VaultStatus {
        let vault = try session(network).vault()
        return try VaultStatus(try mapped { try vault.status() })
    }

    public func createVault(on network: DashNetwork, passphrase: SecretBytes?) async throws(DashKitError) -> VaultStatus {
        let vault = try session(network).vault()
        let status = try await mapped {
            if let passphrase {
                try await passphrase.withTemporaryData { try await vault.create(passphrase: $0) }
            } else {
                try await vault.create(passphrase: nil)
            }
        }
        return try VaultStatus(status)
    }

    public func encryptVault(on network: DashNetwork, newPassphrase: SecretBytes, grantID: String) async throws(DashKitError)
        -> VaultStatus
    {
        let vault = try session(network).vault()
        let status = try await mapped {
            try await newPassphrase.withTemporaryData { try await vault.encrypt(newPassphrase: $0, grantId: grantID) }
        }
        return try VaultStatus(status)
    }

    public func changeVaultPassphrase(on network: DashNetwork, old: SecretBytes, new: SecretBytes) async throws(DashKitError)
        -> VaultStatus
    {
        let vault = try session(network).vault()
        let status = try await mapped {
            try await old.withTemporaryData { oldData in
                try await new.withTemporaryData { newData in
                    try await vault.changePassphrase(oldPassphrase: oldData, newPassphrase: newData)
                }
            }
        }
        return try VaultStatus(status)
    }

    public func unlockVault(on network: DashNetwork, passphrase: SecretBytes, scope: UnlockScope) async throws(DashKitError)
        -> VaultStatus
    {
        let vault = try session(network).vault()
        let status = try await mapped {
            try await passphrase.withTemporaryData { try await vault.unlock(passphrase: $0, scope: scope.ffi) }
        }
        return try VaultStatus(status)
    }

    public func lockVault(on network: DashNetwork) throws(DashKitError) -> VaultStatus {
        let vault = try session(network).vault()
        return try VaultStatus(try mapped { try vault.lock() })
    }

    public func authorize(on network: DashNetwork, purpose: GrantPurpose, wallet: WalletID?, credential: VaultCredential)
        async throws(DashKitError) -> AuthGrant
    {
        let vault = try session(network).vault()
        let ffiPurpose = try purpose.ffi()
        let walletID = wallet?.hex
        let grant = try await mapped {
            switch credential {
            case .passphrase(let secret):
                try await secret.withTemporaryData {
                    try await vault.authorize(
                        purpose: ffiPurpose, walletId: walletID, credential: .passphrase(passphrase: $0))
                }
            case .quickUnlock(let key):
                try await key.withTemporaryData {
                    try await vault.authorize(
                        purpose: ffiPurpose, walletId: walletID, credential: .quickUnlock(wrapKey: $0))
                }
            case .unencrypted:
                try await vault.authorize(purpose: ffiPurpose, walletId: walletID, credential: .unencrypted)
            }
        }
        return try AuthGrant(grant)
    }

    public func revokeGrant(on network: DashNetwork, grantID: String) throws(DashKitError) {
        let vault = try session(network).vault()
        try mapped { try vault.revokeGrant(grantId: grantID) }
    }

    public func revealMnemonic(on network: DashNetwork, wallet: WalletID, grantID: String) async throws(DashKitError)
        -> RevealedMnemonic
    {
        let vault = try session(network).vault()
        let revealed = try await mapped { try await vault.revealMnemonic(walletId: wallet.hex, grantId: grantID) }
        // Copy into zeroing buffers and wipe the binding's Data at once (M5).
        var phrase = revealed.phrase
        var passphrase = revealed.bip39Passphrase
        return RevealedMnemonic(phrase: SecretBytes(consuming: &phrase), bip39Passphrase: SecretBytes(consuming: &passphrase))
    }

    // MARK: Sync

    public func syncSnapshot(on network: DashNetwork) throws(DashKitError) -> SyncSnapshot {
        let session = try session(network)
        return SyncSnapshot(try mapped { try session.syncSnapshot() })
    }

    public func peers(on network: DashNetwork) throws(DashKitError) -> [PeerInfo] {
        let session = try session(network)
        return try mapped { try session.peers() }.map(PeerInfo.init)
    }

    public func rotatePeers(on network: DashNetwork) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.rotatePeers() }
    }

    public func rescan(on network: DashNetwork, from start: RescanStart) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.rescan(from: start.ffi) }
    }

    // MARK: History

    public func historyPage(on network: DashNetwork, wallet: WalletID, query: HistoryQuery) async throws(DashKitError)
        -> HistoryPage
    {
        let session = try session(network)
        let ffiQuery = try query.ffi()
        return try HistoryPage(try await mapped { try await session.historyPage(walletId: wallet.hex, query: ffiQuery) })
    }

    public func txDetail(on network: DashNetwork, wallet: WalletID, txid: String) async throws(DashKitError) -> TxDetail {
        let session = try session(network)
        return try TxDetail(try await mapped { try await session.txDetail(walletId: wallet.hex, txid: txid) })
    }

    public func setTxLabel(on network: DashNetwork, wallet: WalletID, txid: String, label: String?) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.setTxLabel(walletId: wallet.hex, txid: txid, label: label) }
    }

    // MARK: Receive

    public func currentReceiveAddress(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> AddressInfo {
        let session = try session(network)
        return try AddressInfo(try await mapped { try await session.currentReceiveAddress(walletId: wallet.hex) })
    }

    public func nextReceiveAddress(on network: DashNetwork, wallet: WalletID, label: String?) async throws(DashKitError)
        -> AddressInfo
    {
        let session = try session(network)
        return try AddressInfo(try await mapped { try await session.nextReceiveAddress(walletId: wallet.hex, label: label) })
    }

    public func addresses(on network: DashNetwork, wallet: WalletID, filter: AddressFilter) async throws(DashKitError)
        -> [AddressInfo]
    {
        let session = try session(network)
        let rows = try await mapped { try await session.addresses(walletId: wallet.hex, filter: filter.ffi) }
        var result: [AddressInfo] = []
        for row in rows { result.append(try AddressInfo(row)) }
        return result
    }

    public func createReceiveRequest(
        on network: DashNetwork, wallet: WalletID, amount: Amount?, label: String?, message: String?
    ) async throws(DashKitError) -> ReceiveRequest {
        let session = try session(network)
        let duffs = try amount.map { (a) throws(DashKitError) in try a.engineDuffs() }
        let request = try await mapped {
            try await session.createReceiveRequest(walletId: wallet.hex, amount: duffs, label: label, message: message)
        }
        return try ReceiveRequest(request)
    }

    public func receiveRequests(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> [ReceiveRequest] {
        let session = try session(network)
        let rows = try await mapped { try await session.receiveRequests(walletId: wallet.hex) }
        var result: [ReceiveRequest] = []
        for row in rows { result.append(try ReceiveRequest(row)) }
        return result
    }

    public func deleteReceiveRequest(on network: DashNetwork, wallet: WalletID, id: UInt64) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.deleteReceiveRequest(walletId: wallet.hex, id: id) }
    }

    // MARK: Send

    public func newTxDraft(on network: DashNetwork, wallet: WalletID) throws(DashKitError) -> any TxDraftHandle {
        let session = try session(network)
        let draft = try mapped { try session.newTxDraft(walletId: wallet.hex) }
        return EngineTxDraft(draft, walletID: wallet)
    }

    public func maxSpendable(on network: DashNetwork, wallet: WalletID, source: CoinSource, fee: FeeMode)
        async throws(DashKitError) -> Amount
    {
        let session = try session(network)
        let ffiFee = try fee.ffi()
        let duffs = try await mapped {
            try await session.maxSpendable(walletId: wallet.hex, source: source.ffi, fee: ffiFee)
        }
        return try Amount(engine: duffs)
    }

    // MARK: Coins and labels

    public func utxos(on network: DashNetwork, wallet: WalletID, filter: UtxoFilter) async throws(DashKitError) -> [Utxo] {
        let session = try session(network)
        let rows = try await mapped { try await session.utxos(walletId: wallet.hex, filter: filter.ffi) }
        var result: [Utxo] = []
        for row in rows { result.append(try Utxo(row)) }
        return result
    }

    public func lockOutpoints(on network: DashNetwork, wallet: WalletID, outpoints: [OutPoint]) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.lockOutpoints(walletId: wallet.hex, outpoints: outpoints.map(\.ffi)) }
    }

    public func unlockOutpoints(on network: DashNetwork, wallet: WalletID, outpoints: [OutPoint]) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.unlockOutpoints(walletId: wallet.hex, outpoints: outpoints.map(\.ffi)) }
    }

    public func lockedOutpoints(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> [OutPoint] {
        let session = try session(network)
        return try await mapped { try await session.lockedOutpoints(walletId: wallet.hex) }.map(OutPoint.init)
    }

    public func addressBook(on network: DashNetwork, wallet: WalletID, purpose: AddressPurpose?, search: String?)
        async throws(DashKitError) -> [AddressBookEntry]
    {
        let session = try session(network)
        return try await mapped {
            try await session.addressBook(walletId: wallet.hex, purpose: purpose?.ffi, search: search)
        }.map(AddressBookEntry.init)
    }

    public func saveAddressBookEntry(
        on network: DashNetwork, wallet: WalletID, address: String, label: String, purpose: AddressPurpose,
        replace: Bool
    ) async throws(DashKitError) -> AddressBookEntry {
        let session = try session(network)
        let entry = try await mapped {
            try await session.saveAddressBookEntry(
                walletId: wallet.hex, address: address, label: label, purpose: purpose.ffi, replace: replace)
        }
        return AddressBookEntry(entry)
    }

    public func deleteAddressBookEntry(on network: DashNetwork, wallet: WalletID, address: String) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.deleteAddressBookEntry(walletId: wallet.hex, address: address) }
    }

    // MARK: Messages

    public func signMessage(on network: DashNetwork, wallet: WalletID, address: String, message: String, grantID: String)
        async throws(DashKitError) -> String
    {
        let session = try session(network)
        return try await mapped {
            try await session.signMessage(walletId: wallet.hex, address: address, message: message, grantId: grantID)
        }
    }

    // MARK: Private

    /// The open session for `network`. A cached session the engine has
    /// closed in the meantime is dropped, so callers see `network_not_open`
    /// rather than a dead handle (review M2).
    func session(_ network: DashNetwork) throws(DashKitError) -> DashWalletCore.NetworkSession {
        guard let session = sessions[network] else { throw .networkNotOpen(detail: network.description) }
        guard session.isOpen() else {
            sessions[network] = nil
            throw .networkNotOpen(detail: "\(network) was closed by the engine")
        }
        return session
    }
}

/// Holds the engine object so `shutdown()` can take it out and release it on
/// the actor's executor while nonisolated readers (`directory(for:)`) still
/// see it until then.
final class EngineCell: @unchecked Sendable {
    // `lock` guards `engine`.
    private let lock = NSLock()
    private var engine: DashWalletCore.Engine?

    init(_ engine: DashWalletCore.Engine) {
        self.engine = engine
    }

    func get() throws(DashKitError) -> DashWalletCore.Engine {
        guard let engine = peek() else { throw DashKitError.networkNotOpen(detail: "engine was shut down") }
        return engine
    }

    func peek() -> DashWalletCore.Engine? {
        lock.withLock { engine }
    }

    func take() -> DashWalletCore.Engine? {
        lock.withLock {
            defer { engine = nil }
            return engine
        }
    }
}

/// `TxDraftHandle` over the engine's `TxDraft`.
final class EngineTxDraft: TxDraftHandle {
    let walletID: WalletID
    private let draft: DashWalletCore.TxDraft

    init(_ draft: DashWalletCore.TxDraft, walletID: WalletID) {
        self.draft = draft
        self.walletID = walletID
    }

    func setRecipients(_ recipients: [Recipient]) throws(DashKitError) {
        var rows: [DashWalletCore.Recipient] = []
        for (index, recipient) in recipients.enumerated() {
            rows.append(try recipient.ffi(index: index))
        }
        try mapped { try draft.setRecipients(recipients: rows) }
    }

    func setSource(_ source: CoinSource) throws(DashKitError) {
        try mapped { try draft.setSource(source: source.ffi) }
    }

    func setFee(_ fee: FeeMode) throws(DashKitError) {
        let ffi = try fee.ffi()
        try mapped { try draft.setFee(fee: ffi) }
    }

    func setChange(_ change: ChangePolicy) throws(DashKitError) {
        try mapped { try draft.setChange(change: change.ffi) }
    }

    func estimate() async throws(DashKitError) -> TxEstimate {
        try TxEstimate(try await mapped { try await draft.estimate() })
    }

    func prepare(grantID: String) async throws(DashKitError) -> PreparedTxHandle {
        try PreparedTxHandle(try await mapped { try await draft.prepare(grantId: grantID) })
    }

    func broadcast(_ prepared: PreparedTxHandle) async throws(DashKitError) -> BroadcastOutcome {
        let tx = try engineObject(of: prepared)
        let outcome = try await mapped { try await draft.broadcast(prepared: tx) }
        return BroadcastOutcome(txid: outcome.txid, peersAnnounced: outcome.peersAnnounced)
    }

    func abandon(_ prepared: PreparedTxHandle) async throws(DashKitError) {
        let tx = try engineObject(of: prepared)
        try await mapped { try await draft.abandon(prepared: tx) }
    }

    func createUnsigned() async throws(DashKitError) -> PSBTHandle {
        PSBTHandle(try await mapped { try await draft.createUnsigned() })
    }

    private func engineObject(of prepared: PreparedTxHandle) throws(DashKitError) -> DashWalletCore.PreparedTx {
        guard let tx = prepared.engineObject else {
            throw .invalidArgument(detail: "prepared transaction was not made by the engine")
        }
        return tx
    }
}
