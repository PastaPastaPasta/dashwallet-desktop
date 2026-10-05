// M1 service contracts: engine hosting and the lifecycle queue (DESIGN-opus §1.12).
import Foundation

/// How the active network is reached. Empty lists mean network defaults
/// (devnet and regtest have none).
public struct NetworkOptions: Sendable, Hashable, Codable {
    public var dapiAddresses: [String]
    public var quorumURL: String?
    public var spvPeers: [String]

    public init(dapiAddresses: [String] = [], quorumURL: String? = nil, spvPeers: [String] = []) {
        self.dapiAddresses = dapiAddresses
        self.quorumURL = quorumURL
        self.spvPeers = spvPeers
    }
}

/// Owns the engine and the active network session (iOS `SwiftDashSDKHost`).
/// Only `LifecycleQueueing` calls `start`/`stop`; view models read state.
public protocol WalletHosting: AnyObject, Sendable {
    /// The network whose session is open, if any.
    var activeNetwork: DashNetwork? { get async }
    /// Opens `network` (idempotent for the same network) and loads its wallets.
    func start(network: DashNetwork, options: NetworkOptions) async throws(ServiceError)
    /// Stops SPV and closes the session. Idempotent.
    func stop() async throws(ServiceError)
    /// Data directory of `network` ("Open data folder").
    func dataDirectory(for network: DashNetwork) -> URL
}

/// A lifecycle operation in progress, for the overlay (IOS-018).
public enum LifecycleTransition: Sendable, Hashable {
    case idle
    case starting(DashNetwork)
    case stopping(DashNetwork)
    case switchingNetwork(from: DashNetwork?, to: DashNetwork)
    case addingWallet
    case removingWallet(WalletID)
}

/// Options for adding a wallet from a recovery phrase.
public struct WalletImportOptions: Sendable, Hashable {
    public var name: String?
    /// `0` scans from genesis; `nil` lets the engine choose.
    public var birthHeight: UInt32?
    /// Dash Core BIP39 quirks (QT-104).
    public var coreCompatible: Bool
    /// Restore lookahead; dash-qt restores use 1000 (QT-105).
    public var lookahead: UInt32?

    public init(name: String? = nil, birthHeight: UInt32? = nil, coreCompatible: Bool = false, lookahead: UInt32? = nil) {
        self.name = name
        self.birthHeight = birthHeight
        self.coreCompatible = coreCompatible
        self.lookahead = lookahead
    }
}

/// Serialises every start/stop/network switch/wallet add/remove (iOS
/// `SerialAsyncLifecycleQueue`). Operations queue behind each other; none
/// runs concurrently with another.
public protocol LifecycleQueueing: AnyObject, Sendable {
    var transition: LifecycleTransition { get async }
    /// Current transition followed by every change.
    func transitions() -> AsyncStream<LifecycleTransition>
    /// Host → SPV (→ later: Platform, CoinJoin), in that order.
    func start(network: DashNetwork) async throws(ServiceError)
    /// Reverse of `start`.
    func stop() async throws(ServiceError)
    /// Stops the current network and starts `network` (IOS-106, QT-002).
    func switchNetwork(to network: DashNetwork) async throws(ServiceError)
    /// Stores the phrase in the vault and registers the wallet (engine
    /// `import_wallet`). The vault must exist and be unlocked.
    func importWallet(
        mnemonic: any SecretBuffer,
        bip39Passphrase: any SecretBuffer,
        options: WalletImportOptions
    ) async throws(ServiceError) -> WalletID
    /// Unloads and deletes the wallet and its secrets. Needs a `.wipe` grant.
    func removeWallet(_ id: WalletID, grant: AuthGrant) async throws(ServiceError)
}
