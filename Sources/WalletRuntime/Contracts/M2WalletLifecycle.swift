// M2 service contracts: multiwallet load/unload, watch-only wallets, xpub
// export and the data inventory (engine multiwallet.rs; m2-swift.md §2.1).
import Foundation

/// Load state of one registered wallet (QT-101). Unloaded wallets are not in
/// `WalletStateProviding.wallets`; the "Open Wallet" menu lists them from here.
public struct WalletLoadState: Sendable, Hashable, Identifiable {
    public let walletID: WalletID
    public let name: String
    public let loaded: Bool
    public let loadOnStartup: Bool
    public let watchOnly: Bool

    public var id: WalletID { walletID }

    public init(walletID: WalletID, name: String, loaded: Bool, loadOnStartup: Bool, watchOnly: Bool) {
        self.walletID = walletID
        self.name = name
        self.loaded = loaded
        self.loadOnStartup = loadOnStartup
        self.watchOnly = watchOnly
    }
}

/// Options for a watch-only wallet from an account xpub (QT-114).
public struct WatchOnlyImportOptions: Sendable, Hashable {
    public var name: String?
    /// `nil` scans from genesis (an xpub carries no birth date).
    public var birthHeight: UInt32?
    /// Gap limit 1...1000; `nil` = 30.
    public var lookahead: UInt32?

    public init(name: String? = nil, birthHeight: UInt32? = nil, lookahead: UInt32? = nil) {
        self.name = name
        self.birthHeight = birthHeight
        self.lookahead = lookahead
    }
}

/// A BIP44 account xpub (IOS-111). Public data; shown without a grant.
public struct AccountXpub: Sendable, Hashable {
    public let account: UInt32
    public let derivationPath: String
    public let xpub: String

    public init(account: UInt32, derivationPath: String, xpub: String) {
        self.account = account
        self.derivationPath = derivationPath
        self.xpub = xpub
    }
}

/// What the data root holds for one network (IOS-009).
public struct NetworkDataInfo: Sendable, Hashable {
    public let network: DashNetwork
    public let directory: URL
    public let hasWalletState: Bool
    public let hasVault: Bool
    /// The OS secret store still holds an unencrypted vault's key.
    public let hasOSStoreKey: Bool

    public init(network: DashNetwork, directory: URL, hasWalletState: Bool, hasVault: Bool, hasOSStoreKey: Bool) {
        self.network = network
        self.directory = directory
        self.hasWalletState = hasWalletState
        self.hasVault = hasVault
        self.hasOSStoreKey = hasOSStoreKey
    }
}

/// Wallet lifecycle beyond M1's import/remove (owner S1 adapter over R1's
/// engine calls). Load and unload run on the lifecycle queue behind any
/// running start/stop/import/remove, without an overlay transition.
public protocol WalletLifecycleManaging: AnyObject, Sendable {
    /// Every network with data under the data root; opens nothing.
    func existingNetworks() async throws(ServiceError) -> [NetworkDataInfo]
    /// Registered wallets of the active network with their load state.
    func loadStates() async throws(ServiceError) -> [WalletLoadState]
    /// Load-state changes of the active network (engine `WalletLoadChanged`,
    /// plus wallet created/removed). Consumers re-query `loadStates()`.
    func loadStateChanges() -> AsyncStream<Void>
    /// dash-qt "Open Wallet". Idempotent.
    func load(_ wallet: WalletID) async throws(ServiceError)
    /// dash-qt "Close Wallet": keeps the data. Idempotent.
    func unload(_ wallet: WalletID) async throws(ServiceError)
    func setLoadOnStartup(_ wallet: WalletID, _ loadOnStartup: Bool) async throws(ServiceError)
    /// Registers a watch-only wallet; returns its id (`wallet.invalid_xpub`).
    func importWatchOnly(xpub: String, options: WatchOnlyImportOptions) async throws(ServiceError) -> WalletID
    func accountXpub(wallet: WalletID, account: UInt32) async throws(ServiceError) -> AccountXpub
}
