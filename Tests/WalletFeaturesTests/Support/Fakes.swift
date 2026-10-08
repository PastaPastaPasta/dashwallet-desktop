// In-memory fakes of the WalletRuntime service contracts. Each fake records
// the calls it receives and answers from state the test sets up; a fake
// never pretends an unconfigured call succeeded (it throws not_implemented).
import Foundation
import PlatformServices
import WalletFeatures
import WalletRuntime

/// A value guarded by a lock, for fakes whose async methods run off the main actor.
final class Locked<Value>: @unchecked Sendable {
    private var value: Value
    private let lock = NSLock()

    init(_ value: Value) {
        self.value = value
    }

    func withLock<R, E: Error>(_ body: (inout Value) throws(E) -> R) throws(E) -> R {
        lock.lock()
        defer { lock.unlock() }
        return try body(&value)
    }

    var current: Value { withLock { $0 } }
}

/// Fans values out to every stream handed out by `stream()`.
final class Broadcast<Element: Sendable>: @unchecked Sendable {
    private let continuations = Locked<[UUID: AsyncStream<Element>.Continuation]>([:])

    func stream(initial: Element? = nil) -> AsyncStream<Element> {
        AsyncStream { continuation in
            let id = UUID()
            if let initial { continuation.yield(initial) }
            continuations.withLock { $0[id] = continuation }
            continuation.onTermination = { [weak self] _ in
                self?.continuations.withLock { $0[id] = nil }
            }
        }
    }

    func send(_ element: Element) {
        for continuation in continuations.current.values { continuation.yield(element) }
    }

    var subscriberCount: Int { continuations.current.count }
}

func notConfigured(_ call: String) -> ServiceError {
    ServiceError(code: .notImplemented, detail: "fake: \(call) not configured")
}

// MARK: Secrets

final class FakeSecret: SecretBuffer, @unchecked Sendable {
    private(set) var bytes: [UInt8]

    init(_ text: String) {
        bytes = Array(text.utf8)
    }

    var count: Int { bytes.count }

    func withUnsafeBytes<R>(_ body: (UnsafeRawBufferPointer) throws -> R) rethrows -> R {
        try bytes.withUnsafeBytes(body)
    }

    var string: String { String(decoding: bytes, as: UTF8.self) }
}

extension SecretBuffer {
    var testString: String { withUnsafeBytes { String(decoding: $0, as: UTF8.self) } }
}

// MARK: Host and lifecycle

final class FakeHost: WalletHosting, @unchecked Sendable {
    let network = Locked<DashNetwork?>(.testnet)

    var activeNetwork: DashNetwork? { get async { network.current } }

    func start(network: DashNetwork, options: NetworkOptions) async throws(ServiceError) {
        self.network.withLock { $0 = network }
    }

    func stop() async throws(ServiceError) {
        network.withLock { $0 = nil }
    }

    func dataDirectory(for network: DashNetwork) -> URL {
        URL(fileURLWithPath: "/tmp/fake-dwd/\(network)")
    }
}

struct ImportCall: Sendable {
    let mnemonic: String
    let bip39Passphrase: String
    let options: WalletImportOptions
}

final class FakeLifecycle: LifecycleQueueing, @unchecked Sendable {
    let host: FakeHost
    let transitionBroadcast = Broadcast<LifecycleTransition>()
    let state = Locked<LifecycleTransition>(.idle)
    let imports = Locked<[ImportCall]>([])
    let switches = Locked<[DashNetwork]>([])
    /// Errors thrown by the next `importWallet` calls, in order.
    let importErrors = Locked<[ServiceError]>([])
    let switchError = Locked<ServiceError?>(nil)
    /// Called after a successful import, e.g. to update a fake wallet state.
    let onImport = Locked<(@Sendable (WalletID) -> Void)?>(nil)
    var nextWalletID = WalletID(hex: String(repeating: "ab", count: 32))!

    init(host: FakeHost) {
        self.host = host
    }

    var transition: LifecycleTransition { get async { state.current } }

    func transitions() -> AsyncStream<LifecycleTransition> {
        transitionBroadcast.stream(initial: state.current)
    }

    func publish(_ transition: LifecycleTransition) {
        state.withLock { $0 = transition }
        transitionBroadcast.send(transition)
    }

    func start(network: DashNetwork) async throws(ServiceError) {
        try await host.start(network: network, options: NetworkOptions())
    }

    func stop() async throws(ServiceError) {
        try await host.stop()
    }

    func switchNetwork(to network: DashNetwork) async throws(ServiceError) {
        if let error = switchError.current { throw error }
        let from = host.network.current
        publish(.switchingNetwork(from: from, to: network))
        switches.withLock { $0.append(network) }
        try await host.start(network: network, options: NetworkOptions())
        publish(.idle)
    }

    func importWallet(
        mnemonic: any SecretBuffer, bip39Passphrase: any SecretBuffer, options: WalletImportOptions
    ) async throws(ServiceError) -> WalletID {
        let error: ServiceError? = importErrors.withLock { $0.isEmpty ? nil : $0.removeFirst() }
        if let error { throw error }
        imports.withLock {
            $0.append(ImportCall(mnemonic: mnemonic.testString, bip39Passphrase: bip39Passphrase.testString, options: options))
        }
        let id = nextWalletID
        onImport.current?(id)
        return id
    }

    /// `nil` = not configured (not_implemented); else the error to throw or success.
    let removeResult = Locked<Result<Void, ServiceError>?>(nil)
    let removals = Locked<[(WalletID, AuthGrant)]>([])
    /// Called after a successful removal, e.g. to update a fake wallet state.
    let onRemove = Locked<(@Sendable (WalletID) -> Void)?>(nil)

    func removeWallet(_ id: WalletID, grant: AuthGrant) async throws(ServiceError) {
        guard let result = removeResult.current else { throw notConfigured("removeWallet") }
        removals.withLock { $0.append((id, grant)) }
        try result.get()
        onRemove.current?(id)
    }
}

// MARK: Wallet state and sync

@MainActor
final class FakeWalletState: WalletStateProviding {
    var wallets: [WalletInfo]?
    var selectedWalletID: WalletID?
    var balances: WalletBalances?
    let changeBroadcast = Broadcast<Void>()
    var renames: [(WalletID, String)] = []

    init(wallets: [WalletInfo]? = nil, selected: WalletID? = nil, balances: WalletBalances? = nil) {
        self.wallets = wallets
        self.selectedWalletID = selected
        self.balances = balances
    }

    func changes() -> AsyncStream<Void> { changeBroadcast.stream() }

    func notify() { changeBroadcast.send(()) }

    func select(_ id: WalletID) {
        selectedWalletID = id
    }

    func rename(_ id: WalletID, to name: String) async throws(ServiceError) {
        renames.append((id, name))
    }
}

@MainActor
final class FakeSync: SyncStatusProviding {
    var status: SyncStatus?
    let broadcast = Broadcast<SyncStatus>()
    var rotateCount = 0
    var rotateError: ServiceError?

    init(status: SyncStatus? = nil) {
        self.status = status
    }

    func changes() -> AsyncStream<SyncStatus> { broadcast.stream(initial: status) }

    func publish(_ status: SyncStatus) {
        self.status = status
        broadcast.send(status)
    }

    func peers() async throws(ServiceError) -> [PeerInfo] { [] }

    func rotatePeers() async throws(ServiceError) {
        rotateCount += 1
        if let rotateError { throw rotateError }
    }

    var rescans: [RescanStart] = []
    /// `nil` = not configured (not_implemented).
    var rescanResult: Result<Void, ServiceError>?

    func rescan(from start: RescanStart) async throws(ServiceError) {
        guard let rescanResult else { throw notConfigured("rescan") }
        rescans.append(start)
        try rescanResult.get()
    }

    static func synced(peers: UInt32 = 8) -> SyncStatus {
        SyncStatus(
            running: true, phases: [], activePhase: nil, tipHeight: 1_000_000, tipDate: nil, chainLockHeight: nil,
            connectedPeers: peers, progress: 1, isDone: true, isStalled: false)
    }

    static func syncing(phase: SyncPhase, progress: Double, peers: UInt32 = 8, stalled: Bool = false) -> SyncStatus {
        SyncStatus(
            running: true, phases: [], activePhase: phase, tipHeight: nil, tipDate: nil, chainLockHeight: nil,
            connectedPeers: peers, progress: progress, isDone: false, isStalled: stalled)
    }
}

// MARK: Vault and authentication

final class FakeVault: VaultProviding, @unchecked Sendable {
    struct State {
        var status = VaultStatus(
            state: .noVault, encrypted: false, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSeconds: nil,
            walletsWithSecrets: [])
        var generated = "abandon ability able about above absent absorb abstract absurd abuse access accident"
        var checks: [String: MnemonicCheck] = [:]
        var creates: [String?] = []
        var encrypts: [(passphrase: String, grant: AuthGrant)] = []
        var passphraseChanges: [(old: String, new: String)] = []
        var reveals: [(WalletID, AuthGrant)] = []
        var generateCalls: [Int] = []
        var errors: [String: ServiceError] = [:]
        var madeSecrets = 0
    }

    let state = Locked(State())

    func setError(_ error: ServiceError?, for call: String) {
        state.withLock { $0.errors[call] = error }
    }

    private func check(_ call: String) throws(ServiceError) {
        if let error = state.current.errors[call] { throw error }
    }

    func status() async throws(ServiceError) -> VaultStatus {
        try check("status")
        return state.current.status
    }

    func create(passphrase: (any SecretBuffer)?) async throws(ServiceError) -> VaultStatus {
        try check("create")
        return state.withLock {
            $0.creates.append(passphrase?.testString)
            $0.status = VaultStatus(
                state: passphrase == nil ? .unencrypted : .unlocked, encrypted: passphrase != nil,
                quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSeconds: nil, walletsWithSecrets: [])
            return $0.status
        }
    }

    func encrypt(newPassphrase: any SecretBuffer, grant: AuthGrant) async throws(ServiceError) -> VaultStatus {
        try check("encrypt")
        return state.withLock {
            $0.encrypts.append((newPassphrase.testString, grant))
            $0.status = VaultStatus(
                state: .unlocked, encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0,
                retryAfterSeconds: nil, walletsWithSecrets: $0.status.walletsWithSecrets)
            return $0.status
        }
    }

    func changePassphrase(old: any SecretBuffer, new: any SecretBuffer) async throws(ServiceError) -> VaultStatus {
        try check("changePassphrase")
        return state.withLock {
            $0.passphraseChanges.append((old.testString, new.testString))
            return $0.status
        }
    }

    func revealMnemonic(wallet: WalletID, grant: AuthGrant) async throws(ServiceError) -> RevealedMnemonic {
        try check("revealMnemonic")
        return state.withLock {
            $0.reveals.append((wallet, grant))
            return RevealedMnemonic(phrase: FakeSecret($0.generated), bip39Passphrase: FakeSecret(""))
        }
    }

    func generateMnemonic(wordCount: Int, language: MnemonicLanguage) async throws(ServiceError) -> any SecretBuffer {
        try check("generateMnemonic")
        return state.withLock {
            $0.generateCalls.append(wordCount)
            return FakeSecret($0.generated)
        }
    }

    func checkMnemonic(_ phrase: any SecretBuffer) async throws(ServiceError) -> MnemonicCheck {
        try check("checkMnemonic")
        let text = phrase.testString
        if let configured = state.current.checks[text] { return configured }
        let words = text.split(separator: " ")
        return MnemonicCheck(wordCount: words.count, unknownWordIndices: [], language: .english, checksum: .valid)
    }

    func makeSecret(utf8 text: String) -> any SecretBuffer {
        state.withLock { $0.madeSecrets += 1 }
        return FakeSecret(text)
    }
}

struct AuthorizeCall {
    let purpose: GrantPurpose
    let wallet: WalletID?
    let passphrase: String?
}

/// The authentication gate over a fake vault (review L4):
/// - `requirement(for:)` is `AuthenticationGate`'s rule for `lockState`
///   (a real-engine test checks that rule against dw-vault);
/// - `authorize` answers like dw-vault's credential table for `lockState`:
///   `vault.no_vault`, `vault.not_encrypted` (passphrase on an unencrypted
///   vault), `vault.credential_required` (reveal, wipe and credential change
///   without the passphrase on an encrypted vault), `vault.locked` and
///   `vault.mixing_only` (no credential); the lock state does not change.
///   Scripted `authorizeErrors` (e.g. a wrong passphrase) come first.
@MainActor
final class FakeAuth: AuthenticationGating {
    var lockState: VaultLockState?
    let broadcast = Broadcast<VaultLockState>()
    /// The iOS "require authentication for every payment" setting.
    var requireAuthenticationForEveryPayment = true
    var authorizeCalls: [AuthorizeCall] = []
    var authorizeErrors: [ServiceError] = []
    var unlockCalls: [(String, UnlockScope)] = []
    var unlockErrors: [ServiceError] = []
    var revoked: [AuthGrant] = []
    var lockCount = 0
    /// When set, `authorize` waits for this gate after recording the call
    /// (the engine's passphrase check takes about a second).
    var authorizeGate: Gate?
    private var grantCounter = 0

    init(lockState: VaultLockState? = .unencrypted) {
        self.lockState = lockState
    }

    func lockStateChanges() -> AsyncStream<VaultLockState> { broadcast.stream(initial: lockState) }

    func publish(_ state: VaultLockState) {
        lockState = state
        broadcast.send(state)
    }

    func requirement(for purpose: GrantPurpose) -> CredentialRequirement {
        AuthenticationGate.requirement(
            for: purpose, lockState: lockState, quickUnlockEnrolled: false,
            requireAuthenticationForEveryPayment: requireAuthenticationForEveryPayment)
    }

    /// dw-vault `Vault::authorize`'s credential table.
    private func checkCredential(_ purpose: GrantPurpose, passphrase: String?) throws(ServiceError) {
        let encrypted: Bool
        switch lockState {
        case .noVault, nil: throw ServiceError(code: .vaultNoVault)
        case .noKeys, .unencrypted: encrypted = false
        case .locked, .unlockedMixingOnly, .unlocked: encrypted = true
        }
        if passphrase != nil {
            if !encrypted { throw ServiceError(code: .vaultNotEncrypted) }
            return
        }
        guard encrypted else { return }
        switch purpose {
        case .revealSecret, .wipe, .changeCredential: throw ServiceError(code: .vaultCredentialRequired)
        case .spend, .signMessage, .platformOperation: break
        }
        if lockState == .locked { throw ServiceError(code: .vaultLocked) }
        if lockState == .unlockedMixingOnly { throw ServiceError(code: .vaultMixingOnly) }
    }

    func authorize(_ purpose: GrantPurpose, wallet: WalletID?, credential: Credential) async throws(ServiceError)
        -> AuthGrant
    {
        let passphrase: String?
        switch credential {
        case .passphrase(let secret): passphrase = secret.testString
        // No M1 view model offers quick unlock; the engine refuses it until slot B lands.
        case .quickUnlock: throw ServiceError(code: .vaultQuickUnlockUnavailable)
        case .unencrypted: passphrase = nil
        }
        authorizeCalls.append(AuthorizeCall(purpose: purpose, wallet: wallet, passphrase: passphrase))
        // The engine's wallet binding rule (m1-engine.md §2.2).
        guard (purpose == .changeCredential) == (wallet == nil) else {
            throw ServiceError(code: .invalidArgument, detail: "wallet binding")
        }
        if let authorizeGate { await authorizeGate.wait() }
        if !authorizeErrors.isEmpty { throw authorizeErrors.removeFirst() }
        try checkCredential(purpose, passphrase: passphrase)
        grantCounter += 1
        return AuthGrant(
            id: "grant-\(grantCounter)", purpose: purpose, expiresAt: Date(timeIntervalSince1970: 2_000_000_000),
            singleUse: true)
    }

    func revoke(_ grant: AuthGrant) {
        revoked.append(grant)
    }

    func unlock(passphrase: any SecretBuffer, scope: UnlockScope) async throws(ServiceError) {
        unlockCalls.append((passphrase.testString, scope))
        if !unlockErrors.isEmpty { throw unlockErrors.removeFirst() }
        publish(scope == .full ? .unlocked : .unlockedMixingOnly)
    }

    func lock() async throws(ServiceError) {
        lockCount += 1
        publish(.locked)
    }
}

// MARK: Sending

/// A `TransactionDrafting` that follows the live `TransactionDraft` adapter
/// over the engine's `TxDraft` (rust/crates/dw-engine/src/send/mod.rs):
/// - `setRecipients` validates like the engine's `validate_recipients`, in
///   its order per recipient: address (`FakeURI` rules; Platform refused),
///   amount 1…21M DASH, dust (546 duffs P2PKH, 540 P2SH), no address twice
///   (`send.duplicate_address{index}`); then the total. A failure leaves the
///   recipients unchanged. Every setter first abandons the prepared
///   transactions that were never broadcast.
/// - `estimate` and `prepare` refuse an empty draft (`send.no_recipients`);
///   fee, size and change are scripted.
/// - A prepared transaction is held until it is sent, released or the draft
///   forgets it. `broadcast` of one the draft does not hold fails with
///   `send.prepared_tx_unknown`, of one in flight with
///   `send.broadcast_outcome_unknown`. Success forgets it. `send.no_peers`,
///   `send.broadcast_rejected` and `send.prepared_tx_spent` release its inputs
///   and forget it. Errors raised before dispatch (argument, session,
///   `wallet`, `storage`, `spv`, `io`) leave it as it was. Any other
///   error marks the outcome unknown: it may be broadcast again but not
///   abandoned (`send.broadcast_outcome_unknown`). A repeat of an unknown
///   outcome is never released: the engine reports any `send.*` failure of a
///   repeat as `send.broadcast_unknown` (dw-engine `dispatch`).
/// - `abandon` releases a ready transaction and is a no-op for one the draft
///   does not hold.
final class FakeDraft: TransactionDrafting, @unchecked Sendable {
    enum Held: Equatable {
        case ready, broadcasting, outcomeUnknown
    }

    struct State {
        var recipients: [PaymentRecipient] = []
        /// Every list handed to `setRecipients`, accepted or not.
        var submittedRecipients: [[PaymentRecipient]] = []
        var source: CoinSourceChoice?
        var fee: FeeChoice?
        var change: ChangeChoice?
        var estimate = TxEstimate(
            fee: Amount(duffs: 226), sizeBytes: 226, inputCount: 1, change: Amount(duffs: 1000), totalSent: .zero)
        var prepareGrants: [AuthGrant] = []
        var prepareError: ServiceError?
        var estimateError: ServiceError?
        /// Thrown by the next broadcasts, in order, before `broadcastError`.
        var broadcastErrors: [ServiceError] = []
        /// Thrown by every broadcast once `broadcastErrors` is empty.
        var broadcastError: ServiceError?
        var broadcasts: [PreparedTransaction] = []
        /// Released by `abandon` (or by a setter abandoning unsent ones).
        var abandoned: [PreparedTransaction] = []
        /// Every transaction whose inputs were released: abandoned, or a
        /// broadcast the network certainly did not take.
        var released: [PreparedTransaction] = []
        var issued: [PreparedTransaction] = []
        var held: [UUID: Held] = [:]
    }

    static let maxMoney: Int64 = 21_000_000 * 100_000_000

    let state = Locked(State())
    /// When set, `prepare` waits for this gate before answering.
    let prepareGate = Locked<Gate?>(nil)
    /// When set, `broadcast` waits for this gate before answering.
    let broadcastGate = Locked<Gate?>(nil)
    private let addresses: any URIHandling

    init(addresses: any URIHandling) {
        self.addresses = addresses
    }

    func setRecipients(_ recipients: [PaymentRecipient]) async throws(ServiceError) {
        abandonUnsent()
        state.withLock { $0.submittedRecipients.append(recipients) }
        try validate(recipients)
        state.withLock { $0.recipients = recipients }
    }

    private func validate(_ recipients: [PaymentRecipient]) throws(ServiceError) {
        func refuse(_ code: ServiceErrorCode, _ index: Int) -> ServiceError {
            ServiceError(code: code, recipientIndex: index, parameters: ["index": Int64(index)])
        }
        guard !recipients.isEmpty else { throw ServiceError(code: .sendNoRecipients) }
        var seen = Set<String>()
        var total: Int64 = 0
        for (index, recipient) in recipients.enumerated() {
            let scriptHash: Bool
            switch addresses.classifyAddress(recipient.address) {
            case .core(let p2sh): scriptHash = p2sh
            case .platform: throw refuse(.sendPlatformAddress, index)
            case .shielded, .invalid: throw refuse(.sendInvalidAddress, index)
            }
            let amount = recipient.amount.duffs
            guard amount > 0, amount <= Self.maxMoney else { throw refuse(.sendInvalidAmount, index) }
            guard amount >= (scriptHash ? 540 : 546) else { throw refuse(.sendDustAmount, index) }
            guard seen.insert(recipient.address).inserted else { throw refuse(.sendDuplicateAddress, index) }
            total += amount
        }
        guard total <= Self.maxMoney else { throw refuse(.sendInvalidAmount, recipients.count - 1) }
    }

    func setSource(_ source: CoinSourceChoice) async throws(ServiceError) {
        abandonUnsent()
        state.withLock { $0.source = source }
    }

    func setFee(_ fee: FeeChoice) async throws(ServiceError) {
        abandonUnsent()
        state.withLock { $0.fee = fee }
    }

    func setChange(_ change: ChangeChoice) async throws(ServiceError) {
        abandonUnsent()
        state.withLock { $0.change = change }
    }

    func estimate() async throws(ServiceError) -> TxEstimate {
        try state.withLock { (s) throws(ServiceError) -> TxEstimate in
            if s.recipients.isEmpty { throw ServiceError(code: .sendNoRecipients) }
            if let error = s.estimateError { throw error }
            let sent = s.recipients.reduce(Int64(0)) { $0 + $1.amount.duffs }
            return TxEstimate(
                fee: s.estimate.fee, sizeBytes: s.estimate.sizeBytes, inputCount: s.estimate.inputCount,
                change: s.estimate.change, totalSent: Amount(duffs: sent))
        }
    }

    func prepare(grant: AuthGrant) async throws(ServiceError) -> PreparedTransaction {
        if let gate = prepareGate.current { await gate.wait() }
        return try state.withLock { (s) throws(ServiceError) -> PreparedTransaction in
            s.prepareGrants.append(grant)
            if s.recipients.isEmpty { throw ServiceError(code: .sendNoRecipients) }
            if let error = s.prepareError { throw error }
            let sent = s.recipients.reduce(Int64(0)) { $0 + $1.amount.duffs }
            let outputs = s.recipients.map {
                PreparedOutput(address: $0.address, amount: $0.amount, isChange: false, label: $0.label)
            } + [PreparedOutput(address: "Xchange", amount: Amount(duffs: 1000), isChange: true, label: nil)]
            let summary = PreparedTxSummary(
                txid: String(repeating: "f", count: 64), fee: s.estimate.fee, feeRatePerKilobyte: Amount(duffs: 1000),
                sizeBytes: s.estimate.sizeBytes, inputCount: Int(s.estimate.inputCount), outputs: outputs,
                totalSent: Amount(duffs: sent), totalDebit: Amount(duffs: sent + s.estimate.fee.duffs))
            let prepared = PreparedTransaction(id: UUID(), summary: summary)
            s.issued.append(prepared)
            s.held[prepared.id] = .ready
            return prepared
        }
    }

    func broadcast(_ prepared: PreparedTransaction) async throws(ServiceError) -> BroadcastResult {
        let previous = try state.withLock { (s) throws(ServiceError) -> Held in
            guard let held = s.held[prepared.id] else { throw ServiceError(code: .sendPreparedTxUnknown) }
            if held == .broadcasting { throw ServiceError(code: .sendBroadcastOutcomeUnknown) }
            s.held[prepared.id] = .broadcasting
            s.broadcasts.append(prepared)
            return held
        }
        if let gate = broadcastGate.current { await gate.wait() }
        return try state.withLock { (s) throws(ServiceError) -> BroadcastResult in
            var error = s.broadcastErrors.isEmpty ? s.broadcastError : s.broadcastErrors.removeFirst()
            if previous == .outcomeUnknown, let failure = error, failure.code.rawValue.hasPrefix("send.") {
                // A repeat may follow a first dispatch that reached the network.
                error = ServiceError(code: .sendBroadcastUnknown, detail: "not sent this time: \(failure.code.rawValue)")
            }
            guard let error else {
                s.held[prepared.id] = nil
                return BroadcastResult(txid: prepared.summary.txid, peersAnnounced: 3)
            }
            switch error.code {
            case .invalidArgument, .networkNotOpen, .notImplemented, .walletNotFound, .wallet, .storage, .spv, .io:
                // Raised before dispatch (m1-engine.md §2.7.1 "not dispatched").
                s.held[prepared.id] = previous
            case .sendNoPeers, .sendPreparedTxSpent, .sendBroadcastRejected:
                s.held[prepared.id] = nil
                s.released.append(prepared)
            default:
                s.held[prepared.id] = .outcomeUnknown
            }
            throw error
        }
    }

    func abandon(_ prepared: PreparedTransaction) async throws(ServiceError) {
        try state.withLock { (s) throws(ServiceError) in
            guard let held = s.held[prepared.id] else { return }
            guard held == .ready else { throw ServiceError(code: .sendBroadcastOutcomeUnknown) }
            s.held[prepared.id] = nil
            s.abandoned.append(prepared)
            s.released.append(prepared)
        }
    }

    private func abandonUnsent() {
        state.withLock { s in
            for prepared in s.issued where s.held[prepared.id] == .ready {
                s.held[prepared.id] = nil
                s.abandoned.append(prepared)
                s.released.append(prepared)
            }
        }
    }
}

final class FakeSender: TransactionSending, @unchecked Sendable {
    let drafts = Locked<[FakeDraft]>([])
    let maxSpendableValue = Locked<Result<Amount, ServiceError>>(.success(Amount(duffs: 500_000_000)))
    let maxCalls = Locked<[(CoinSourceChoice, FeeChoice)]>([])
    /// Configures each new draft before it is returned.
    let configure = Locked<(@Sendable (FakeDraft) -> Void)?>(nil)
    private let addresses: any URIHandling

    /// - Parameter addresses: the address rules drafts validate recipients with.
    init(addresses: any URIHandling) {
        self.addresses = addresses
    }

    func makeDraft(wallet: WalletID) async throws(ServiceError) -> any TransactionDrafting {
        let draft = FakeDraft(addresses: addresses)
        configure.current?(draft)
        drafts.withLock { $0.append(draft) }
        return draft
    }

    func maxSpendable(wallet: WalletID, source: CoinSourceChoice, fee: FeeChoice) async throws(ServiceError) -> Amount {
        maxCalls.withLock { $0.append((source, fee)) }
        return try maxSpendableValue.current.get()
    }

    var lastDraft: FakeDraft? { drafts.current.last }
}

/// A one-shot gate a fake awaits until the test opens it.
final class Gate: @unchecked Sendable {
    private let state = Locked<(open: Bool, waiters: [CheckedContinuation<Void, Never>])>((false, []))

    func wait() async {
        await withCheckedContinuation { continuation in
            let resumeNow = state.withLock { s -> Bool in
                if s.open { return true }
                s.waiters.append(continuation)
                return false
            }
            if resumeNow { continuation.resume() }
        }
    }

    func open() {
        let waiters = state.withLock { s -> [CheckedContinuation<Void, Never>] in
            s.open = true
            defer { s.waiters = [] }
            return s.waiters
        }
        waiters.forEach { $0.resume() }
    }

    var waiterCount: Int { state.current.waiters.count }
}

// MARK: History, receive, address book, messages

final class FakeHistory: HistoryProviding, @unchecked Sendable {
    struct State {
        var records: [TxRecord] = []
        var queries: [HistoryQuery] = []
        var details: [String: TransactionDetail] = [:]
        var labels: [(String, String?)] = []
        /// Errors thrown by the next `page` calls, in order.
        var pageErrors: [ServiceError] = []
        /// Errors thrown once by the n-th `page` call (1-based).
        var failAtQuery: [Int: ServiceError] = [:]
    }

    let state = Locked(State())
    let broadcast = Broadcast<[String]>()

    /// Pages `records` (already in the order to return) by `limit`, with the
    /// cursor being the next offset.
    func page(wallet: WalletID, query: HistoryQuery) async throws(ServiceError) -> HistoryPage {
        try state.withLock { (s) throws(ServiceError) -> HistoryPage in
            s.queries.append(query)
            if !s.pageErrors.isEmpty { throw s.pageErrors.removeFirst() }
            if let error = s.failAtQuery.removeValue(forKey: s.queries.count) { throw error }
            let filtered = s.records.filter {
                (query.filter.types.isEmpty || query.filter.types.contains($0.type))
                    && (query.filter.categories.isEmpty || query.filter.categories.contains($0.category))
            }
            let start = query.cursor.flatMap(Int.init) ?? 0
            let end = min(filtered.count, start + query.limit)
            let slice = start < end ? Array(filtered[start..<end]) : []
            return HistoryPage(records: slice, nextCursor: end < filtered.count ? String(end) : nil, totalMatching: filtered.count)
        }
    }

    func detail(wallet: WalletID, txid: String) async throws(ServiceError) -> TransactionDetail {
        guard let detail = state.current.details[txid] else {
            throw ServiceError(code: .historyTxNotFound, detail: txid)
        }
        return detail
    }

    func setLabel(wallet: WalletID, txid: String, label: String?) async throws(ServiceError) {
        state.withLock { $0.labels.append((txid, label)) }
    }

    func changes(wallet: WalletID) -> AsyncStream<[String]> { broadcast.stream() }
}

final class FakeReceive: ReceiveProviding, @unchecked Sendable {
    struct State {
        var current: AddressInfo
        var nextIndex: UInt32 = 1
        var requests: [ReceiveRequest] = []
        var nextRequestID: UInt64 = 1
        var nextAddressLabels: [String?] = []
        var createError: ServiceError?
        /// Further addresses of the wallet (change chain, used ones).
        var others: [AddressInfo] = []
    }

    let state: Locked<State>

    init(address: String = "XcurrentAddress000000000000000001") {
        state = Locked(State(current: FakeReceive.info(address, index: 0)))
    }

    static func info(_ address: String, index: UInt32) -> AddressInfo {
        AddressInfo(
            address: address, chain: .receiving, index: index, derivationPath: "m/44'/5'/0'/0/\(index)", used: false,
            label: nil, balance: nil, txCount: 0)
    }

    func currentAddress(wallet: WalletID) async throws(ServiceError) -> AddressInfo { state.current.current }

    func nextAddress(wallet: WalletID, label: String?) async throws(ServiceError) -> AddressInfo {
        state.withLock {
            let info = FakeReceive.info("XnextAddress0000000000000000000\($0.nextIndex)", index: $0.nextIndex)
            $0.nextIndex += 1
            $0.nextAddressLabels.append(label)
            return info
        }
    }

    func addresses(wallet: WalletID, filter: AddressFilter) async throws(ServiceError) -> [AddressInfo] {
        let s = state.current
        return [s.current] + s.others
    }

    func createRequest(wallet: WalletID, amount: Amount?, label: String?, message: String?) async throws(ServiceError)
        -> ReceiveRequest
    {
        try state.withLock { (s) throws(ServiceError) -> ReceiveRequest in
            if let error = s.createError { throw error }
            var uri = "dash:\(s.current.address)"
            var query: [String] = []
            if let amount { query.append("amount=\(AmountFormatter.plain(amount.duffs, unit: .dash, plusSign: false, separators: .never, justify: false))") }
            if let label { query.append("label=\(label)") }
            if let message { query.append("message=\(message)") }
            if !query.isEmpty { uri += "?" + query.joined(separator: "&") }
            let request = ReceiveRequest(
                id: s.nextRequestID, createdAt: Date(timeIntervalSince1970: 1_700_000_000 + Double(s.nextRequestID)),
                address: s.current.address, amount: amount, label: label, message: message, uri: uri)
            s.nextRequestID += 1
            s.requests.append(request)
            return request
        }
    }

    func requests(wallet: WalletID) async throws(ServiceError) -> [ReceiveRequest] { state.current.requests }

    func deleteRequest(wallet: WalletID, id: UInt64) async throws(ServiceError) {
        state.withLock { $0.requests.removeAll { $0.id == id } }
    }
}

/// Coins and locks as the engine's coins.rs: locking an unknown outpoint is
/// `coins.outpoint_not_found`; unlocking is idempotent. Unconfigured
/// (`coins == nil`) every call is not_implemented.
final class FakeCoinControl: CoinControlProviding, @unchecked Sendable {
    let coins = Locked<[Utxo]?>(nil)
    let locked = Locked<Set<OutPoint>>([])
    let unlockCalls = Locked<[[OutPoint]]>([])

    func utxos(wallet: WalletID, filter: UtxoFilter) async throws(ServiceError) -> [Utxo] {
        guard let coins = coins.current else { throw notConfigured("utxos") }
        let locked = locked.current
        return coins.compactMap { coin in
            let isLocked = locked.contains(coin.outpoint)
            if isLocked && !filter.includeLocked { return nil }
            return Utxo(
                outpoint: coin.outpoint, address: coin.address, amount: coin.amount,
                confirmations: coin.confirmations, date: coin.date, instantLocked: coin.instantLocked,
                chainLocked: coin.chainLocked, userLocked: isLocked, reserved: coin.reserved, label: coin.label,
                isChange: coin.isChange, coinJoinDenominated: coin.coinJoinDenominated,
                coinJoinRounds: coin.coinJoinRounds, spendable: coin.spendable && !isLocked)
        }
    }

    func lock(wallet: WalletID, outpoints: [OutPoint]) async throws(ServiceError) {
        guard let coins = coins.current else { throw notConfigured("lock") }
        let known = Set(coins.map(\.outpoint))
        if let missing = outpoints.first(where: { !known.contains($0) }) {
            throw ServiceError(code: .coinsOutpointNotFound, detail: missing.txid)
        }
        locked.withLock { $0.formUnion(outpoints) }
    }

    func unlock(wallet: WalletID, outpoints: [OutPoint]) async throws(ServiceError) {
        guard coins.current != nil else { throw notConfigured("unlock") }
        unlockCalls.withLock { $0.append(outpoints) }
        locked.withLock { $0.subtract(outpoints) }
    }

    func lockedOutpoints(wallet: WalletID) async throws(ServiceError) -> [OutPoint] {
        guard coins.current != nil else { throw notConfigured("locked") }
        return locked.current.sorted { ($0.txid, $0.vout) < ($1.txid, $1.vout) }
    }
}

final class FakeAddressBook: AddressBookProviding, @unchecked Sendable {
    struct SaveCall: Sendable {
        let address: String
        let label: String
        let purpose: AddressPurpose
        let replace: Bool
    }

    let entries = Locked<[AddressBookEntry]>([])
    let saves = Locked<[SaveCall]>([])
    let deletes = Locked<[String]>([])
    let saveError = Locked<ServiceError?>(nil)

    func entries(wallet: WalletID, purpose: AddressPurpose?, search: String?) async throws(ServiceError)
        -> [AddressBookEntry]
    {
        entries.current.filter { purpose == nil || $0.purpose == purpose }
    }

    func save(wallet: WalletID, address: String, label: String, purpose: AddressPurpose, replace: Bool)
        async throws(ServiceError) -> AddressBookEntry
    {
        saves.withLock { $0.append(SaveCall(address: address, label: label, purpose: purpose, replace: replace)) }
        if let error = saveError.current { throw error }
        let entry = AddressBookEntry(address: address, label: label, purpose: purpose, createdAt: nil)
        entries.withLock { list in
            list.removeAll { $0.address == address }
            list.append(entry)
        }
        return entry
    }

    func delete(wallet: WalletID, address: String) async throws(ServiceError) {
        deletes.withLock { $0.append(address) }
        entries.withLock { $0.removeAll { $0.address == address } }
    }
}

final class FakeMessages: MessageSigning, @unchecked Sendable {
    let signCalls = Locked<[(String, String, AuthGrant)]>([])
    let signResult = Locked<Result<String, ServiceError>>(.success("H+signature=="))
    let verifyError = Locked<ServiceError?>(nil)

    func sign(wallet: WalletID, address: String, message: String, grant: AuthGrant) async throws(ServiceError) -> String {
        signCalls.withLock { $0.append((address, message, grant)) }
        return try signResult.current.get()
    }

    func verify(address: String, message: String, signature: String) throws(ServiceError) {
        if let error = verifyError.current { throw error }
    }
}

// MARK: URIs, settings, preferences, screen capture

/// Address rules of the fake: 34-character base58-looking strings; mainnet
/// `X` = P2PKH, `7` = P2SH; testnet `y` = P2PKH, `8`/`9` = P2SH;
/// `dash1…` = Platform; `dashs…` = shielded.
final class FakeURI: URIHandling, @unchecked Sendable {
    let network: DashNetwork
    let qrRequests = Locked<[String]>([])

    init(network: DashNetwork) {
        self.network = network
    }

    func classifyAddress(_ text: String) -> AddressClass {
        if text.hasPrefix("dash1") || text.hasPrefix("tdash1") { return .platform }
        if text.hasPrefix("dashs") { return .shielded }
        guard text.count == 34, text.allSatisfy({ $0.isLetter || $0.isNumber }) else {
            return .invalid(.notBech32mOrBase58)
        }
        let main = network == .mainnet
        switch text.first {
        case "X": return main ? .core(scriptHash: false) : .invalid(.invalidBase58Prefix)
        case "7": return main ? .core(scriptHash: true) : .invalid(.invalidBase58Prefix)
        case "y": return main ? .invalid(.invalidBase58Prefix) : .core(scriptHash: false)
        case "8", "9": return main ? .invalid(.invalidBase58Prefix) : .core(scriptHash: true)
        default: return .invalid(.invalidBase58Prefix)
        }
    }

    func parsePaymentURI(_ text: String) throws(ServiceError) -> PaymentURI {
        guard text.lowercased().hasPrefix("dash:") else { throw ServiceError(code: .uriUnparsable) }
        let rest = text.dropFirst(5)
        let parts = rest.split(separator: "?", maxSplits: 1)
        let address = String(parts.first ?? "")
        guard case .core = classifyAddress(address) else { throw ServiceError(code: .uriInvalidAddress) }
        var amount: Amount?
        var label: String?
        var message: String?
        if parts.count == 2 {
            for pair in parts[1].split(separator: "&") {
                let kv = pair.split(separator: "=", maxSplits: 1).map(String.init)
                guard kv.count == 2 else { continue }
                let value = kv[1].removingPercentEncoding ?? kv[1]
                switch kv[0] {
                case "amount":
                    guard let duffs = AmountFormatter.parseDuffs(value, unit: .dash) else {
                        throw ServiceError(code: .init(rawValue: "uri.invalid_amount"))
                    }
                    amount = Amount(duffs: duffs)
                case "label": label = value
                case "message": message = value
                default: break
                }
            }
        }
        return PaymentURI(address: address, amount: amount, label: label, message: message)
    }

    /// dash-qt `formatBitcoinURI`: amount in DASH without trailing zeros,
    /// percent-encoded label and message, empty ones left out.
    func buildPaymentURI(address: String, amount: Amount?, label: String?, message: String?) throws(ServiceError) -> String {
        var query: [String] = []
        if let amount {
            var text = String(amount.duffs / 100_000_000)
            let fraction = amount.duffs % 100_000_000
            if fraction != 0 {
                var digits = String(format: "%08lld", fraction)
                while digits.hasSuffix("0") { digits.removeLast() }
                text += "." + digits
            }
            query.append("amount=\(text)")
        }
        let allowed = CharacterSet.urlQueryAllowed.subtracting(CharacterSet(charactersIn: "&=?#+"))
        if let label, !label.isEmpty { query.append("label=" + (label.addingPercentEncoding(withAllowedCharacters: allowed) ?? label)) }
        if let message, !message.isEmpty {
            query.append("message=" + (message.addingPercentEncoding(withAllowedCharacters: allowed) ?? message))
        }
        return "dash:\(address)" + (query.isEmpty ? "" : "?" + query.joined(separator: "&"))
    }

    func qrMatrix(for text: String) throws(ServiceError) -> QRMatrix {
        qrRequests.withLock { $0.append(text) }
        if text.count > 255 { throw ServiceError(code: .init(rawValue: "uri.too_long_for_qr")) }
        return QRMatrix(size: 21, modules: (0..<441).map { $0 % 3 == 0 })
    }
}

@MainActor
final class FakeSettings: SettingsProviding {
    var display: DisplaySettings
    var lastNetwork: DashNetwork?
    var updateError: ServiceError?
    let broadcast = Broadcast<DisplaySettings>()
    var updates: [DisplaySettings] = []

    init(display: DisplaySettings = DisplaySettings(decimalDigits: 2), lastNetwork: DashNetwork? = .testnet) {
        self.display = display
        self.lastNetwork = lastNetwork
    }

    func update(_ display: DisplaySettings) throws(ServiceError) {
        if let updateError { throw updateError }
        updates.append(display)
        self.display = display
        broadcast.send(display)
    }

    func changes() -> AsyncStream<DisplaySettings> { broadcast.stream() }
}

@MainActor
final class FakePreferences: UIPreferencesStoring {
    var preferences: UIPreferences
    var updateError: ServiceError?
    var updates = 0

    init(_ preferences: UIPreferences = UIPreferences()) {
        self.preferences = preferences
    }

    func update(_ preferences: UIPreferences) throws(ServiceError) {
        if let updateError { throw updateError }
        updates += 1
        self.preferences = preferences
    }
}

@MainActor
final class FakeScreenCapture: ScreenCaptureGuard {
    var visibleCalls: [Bool] = []
    var protects = true
    let broadcast = Broadcast<ScreenCaptureEvent>()

    func setSecretContentVisible(_ visible: Bool) -> Bool {
        visibleCalls.append(visible)
        return protects
    }

    func events() -> AsyncStream<ScreenCaptureEvent> { broadcast.stream() }
}
