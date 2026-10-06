// The live app: WalletRuntime's adapters over the Rust engine
// (docs/contracts/m1-swift.md §2.2, DESIGN-opus §1.12), one per app run.
import CrossUI
import Foundation
import WalletFeatures
import WalletRuntime

/// One run of the engine-backed wallet: the runtime services, the
/// environment the view models use and the window state built on it.
@MainActor
final class LiveSession {
    let dataRoot: URL
    let runtime: WalletRuntimeServices
    let state: CrossAppState
    /// The network named on the command line; `nil` opens the last one.
    let requestedNetwork: DashNetwork?

    private var shutdownStarted = false

    /// Opens the engine on `dataRoot` and builds every service. Nothing is
    /// started yet; call `launch()`.
    ///
    /// - Parameters:
    ///   - network: `--network`; `nil` reopens the last network, else mainnet.
    ///   - options: `--connect` peers and `--dapi` endpoints, used for every network.
    init(dataRoot: URL, network: DashNetwork?, options: NetworkOptions) throws(ServiceError) {
        runtime = try WalletRuntimeServices.live(
            dataRoot: dataRoot, networkOptions: { _ in options }, fallbackNetwork: network ?? .mainnet)
        let env = AppEnvironment(runtime: runtime)
        self.dataRoot = dataRoot
        requestedNetwork = network
        state = CrossAppState(env: env, main: MainViewModel(env: env))
    }

    /// Opens the requested network, or the last one (else mainnet).
    func launch() async throws(ServiceError) {
        if let requestedNetwork {
            try await runtime.lifecycle.start(network: requestedNetwork)
        } else {
            try await runtime.launch(defaultNetwork: .mainnet)
        }
    }

    /// Stops SPV, closes the session and releases the engine, waiting for it
    /// on the main thread (at most `timeout`). For quit paths that must not
    /// return before the engine is down: AppKit's `willTerminate` and the
    /// GTK window's `destroy`. Runs once; later calls return at once.
    func shutdownBeforeExit(timeout: TimeInterval = 15) {
        guard !shutdownStarted else { return }
        shutdownStarted = true
        state.main.stop()
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
