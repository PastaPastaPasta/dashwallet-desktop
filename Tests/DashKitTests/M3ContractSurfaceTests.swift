@testable import DashKit
import DashWalletCore
import Foundation
import Testing

/// The M3 contract surface (docs/contracts/m3-engine.md) through the
/// generated bindings: the constant calls answer, the stubs fail with their
/// domain's typed `NotImplemented` after the argument and session checks,
/// and DashKit maps the M3 errors and events.
@Suite struct M3ContractSurfaceTests {
    final class NullObserver: DashWalletCore.EngineObserver, @unchecked Sendable {
        func onEvent(event: DashWalletCore.EngineEvent) {}
    }

    @Test func constantsAnswerThroughTheBindings() {
        let limits = coinjoinLimits()
        #expect(limits.minMixingBalance == 140_001)
        #expect(limits.denominations == [1_000_010_000, 100_001_000, 10_000_100, 1_000_010, 100_001])
        #expect(limits.defaults.rounds == 4 && limits.defaults.enabled)
        let gov = governanceParams(network: .mainnet)
        #expect(gov.superblockCycle == 16_616 && gov.maturityWindow == 1_662 && gov.proposalFee == 100_000_000)
        let mn = masternodeNetworkDefaults(network: .regtest)
        #expect(mn.coreP2pPort == 19_899 && mn.maxShares == 8)
    }

    @Test func m3StubsFailTypedThroughTheBindings() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("m3-contract-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let engine = try Engine(config: EngineConfig(dataRoot: dir.path, workerThreads: 2), observer: NullObserver())
        let session = try await engine.openNetwork(
            network: .regtest,
            options: SessionOptions(dapiAddresses: ["http://127.0.0.1:1"], quorumUrl: "http://127.0.0.1:1", spvPeers: []))
        let wallet = String(repeating: "ab", count: 32)

        #expect(throws: CoinJoinError.NotImplemented(call: "NetworkSession.coinjoin_status")) {
            try session.coinjoinStatus(walletId: wallet)
        }
        // Arguments are checked before the stub answers.
        #expect(throws: CoinJoinError.self) { try session.coinjoinStatus(walletId: "zz") }
        await #expect(throws: GovernanceError.NotImplemented(call: "NetworkSession.governance_info")) {
            try await session.governanceInfo()
        }
        await #expect(throws: MasternodeError.SharedEnvelopeTooLarge(sizeBytes: 2 * 1024 * 1024 + 1)) {
            try await session.importSharedMessage(walletId: wallet, text: String(repeating: "x", count: 2 * 1024 * 1024 + 1))
        }
        try await engine.shutdown()
    }

    @Test func m3ErrorsMapToEngineCodes() {
        let funds = DashKitError.from(DashWalletCore.CoinJoinError.InsufficientFunds(minDuffs: 140_001))
        #expect(funds.code == "coinjoin.insufficient_funds")
        #expect(funds.parameters == ["min_duffs": 140_001])
        let field = DashKitError.from(DashWalletCore.GovernanceError.InvalidProposal(field: .url))
        #expect(field.code == "governance.invalid_proposal" && field.parameters == ["field": 1])
        let key = DashKitError.from(DashWalletCore.MasternodeError.KeyNotInWallet(role: .operator))
        #expect(key.code == "masternode.key_not_in_wallet" && key.parameters == ["role": 2])
        #expect(DashKitError.from(DashWalletCore.MasternodeError.NotImplemented(call: "x")).code == "not_implemented")
    }

    @Test func m3EventsAreReQuerySignals() {
        let wallet = String(repeating: "ab", count: 32)
        let coinJoin = DashKit.EngineEvent(.coinJoin(network: .testnet, walletId: wallet))
        #expect(coinJoin == .coinJoinChanged(.testnet, WalletID(engine: wallet)))
        #expect(!coinJoin.isLifecycle && coinJoin.network == .testnet)
        #expect(!DashKit.EngineEvent(.governance(network: .mainnet)).isLifecycle)
        #expect(DashKit.EngineEvent(.masternodes(network: .regtest)).network == .regtest)
    }
}
