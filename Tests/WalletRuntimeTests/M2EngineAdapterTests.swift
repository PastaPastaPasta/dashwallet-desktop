// The R1/R2 engine adapters of `DesktopRuntimeServices` over `FakeEngine`:
// wallet open/close on the lifecycle queue (QT-101), dust protection
// (QT-075), the console's authorization round trip (QT-145), the chain-data
// reset sequence (QT-148) and calls the fake leaves unimplemented.
import DashKit
import Foundation
import Testing
@testable import WalletRuntime

@MainActor
private func desktop(_ h: Harness) -> DesktopRuntimeServices {
    DesktopRuntimeServices(runtime: h.services, platform: .fake(), clock: h.clock, onQuit: {})
}

private final class Line: WalletRuntime.SecretBuffer, @unchecked Sendable {
    let bytes: [UInt8]
    init(_ text: String) { bytes = Array(text.utf8) }
    var count: Int { bytes.count }
    func withUnsafeBytes<R>(_ body: (UnsafeRawBufferPointer) throws -> R) rethrows -> R {
        try bytes.withUnsafeBytes(body)
    }
}

@MainActor
@Suite struct M2EngineAdapterTests {
    @Test func QT101_closeAndOpenRunOnTheQueueAndReloadTheWalletList() async throws {
        let h = Harness {
            $0.with {
                $0.walletInfos = .success([Fixtures.info(Fixtures.walletA, name: "A", confirmed: 5)])
                $0.loadStates = .success([
                    DashKit.WalletLoadState(
                        walletID: Fixtures.walletA, name: "A", loaded: true, loadOnStartup: true, watchOnly: false)
                ])
            }
        }
        try await h.start()
        let lifecycle = desktop(h).walletLifecycle
        let walletA = WalletRuntime.WalletID(Fixtures.walletA)

        try await lifecycle.unload(walletA)
        #expect(try await lifecycle.loadStates().map(\.loaded) == [false])
        try await lifecycle.load(walletA)
        #expect(try await lifecycle.loadStates().map(\.loaded) == [true])
        let calls = h.engine.calls
        let unload = try #require(calls.firstIndex(of: "unloadWallet \(Fixtures.walletA)"))
        // The wallet list is read again after the close, as after an import.
        #expect(calls[unload...].contains("walletInfos"))

        let unknown = WalletRuntime.WalletID(Fixtures.walletB)
        await #expect(throws: ServiceError.self) { try await lifecycle.load(unknown) }
    }

    @Test func QT075_dustProtectionKeepsTheEngineRange() async throws {
        let h = Harness { $0.with { $0.dustThreshold = .success(nil) } }
        try await h.start()
        let dust = desktop(h).dustProtection
        #expect(try await dust.threshold() == nil)
        try await dust.setThreshold(WalletRuntime.Amount(duffs: 5_460))
        #expect(try await dust.threshold() == WalletRuntime.Amount(duffs: 5_460))
        await #expect(throws: ServiceError.self) { try await dust.setThreshold(WalletRuntime.Amount(duffs: 0)) }
        try await dust.setThreshold(nil)
        #expect(try await dust.threshold() == nil)
    }

    @Test func QT145_consoleAuthorizationRequiredBecomesAResultAndTheGrantIsPassed() async throws {
        let h = Harness {
            $0.with { $0.console = .success(.authorizationRequired(.signMessage, wallet: Fixtures.walletA)) }
        }
        try await h.start()
        let console = desktop(h).console
        let first = try await console.execute(Line("signmessage yXa hi"), wallet: nil, grant: nil)
        #expect(first == .authorizationRequired(.signMessage, wallet: WalletRuntime.WalletID(Fixtures.walletA)))

        h.engine.with { $0.console = .success(.output(text: "\"sig\"", isJSON: true)) }
        let grant = WalletRuntime.AuthGrant(
            id: "g7", purpose: .signMessage, expiresAt: Date().addingTimeInterval(60), singleUse: true)
        let second = try await console.execute(
            Line("signmessage yXa hi"), wallet: WalletRuntime.WalletID(Fixtures.walletA), grant: grant)
        #expect(second == .output(text: "\"sig\"", isJSON: true))
        #expect(h.engine.with { $0.consoleGrants } == [nil, "g7"])
    }

    @Test func QT148_resetChainDataStopsSPVFirstAndStartsItAgain() async throws {
        let h = Harness { $0.with { $0.walletInfos = .success([]) } }
        try await h.start()
        let before = h.engine.calls.count
        try await desktop(h).repair.resetChainData()
        let calls = Array(h.engine.calls[before...])
        let stop = try #require(calls.firstIndex(of: "stopSPV regtest"))
        let reset = try #require(calls.firstIndex(of: "resetChainData"))
        let start = try #require(calls.lastIndex(of: "startSPV regtest"))
        #expect(stop < reset && reset < start)
    }

    @Test func unimplementedEngineCallsSurfaceAsNotImplemented() async throws {
        let h = Harness { $0.with { $0.walletInfos = .success([]) } }
        try await h.start()
        let services = desktop(h)
        await #expect(throws: ServiceError.self) { _ = try await services.nodeInformation.information() }
        do {
            _ = try await services.peerModeration.bannedPeers()
            Issue.record("expected not_implemented")
        } catch {
            #expect(error.code == .notImplemented)
        }
    }

    @Test func callsWithoutAnOpenNetworkFailWithNetworkNotOpen() async {
        let h = Harness()
        let services = desktop(h)
        do {
            _ = try await services.fees.feePolicy()
            Issue.record("expected network_not_open")
        } catch {
            #expect(error.code == .networkNotOpen)
        }
    }
}
