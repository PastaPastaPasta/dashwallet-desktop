// The macOS composition root (DESIGN-opus §1.11, m1-swift.md §2.2,
// m2-swift.md §4): builds either the demo services or the live
// WalletRuntime services over the Rust engine, with the M2 services and the
// macOS OS services (PlatformServicesMac).
#if os(macOS)
import Foundation
import PlatformServices
import PlatformServicesMac
import WalletDemo
import WalletFeatures
import WalletRuntime

/// Starts and stops the services behind an `AppEnvironment`. The live
/// runtime opens the last network on `launch` and releases the engine on
/// `shutdown`; demo services need neither, so the demo has no lifecycle.
public struct RuntimeLifecycle: Sendable {
    public var launch: @MainActor @Sendable () async throws(ServiceError) -> Void
    public var shutdown: @MainActor @Sendable () async throws(ServiceError) -> Void

    public init(
        launch: @escaping @MainActor @Sendable () async throws(ServiceError) -> Void,
        shutdown: @escaping @MainActor @Sendable () async throws(ServiceError) -> Void
    ) {
        self.launch = launch
        self.shutdown = shutdown
    }
}

public enum MacAppComposition {
    /// `UserDefaults` key of the data directory the chooser picked (dash-qt
    /// keeps it in its QSettings as `strDataDir`).
    public static let dataDirectoryDefaultsKey = "DataDirectory"

    /// Builds the root model for `launch`.
    ///
    /// - Demo (`--demo`, `--demo-scenario`): in-memory fakes, nothing touches disk.
    /// - Otherwise the live runtime for `--datadir`, else the directory the
    ///   chooser stored, else `dataLocation`'s root
    ///   (`~/Library/Application Support/org.dashfoundation.DashWallet/`).
    ///   The chooser opens first on `-choosedatadir`, and on the first run
    ///   when nothing was stored and the default directory does not exist
    ///   (QT-004). When the runtime cannot be built, the window shows why
    ///   instead of a wallet.
    @MainActor
    public static func makeModel(
        launch: LaunchOptions,
        dataLocation: any DataLocating = MacDataLocation(),
        defaults: UserDefaults = .standard,
        singleInstance: MacSingleInstance = MacSingleInstance()
    ) -> MacAppModel {
        let screenCapture = MacScreenCaptureGuard()
        if let scenario = launch.demoScenario {
            let (environment, m2) = DemoEnvironment.makeWithM2(
                scenario: scenario, screenCapture: screenCapture, launchOptions: launch.runtime,
                platform: DemoPlatformServices(clipboard: MacClipboard(), dataDirectories: FileSystemDataDirectoryInspector()))
            return MacAppModel(environment: environment, m2: m2, launch: launch)
        }
        let factory: LiveServicesFactory = { (root: URL) throws(ServiceError) -> MacAppServices in
            try live(
                dataRoot: root, defaultNetwork: launch.network ?? .mainnet, networkOptions: launch.networkOptions,
                screenCapture: screenCapture, launchOptions: launch.runtime, singleInstance: singleInstance)
        }
        let defaultRoot: URL
        do {
            defaultRoot = try dataLocation.defaultDataRoot()
        } catch {
            let path = launch.dataDirectory?.path ?? MacDataLocation.bundleDirectoryName
            return MacAppModel(
                unavailableReason: "\(MacStrings.App.dataFolder): \(path)\n\(error.localizedDescription)",
                launch: launch)
        }
        let stored = defaults.string(forKey: dataDirectoryDefaultsKey).map { URL(fileURLWithPath: $0, isDirectory: true) }
        let firstRun = launch.dataDirectory == nil && stored == nil
            && !FileManager.default.fileExists(atPath: defaultRoot.path)
        if launch.runtime.chooseDataDirectory || firstRun {
            let chooser = DataDirectoryChooserViewModel(
                defaultDirectory: stored ?? defaultRoot, inspector: FileSystemDataDirectoryInspector())
            return MacAppModel(chooser: chooser, launch: launch, factory: factory)
        }
        let dataRoot = launch.dataDirectory ?? stored ?? defaultRoot
        do {
            try FileManager.default.createDirectory(at: dataRoot, withIntermediateDirectories: true)
        } catch {
            return MacAppModel(
                unavailableReason: "\(MacStrings.App.dataFolder): \(dataRoot.path)\n\(error.localizedDescription)",
                launch: launch)
        }
        do {
            let services = try factory(dataRoot)
            return MacAppModel(
                environment: services.environment, m2: services.m2, launch: launch, lifecycle: services.lifecycle)
        } catch {
            return MacAppModel(unavailableReason: unavailableText(error, dataDirectory: dataRoot), launch: launch)
        }
    }

    /// The live services: one engine for `dataRoot` (one sub-directory per
    /// network, plus `settings.json` and `global.json`), the M2 services over
    /// it and the macOS OS services. Transaction notifications and forwarded
    /// URIs start here.
    @MainActor
    public static func live(
        dataRoot: URL,
        defaultNetwork: DashNetwork,
        networkOptions: NetworkOptions,
        screenCapture: (any ScreenCaptureGuard)?,
        launchOptions: RuntimeLaunchOptions = RuntimeLaunchOptions(),
        singleInstance: MacSingleInstance = MacSingleInstance()
    ) throws(ServiceError) -> MacAppServices {
        let runtime = try WalletRuntimeServices.live(
            dataRoot: dataRoot, networkOptions: { _ in networkOptions }, fallbackNetwork: defaultNetwork)
        let environment = AppEnvironment(runtime: runtime, screenCapture: screenCapture)
        let desktop = DesktopRuntimeServices(
            runtime: runtime, platform: macOSServices(singleInstance: singleInstance),
            onQuit: { MacApplication.terminate() })
        let m2 = try M2Services.live(desktop: desktop, launchOptions: launchOptions)
        desktop.start()
        let startup = desktop.startup
        let lifecycle = RuntimeLifecycle(
            launch: { () throws(ServiceError) in
                try await startup.run(network: defaultNetwork) { () throws(ServiceError) in
                    try await runtime.launch(defaultNetwork: defaultNetwork)
                }
            },
            shutdown: { () throws(ServiceError) in try await runtime.shutdown() })
        return MacAppServices(environment: environment, m2: m2, lifecycle: lifecycle)
    }

    /// The macOS implementations of the M2 OS services (m2-swift.md §2.7).
    @MainActor
    static func macOSServices(singleInstance: MacSingleInstance) -> DesktopOSServices {
        DesktopOSServices(
            singleInstance: singleInstance, uriSchemes: MacURISchemeRegistration(), launchAtLogin: MacLaunchAtLogin(),
            notifications: MacNotifier(), biometricKeys: MacBiometricKeyStore(), idle: MacIdleMonitor(),
            clipboard: MacClipboard(), qrDecoder: MacQRImageDecoder(),
            dataDirectories: FileSystemDataDirectoryInspector(), fileRevealer: MacFileRevealer())
    }

    static func unavailableText(_ error: ServiceError, dataDirectory: URL) -> String {
        "\(MacStrings.App.runtimeFailed(error.code.rawValue))\n\(MacStrings.App.dataFolder): \(dataDirectory.path)"
    }
}
#endif
