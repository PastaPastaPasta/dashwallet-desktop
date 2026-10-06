// S1 adapters for logs (IOS-112) and QR images (IOS-043), and the M2
// composition of the desktop services (append-only per DESIGN-opus §5.3
// rule 3: a new type instead of edits to `WalletRuntimeServices`).
import DashKit
import Foundation
import PlatformServices

/// `LogExporting` over `Engine.export_logs`: the zip holds the data root's
/// log files, the app's own log files (`appLogFiles`) and a manifest.
public final class LogExportService: LogExporting {
    private let engine: any EngineProtocol
    private let appLogFiles: @Sendable () -> [URL]

    public init(engine: any EngineProtocol, appLogFiles: @escaping @Sendable () -> [URL] = { [] }) {
        self.engine = engine
        self.appLogFiles = appLogFiles
    }

    /// `file` must not exist; it is never replaced.
    public func exportLogs(to file: URL) async throws(ServiceError) -> URL {
        let engine = engine
        let extras = appLogFiles()
        return try await serviceCall { () async throws(DashKitError) in
            try await engine.exportLogs(to: file, extraFiles: extras)
        }.file
    }
}

/// QR codes from an image file or the clipboard (IOS-043; camera is M6),
/// as text for `URIHandling.parsePaymentURI`.
public struct QRImageImport: Sendable {
    /// Image files larger than this are refused before reading.
    public static let maximumFileBytes = 32 * 1024 * 1024

    private let decoder: any QRImageDecoding
    private let clipboard: (any ClipboardProviding)?

    public init(decoder: any QRImageDecoding, clipboard: (any ClipboardProviding)?) {
        self.decoder = decoder
        self.clipboard = clipboard
    }

    public func decode(file: URL) throws(ServiceError) -> [String] {
        let data: Data
        do {
            let size = try file.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            guard size <= Self.maximumFileBytes else {
                throw ServiceError(code: .desktopImageUnreadable, detail: "\(size) bytes")
            }
            data = try Data(contentsOf: file)
        } catch let error as ServiceError {
            throw error
        } catch {
            throw ServiceError(code: .desktopImageUnreadable, detail: "\(file.lastPathComponent): \(error)")
        }
        let decoder = decoder
        return try platformCall { () throws(PlatformServiceError) in try decoder.decode(imageData: data) }
    }

    /// `desktop.unsupported` without a clipboard service;
    /// `desktop.image_unreadable` when the clipboard holds no image.
    public func decodeClipboard() throws(ServiceError) -> [String] {
        guard let clipboard else {
            throw ServiceError(code: .desktopUnsupported, detail: "clipboard images")
        }
        guard let data = clipboard.imageData() else {
            throw ServiceError(code: .desktopImageUnreadable, detail: "the clipboard holds no image")
        }
        let decoder = decoder
        return try platformCall { () throws(PlatformServiceError) in try decoder.decode(imageData: data) }
    }
}

/// The OS services one app run uses, built by each app's `@main` for its
/// OS (`PlatformServicesMac` or `PlatformServicesDesktop`). `nil` where the
/// OS has none.
public struct DesktopOSServices: Sendable {
    public var singleInstance: any SingleInstanceCoordinating
    public var uriSchemes: any URISchemeRegistering
    public var launchAtLogin: any LaunchAtLoginManaging
    public var notifications: any SystemNotifying
    public var biometricKeys: any BiometricKeyStoring
    public var idle: (any IdleMonitoring)?
    public var clipboard: (any ClipboardProviding)?
    public var qrDecoder: any QRImageDecoding
    public var dataDirectories: any DataDirectoryInspecting
    public var fileRevealer: any FileRevealing

    public init(
        singleInstance: any SingleInstanceCoordinating, uriSchemes: any URISchemeRegistering,
        launchAtLogin: any LaunchAtLoginManaging, notifications: any SystemNotifying,
        biometricKeys: any BiometricKeyStoring, idle: (any IdleMonitoring)?, clipboard: (any ClipboardProviding)?,
        qrDecoder: any QRImageDecoding, dataDirectories: any DataDirectoryInspecting, fileRevealer: any FileRevealing
    ) {
        self.singleInstance = singleInstance
        self.uriSchemes = uriSchemes
        self.launchAtLogin = launchAtLogin
        self.notifications = notifications
        self.biometricKeys = biometricKeys
        self.idle = idle
        self.clipboard = clipboard
        self.qrDecoder = qrDecoder
        self.dataDirectories = dataDirectories
        self.fileRevealer = fileRevealer
    }
}

/// The M2 services over one `WalletRuntimeServices` and the OS services:
/// quick unlock and vault recovery, auto-lock, transaction notifications,
/// startup and shutdown, shell settings, incoming URIs, log export, QR
/// images, and the engine adapters for wallet lifecycle, transaction
/// actions, fees, dust, Dash Core imports/exports, backups, PSBT and the
/// Tools window. No singletons: each app builds one.
@MainActor
public final class DesktopRuntimeServices {
    public let runtime: WalletRuntimeServices
    public let platform: DesktopOSServices
    public let launchArguments: LaunchArgumentsParser
    public let quickUnlock: QuickUnlockService
    public let vaultRecovery: VaultRecoveryService
    public let autoLock: AutoLockController
    public let shellSettings: ShellSettingsStore
    public let transactionFeed: TransactionNotificationFeed
    public let notifications: NotificationPresenter
    public let startup: StartupProgress
    public let shutdown: ShutdownCoordinator
    public let incomingURIs: IncomingURIRouter
    public let logs: LogExportService
    public let qrImages: QRImageImport
    // The R1/R2 engine adapters (EngineToolAdapters.swift).
    public let walletLifecycle: WalletLifecycleService
    public let transactionActions: TransactionActionService
    public let fees: FeeService
    public let dustProtection: DustProtectionService
    public let fileImporter: WalletFileImportService
    public let coreExporter: CoreExportService
    public let backups: BackupService
    public let psbt: PSBTService
    public let nodeInformation: NodeInformationService
    public let peerModeration: PeerModerationService
    public let repair: RepairService
    public let console: ConsoleService

    /// - Parameters:
    ///   - onQuit: terminates the process once `shutdown` finished (or on
    ///     the splash's emergency quit, after it).
    ///   - appLogFiles: the Swift log files for the log export.
    ///   - notificationText: localized notification texts.
    public init(
        runtime: WalletRuntimeServices, platform: DesktopOSServices, clock: any RuntimeClock = SystemClock(),
        appLogFiles: @escaping @Sendable () -> [URL] = { [] },
        notificationText: TransactionNotificationText = TransactionNotificationText(),
        onQuit: @escaping @MainActor () -> Void
    ) {
        self.runtime = runtime
        self.platform = platform
        launchArguments = LaunchArgumentsParser()
        let engine = runtime.host.engine
        // The M2 adapters make session calls only; the fallback network
        // (for pure functions) is never used by them.
        let context = EngineContext(engine: engine, active: runtime.host.active, fallbackNetwork: .mainnet)
        let auth = runtime.auth
        let report: @Sendable (VaultStatus) async -> Void = { [weak auth] status in await auth?.apply(status) }
        quickUnlock = QuickUnlockService(context: context, keyStore: platform.biometricKeys, statusObserver: report)
        vaultRecovery = VaultRecoveryService(context: context, quickUnlock: quickUnlock, statusObserver: report)
        autoLock = AutoLockController(
            settings: runtime.settings, idle: platform.idle, clock: clock,
            lock: { [weak auth] () async throws(ServiceError) in try await auth?.lock() })
        shellSettings = ShellSettingsStore(settings: runtime.settings)
        transactionFeed = TransactionNotificationFeed(context: context)
        let walletState = runtime.walletState
        let settings = runtime.settings
        notifications = NotificationPresenter(
            notifier: platform.notifications, shell: shellSettings, formatter: runtime.amounts,
            unit: { [weak settings] in settings?.display.unit ?? .dash },
            walletNames: { [weak walletState] in
                Dictionary((walletState?.wallets ?? []).map { ($0.id, $0.name) }, uniquingKeysWith: { a, _ in a })
            },
            text: notificationText)
        let shutdown = ShutdownCoordinator(stop: { [weak runtime] () async throws(ServiceError) in
            try await runtime?.shutdown()
        })
        self.shutdown = shutdown
        startup = StartupProgress(events: engine.events, onEmergencyQuit: {
            Task { @MainActor in
                await shutdown.shutdown()
                onQuit()
            }
        })
        incomingURIs = IncomingURIRouter(uriHandler: runtime.uri)
        logs = LogExportService(engine: engine, appLogFiles: appLogFiles)
        qrImages = QRImageImport(decoder: platform.qrDecoder, clipboard: platform.clipboard)
        let queue = runtime.lifecycle
        walletLifecycle = WalletLifecycleService(context: context, queue: queue)
        transactionActions = TransactionActionService(context: context)
        fees = FeeService(context: context)
        dustProtection = DustProtectionService(context: context)
        fileImporter = WalletFileImportService(context: context, queue: queue)
        coreExporter = CoreExportService(context: context)
        backups = BackupService(context: context, queue: queue)
        psbt = PSBTService(context: context)
        nodeInformation = NodeInformationService(context: context)
        peerModeration = PeerModerationService(context: context)
        repair = RepairService(context: context, queue: queue)
        console = ConsoleService(context: context)
    }

    /// Starts the background parts: notifications and forwarded launches.
    public func start() {
        notifications.start(transactionFeed)
        incomingURIs.follow(platform.singleInstance)
    }
}
