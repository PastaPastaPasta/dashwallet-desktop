// The live app: WalletRuntime's adapters over the Rust engine
// (docs/contracts/m1-swift.md §2.2, DESIGN-opus §1.12) and the M2 desktop
// services over them (m2-swift.md §4), one per app run.
import CrossUI
import Foundation
import PlatformServices
import WalletFeatures
import WalletRuntime

/// One run of the engine-backed wallet: the runtime services, the M2
/// services, the environment the view models use and the window state.
@MainActor
final class LiveSession {
    let dataRoot: URL
    let runtime: WalletRuntimeServices
    let desktop: DesktopRuntimeServices
    let state: CrossAppState
    let startup: StartupViewModel
    /// The network named on the command line; `nil` opens the last one.
    let requestedNetwork: DashNetwork?

    private var shutdownStarted = false

    /// Opens the engine on `dataRoot` and builds every service. Nothing is
    /// started yet; call `launch()`.
    ///
    /// - Parameters:
    ///   - network: `--network`; `nil` reopens the last network, else mainnet.
    ///   - options: `--connect` peers and `--dapi` endpoints, used for every network.
    ///   - launch: the parsed command line (dash-qt options for the shell).
    init(dataRoot: URL, network: DashNetwork?, options: NetworkOptions, launch: AppLaunchOptions) throws(ServiceError) {
        runtime = try WalletRuntimeServices.live(
            dataRoot: dataRoot, networkOptions: { _ in options }, fallbackNetwork: network ?? .mainnet)
        if launch.shell.resetGUISettings {
            // -resetguisettings: back both settings files up and start on defaults.
            _ = try runtime.settings.resetToDefaults()
        }
        let os = AppOSServices.make(network: network ?? .mainnet)
        desktop = DesktopRuntimeServices(runtime: runtime, platform: os, onQuit: { exit(0) })
        let m2 = try M2Services.live(desktop: desktop, launchOptions: launch.shell, clipboard: os.clipboard)
        let env = AppEnvironment(runtime: runtime)
        self.dataRoot = dataRoot
        requestedNetwork = network
        state = CrossAppState(
            env: env, m2: m2, main: MainViewModel(env: env), capabilities: AppOSServices.capabilities())
        state.appUsage = AppLaunchOptions.usage
        startup = StartupViewModel(m2: m2)
        startup.start()
        desktop.start()
        state.quitApplication = { [weak self] in
            // File ▸ Exit already stopped the engine through the shutdown page.
            self?.shutdownStarted = true
            exit(0)
        }
    }

    /// Opens the requested network, or the last one (else mainnet), then
    /// the `dash:` URIs from the command line.
    func launch(uris: [String]) async throws(ServiceError) {
        if let requestedNetwork {
            try await runtime.lifecycle.start(network: requestedNetwork)
        } else {
            try await runtime.launch(defaultNetwork: .mainnet)
        }
        if let uri = uris.first { await state.main.open(uri: uri) }
    }

    /// Stops SPV, closes the session and releases the engine, waiting for it
    /// on the main thread (at most `timeout`). For quit paths that must not
    /// return before the engine is down: AppKit's `willTerminate` and the
    /// GApplication's `shutdown` signal. The main actor's jobs run on the
    /// main dispatch queue, which `RunLoop.main` drains here (the GLib main
    /// loop has already ended on Linux). Runs once; later calls return at once.
    func shutdownBeforeExit(timeout: TimeInterval = 15) {
        guard !shutdownStarted else { return }
        shutdownStarted = true
        state.main.stop()
        state.shell.stop()
        startup.stop()
        let done = Flag()
        let runtime = runtime
        Task { @MainActor in
            do throws(ServiceError) {
                try await runtime.shutdown()
                FileHandle.standardError.write(Data("dash-wallet: engine shut down\n".utf8))
            } catch {
                FileHandle.standardError.write(Data("dash-wallet: engine shutdown failed: \(error.code)\n".utf8))
            }
            done.isSet = true
        }
        // The shutdown runs on the main actor and the engine's actor; the
        // main run loop drains the main queue that the main actor uses.
        let deadline = Date().addingTimeInterval(timeout)
        while !done.isSet && Date() < deadline {
            _ = RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.05))
        }
        if !done.isSet {
            FileHandle.standardError.write(Data("dash-wallet: engine shutdown did not finish in \(Int(timeout)) s\n".utf8))
        }
    }

    @MainActor
    private final class Flag {
        var isSet = false
    }
}
