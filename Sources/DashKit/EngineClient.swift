import DashWalletCore
import Foundation

/// The Swift entry point to the Rust engine. One per process.
///
/// All calls are serialised through the actor; long operations run on the
/// engine's own tokio runtime, so awaiting them never blocks a Swift executor
/// thread. Events arrive on `events`.
public actor EngineClient {
    public nonisolated let events: EventBus
    public nonisolated let dataRoot: URL
    private let engine: DashWalletCore.Engine
    private var sessions: [DashNetwork: DashWalletCore.NetworkSession] = [:]

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
        engine = try mapped { try DashWalletCore.Engine(config: config, observer: observer) }
    }

    /// Opens `network` (idempotent). `options` apply only when this call opens it.
    public func open(_ network: DashNetwork, options: SessionOptions = SessionOptions()) async throws(DashKitError) {
        if sessions[network] != nil { return }
        let engine = engine
        let session = try await mapped { try await engine.openNetwork(network: network.ffi, options: options.ffi) }
        sessions[network] = session
    }

    /// Closes `network`; returns `false` if it was not open.
    @discardableResult
    public func close(_ network: DashNetwork) async throws(DashKitError) -> Bool {
        sessions[network] = nil
        let engine = engine
        return try await mapped { try await engine.closeNetwork(network: network.ffi) }
    }

    /// Closes every network and ends all event streams.
    public func shutdown() async throws(DashKitError) {
        sessions.removeAll()
        let engine = engine
        defer { events.finish() }
        try await mapped { try await engine.shutdown() }
    }

    public func isOpen(_ network: DashNetwork) -> Bool {
        sessions[network]?.isOpen() ?? false
    }

    public nonisolated func directory(for network: DashNetwork) -> URL {
        URL(fileURLWithPath: engine.networkDir(network: network.ffi), isDirectory: true)
    }

    // MARK: Wallets

    /// Creates a wallet from a fresh 12- or 24-word mnemonic.
    public func createWallet(on network: DashNetwork, wordCount: UInt8 = 12) async throws(DashKitError) -> CreatedWallet {
        let session = try session(network)
        let created = try await mapped { try await session.createWallet(wordCount: wordCount) }
        return CreatedWallet(walletID: WalletID(engine: created.walletId), mnemonic: SecretBytes(utf8: created.mnemonic))
    }

    /// Restores a wallet. `birthHeight` 0 scans from genesis; `nil` lets the
    /// engine choose (SPV tip or latest checkpoint). A non-empty
    /// `bip39Passphrase` fails with `notImplemented` until the vault lands.
    public func importWallet(
        on network: DashNetwork,
        mnemonic: SecretBytes,
        bip39Passphrase: SecretBytes = SecretBytes([]),
        birthHeight: UInt32? = nil
    ) async throws(DashKitError) -> WalletID {
        let session = try session(network)
        // The binding takes `Data`; the copies are zeroed once the call returns.
        var phrase = mnemonic.withUnsafeBytes { Data($0) }
        var passphrase = bip39Passphrase.withUnsafeBytes { Data($0) }
        defer {
            phrase.resetBytes(in: 0..<phrase.count)
            passphrase.resetBytes(in: 0..<passphrase.count)
        }
        let options = DashWalletCore.ImportOptions(name: nil, birthHeight: birthHeight, coreCompat: false, lookahead: nil)
        let id = try await mapped {
            try await session.importWallet(mnemonic: phrase, bip39Passphrase: passphrase, options: options)
        }
        return WalletID(engine: id)
    }

    public func wallets(on network: DashNetwork) throws(DashKitError) -> [WalletSummary] {
        let session = try session(network)
        let rows = try mapped { try session.listWallets() }
        var result: [WalletSummary] = []
        for row in rows {
            result.append(WalletSummary(walletID: WalletID(engine: row.walletId), balances: try WalletBalances(row.balances)))
        }
        return result
    }

    public func balances(on network: DashNetwork, wallet: WalletID) throws(DashKitError) -> WalletBalances {
        let session = try session(network)
        let raw = try mapped { try session.balances(walletId: wallet.hex) }
        return try WalletBalances(raw)
    }

    // MARK: SPV

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

    private func session(_ network: DashNetwork) throws(DashKitError) -> DashWalletCore.NetworkSession {
        guard let session = sessions[network] else { throw .networkNotOpen(detail: network.description) }
        return session
    }
}
