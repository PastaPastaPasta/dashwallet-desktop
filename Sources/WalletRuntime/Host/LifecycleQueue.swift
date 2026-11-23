import DashKit
import Foundation

/// A runtime component that follows the open session (wallet list, sync
/// status, lock state). The lifecycle queue calls it at fixed points so its
/// state is loaded before SPV starts and cleared before the session closes.
@MainActor
public protocol SessionObserving: AnyObject, Sendable {
    /// The session of `network` is open; SPV has not started yet.
    func sessionDidStart(_ network: DashKit.DashNetwork) async
    /// The session of `network` is about to stop.
    func sessionWillStop(_ network: DashKit.DashNetwork) async
    /// A wallet was added or removed through the queue.
    func walletsDidChange(_ network: DashKit.DashNetwork) async
}

/// Serialises every start / stop / network switch / wallet add / remove
/// (iOS `SerialAsyncLifecycleQueue`). Each operation waits for the previous
/// one to finish, whatever its outcome, and publishes its
/// `LifecycleTransition` while it runs (IOS-018).
///
/// Start order: host session → observers (`sessionDidStart`) → SPV.
/// Stop order is the reverse: observers (`sessionWillStop`) → SPV → host.
public actor LifecycleQueue: LifecycleQueueing {
    private let host: WalletHost
    private let options: @Sendable (DashNetwork) -> NetworkOptions
    private let observers: [any SessionObserving]
    private nonisolated let transitionState = StateBroadcaster<LifecycleTransition>(.idle)
    /// Completes when the last queued operation has finished.
    private var tail: Task<Void, Never>?

    /// - Parameters:
    ///   - options: endpoints for a network, read each time it starts.
    ///   - observers: notified in order on start, in reverse order on stop.
    public init(
        host: WalletHost,
        options: @escaping @Sendable (DashNetwork) -> NetworkOptions,
        observers: [any SessionObserving] = []
    ) {
        self.host = host
        self.options = options
        self.observers = observers
    }

    public var transition: LifecycleTransition {
        transitionState.value ?? .idle
    }

    public nonisolated func transitions() -> AsyncStream<LifecycleTransition> {
        transitionState.stream()
    }

    public func start(network: DashNetwork) async throws(ServiceError) {
        try await enqueue { () async throws(ServiceError) in
            let target = network.kit
            if let current = self.host.active.network, current != target {
                self.transitionState.send(.switchingNetwork(from: DashNetwork(current), to: network))
                try await self.stopSession(current)
            } else {
                self.transitionState.send(.starting(network))
            }
            try await self.startSession(network)
        }
    }

    public func stop() async throws(ServiceError) {
        try await enqueue { () async throws(ServiceError) in
            guard let current = self.host.active.network else { return }
            self.transitionState.send(.stopping(DashNetwork(current)))
            try await self.stopSession(current)
        }
    }

    public func switchNetwork(to network: DashNetwork) async throws(ServiceError) {
        try await enqueue { () async throws(ServiceError) in
            let from = self.host.active.network
            self.transitionState.send(.switchingNetwork(from: from.map(DashNetwork.init), to: network))
            if let from, from != network.kit {
                try await self.stopSession(from)
            }
            try await self.startSession(network)
        }
    }

    public func importWallet(
        mnemonic: any SecretBuffer,
        bip39Passphrase: any SecretBuffer,
        options: WalletImportOptions
    ) async throws(ServiceError) -> WalletID {
        let phrase = secretBytes(mnemonic)
        let passphrase = secretBytes(bip39Passphrase)
        return try await enqueue { () async throws(ServiceError) in
            self.transitionState.send(.addingWallet)
            let network = try self.host.active.require()
            let id = try await serviceCall { () async throws(DashKitError) in
                try await self.host.engine.importWallet(
                    on: network, mnemonic: phrase, bip39Passphrase: passphrase, options: options.kit)
            }
            await self.notifyWalletsChanged(network)
            return WalletID(id)
        }
    }

    public func removeWallet(_ id: WalletID, grant: AuthGrant) async throws(ServiceError) {
        try await enqueue { () async throws(ServiceError) in
            self.transitionState.send(.removingWallet(id))
            guard case .wipe = grant.purpose else {
                throw ServiceError(code: .vaultGrantPurposeMismatch, detail: "removing a wallet needs a wipe grant")
            }
            let network = try self.host.active.require()
            let wallet = try id.kit
            try await serviceCall { () async throws(DashKitError) in
                try await self.host.engine.removeWallet(on: network, wallet: wallet, grantID: grant.id)
            }
            await self.notifyWalletsChanged(network)
        }
    }

    /// Runs an M2 wallet operation (open/close wallet, Dash Core imports,
    /// backup restore, chain-data reset) on the open network, after every
    /// earlier operation. `transition` is published while it runs (`nil`:
    /// no overlay); with `walletsChanged` the observers reload the wallet
    /// list afterwards, as they do after an import or removal.
    func runWalletOperation<T: Sendable>(
        transition: LifecycleTransition?,
        walletsChanged: Bool,
        _ operation: @escaping @Sendable (any EngineProtocol, DashKit.DashNetwork) async throws(ServiceError) -> T
    ) async throws(ServiceError) -> T {
        try await enqueue { () async throws(ServiceError) in
            if let transition { self.transitionState.send(transition) }
            let network = try self.host.active.require()
            let result = try await operation(self.host.engine, network)
            if walletsChanged { await self.notifyWalletsChanged(network) }
            return result
        }
    }

    /// Stops the open network (if any) and shuts the engine down, after
    /// every queued operation. Call once, when the app quits.
    public func shutdown() async throws(ServiceError) {
        try await enqueue { () async throws(ServiceError) in
            var firstError: ServiceError?
            if let current = self.host.active.network {
                self.transitionState.send(.stopping(DashNetwork(current)))
                do throws(ServiceError) { try await self.stopSession(current) } catch { firstError = error }
            }
            do throws(ServiceError) { try await self.host.shutdown() } catch { firstError = firstError ?? error }
            if let firstError { throw firstError }
        }
    }

    // MARK: Steps

    private func startSession(_ network: DashNetwork) async throws(ServiceError) {
        let target = network.kit
        try await host.start(network: network, options: options(network))
        for observer in observers {
            await observer.sessionDidStart(target)
        }
        let engine = host.engine
        let running = try await serviceCall { () async throws(DashKitError) in try await engine.isSPVRunning(on: target) }
        if !running {
            try await serviceCall { () async throws(DashKitError) in try await engine.startSPV(on: target) }
        }
    }

    /// Runs every stop step even when one fails, then throws the first error.
    private func stopSession(_ network: DashKit.DashNetwork) async throws(ServiceError) {
        var firstError: ServiceError?
        for observer in observers.reversed() {
            await observer.sessionWillStop(network)
        }
        let engine = host.engine
        do {
            try await serviceCall { () async throws(DashKitError) in
                if try await engine.isSPVRunning(on: network) {
                    try await engine.stopSPV(on: network)
                }
            }
        } catch {
            firstError = error
        }
        do { try await host.stop() } catch { firstError = firstError ?? error }
        if let firstError { throw firstError }
    }

    private func notifyWalletsChanged(_ network: DashKit.DashNetwork) async {
        for observer in observers {
            await observer.walletsDidChange(network)
        }
    }

    // MARK: Queue

    /// Runs `operation` after every earlier one, with the transition reset
    /// to `.idle` when it ends.
    private func enqueue<T: Sendable>(
        _ operation: @escaping @Sendable () async throws(ServiceError) -> T
    ) async throws(ServiceError) -> T {
        let previous = tail
        let state = transitionState
        let task = Task<Result<T, ServiceError>, Never> {
            await previous?.value
            defer { state.send(.idle) }
            do throws(ServiceError) {
                return .success(try await operation())
            } catch {
                return .failure(error)
            }
        }
        tail = Task { _ = await task.value }
        return try await task.value.get()
    }
}
