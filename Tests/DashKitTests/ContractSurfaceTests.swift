import DashWalletCore
import Foundation
import Testing

/// Calls the M1 contract surface (docs/contracts/m1-engine.md) through the
/// generated bindings: pure calls, offline session calls, and typed
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

    @Test func qrMatrixEncodesAsDashQtDoes() throws {
        // testdata/qr_cases.json (qrencode -l L -8 -m 0).
        let matrix = try qrMatrix(text: "dash:XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg")
        #expect(matrix.size == 29)
        #expect(matrix.modules.count == 29 * 29)
        let firstRow = matrix.modules.prefix(29).map { $0 ? "1" : "0" }.joined()
        #expect(firstRow == "11111110111101010011001111111")
        #expect(throws: UriError.TooLongForQr) { try qrMatrix(text: String(repeating: "a", count: 256)) }
    }

    @Test func mnemonicsAreGeneratedAndCheckedAsBytes() throws {
        let phrase = try generateMnemonic(wordCount: 12, language: .english)
        let check = try checkMnemonic(phrase: phrase)
        #expect(check.wordCount == 12)
        #expect(check.language == .english)
        #expect(check.checksum == .valid)
        #expect(throws: WalletError.UnsupportedWordCount(wordCount: 13)) {
            try generateMnemonic(wordCount: 13, language: .english)
        }
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

        #expect(try session.vault().status().state == .noVault)
        await #expect(throws: VaultError.NoVault) {
            try await session.vault().unlock(passphrase: Data("pw".utf8), scope: .full)
        }
        // Biometric quick unlock lands in M2.
        await #expect(throws: VaultError.NotImplemented(call: "Vault.enroll_quick_unlock")) {
            try await session.vault().enrollQuickUnlock(grantId: "none")
        }
        // E1 calls answer offline: no SPV yet, nothing synced.
        let snapshot = try session.syncSnapshot()
        #expect(!snapshot.running && !snapshot.caughtUp)
        #expect(snapshot.phases.count == 4)
        await #expect(throws: SyncError.SpvNotRunning) {
            try await session.rescan(from: .genesis)
        }
        #expect(try session.walletInfos().isEmpty)
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
