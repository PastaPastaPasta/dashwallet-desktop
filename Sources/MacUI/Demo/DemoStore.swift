// Demo mode state (DESIGN-opus §4.2 fixture mode, IOS-001): one in-memory
// wallet world that every demo service reads and changes. Nothing here
// touches the engine, the network or the disk.
#if os(macOS)
import Foundation
import WalletFeatures
import WalletRuntime

/// Starting state of demo mode, chosen with `--demo-scenario`.
public enum DemoScenario: String, Sendable, CaseIterable {
    /// One funded testnet wallet, unencrypted vault, sync done.
    case funded
    /// No vault and no wallet: the app opens onboarding.
    case fresh
    /// The funded wallet with an encrypted, locked vault (passphrase `demo`).
    case locked
}

@MainActor
final class DemoStore {
    /// The passphrase of the `locked` scenario's vault.
    nonisolated static let lockedPassphrase = "demo"
    /// The phrase `generateMnemonic` returns; every word is in the BIP39 English list.
    nonisolated static let phrase = [
        "galaxy", "rocket", "velvet", "harbor", "tiny", "maple", "oyster", "crane", "sunset", "ribbon", "empty", "orbit",
        "tunnel", "author", "lizard", "bamboo", "ripple", "wisdom", "canyon", "frost", "pulse", "gentle", "vivid", "echo",
    ]

    let now: () -> Date
    private(set) var network: DashNetwork = .testnet
    private(set) var transition: LifecycleTransition = .idle
    private(set) var wallets: [WalletInfo] = []
    private(set) var selected: WalletID?
    private var ledgers: [WalletID: DemoLedger] = [:]
    private var phrases: [WalletID: String] = [:]

    // Vault.
    private(set) var vaultState: VaultLockState = .noVault
    private(set) var vaultEncrypted = false
    private var vaultPassphrase: String?
    private(set) var failedAttempts: UInt32 = 0

    var display = DisplaySettings(unit: .dash, decimalDigits: 8, hideBalances: false)
    var preferences = UIPreferences()
    private(set) var sync: SyncStatus?

    nonisolated let walletChanges = DemoBroadcaster<Void>()
    nonisolated let syncChanges = DemoBroadcaster<SyncStatus>()
    nonisolated let lockChanges = DemoBroadcaster<VaultLockState>()
    nonisolated let transitionChanges = DemoBroadcaster<LifecycleTransition>()
    nonisolated let displayChanges = DemoBroadcaster<DisplaySettings>()
    nonisolated let historyChanges = DemoBroadcaster<(WalletID, [String])>()
    /// Network the sync formatter and URI handler use.
    nonisolated let activeNetwork = DemoLocked<DashNetwork>(.testnet)

    init(scenario: DemoScenario, now: @escaping () -> Date = { Date() }) {
        self.now = now
        switch scenario {
        case .fresh:
            break
        case .funded:
            vaultState = .unencrypted
            addWallet(name: "Demo wallet", phrase: Self.phrase.prefix(12).joined(separator: " "), funded: true)
        case .locked:
            vaultState = .locked
            vaultEncrypted = true
            vaultPassphrase = Self.lockedPassphrase
            addWallet(name: "Demo wallet", phrase: Self.phrase.prefix(12).joined(separator: " "), funded: true)
        }
        sync = Self.syncedStatus(now: now())
    }

    // MARK: Wallets

    var balances: WalletBalances? { selected.flatMap { ledgers[$0]?.balances } }

    func ledger(_ id: WalletID) throws(ServiceError) -> DemoLedger {
        guard let ledger = ledgers[id] else { throw .demo(.walletNotFound) }
        return ledger
    }

    func updateLedger(_ id: WalletID, _ change: (inout DemoLedger) -> Void) throws(ServiceError) {
        guard var ledger = ledgers[id] else { throw .demo(.walletNotFound) }
        change(&ledger)
        ledgers[id] = ledger
        refreshWalletInfo(id)
    }

    @discardableResult
    private func addWallet(name: String, phrase: String, funded: Bool) -> WalletID {
        var seed = DemoRandom(text: phrase + network.description)
        let id = WalletID(hex: seed.hex(bytes: 32))!
        let ledger = funded ? DemoLedger.funded(seed: phrase, now: now()) : DemoLedger(seed: phrase)
        ledgers[id] = ledger
        phrases[id] = phrase
        wallets.append(Self.info(id: id, name: name, ledger: ledger, createdAt: now()))
        if selected == nil { selected = id }
        return id
    }

    private func refreshWalletInfo(_ id: WalletID) {
        guard let index = wallets.firstIndex(where: { $0.id == id }), let ledger = ledgers[id] else { return }
        wallets[index] = Self.info(id: id, name: wallets[index].name, ledger: ledger, createdAt: wallets[index].createdAt)
        walletChanges.send(())
    }

    private static func info(id: WalletID, name: String, ledger: DemoLedger, createdAt: Date?) -> WalletInfo {
        WalletInfo(
            id: id, name: name, watchOnly: false, hasMnemonic: true, hd: true, birthHeight: 0, createdAt: createdAt,
            balances: ledger.balances)
    }

    func select(_ id: WalletID) {
        guard wallets.contains(where: { $0.id == id }) else { return }
        selected = id
        walletChanges.send(())
    }

    func rename(_ id: WalletID, to name: String) throws(ServiceError) {
        guard let index = wallets.firstIndex(where: { $0.id == id }) else { throw .demo(.walletNotFound) }
        let old = wallets[index]
        wallets[index] = WalletInfo(
            id: old.id, name: name, watchOnly: old.watchOnly, hasMnemonic: old.hasMnemonic, hd: old.hd,
            birthHeight: old.birthHeight, createdAt: old.createdAt, balances: old.balances)
        walletChanges.send(())
    }

    // MARK: Lifecycle

    func setTransition(_ transition: LifecycleTransition) {
        self.transition = transition
        transitionChanges.send(transition)
    }

    func switchNetwork(to network: DashNetwork) {
        self.network = network
        activeNetwork.current = network
        // Each network has its own wallets; demo mode starts the other networks empty.
        wallets = []
        ledgers = [:]
        phrases = [:]
        selected = nil
        walletChanges.send(())
    }

    func importWallet(phrase: String, options: WalletImportOptions) throws(ServiceError) -> WalletID {
        guard vaultState != .noVault else { throw .demo(EngineCodes.walletNoVault) }
        guard vaultState != .locked else { throw .demo(.vaultLocked) }
        if phrases.values.contains(phrase) { throw .demo(.walletAlreadyExists) }
        // A restored demo phrase gets the scripted history; a new one starts empty.
        let restoredDemo = phrase == Self.phrase.prefix(12).joined(separator: " ")
        let id = addWallet(name: options.name ?? "Wallet \(wallets.count + 1)", phrase: phrase, funded: restoredDemo)
        selected = id
        walletChanges.send(())
        return id
    }

    func phrase(of id: WalletID) -> String? { phrases[id] }

    // MARK: Vault

    var vaultStatus: VaultStatus {
        VaultStatus(
            state: vaultState, encrypted: vaultEncrypted, quickUnlockEnrolled: false, failedAttempts: failedAttempts,
            retryAfterSeconds: nil, walletsWithSecrets: wallets.map(\.id))
    }

    func createVault(passphrase: String?) throws(ServiceError) -> VaultStatus {
        guard vaultState == .noVault else { throw .demo(.vaultAlreadyExists) }
        vaultEncrypted = passphrase != nil
        vaultPassphrase = passphrase
        setLockState(passphrase == nil ? .unencrypted : .unlocked)
        return vaultStatus
    }

    func checkPassphrase(_ text: String) throws(ServiceError) {
        guard vaultEncrypted else { return }
        guard text == vaultPassphrase else {
            failedAttempts += 1
            throw .demo(.vaultWrongPassphrase)
        }
        failedAttempts = 0
    }

    func encrypt(newPassphrase: String) throws(ServiceError) -> VaultStatus {
        guard !vaultEncrypted else { throw .demo(EngineCodes.vaultAlreadyEncrypted) }
        vaultEncrypted = true
        vaultPassphrase = newPassphrase
        setLockState(.unlocked)
        return vaultStatus
    }

    func changePassphrase(old: String, new: String) throws(ServiceError) -> VaultStatus {
        guard vaultEncrypted else { throw .demo(EngineCodes.vaultNotEncrypted) }
        try checkPassphrase(old)
        vaultPassphrase = new
        return vaultStatus
    }

    func setLockState(_ state: VaultLockState) {
        vaultState = state
        lockChanges.send(state)
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

    func setSync(_ status: SyncStatus) {
        sync = status
        syncChanges.send(status)
    }

    // MARK: Settings

    func updateDisplay(_ display: DisplaySettings) {
        self.display = display
        displayChanges.send(display)
    }

    // MARK: History

    func notifyHistory(_ wallet: WalletID, txids: [String]) {
        historyChanges.send((wallet, txids))
    }
}

/// Engine codes the demo throws that `ServiceErrorCode` does not name
/// (docs/contracts/m1-engine.md §4).
enum EngineCodes {
    static let walletNoVault = ServiceErrorCode(rawValue: "wallet.no_vault")
    static let vaultAlreadyEncrypted = ServiceErrorCode(rawValue: "vault.already_encrypted")
    static let vaultNotEncrypted = ServiceErrorCode(rawValue: "vault.not_encrypted")
    static let messageAddressNotMine = ServiceErrorCode(rawValue: "message.address_not_mine")
    static let messageMalformedSignature = ServiceErrorCode(rawValue: "message.malformed_signature")
    static let uriTooLongForQR = ServiceErrorCode(rawValue: "uri.too_long_for_qr")
    static let labelsEntryNotFound = ServiceErrorCode(rawValue: "labels.entry_not_found")
    static let receiveRequestNotFound = ServiceErrorCode(rawValue: "receive.request_not_found")
}
#endif
