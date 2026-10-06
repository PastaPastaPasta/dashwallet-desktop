@testable import DashKit
import DashWalletCore
import Foundation
import Testing

/// The M2 contract surface (docs/contracts/m2-engine.md) through the
/// generated bindings: stubs fail with their domain's typed `NotImplemented`
/// after the argument and session checks, implemented calls answer, and
/// DashKit maps every M2 error domain to the engine's code.
@Suite struct M2ContractSurfaceTests {
    final class NullObserver: DashWalletCore.EngineObserver, @unchecked Sendable {
        func onEvent(event: DashWalletCore.EngineEvent) {}
    }

    @Test func m2StubsFailTypedThroughTheBindings() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("m2-contract-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let engine = try Engine(config: EngineConfig(dataRoot: dir.path, workerThreads: 2), observer: NullObserver())
        let session = try await engine.openNetwork(
            network: .regtest,
            options: SessionOptions(dapiAddresses: ["http://127.0.0.1:1"], quorumUrl: "http://127.0.0.1:1", spvPeers: []))
        let wallet = String(repeating: "ab", count: 32)

        #expect(throws: SyncError.NotImplemented(call: "NetworkSession.node_info")) { try session.nodeInfo() }
        #expect(throws: WalletError.NotImplemented(call: "NetworkSession.wallet_load_states")) {
            try session.walletLoadStates()
        }
        await #expect(throws: TxActionError.NotImplemented(call: "NetworkSession.abandon_transaction")) {
            try await session.abandonTransaction(walletId: wallet, txid: String(repeating: "01", count: 32))
        }
        // The txid is checked before the stub answers.
        await #expect(throws: TxActionError.self) {
            try await session.resendTransaction(walletId: wallet, txid: "zz")
        }
        await #expect(throws: CompatError.NotImplemented(call: "Engine.inspect_wallet_file")) {
            try await engine.inspectWalletFile(path: "/nonexistent")
        }
        #expect(throws: ConsoleError.NotImplemented(call: "console_commands")) { try consoleCommands() }
        #expect(throws: PsbtError.TooLarge(sizeBytes: 100 * 1024 * 1024 + 1)) {
            try parsePsbt(data: Data(count: 100 * 1024 * 1024 + 1))
        }
        // S1's vault and desktop calls are implemented: defaults before a
        // vault exists, typed image errors.
        let policy = try session.vault().quickUnlockPolicy()
        #expect(!policy.enrolled && policy.spendLimitDuffs == 50_000_000 && policy.passphraseMaxAgeSecs == 604_800)
        #expect(throws: DesktopError.self) { try decodeQrCodes(image: Data("x".utf8)) }
        #expect(desktopQuickUnlockProvider() == .unavailable)
        try await engine.shutdown()
    }

    @Test func m2ErrorsMapToEngineCodes() {
        let refused = DashKitError.from(DashWalletCore.TxActionError.Refused(refusal: .inMempool))
        #expect(refused.code == "tx_action.refused")
        #expect(refused.parameters == ["refusal": 4])
        let rpc = DashKitError.from(DashWalletCore.ConsoleError.RpcError(code: -5, message: "Invalid address"))
        #expect(rpc.code == "console.rpc_error")
        #expect(rpc.detail == "Invalid address (code -5)")
        let limit = DashKitError.from(DashWalletCore.VaultError.QuickUnlockLimitExceeded(limitDuffs: 50_000_000))
        #expect(limit.code == "vault.quick_unlock_limit_exceeded")
        #expect(limit.parameters == ["limit_duffs": 50_000_000])
        #expect(DashKitError.from(DashWalletCore.DesktopError.NotImplemented(call: "decode_qr_codes")).code
            == "not_implemented")
        #expect(DashKitError.from(DashWalletCore.BackupError.VaultLocked).code == "backup.vault_locked")
        #expect(DashKitError.from(DashWalletCore.CompatError.NoHdChain).code == "compat.no_hd_chain")
        #expect(DashKitError.from(DashWalletCore.PsbtError.GrantExceeded(maxDuffs: 7)).parameters == ["max_duffs": 7])
        #expect(DashKitError.from(DashWalletCore.SyncError.RescanInProgress).code == "sync.rescan_in_progress")
        #expect(DashKitError.from(DashWalletCore.WalletError.InvalidXpub(detail: "")).code == "wallet.invalid_xpub")
    }

    @Test func m2EventsMapAndNotificationsAreNeverDropped() {
        let event = DashKit.EngineEvent(
            .newTransactions(network: .regtest, walletId: String(repeating: "ab", count: 32), txids: ["t"], catchUp: true))
        #expect(event.isLifecycle)
        #expect(event.network == .regtest)
        let load = DashKit.EngineEvent(
            .walletLoadChanged(network: .testnet, walletId: String(repeating: "cd", count: 32), loaded: false))
        #expect(load.isLifecycle)
    }
}
