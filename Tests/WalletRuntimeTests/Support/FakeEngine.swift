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
        /// The wallet each `authorize` call named, in order.
        var grantWallets: [WalletID?] = []
        var revokedGrants: [String] = []
        var snapshot: Result<SyncSnapshot, DashKitError> = .failure(.notImplemented(detail: "sync_snapshot"))
        var imported: [WalletID] = []
        var drafts: [FakeTxDraft] = []
        var nextDraft: FakeTxDraft?
        var historyPage: Result<HistoryPage, DashKitError> = .failure(.notImplemented(detail: "history_page"))
        var signature: Result<String, DashKitError> = .failure(.notImplemented(detail: "sign_message"))
        // M2 (FakeEngine+M2.swift).
        var quickUnlockPolicy: Result<QuickUnlockPolicy, DashKitError> =
            .failure(.notImplemented(detail: "quick_unlock_policy"))
        var enrollKey: Result<[UInt8], DashKitError> = .failure(.notImplemented(detail: "enroll_quick_unlock"))
        var removeQuickUnlock: Result<VaultStatus, DashKitError> =
            .failure(.notImplemented(detail: "remove_quick_unlock"))
        var spendLimits: [Amount] = []
        var recovery: Result<VaultRecovery, DashKitError> = .failure(.notImplemented(detail: "recover_with_mnemonic"))
        var destroy: Result<VaultStatus, DashKitError> = .failure(.notImplemented(detail: "destroy"))
        var destroyCredentials: [String] = []
        var txNotices: Result<[TxNotice], DashKitError> = .failure(.notImplemented(detail: "tx_notices"))
        var logExport: Result<LogExport, DashKitError> = .failure(.notImplemented(detail: "export_logs"))
        var exportedExtraFiles: [URL] = []
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

    func requireOpen(_ network: DashNetwork, _ name: String) throws(DashKitError) {
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

    func balances(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> WalletBalances? {
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

    func authorize(on network: DashNetwork, purpose: GrantPurpose, wallet: WalletID?, credential: VaultCredential)
        async throws(DashKitError) -> AuthGrant
    {
        try requireOpen(network, "authorize")
        // The engine's wallet binding rule (m1-engine.md §2.2).
        guard (purpose == .changeCredential) == (wallet == nil) else {
            throw .invalidArgument(detail: "wallet_id must be given for every purpose except ChangeCredential")
        }
        with { $0.grantWallets.append(wallet) }
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
            let d = s.nextDraft ?? FakeTxDraft(walletID: wallet, network: network)
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

/// A scriptable `TxDraftHandle` that follows the engine's `TxDraft` rules
/// (rust/crates/dw-engine/src/send/mod.rs):
/// - `setRecipients` validates like `validate_recipients`, in its order per
///   recipient: address on the draft's network (Platform addresses refused),
///   amount 1…21M DASH, dust threshold (546 duffs P2PKH, 540 P2SH), no
///   address twice; then the total. A failure leaves the recipients unchanged.
/// - Each prepared transaction has the engine's phase. `broadcast` runs only
///   from pending or unknown (else `send.prepared_tx_spent`). A first
///   attempt (from pending) follows `settle(first: true)`: `send.no_peers`,
///   `send.broadcast_rejected` and `send.prepared_tx_spent` release the
///   inputs; `send.broadcast_unknown` keeps them reserved; any other error
///   leaves the transaction pending. A repeat (from unknown) follows
///   `settle(first: false)` and `dispatch`: it is never released, a scripted
///   domain error is reported as `send.broadcast_unknown`, and a session
///   error (`network_not_open`, `wallet_not_found`) is returned as is. `abandon` releases a pending
///   one, is a no-op for a released one and fails with
///   `send.prepared_tx_spent` while broadcasting, sent or unknown.
/// Fees and coin selection are scripted, not computed.
final class FakeTxDraft: TxDraftHandle, @unchecked Sendable {
    enum Phase: Equatable {
        case pending, broadcasting, sent, unknown, released
    }

    struct State {
        var calls: [String] = []
        var recipients: [Recipient] = []
        /// Every list handed to `setRecipients`, accepted or not.
        var submitted: [[Recipient]] = []
        var broadcastErrors: [DashKitError] = []
        /// Txids released by `abandon`.
        var abandoned: [String] = []
        /// Txids whose inputs the engine released (abandon or a never-sent broadcast).
        var released: [String] = []
        var phases: [String: Phase] = [:]
        /// Awaited inside `broadcast` before it answers (to hold it in flight).
        var broadcastHold: (@Sendable () async -> Void)?
    }

    static let maxMoney: Int64 = 21_000_000 * 100_000_000

    let walletID: WalletID
    let network: DashNetwork
    private let lock = NSLock()
    private var state = State()

    init(walletID: WalletID, network: DashNetwork = .regtest) {
        self.walletID = walletID
        self.network = network
    }

    func with<T>(_ body: (inout State) -> T) -> T {
        lock.withLock { body(&state) }
    }

    var calls: [String] { with { $0.calls } }

    func phase(of txid: String) -> Phase? { with { $0.phases[txid] } }

    func setRecipients(_ recipients: [Recipient]) async throws(DashKitError) {
        with {
            $0.calls.append("setRecipients")
            $0.submitted.append(recipients)
        }
        try validate(recipients)
        with { $0.recipients = recipients }
    }

    private func validate(_ recipients: [Recipient]) throws(DashKitError) {
        guard !recipients.isEmpty else { throw .domain(code: "send.no_recipients", detail: "") }
        var seen = Set<String>()
        var total: Int64 = 0
        for (index, recipient) in recipients.enumerated() {
            let scriptHash: Bool
            switch CoreFunctions.classifyAddress(recipient.address, network: network) {
            case .core(let p2sh): scriptHash = p2sh
            case .platform: throw .recipient(code: "send.platform_address", index: index)
            case .shielded, .invalid: throw .recipient(code: "send.invalid_address", index: index)
            }
            let amount = recipient.amount.duffs
            guard amount > 0, amount <= Self.maxMoney else {
                throw .recipient(code: "send.invalid_amount", index: index)
            }
            guard amount >= (scriptHash ? 540 : 546) else { throw .recipient(code: "send.dust_amount", index: index) }
            guard seen.insert(recipient.address).inserted else {
                throw .recipient(code: "send.duplicate_address", index: index)
            }
            total += amount
        }
        guard total <= Self.maxMoney else {
            throw .recipient(code: "send.invalid_amount", index: recipients.count - 1)
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
        let empty = with { s in
            s.calls.append("estimate")
            return s.recipients.isEmpty
        }
        if empty { throw .domain(code: "send.no_recipients", detail: "") }
        return TxEstimate(fee: Amount(duffs: 226), sizeBytes: 226, inputCount: 1, change: nil, totalSent: Amount(duffs: 1000))
    }

    func prepare(grantID: String) async throws(DashKitError) -> PreparedTxHandle {
        let (n, empty) = with { s in
            s.calls.append("prepare \(grantID)")
            return (s.calls.count, s.recipients.isEmpty)
        }
        if empty { throw .domain(code: "send.no_recipients", detail: "") }
        let txid = String(repeating: String(n % 10), count: 64)
        let summary = PreparedTxSummary(
            txid: txid, fee: Amount(duffs: 226), feeRatePerKilobyte: Amount(duffs: 1000), sizeBytes: 226, inputs: [],
            outputs: [], totalSent: Amount(duffs: 1000), totalDebit: Amount(duffs: 1226))
        with { $0.phases[txid] = .pending }
        return PreparedTxHandle(summary: summary)
    }

    func broadcast(_ prepared: PreparedTxHandle) async throws(DashKitError) -> BroadcastOutcome {
        let txid = prepared.summary.txid
        let start: Result<(Bool, DashKitError?, (@Sendable () async -> Void)?), DashKitError> = with { s in
            s.calls.append("broadcast \(txid)")
            guard s.phases[txid] == .pending || s.phases[txid] == .unknown else {
                return .failure(.domain(code: "send.prepared_tx_spent", detail: txid))
            }
            let first = s.phases[txid] == .pending
            s.phases[txid] = .broadcasting
            return .success((first, s.broadcastErrors.isEmpty ? nil : s.broadcastErrors.removeFirst(), s.broadcastHold))
        }
        let (first, scripted, hold) = try start.get()
        await hold?()
        var error = scripted
        if !first, let failure = scripted, failure.code.hasPrefix("send.") {
            // A repeat may follow a first dispatch that reached the network.
            error = .domain(code: "send.broadcast_unknown", detail: "not sent this time: \(failure.code)")
        }
        with { s in
            switch error?.code {
            case nil:
                s.phases[txid] = .sent
            case "send.broadcast_unknown":
                s.phases[txid] = .unknown
            case "send.no_peers", "send.broadcast_rejected", "send.prepared_tx_spent":
                s.phases[txid] = .released
                s.released.append(txid)
            default:
                s.phases[txid] = first ? .pending : .unknown
            }
        }
        if let error { throw error }
        return BroadcastOutcome(txid: txid, peersAnnounced: 3)
    }

    func abandon(_ prepared: PreparedTxHandle) async throws(DashKitError) {
        let txid = prepared.summary.txid
        let refused: Bool = with { s in
            s.calls.append("abandon \(txid)")
            switch s.phases[txid] {
            case .pending:
                s.phases[txid] = .released
                s.abandoned.append(txid)
                s.released.append(txid)
                return false
            case .released, nil:
                return false
            case .broadcasting, .sent, .unknown:
                return true
            }
        }
        if refused { throw .domain(code: "send.prepared_tx_spent", detail: txid) }
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
