@testable import DashKit
import DashWalletCore
import Foundation
import Testing

/// The M3 contract surface (docs/contracts/m3-engine.md) through the
/// generated bindings: the constant calls answer, the calls check their
/// arguments and wallet first, and DashKit maps the M3 errors and events.
@Suite struct M3ContractSurfaceTests {
    final class NullObserver: DashWalletCore.EngineObserver, @unchecked Sendable {
        func onEvent(event: DashWalletCore.EngineEvent) {}
    }

    @Test func constantsAnswerThroughTheBindings() {
        let limits = coinjoinLimits()
        #expect(limits.minMixingBalance == 140_001)
        #expect(limits.denominations == [1_000_010_000, 100_001_000, 10_000_100, 1_000_010, 100_001])
        #expect(limits.defaults.rounds == 4 && limits.defaults.enabled)
    }

    @Test func m3CallsFailTypedThroughTheBindings() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("m3-contract-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let engine = try Engine(config: EngineConfig(dataRoot: dir.path, workerThreads: 2), observer: NullObserver())
        let session = try await engine.openNetwork(
            network: .regtest,
            options: SessionOptions(dapiAddresses: ["http://127.0.0.1:1"], quorumUrl: "http://127.0.0.1:1", spvPeers: []))
        let wallet = String(repeating: "ab", count: 32)

        // A wallet the session does not hold, and a malformed id.
        #expect(DashKitError.code(of: { try session.coinjoinStatus(walletId: wallet) }) == "wallet_not_found")
        #expect(DashKitError.code(of: { try session.coinjoinStatus(walletId: "zz") }) == "invalid_argument")
        await #expect(throws: MasternodeError.InvalidArgument(detail: "count must be at most 100")) {
            try await session.masternodeKeys(walletId: wallet, role: .owner, start: 0, count: 101)
        }
        await #expect(throws: MasternodeError.self) {
            try await session.vault().revealMasternodeKey(walletId: "zz", role: .operator, index: 0, grantId: "g")
        }
        try await engine.shutdown()
    }

    @Test func m3ErrorsMapToEngineCodes() {
        #expect(DashKitError.from(DashWalletCore.CoinJoinError.WalletNotFound(detail: "x")).code == "wallet_not_found")
        let funds = DashKitError.from(DashWalletCore.CoinJoinError.InsufficientFunds(minDuffs: 140_001))
        #expect(funds.code == "coinjoin.insufficient_funds")
        #expect(funds.parameters == ["min_duffs": 140_001])
        #expect(DashKitError.from(DashWalletCore.MasternodeError.WatchOnly).code == "masternode.watch_only")
        #expect(DashKitError.from(DashWalletCore.MasternodeError.GrantInvalid).code == "masternode.grant_invalid")
        #expect(DashKitError.from(DashWalletCore.MasternodeError.NotImplemented(call: "x")).code == "not_implemented")
    }

    @Test func m3EventsAreReQuerySignals() {
        let wallet = String(repeating: "ab", count: 32)
        let coinJoin = DashKit.EngineEvent(.coinJoin(network: .testnet, walletId: wallet))
        #expect(coinJoin == .coinJoinChanged(.testnet, WalletID(engine: wallet)))
        #expect(!coinJoin.isLifecycle && coinJoin.network == .testnet)
    }
}

extension DashKitError {
    /// The engine code a binding call fails with, or `nil` when it succeeds.
    static func code(of body: () throws -> Void) -> String? {
        do {
            try body()
            return nil
        } catch {
            return DashKitError.from(error).code
        }
    }
}
