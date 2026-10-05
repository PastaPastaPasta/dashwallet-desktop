import DashWalletCore
import Foundation
import Testing

/// Calls the M1 contract surface (docs/contracts/m1-engine.md) through the
/// generated bindings: pure calls that already work, and typed
/// `NotImplemented` errors from calls whose engine side has not landed.
@Suite struct ContractSurfaceTests {
    @Test func unitsFormatAndParseThroughTheFFI() throws {
        let text = try formatAmount(
            amount: 123_456_789_012, unit: .dash, network: .mainnet,
            style: .withUnit(plusSign: true, separators: .always))
        #expect(text == "+1\u{2009}234.56789012 DASH")
        #expect(try parseAmount(text: "1.5", unit: .dash) == 150_000_000)
        #expect(throws: UnitsError.Unparsable) { try parseAmount(text: "1.123456789", unit: .dash) }
        #expect(unitName(unit: .milliDash, network: .testnet) == "mtDASH")
    }

    @Test func paymentURIsRoundTrip() throws {
        let address = "XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg"
        let uri = try buildPaymentUri(address: address, amount: 100_000, label: "café", message: nil)
        let parsed = try parsePaymentUri(network: .mainnet, text: uri)
        #expect(parsed.address == address)
        #expect(parsed.amount == 100_000)
        #expect(parsed.label == "café")
        #expect(classifyAddress(network: .mainnet, text: address) == .core(scriptHash: false))
        #expect(throws: UriError.DoubleSlash) { try parsePaymentUri(network: .mainnet, text: "dash://\(address)") }
    }

    @Test func verifyMessageUsesDashCoreRules() throws {
        // testdata/message_cases.json (dashd 24 signmessagewithprivkey).
        let address = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n"
        let signature = "IIOzMDkvw3GtLWXkeEYRRRH53MOLHM44sJ428Nu4NNacTPJTGcKesMJ+3s3OadYK34tpSQIhu922EviNNWTsiQg="
        try verifyMessage(network: .regtest, address: address, message: "Trust no one", signature: signature)
        #expect(throws: MessageError.NotSigned) {
            try verifyMessage(network: .regtest, address: address, message: "Trust everyone", signature: signature)
        }
    }

    @Test func unimplementedCallsFailTyped() {
        #expect(throws: WalletError.NotImplemented(call: "generate_mnemonic")) {
            try generateMnemonic(wordCount: 12, language: .english)
        }
        #expect(throws: UriError.NotImplemented(call: "qr_matrix")) { try qrMatrix(text: "dash:x") }
    }

    final class NullObserver: DashWalletCore.EngineObserver, @unchecked Sendable {
        func onEvent(event: DashWalletCore.EngineEvent) {}
    }

    @Test func sessionStubsFailTyped() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("contract-surface-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let engine = try Engine(config: EngineConfig(dataRoot: dir.path, workerThreads: 2), observer: NullObserver())
        let session = try await engine.openNetwork(
            network: .regtest,
            options: SessionOptions(dapiAddresses: ["http://127.0.0.1:1"], quorumUrl: "http://127.0.0.1:1", spvPeers: []))
        let wallet = String(repeating: "ab", count: 32)

        #expect(throws: VaultError.NotImplemented(call: "Vault.status")) { try session.vault().status() }
        await #expect(throws: VaultError.NotImplemented(call: "Vault.unlock")) {
            try await session.vault().unlock(passphrase: Data("pw".utf8), scope: .full)
        }
        #expect(throws: SyncError.NotImplemented(call: "NetworkSession.sync_snapshot")) {
            try session.syncSnapshot()
        }
        #expect(throws: SendError.NotImplemented(call: "NetworkSession.new_tx_draft")) {
            try session.newTxDraft(walletId: wallet)
        }
        // Arguments are still validated before the stub answers.
        await #expect(throws: HistoryError.self) {
            try await session.txDetail(walletId: "not-hex", txid: "00")
        }
        try await engine.shutdown()
    }
}
