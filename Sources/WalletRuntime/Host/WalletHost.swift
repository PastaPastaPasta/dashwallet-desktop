import DashKit
import Foundation

/// The network whose session is open, readable synchronously from any
/// thread. Only `WalletHost` writes it.
public final class ActiveNetwork: @unchecked Sendable {
    // `lock` guards `current` and `last`.
    private let lock = NSLock()
    private var current: DashKit.DashNetwork?
    private var last: DashKit.DashNetwork?

    public init() {}

    /// The open network, if any.
    public var network: DashKit.DashNetwork? {
        lock.withLock { current }
    }

    /// The open network, or the one open most recently. Used where a network
    /// is needed for naming only (unit names), never for engine calls.
    public var networkOrLast: DashKit.DashNetwork? {
        lock.withLock { current ?? last }
    }

    /// The open network; `network_not_open` when none is.
    func require() throws(ServiceError) -> DashKit.DashNetwork {
        guard let network else { throw ServiceError(code: .networkNotOpen, detail: "no network is open") }
        return network
    }

    func set(_ network: DashKit.DashNetwork?) {
        lock.withLock {
            current = network
            if let network { last = network }
        }
    }
}

/// Owns the engine and the active network session (iOS `SwiftDashSDKHost`).
/// Only `LifecycleQueue` calls `start`/`stop`.
public actor WalletHost: WalletHosting {
    public nonisolated let engine: any EngineProtocol
    public nonisolated let active: ActiveNetwork

    public init(engine: any EngineProtocol, active: ActiveNetwork = ActiveNetwork()) {
        self.engine = engine
        self.active = active
    }

    public var activeNetwork: DashNetwork? {
        active.network.map(DashNetwork.init)
    }

    /// Opens `network`. Idempotent for the open network; another open network
    /// must be stopped first (the lifecycle queue does that).
    public func start(network: DashNetwork, options: NetworkOptions) async throws(ServiceError) {
        let target = network.kit
        if let current = active.network {
            guard current == target else {
                throw ServiceError(code: .invalidArgument, detail: "\(current) is open; stop it before starting \(target)")
            }
            if await engine.isOpen(target) { return }
        }
        try await serviceCall { () async throws(DashKitError) in try await engine.open(target, options: options.kit) }
        active.set(target)
    }

    /// Closes the open network. Idempotent. The network counts as closed
    /// afterwards even if the engine reports an error closing it.
    public func stop() async throws(ServiceError) {
        guard let current = active.network else { return }
        defer { active.set(nil) }
        try await serviceCall { () async throws(DashKitError) in _ = try await engine.close(current) }
    }

    public nonisolated func dataDirectory(for network: DashNetwork) -> URL {
        engine.directory(for: network.kit)
    }

    /// Closes the network and shuts the engine down. Runs on this actor, so
    /// the engine is released off the main thread (review L2).
    public func shutdown() async throws(ServiceError) {
        var firstError: ServiceError?
        do { try await stop() } catch { firstError = error }
        do {
            try await serviceCall { () async throws(DashKitError) in try await engine.shutdown() }
        } catch {
            firstError = firstError ?? error
        }
        if let firstError { throw firstError }
    }
}
