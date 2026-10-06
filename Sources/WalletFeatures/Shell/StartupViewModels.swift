// Startup and shutdown screens: the data-directory chooser (QT-004), the
// splash (QT-005), the corrupt-settings question (QT-007) and the shutdown
// window (QT-008).
import Foundation
import Observation
import PlatformServices
import WalletRuntime

/// Where startup stands, as the splash and the composition root see it.
public enum StartupStage: Sendable, Hashable {
    /// `settings.json` or `global.json` was unreadable: ask Reset / Abort first.
    case settingsUnreadable([URL])
    case starting
    case ready
    case failed(ServiceErrorCode)
    /// The user quit (Q, closing the splash, or Abort): exit without writing.
    case quitting
}

@MainActor
@Observable
public final class StartupViewModel {
    public private(set) var stage: StartupStage
    public private(set) var phase: StartupPhase
    /// 0...1, never decreasing (dash-qt).
    public private(set) var progress: Double
    public let showsSplash: Bool

    public var statusText: String {
        switch phase {
        case .loadingSettings: L10n.Shell.loadingSettings
        case .openingNetwork(let network): L10n.Shell.openingNetwork(network)
        case .loadingWallets: L10n.Shell.loadingWallets
        case .startingSync: L10n.Shell.startingSync
        case .ready: L10n.Shell.doneLoading
        case .failed(let code): ErrorText.m2(code) == L10n.Common.unexpected ? L10n.Shell.startupFailed : ErrorText.m2(code)
        }
    }

    private let startup: any StartupProgressing
    private let optionsReset: any OptionsResetting
    private var task: Task<Void, Never>?

    /// `showSplash` is `--splash`, off with `--min` (QT-006).
    public init(startup: any StartupProgressing, optionsReset: any OptionsResetting, launchOptions: LaunchOptions) {
        self.startup = startup
        self.optionsReset = optionsReset
        self.showsSplash = launchOptions.showSplash && !launchOptions.startMinimized
        self.phase = startup.phase
        self.progress = startup.progress
        let unreadable = optionsReset.recoveredFromCorruption
        self.stage = unreadable.isEmpty ? .starting : .settingsUnreadable(unreadable)
        if unreadable.isEmpty { apply(startup.phase) }
    }

    public convenience init(m2: M2Services) {
        self.init(startup: m2.startup, optionsReset: m2.optionsReset, launchOptions: m2.launchOptions)
    }

    /// Follows the startup phases until `stop()`.
    public func start() {
        stop()
        let changes = startup.changes()
        task = Task { [weak self] in
            for await phase in changes {
                guard let self else { return }
                self.apply(phase)
            }
        }
    }

    public func stop() {
        task?.cancel()
        task = nil
    }

    /// Q on the splash or closing it: emergency shutdown (dash-qt).
    public func quit() {
        guard stage != .quitting else { return }
        stage = .quitting
        startup.requestEmergencyQuit()
    }

    // MARK: Corrupt settings (QT-007)

    /// "Reset": the store already runs on defaults; the old files stay as `.bak`.
    public func resetSettings() {
        guard case .settingsUnreadable = stage else { return }
        stage = .starting
        apply(startup.phase)
    }

    /// "Abort": quit without writing anything.
    public func abortSettings() {
        guard case .settingsUnreadable = stage else { return }
        stage = .quitting
    }

    private func apply(_ phase: StartupPhase) {
        self.phase = phase
        progress = max(progress, startup.progress)
        switch stage {
        case .settingsUnreadable, .quitting:
            return
        case .starting, .ready, .failed:
            break
        }
        switch phase {
        case .ready: stage = .ready
        case .failed(let code): stage = .failed(code)
        default: stage = .starting
        }
    }
}

/// The first-run data-directory chooser (QT-004, dash-qt `Intro`).
@MainActor
@Observable
public final class DataDirectoryChooserViewModel {
    public enum Choice: Sendable, Hashable {
        case defaultDirectory, custom
    }

    public enum Outcome: Sendable, Hashable {
        case choosing
        case accepted(URL)
        /// Cancel exits the app (dash-qt).
        case cancelled
    }

    public let defaultDirectory: URL
    public private(set) var choice: Choice = .defaultDirectory
    public private(set) var customDirectory: URL?
    public private(set) var status: DataDirectoryStatus?
    public private(set) var outcome: Outcome = .choosing
    public private(set) var errorMessage: String?

    public var selectedDirectory: URL? {
        choice == .defaultDirectory ? defaultDirectory : customDirectory
    }

    public var statusText: String? {
        switch status?.state {
        case .willCreate: L10n.Shell.willCreate
        case .exists: L10n.Shell.exists
        case .notADirectory: L10n.Shell.notADirectory
        case .cannotCreate: L10n.Shell.cannotCreate
        case nil: nil
        }
    }

    /// "%n GB of space available"; `nil` when unknown.
    public var freeSpaceText: String? {
        status?.availableBytes.map { L10n.Shell.spaceAvailable($0 / 1_000_000_000) }
    }

    /// OK is enabled for a directory that exists or can be created.
    public var canAccept: Bool {
        switch status?.state {
        case .willCreate, .exists: true
        case .notADirectory, .cannotCreate, nil: false
        }
    }

    private let inspector: any DataDirectoryInspecting

    public init(defaultDirectory: URL, inspector: any DataDirectoryInspecting) {
        self.defaultDirectory = defaultDirectory
        self.inspector = inspector
    }

    public func load() async {
        await inspect()
    }

    public func choose(_ choice: Choice) async {
        self.choice = choice
        await inspect()
    }

    public func setCustomDirectory(_ url: URL) async {
        customDirectory = url
        choice = .custom
        await inspect()
    }

    /// OK: creates the directory (dash-qt also creates `wallets/`; the engine
    /// creates its own subdirectories).
    public func accept() async {
        guard let directory = selectedDirectory else { return }
        await inspect()
        guard canAccept else { return }
        do {
            if status?.state == .willCreate { try inspector.create(directory) }
            outcome = .accepted(directory)
        } catch {
            errorMessage = L10n.Shell.cannotCreateDirectory(directory.path)
        }
    }

    public func cancel() {
        outcome = .cancelled
    }

    private func inspect() async {
        errorMessage = nil
        guard let directory = selectedDirectory else {
            status = nil
            return
        }
        let result = await inspector.inspect(directory)
        // A later choice may have replaced this one while it was inspected.
        guard selectedDirectory == directory else { return }
        status = result
    }
}

/// "Dash Wallet is shutting down…" (QT-008). The window cannot be closed
/// while the shutdown runs.
@MainActor
@Observable
public final class ShutdownViewModel {
    public enum State: Sendable, Hashable {
        case idle, shuttingDown, finished
    }

    public private(set) var state: State = .idle
    public let title = L10n.Shell.shuttingDown
    public let message = L10n.Shell.doNotShutDown

    public var isVisible: Bool { state == .shuttingDown }
    public var canClose: Bool { state != .shuttingDown }

    private let coordinator: any ShutdownCoordinating

    public init(coordinator: any ShutdownCoordinating) {
        self.coordinator = coordinator
        if coordinator.isShuttingDown { state = .shuttingDown }
    }

    /// Every quit path (Exit, Cmd+Q, Dock, tray) ends here; idempotent.
    public func quit() async {
        guard state == .idle else { return }
        state = .shuttingDown
        await coordinator.shutdown()
        state = .finished
    }
}
