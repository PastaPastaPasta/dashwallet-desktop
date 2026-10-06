// Demo mode state (DESIGN-opus §4.2 fixture mode, IOS-001): one in-memory
// wallet world that every demo service reads and changes. Nothing here
// touches the engine's session, the network or the disk.
//
// The rules are the engine's (docs/contracts/m1-engine.md): each network
// has its own vault and wallets; grants are single-use, bound to a wallet,
// expire after 120 s and follow the credential table of §2.2; a passphrase
// grant never unlocks the vault; failed passphrases count toward the
// `6^(n−3)·60 s` throttle.
import Foundation
import WalletFeatures
import WalletRuntime

/// One network's vault (m1-engine.md §2.2).
struct DemoVaultState: Sendable {
    var state: VaultLockState = .noVault
    var passphrase: String?
    var failedAttempts: UInt32 = 0
    var lastFailure: Date?

    var encrypted: Bool { passphrase != nil }
}

/// One network's wallets and vault.
struct DemoNetworkState: Sendable {
    var wallets: [WalletInfo] = []
    var selected: WalletID?
    var ledgers: [WalletID: DemoLedger] = [:]
    var phrases: [WalletID: String] = [:]
    var vault = DemoVaultState()
}

/// An issued grant. `ownKey`: issued with the passphrase on a locked or
/// mixing-only vault, so it can act while the vault stays locked.
struct DemoGrant: Sendable {
    let grant: AuthGrant
    let wallet: WalletID?
    let network: DashNetwork
    let ownKey: Bool
}

/// The grant kinds `redeem` checks for (the engine's `GrantKind`).
enum DemoGrantKind: Sendable {
    case spend, revealSecret, signMessage, changeCredential, wipe

    func matches(_ purpose: GrantPurpose) -> Bool {
        switch (self, purpose) {
        case (.spend, .spend), (.revealSecret, .revealSecret), (.signMessage, .signMessage),
            (.changeCredential, .changeCredential), (.wipe, .wipe):
            true
        default:
            false
        }
    }
}

/// The codes a domain refuses a grant with: unknown, used or expired
/// (`invalid`), or of another purpose or wallet (`mismatch`).
struct DemoGrantRefusal: Sendable {
    let invalid: ServiceErrorCode
    let mismatch: ServiceErrorCode

    static let vault = DemoGrantRefusal(invalid: .vaultGrantInvalid, mismatch: .vaultGrantPurposeMismatch)
    static let send = DemoGrantRefusal(invalid: .sendGrantInvalid, mismatch: .sendGrantInvalid)
    static let message = DemoGrantRefusal(invalid: .messageGrantInvalid, mismatch: .messageGrantInvalid)
    static let wallet = DemoGrantRefusal(invalid: .walletGrantInvalid, mismatch: .walletGrantInvalid)
}

@MainActor
final class DemoWorld {
    /// Grant lifetime (m1-engine.md §2.2).
    static let grantLifetime: TimeInterval = 120

    let now: @Sendable () -> Date
    /// The open network; read by the URI handler and formatter from any thread.
    nonisolated let activeNetwork: DemoLocked<DashNetwork>
    private(set) var states: [DashNetwork: DemoNetworkState] = [:]
    private var grants: [String: DemoGrant] = [:]
    private(set) var transition: LifecycleTransition = .idle
    private(set) var sync: SyncStatus
    var display = DisplaySettings()
    var preferences = UIPreferences()

    nonisolated let walletChanges = DemoBroadcaster<Void>()
    nonisolated let syncChanges = DemoBroadcaster<SyncStatus>()
    nonisolated let lockChanges = DemoBroadcaster<VaultLockState>()
    nonisolated let transitionChanges = DemoBroadcaster<LifecycleTransition>()
    nonisolated let displayChanges = DemoBroadcaster<DisplaySettings>()
    nonisolated let historyChanges = DemoBroadcaster<(WalletID, [String])>()

    init(network: DashNetwork, scenario: DemoScenario, now: @escaping @Sendable () -> Date) {
        self.now = now
        activeNetwork = DemoLocked(network)
        sync = Self.syncedStatus(now: now())
        var state = DemoNetworkState()
        switch scenario {
        case .fresh:
            break
        case .funded, .locked, .offline:
            state.vault = scenario == .locked
                ? DemoVaultState(state: .locked, passphrase: DemoEnvironment.passphrase)
                : DemoVaultState(state: .unencrypted)
            Self.add(
                to: &state, name: "Demo wallet", phrase: DemoEnvironment.sampleWalletPhrase, network: network,
                funded: true, now: now())
        }
        if scenario == .offline { sync = Self.offlineStatus(now: now()) }
        states[network] = state
    }

    var network: DashNetwork { activeNetwork.current }

    var current: DemoNetworkState {
        get { states[network] ?? DemoNetworkState() }
        set { states[network] = newValue }
    }

    var vault: DemoVaultState { current.vault }

    // MARK: Wallets

    var selectedBalances: WalletBalances? {
        current.selected.flatMap { current.ledgers[$0]?.balances }
    }

    func ledger(_ id: WalletID) throws(ServiceError) -> DemoLedger {
        guard let ledger = current.ledgers[id] else { throw .demo(.walletNotFound) }
        return ledger
    }

    /// Changes a wallet's ledger and announces the new balances.
    func update<T>(_ id: WalletID, _ change: (inout DemoLedger) throws(ServiceError) -> T) throws(ServiceError) -> T {
        guard var ledger = current.ledgers[id] else { throw .demo(.walletNotFound) }
        let result = try change(&ledger)
        current.ledgers[id] = ledger
        refreshWalletInfo(id)
        return result
    }

    private func refreshWalletInfo(_ id: WalletID) {
        guard let index = current.wallets.firstIndex(where: { $0.id == id }), let ledger = current.ledgers[id] else { return }
        let old = current.wallets[index]
        current.wallets[index] = WalletInfo(
            id: id, name: old.name, watchOnly: false, hasMnemonic: true, hd: true, birthHeight: old.birthHeight,
            createdAt: old.createdAt, balances: ledger.balances)
        walletChanges.send(())
    }

    @discardableResult
    private static func add(
        to state: inout DemoNetworkState, name: String, phrase: String, network: DashNetwork, funded: Bool, now: Date
    ) -> WalletID {
        var seed = DemoRandom(text: phrase + "|" + network.description)
        let id = WalletID(hex: seed.hex(bytes: 32))!
        let ledger = funded
            ? DemoLedger.funded(seed: phrase, network: network, now: now)
            : DemoLedger(seed: phrase, network: network)
        state.ledgers[id] = ledger
        state.phrases[id] = phrase
        state.wallets.append(WalletInfo(
            id: id, name: name, watchOnly: false, hasMnemonic: true, hd: true, birthHeight: 0, createdAt: now,
            balances: ledger.balances))
        state.selected = id
        return id
    }

    func select(_ id: WalletID) {
        guard current.wallets.contains(where: { $0.id == id }) else { return }
        current.selected = id
        walletChanges.send(())
    }

    func rename(_ id: WalletID, to name: String) throws(ServiceError) {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard (1...64).contains(trimmed.count) else { throw .demo(.walletNameRejected) }
        guard let index = current.wallets.firstIndex(where: { $0.id == id }) else { throw .demo(.walletNotFound) }
        let old = current.wallets[index]
        current.wallets[index] = WalletInfo(
            id: old.id, name: trimmed, watchOnly: old.watchOnly, hasMnemonic: old.hasMnemonic, hd: old.hd,
            birthHeight: old.birthHeight, createdAt: old.createdAt, balances: old.balances)
        walletChanges.send(())
    }

    /// The engine's `import_wallet`: needs a vault with its key available.
    /// The sample phrase restores the scripted history; any other phrase
    /// starts empty.
    func importWallet(phrase: String, name: String?) throws(ServiceError) -> WalletID {
        switch vault.state {
        case .noVault: throw .demo(.walletNoVault)
        case .locked, .unlockedMixingOnly: throw .demo(.walletVaultLocked)
        case .noKeys, .unencrypted, .unlocked: break
        }
        let check = try EngineFunctions.checkMnemonic(DemoSecret(utf8: phrase))
        guard check.checksum != .invalid, check.unknownWordIndices.isEmpty else { throw .demo(.walletInvalidMnemonic) }
        guard !current.phrases.values.contains(phrase) else { throw .demo(.walletAlreadyExists) }
        var state = current
        let id = Self.add(
            to: &state, name: name ?? "Wallet \(state.wallets.count + 1)", phrase: phrase, network: network,
            funded: phrase == DemoEnvironment.sampleWalletPhrase, now: now())
        current = state
        walletChanges.send(())
        return id
    }

    /// The engine's `remove_wallet`: a `Wipe` grant for `id`, checked before
    /// it is used (a locked vault needs a grant with its own key).
    func removeWallet(_ id: WalletID, grant: AuthGrant) throws(ServiceError) {
        guard current.wallets.contains(where: { $0.id == id }) else { throw .demo(.walletNotFound) }
        try check(grant, .wipe, wallet: id, refuse: .wallet, locked: .walletVaultLocked)
        try redeem(grant, .wipe, wallet: id, refuse: .wallet)
        var state = current
        state.wallets.removeAll { $0.id == id }
        state.ledgers[id] = nil
        state.phrases[id] = nil
        if state.selected == id { state.selected = state.wallets.first?.id }
        current = state
        walletChanges.send(())
    }

    func phrase(of id: WalletID) -> String? { current.phrases[id] }

    // MARK: Lifecycle

    func setTransition(_ transition: LifecycleTransition) {
        self.transition = transition
        transitionChanges.send(transition)
    }

    /// Opens `network`: its own vault and wallets (empty the first time).
    func switchNetwork(to network: DashNetwork) {
        grants = [:]
        activeNetwork.current = network
        if states[network] == nil { states[network] = DemoNetworkState() }
        walletChanges.send(())
        lockChanges.send(vault.state)
        syncChanges.send(sync)
    }

    // MARK: Vault

    var vaultStatus: VaultStatus {
        VaultStatus(
            state: vault.state, encrypted: vault.encrypted, quickUnlockEnrolled: false,
            failedAttempts: vault.failedAttempts, retryAfterSeconds: retryAfter(),
            walletsWithSecrets: current.wallets.map(\.id))
    }

    private func setVault(_ change: (inout DemoVaultState) -> Void) {
        let before = vault.state
        var state = current
        change(&state.vault)
        current = state
        if state.vault.state != before { lockChanges.send(state.vault.state) }
    }

    /// The engine's `new_passphrase` check: a new passphrase is non-empty and
    /// at most 1024 bytes.
    private static func validateNew(_ passphrase: String) throws(ServiceError) {
        guard !passphrase.isEmpty, passphrase.utf8.count <= 1024 else { throw .demo(.vaultPassphraseRejected) }
    }

    func createVault(passphrase: String?) throws(ServiceError) -> VaultStatus {
        guard vault.state == .noVault else { throw .demo(.vaultAlreadyExists) }
        if let passphrase { try Self.validateNew(passphrase) }
        setVault { $0 = DemoVaultState(state: passphrase == nil ? .unencrypted : .unlocked, passphrase: passphrase) }
        return vaultStatus
    }

    /// dash-qt "Encrypt Wallet": leaves the vault locked, like the engine.
    func encrypt(newPassphrase: String, grant: AuthGrant) throws(ServiceError) -> VaultStatus {
        try Self.validateNew(newPassphrase)
        guard vault.state != .noVault else { throw .demo(.vaultNoVault) }
        guard !vault.encrypted else { throw .demo(.vaultAlreadyEncrypted) }
        try redeem(grant, .changeCredential, wallet: nil, refuse: .vault)
        grants = [:]
        setVault {
            $0.passphrase = newPassphrase
            $0.state = .locked
        }
        return vaultStatus
    }

    /// Like the engine, the new passphrase is validated before the old one is
    /// checked, so a rejected new passphrase counts no failed attempt.
    func changePassphrase(old: String, new: String) throws(ServiceError) -> VaultStatus {
        try Self.validateNew(new)
        guard vault.encrypted else { throw .demo(.vaultNotEncrypted) }
        try checkPassphrase(old)
        setVault { $0.passphrase = new }
        return vaultStatus
    }

    func unlock(passphrase: String, scope: UnlockScope) throws(ServiceError) {
        guard vault.state != .noVault else { throw .demo(.vaultNoVault) }
        guard vault.encrypted else { throw .demo(.vaultNotEncrypted) }
        try checkPassphrase(passphrase)
        let next: VaultLockState = scope == .full ? .unlocked : .unlockedMixingOnly
        // The engine's `install_key`: a change of the lock state revokes
        // every grant.
        if next != vault.state { grants = [:] }
        setVault { $0.state = next }
    }

    /// Drops the key and every grant; idempotent.
    func lock() {
        grants = [:]
        guard vault.encrypted else { return }
        setVault { $0.state = .locked }
    }

    /// Seconds before the next passphrase attempt is allowed (IOS-012).
    private func retryAfter() -> UInt64? {
        guard vault.failedAttempts >= 3, let last = vault.lastFailure else { return nil }
        let exponent = Double(min(vault.failedAttempts - 3, 20))
        let wait = pow(6, exponent) * 60
        let left = last.addingTimeInterval(wait).timeIntervalSince(max(now(), last))
        return left > 0 ? UInt64(left.rounded(.up)) : nil
    }

    private func checkPassphrase(_ text: String) throws(ServiceError) {
        if let wait = retryAfter() { throw .demo(.vaultThrottled, retryAfter: wait) }
        guard text == vault.passphrase else {
            let time = now()
            setVault {
                $0.failedAttempts += 1
                $0.lastFailure = time
            }
            let wait = retryAfter()
            var parameters = ["failed_attempts": Int64(vault.failedAttempts)]
            if let wait { parameters["retry_after_secs"] = Int64(wait) }
            throw .demo(.vaultWrongPassphrase, retryAfter: wait, parameters: parameters)
        }
        if vault.failedAttempts > 0 { setVault { $0.failedAttempts = 0; $0.lastFailure = nil } }
    }

    // MARK: Grants

    /// The engine's `Vault.authorize` with its credential table (§2.2).
    func authorize(_ purpose: GrantPurpose, wallet: WalletID?, passphrase: String?) throws(ServiceError) -> AuthGrant {
        guard (purpose == .changeCredential) == (wallet == nil) else { throw .demo(.invalidArgument, "wallet binding") }
        let sensitive: Bool
        switch purpose {
        case .revealSecret, .changeCredential, .wipe: sensitive = true
        case .spend, .signMessage, .masternodeOperation, .governance, .platformOperation: sensitive = false
        }
        var ownKey = false
        switch vault.state {
        case .noVault:
            throw .demo(.vaultNoVault)
        case .noKeys, .unencrypted:
            if passphrase != nil { throw .demo(.vaultNotEncrypted) }
        case .locked, .unlockedMixingOnly, .unlocked:
            if let passphrase {
                try checkPassphrase(passphrase)
                ownKey = vault.state != .unlocked
            } else if sensitive {
                throw .demo(.vaultCredentialRequired)
            } else if vault.state == .locked {
                throw .demo(.vaultLocked)
            } else if vault.state == .unlockedMixingOnly {
                throw .demo(.vaultMixingOnly)
            }
        }
        let grant = AuthGrant(
            id: UUID().uuidString.lowercased(), purpose: purpose, expiresAt: now().addingTimeInterval(Self.grantLifetime),
            singleUse: true)
        grants[grant.id] = DemoGrant(grant: grant, wallet: wallet, network: network, ownKey: ownKey)
        return grant
    }

    func revoke(_ grant: AuthGrant) {
        grants[grant.id] = nil
    }

    /// The live, matching grant `grant` names. An expired one is dropped.
    private func valid(_ grant: AuthGrant, _ kind: DemoGrantKind, wallet: WalletID?, refuse: DemoGrantRefusal)
        throws(ServiceError) -> DemoGrant
    {
        guard let issued = grants[grant.id], issued.network == network else { throw .demo(refuse.invalid) }
        guard issued.grant.expiresAt >= now() else {
            grants[grant.id] = nil
            throw .demo(refuse.invalid)
        }
        guard kind.matches(issued.grant.purpose), issued.wallet == wallet else { throw .demo(refuse.mismatch) }
        return issued
    }

    /// Checks a grant without using it, and that a key is available to act
    /// with (the grant's own, or the vault's); else `locked`.
    func check(
        _ grant: AuthGrant, _ kind: DemoGrantKind, wallet: WalletID?, refuse: DemoGrantRefusal,
        locked: ServiceErrorCode
    ) throws(ServiceError) {
        let issued = try valid(grant, kind, wallet: wallet, refuse: refuse)
        switch vault.state {
        case .noKeys, .unencrypted, .unlocked: return
        case .noVault, .locked, .unlockedMixingOnly: if !issued.ownKey { throw .demo(locked) }
        }
    }

    /// Uses up a single-use grant. A grant of another kind or wallet is
    /// refused (`refuse.mismatch`) and stays issued; an unknown, used,
    /// revoked or expired one is refused with `refuse.invalid`.
    @discardableResult
    func redeem(_ grant: AuthGrant, _ kind: DemoGrantKind, wallet: WalletID?, refuse: DemoGrantRefusal)
        throws(ServiceError) -> DemoGrant
    {
        let issued = try valid(grant, kind, wallet: wallet, refuse: refuse)
        grants[grant.id] = nil
        return issued
    }

    // MARK: Sync

    static func syncedStatus(now: Date) -> SyncStatus {
        let tip = DemoLedger.tipHeight
        return SyncStatus(
            running: true,
            phases: SyncPhase.allCases.map { SyncPhaseProgress(phase: $0, currentHeight: tip, targetHeight: tip, done: true) },
            activePhase: nil, tipHeight: tip, tipDate: now.addingTimeInterval(-95), chainLockHeight: tip,
            connectedPeers: 8, progress: 1, isDone: true, isStalled: false)
    }

    /// SPV running without peers, three days behind: the sync overlay shows
    /// and a broadcast fails with `send.no_peers`.
    static func offlineStatus(now: Date) -> SyncStatus {
        let tip = DemoLedger.tipHeight - 1_728
        return SyncStatus(
            running: true,
            phases: [
                SyncPhaseProgress(phase: .headers, currentHeight: tip, targetHeight: DemoLedger.tipHeight, done: false),
            ],
            activePhase: .headers, tipHeight: tip, tipDate: now.addingTimeInterval(-3 * 86_400), chainLockHeight: nil,
            connectedPeers: 0, progress: 0.42, isDone: false, isStalled: true)
    }

    func notifyHistory(_ wallet: WalletID, txids: [String]) {
        historyChanges.send((wallet, txids))
    }

    func updateDisplay(_ display: DisplaySettings) {
        self.display = display
        displayChanges.send(display)
    }
}
