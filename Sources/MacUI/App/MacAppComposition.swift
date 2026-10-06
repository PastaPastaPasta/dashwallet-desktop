// The macOS composition root (DESIGN-opus §1.11, m1-swift.md §2.2): builds
// either the demo services or the live WalletRuntime services over the Rust
// engine, and wires the macOS screen-capture guard in.
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
    /// Builds the root model for `launch`.
    ///
    /// - Demo (`--demo`, `--demo-scenario`): in-memory fakes, nothing touches disk.
    /// - Otherwise the live runtime for `--datadir`, else `dataLocation`'s root
    ///   (`~/Library/Application Support/org.dashfoundation.DashWallet/`).
    ///   When it cannot be built, the window shows why instead of a wallet.
    @MainActor
    public static func makeModel(
        launch: LaunchOptions,
        dataLocation: any DataLocating = MacDataLocation()
    ) -> MacAppModel {
        let screenCapture = MacScreenCaptureGuard()
        if let scenario = launch.demoScenario {
            return MacAppModel(
                environment: DemoEnvironment.make(scenario: scenario, screenCapture: screenCapture), launch: launch)
        }
        let dataRoot: URL
        do {
            dataRoot = try launch.dataDirectory ?? dataLocation.defaultDataRoot()
            try FileManager.default.createDirectory(at: dataRoot, withIntermediateDirectories: true)
        } catch {
            let path = launch.dataDirectory?.path ?? MacDataLocation.bundleDirectoryName
            return MacAppModel(
                unavailableReason: "\(MacStrings.App.dataFolder): \(path)\n\(error.localizedDescription)",
                launch: launch)
        }
        do {
            let (environment, lifecycle) = try live(
                dataRoot: dataRoot, defaultNetwork: launch.network ?? .mainnet,
                networkOptions: launch.networkOptions, screenCapture: screenCapture)
            return MacAppModel(environment: environment, launch: launch, lifecycle: lifecycle)
        } catch {
            return MacAppModel(unavailableReason: unavailableText(error, dataDirectory: dataRoot), launch: launch)
        }
    }

    /// The live services: one engine for `dataRoot` (one sub-directory per
    /// network, plus `settings.json` and `global.json`).
    @MainActor
    public static func live(
        dataRoot: URL,
        defaultNetwork: DashNetwork,
        networkOptions: NetworkOptions,
        screenCapture: (any ScreenCaptureGuard)?
    ) throws(ServiceError) -> (AppEnvironment, RuntimeLifecycle) {
        let runtime = try WalletRuntimeServices.live(
            dataRoot: dataRoot, networkOptions: { _ in networkOptions }, fallbackNetwork: defaultNetwork)
        let environment = AppEnvironment(runtime: runtime, screenCapture: screenCapture)
        let lifecycle = RuntimeLifecycle(
            launch: { () throws(ServiceError) in try await runtime.launch(defaultNetwork: defaultNetwork) },
            shutdown: { () throws(ServiceError) in try await runtime.shutdown() })
        return (environment, lifecycle)
    }

    static func unavailableText(_ error: ServiceError, dataDirectory: URL) -> String {
        "\(MacStrings.App.runtimeFailed(error.code.rawValue))\n\(MacStrings.App.dataFolder): \(dataDirectory.path)"
    }
}
#endif
