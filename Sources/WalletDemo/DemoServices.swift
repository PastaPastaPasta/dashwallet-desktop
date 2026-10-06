// The WalletRuntime service protocols (docs/contracts/m1-swift.md §2) over
// `DemoWorld`: host, lifecycle, state holders, vault and authorization.
import Foundation
import WalletFeatures
import WalletRuntime

// MARK: Host and lifecycle

final class DemoHost: WalletHosting {
    let world: DemoWorld

    init(world: DemoWorld) {
        self.world = world
    }

    var activeNetwork: DashNetwork? {
        get async { await world.network }
    }

    func start(network: DashNetwork, options: NetworkOptions) async throws(ServiceError) {}

    func stop() async throws(ServiceError) {}

    /// A scratch path: demo mode stores nothing.
    func dataDirectory(for network: DashNetwork) -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("dashwallet-demo/\(network.description)")
    }
}

final class DemoLifecycle: LifecycleQueueing {
    let world: DemoWorld

    init(world: DemoWorld) {
        self.world = world
    }

    var transition: LifecycleTransition {
        get async { await world.transition }
    }

    func transitions() -> AsyncStream<LifecycleTransition> {
        world.transitionChanges.stream()
    }

    func start(network: DashNetwork) async throws(ServiceError) {}

    func stop() async throws(ServiceError) {}

    func switchNetwork(to network: DashNetwork) async throws(ServiceError) {
        let from = await world.network
        try await run(.switchingNetwork(from: from, to: network)) { $0.switchNetwork(to: network) }
    }

    func importWallet(
        mnemonic: any SecretBuffer, bip39Passphrase: any SecretBuffer, options: WalletImportOptions
    ) async throws(ServiceError) -> WalletID {
        let phrase = DemoSecret.text(mnemonic)
        return try await run(.addingWallet) { world throws(ServiceError) in
            try world.importWallet(phrase: phrase, name: options.name)
        }
    }

    func removeWallet(_ id: WalletID, grant: AuthGrant) async throws(ServiceError) {
        try await run(.removingWallet(id)) { world throws(ServiceError) in try world.removeWallet(id, grant: grant) }
    }

    /// Shows `transition` briefly (so the overlay is visible), runs `body`,
    /// then returns to idle.
    @MainActor
    private func run<T: Sendable>(
        _ transition: LifecycleTransition, _ body: @MainActor (DemoWorld) throws(ServiceError) -> T
    ) async throws(ServiceError) -> T {
        world.setTransition(transition)
        defer { world.setTransition(.idle) }
        try? await Task.sleep(for: .milliseconds(400))
        return try body(world)
    }
}

// MARK: State holders (main actor)

@MainActor
final class DemoWalletState: WalletStateProviding {
    let world: DemoWorld

    init(world: DemoWorld) {
        self.world = world
    }

    var wallets: [WalletInfo]? { world.current.wallets }
    var selectedWalletID: WalletID? { world.current.selected }
    var balances: WalletBalances? { world.selectedBalances }
    func changes() -> AsyncStream<Void> { world.walletChanges.stream() }
    func select(_ id: WalletID) { world.select(id) }
    func rename(_ id: WalletID, to name: String) async throws(ServiceError) { try world.rename(id, to: name) }
}

@MainActor
final class DemoSync: SyncStatusProviding {
    let world: DemoWorld

    init(world: DemoWorld) {
        self.world = world
    }

    var status: SyncStatus? { world.sync }

    func changes() -> AsyncStream<SyncStatus> { world.syncChanges.stream(initial: world.sync) }

    func peers() async throws(ServiceError) -> [PeerInfo] {
        let count = Int(world.sync.connectedPeers)
        return (0..<count).map { index in
            PeerInfo(
                address: "203.0.113.\(11 + index):19999", userAgent: "/Dash Core:23.1.7/", protocolVersion: 70_235,
                bestHeight: DemoLedger.tipHeight, pingMilliseconds: UInt32(27 + index * 7),
                connectedSince: world.now().addingTimeInterval(-Double(index + 1) * 600), inbound: false)
        }
    }

    func rotatePeers() async throws(ServiceError) {}

    func rescan(from start: RescanStart) async throws(ServiceError) {
        if case .height(let height) = start, height > DemoLedger.tipHeight {
            throw .demo(.syncHeightOutOfRange, parameters: ["height": Int64(height)])
        }
    }
}

@MainActor
final class DemoSettings: SettingsProviding, UIPreferencesStoring {
    let world: DemoWorld

    init(world: DemoWorld) {
        self.world = world
    }

    var display: DisplaySettings { world.display }
    var lastNetwork: DashNetwork? { world.network }
    func update(_ display: DisplaySettings) throws(ServiceError) { world.updateDisplay(display) }
    func changes() -> AsyncStream<DisplaySettings> { world.displayChanges.stream() }

    var preferences: UIPreferences { world.preferences }
    func update(_ preferences: UIPreferences) throws(ServiceError) { world.preferences = preferences }
}

// MARK: Vault and authorization

final class DemoVault: VaultProviding {
    let world: DemoWorld

    init(world: DemoWorld) {
        self.world = world
    }

    func status() async throws(ServiceError) -> VaultStatus { await world.vaultStatus }

    func create(passphrase: (any SecretBuffer)?) async throws(ServiceError) -> VaultStatus {
        let text = passphrase.map(DemoSecret.text)
        return try await world.createVault(passphrase: text)
    }

    func encrypt(newPassphrase: any SecretBuffer, grant: AuthGrant) async throws(ServiceError) -> VaultStatus {
        try await world.encrypt(newPassphrase: DemoSecret.text(newPassphrase), grant: grant)
    }

    func changePassphrase(old: any SecretBuffer, new: any SecretBuffer) async throws(ServiceError) -> VaultStatus {
        try await world.changePassphrase(old: DemoSecret.text(old), new: DemoSecret.text(new))
    }

    func revealMnemonic(wallet: WalletID, grant: AuthGrant) async throws(ServiceError) -> RevealedMnemonic {
        let phrase = try await reveal(wallet: wallet, grant: grant)
        return RevealedMnemonic(phrase: DemoSecret(utf8: phrase), bip39Passphrase: DemoSecret(utf8: ""))
    }

    @MainActor
    private func reveal(wallet: WalletID, grant: AuthGrant) throws(ServiceError) -> String {
        try world.check(grant, .revealSecret, wallet: wallet, refuse: .vault, locked: .vaultLocked)
        try world.redeem(grant, .revealSecret, wallet: wallet, refuse: .vault)
        guard let phrase = world.phrase(of: wallet) else { throw .demo(.vaultNoSecret) }
        return phrase
    }

    /// Always the demo phrase, so screenshots are stable and nothing looks
    /// like a usable new wallet.
    func generateMnemonic(wordCount: Int, language: MnemonicLanguage) async throws(ServiceError) -> any SecretBuffer {
        guard wordCount == 12 || wordCount == 24 else { throw .demo(.walletUnsupportedWordCount, "demo has 12 and 24 words") }
        return DemoSecret(utf8: DemoEnvironment.phrase.prefix(wordCount).joined(separator: " "))
    }

    func checkMnemonic(_ phrase: any SecretBuffer) async throws(ServiceError) -> MnemonicCheck {
        try EngineFunctions.checkMnemonic(phrase)
    }

    func makeSecret(utf8 text: String) -> any SecretBuffer { DemoSecret(utf8: text) }
}

@MainActor
final class DemoAuth: AuthenticationGating {
    let world: DemoWorld

    init(world: DemoWorld) {
        self.world = world
    }

    var lockState: VaultLockState? { world.vault.state }

    func lockStateChanges() -> AsyncStream<VaultLockState> { world.lockChanges.stream(initial: world.vault.state) }

    /// `AuthenticationGate`'s table with "require authentication for every
    /// payment" on (the setting's default).
    func requirement(for purpose: GrantPurpose) -> CredentialRequirement {
        switch world.vault.state {
        case .noVault, .noKeys, .unencrypted: .none
        case .locked, .unlockedMixingOnly, .unlocked: .passphrase
        }
    }

    func authorize(_ purpose: GrantPurpose, wallet: WalletID?, credential: Credential) async throws(ServiceError)
        -> AuthGrant
    {
        switch credential {
        case .unencrypted: try world.authorize(purpose, wallet: wallet, passphrase: nil)
        case .passphrase(let secret): try world.authorize(purpose, wallet: wallet, passphrase: DemoSecret.text(secret))
        // The demo vault has no quick-unlock slot, as an engine vault before enrolment.
        case .quickUnlock: throw ServiceError(code: .vaultQuickUnlockUnavailable)
        }
    }

    func revoke(_ grant: AuthGrant) { world.revoke(grant) }

    func unlock(passphrase: any SecretBuffer, scope: UnlockScope) async throws(ServiceError) {
        try world.unlock(passphrase: DemoSecret.text(passphrase), scope: scope)
    }

    func lock() async throws(ServiceError) { world.lock() }
}
