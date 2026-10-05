// Chooses demo or live services for the app's `@main` (the composition
// root, DESIGN-opus §1.11) and wires the macOS screen-capture guard in.
#if os(macOS)
import Foundation
import PlatformServicesMac
import WalletFeatures
import WalletRuntime

public enum MacAppComposition {
    /// Builds the root model. `live` assembles the real WalletRuntime services
    /// over the engine for the data directory; when it throws, the window
    /// shows why instead of a wallet.
    @MainActor
    public static func makeModel(
        launch: LaunchOptions,
        dataDirectory: URL = AppPaths.dataDirectory(),
        live: @MainActor (URL) throws(ServiceError) -> AppEnvironment
    ) -> MacAppModel {
        let screenCapture = MacScreenCaptureGuard()
        if let scenario = launch.demoScenario {
            return MacAppModel(
                environment: DemoEnvironment.make(scenario: scenario, screenCapture: screenCapture), launch: launch)
        }
        do {
            try FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
        } catch {
            return MacAppModel(
                unavailableReason: "\(MacStrings.App.dataFolder): \(dataDirectory.path)\n\(error.localizedDescription)",
                launch: launch)
        }
        do {
            var environment = try live(dataDirectory)
            environment.screenCapture = environment.screenCapture ?? screenCapture
            return MacAppModel(environment: environment, launch: launch)
        } catch {
            return MacAppModel(unavailableReason: unavailableText(error, dataDirectory: dataDirectory), launch: launch)
        }
    }

    static func unavailableText(_ error: ServiceError, dataDirectory: URL) -> String {
        let summary = error.code == .notImplemented
            ? "The wallet runtime (WalletRuntime adapters over the Rust engine) is not part of this build yet."
            : "The wallet runtime could not start (\(error.code.rawValue))."
        return "\(summary)\n\(MacStrings.App.dataFolder): \(dataDirectory.path)"
    }
}
#endif
