// App shell services (m2-swift.md §2.6): startup phases (QT-005), graceful
// shutdown (QT-008), window/tray/notification settings (QT-028…030,
// QT-033, IOS-105) and the single-instance hand-off of URIs (QT-001,
// QT-019, QT-150).
import DashKit
import Foundation
import Observation
import PlatformServices

// MARK: Startup (QT-005)

/// `StartupProgressing` for the splash. `run(network:launch:)` drives the
/// phases from the engine's events while the lifecycle queue starts the
/// network: `openingNetwork` → `loadingWallets` (session open) →
/// `startingSync` (SPV running) → `ready`. A failure ends in
/// `failed(code)`. The bar never moves backwards; Q or closing the splash
/// calls `requestEmergencyQuit()`, which runs the quit action once.
@MainActor
@Observable
public final class StartupProgress: StartupProgressing {
    public private(set) var phase: StartupPhase = .loadingSettings
    public private(set) var progress: Double
    public private(set) var emergencyQuitRequested = false

    @ObservationIgnored private let broadcaster: StateBroadcaster<StartupPhase>
    @ObservationIgnored private let events: EventBus?
    @ObservationIgnored private let onEmergencyQuit: @MainActor () -> Void

    /// - Parameters:
    ///   - events: the engine's events (`EngineProtocol.events`).
    ///   - onEmergencyQuit: quits the app (through the shutdown coordinator).
    public init(events: EventBus?, onEmergencyQuit: @escaping @MainActor () -> Void) {
        self.events = events
        self.onEmergencyQuit = onEmergencyQuit
        progress = Self.fraction(.loadingSettings)
        broadcaster = StateBroadcaster(.loadingSettings)
    }

    public func changes() -> AsyncStream<StartupPhase> {
        broadcaster.stream()
    }

    public func requestEmergencyQuit() {
        guard !emergencyQuitRequested else { return }
        emergencyQuitRequested = true
        onEmergencyQuit()
    }

    /// Moves to `next` unless the splash is already past it. `failed` is
    /// always taken and ends the sequence.
    public func advance(to next: StartupPhase) {
        if case .failed = phase { return }
        if case .failed = next {
            phase = next
            broadcaster.send(next)
            return
        }
        guard Self.rank(next) > Self.rank(phase) else { return }
        phase = next
        progress = max(progress, Self.fraction(next))
        broadcaster.send(next)
    }

    /// Runs `launch` (normally `LifecycleQueue.start(network:)`) and follows
    /// it on the engine's events. Rethrows its error after `failed(code)`.
    public func run(
        network: DashNetwork, launch: @MainActor () async throws(ServiceError) -> Void
    ) async throws(ServiceError) {
        advance(to: .openingNetwork(network))
        let target = network.kit
        let follower: Task<Void, Never>? = events.map { bus in
            let stream = bus.subscribe()
            return Task { [weak self] in
                for await event in stream {
                    guard let self else { return }
                    switch event {
                    case .sessionOpened(let n) where n == target:
                        self.advance(to: .loadingWallets)
                    case .spvStateChanged(let n, running: true) where n == target:
                        self.advance(to: .startingSync)
                    default:
                        break
                    }
                }
            }
        }
        defer { follower?.cancel() }
        do {
            try await launch()
        } catch {
            advance(to: .failed(error.code))
            throw error
        }
        advance(to: .ready)
    }

    private static func rank(_ phase: StartupPhase) -> Int {
        switch phase {
        case .loadingSettings: 0
        case .openingNetwork: 1
        case .loadingWallets: 2
        case .startingSync: 3
        case .ready: 4
        case .failed: 5
        }
    }

    private static func fraction(_ phase: StartupPhase) -> Double {
        switch phase {
        case .loadingSettings: 0.05
        case .openingNetwork: 0.15
        case .loadingWallets: 0.4
        case .startingSync: 0.8
        case .ready: 1
        case .failed: 0
        }
    }
}

// MARK: Shutdown (QT-008)

/// `ShutdownCoordinating`: runs `stop` (normally
/// `WalletRuntimeServices.shutdown()`: SPV stop → session close → engine
/// release) once, then `flush`. Concurrent and later calls wait for the
/// same run. `isShuttingDown` is true while it runs; the shutdown window
/// cannot be closed meanwhile.
@MainActor
@Observable
public final class ShutdownCoordinator: ShutdownCoordinating {
    public private(set) var isShuttingDown = false
    public private(set) var finished = false
    /// The stop failure, if any; the app quits anyway.
    public private(set) var lastError: ServiceError?

    @ObservationIgnored private let stop: @MainActor () async throws(ServiceError) -> Void
    @ObservationIgnored private let flush: @MainActor () -> Void
    @ObservationIgnored private var running: Task<Void, Never>?

    public init(
        stop: @escaping @MainActor () async throws(ServiceError) -> Void,
        flush: @escaping @MainActor () -> Void = {}
    ) {
        self.stop = stop
        self.flush = flush
    }

    public func shutdown() async {
        if let running {
            await running.value
            return
        }
        isShuttingDown = true
        let task = Task { @MainActor in
            do throws(ServiceError) {
                try await self.stop()
            } catch {
                self.lastError = error
            }
            self.flush()
            self.isShuttingDown = false
            self.finished = true
        }
        running = task
        await task.value
    }
}

// MARK: Shell settings (QT-028…030, QT-033, IOS-105)

/// `ShellSettingsProviding` in `global.json` (section `shell`), shared by
/// every network as dash-qt's window settings are.
@MainActor
@Observable
public final class ShellSettingsStore: ShellSettingsProviding {
    public static let section = "shell"

    public private(set) var shell: ShellSettings

    @ObservationIgnored private let settings: SettingsStore
    @ObservationIgnored private let broadcaster: StateBroadcaster<ShellSettings>

    public init(settings: SettingsStore) {
        self.settings = settings
        let loaded = settings.globalSection(Self.section, as: ShellSettings.self) ?? ShellSettings()
        shell = loaded
        broadcaster = StateBroadcaster(loaded)
    }

    public func update(_ shell: ShellSettings) throws(ServiceError) {
        guard shell != self.shell else { return }
        try settings.setGlobalSection(Self.section, shell)
        self.shell = shell
        broadcaster.send(shell)
    }

    public func changes() -> AsyncStream<ShellSettings> {
        broadcaster.stream()
    }

    /// Re-reads the section, e.g. after `SettingsStore.resetToDefaults()`.
    public func reload() {
        let loaded = settings.globalSection(Self.section, as: ShellSettings.self) ?? ShellSettings()
        guard loaded != shell else { return }
        shell = loaded
        broadcaster.send(loaded)
    }
}

// MARK: Single instance and incoming URIs (QT-001, QT-019, QT-150)

/// Where URIs reach the app: the launch's own arguments, launches the
/// single-instance coordinator forwarded (Windows/Linux socket or pipe;
/// macOS `application(_:open:)`), drag and drop, and the Open URI dialog.
/// Each is parsed with the engine's URI parser (`URIHandling`); valid ones
/// queue in `pending` for the Send page, invalid ones in `rejected` with
/// their error. A forwarded launch without URIs only asks to show the
/// window (`activationRequests`).
@MainActor
@Observable
public final class IncomingURIRouter {
    public struct Incoming: Sendable, Hashable {
        public let text: String
        public let uri: PaymentURI
    }

    public struct Rejected: Sendable, Equatable {
        public let text: String
        public let error: ServiceError
    }

    /// `DashWallet-<network>`: one primary per network, as dash-qt.
    public nonisolated static func instanceKey(for network: DashNetwork) -> String {
        "DashWallet-\(network)"
    }

    public private(set) var pending: [Incoming] = []
    public private(set) var rejected: [Rejected] = []
    /// Bumped for every forwarded launch; the window raises itself.
    public private(set) var activationRequests = 0

    @ObservationIgnored private let uriHandler: any URIHandling
    @ObservationIgnored private let parser: any LaunchArgumentsParsing
    @ObservationIgnored private let tasks = TaskBag()

    public init(uriHandler: any URIHandling, parser: any LaunchArgumentsParsing = LaunchArgumentsParser()) {
        self.uriHandler = uriHandler
        self.parser = parser
    }

    /// Claims the single instance for `network`. `.forwarded`: another
    /// process took `arguments` and this one exits 0. `.primary`: later
    /// launches arrive through `follow(_:)`.
    public nonisolated static func claim(
        _ coordinator: any SingleInstanceCoordinating, network: DashNetwork, arguments: [String]
    ) throws(ServiceError) -> SingleInstanceRole {
        try platformCall { () throws(PlatformServiceError) in
            try coordinator.claim(key: instanceKey(for: network), arguments: arguments)
        }
    }

    /// Takes the URIs of this launch's parsed options.
    public func accept(launch options: LaunchOptions) {
        for uri in options.uris { receive(uri) }
    }

    /// Follows forwarded launches until `stop()`.
    public func follow(_ coordinator: any SingleInstanceCoordinating) {
        let stream = coordinator.forwardedArguments()
        tasks.set("forwarded", Task { [weak self] in
            for await arguments in stream {
                guard let self else { return }
                self.forwarded(arguments)
            }
        })
    }

    /// A forwarded launch's arguments (`argv` without the program).
    public func forwarded(_ arguments: [String]) {
        activationRequests += 1
        do {
            accept(launch: try parser.parse(arguments))
        } catch {
            rejected.append(Rejected(text: arguments.joined(separator: " "), error: error))
        }
    }

    /// Drag and drop or the Open URI dialog.
    public func receive(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            pending.append(Incoming(text: trimmed, uri: try uriHandler.parsePaymentURI(trimmed)))
        } catch {
            rejected.append(Rejected(text: trimmed, error: error))
        }
    }

    /// Removes and returns the oldest pending URI.
    public func takeNext() -> Incoming? {
        pending.isEmpty ? nil : pending.removeFirst()
    }

    public func clearRejected() {
        rejected.removeAll()
    }

    public func stop() {
        tasks.cancelAll()
    }
}
