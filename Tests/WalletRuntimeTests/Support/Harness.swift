// Builds `WalletRuntimeServices` on a `FakeEngine` with a temp settings dir.
import DashKit
import Foundation
@testable import WalletRuntime

/// A fresh directory under the system temp dir, removed on deinit.
final class TempDir: @unchecked Sendable {
    let url: URL

    init() {
        url = FileManager.default.temporaryDirectory
            .appendingPathComponent("walletruntime-tests-\(UUID().uuidString)", isDirectory: true)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    }

    deinit { try? FileManager.default.removeItem(at: url) }
}

@MainActor
struct Harness {
    let engine: FakeEngine
    let clock: ManualClock
    let dir: TempDir
    let services: WalletRuntimeServices

    init(authorizeTimeout: Duration = .seconds(60), configure: (FakeEngine) -> Void = { _ in }) {
        let engine = FakeEngine()
        configure(engine)
        let clock = ManualClock()
        let dir = TempDir()
        self.engine = engine
        self.clock = clock
        self.dir = dir
        services = WalletRuntimeServices(
            engine: engine, settings: SettingsStore(directory: dir.url), fallbackNetwork: .regtest, clock: clock,
            authorizeTimeout: authorizeTimeout)
    }

    /// Starts regtest through the lifecycle queue.
    func start() async throws {
        try await services.lifecycle.start(network: .regtest)
    }
}
