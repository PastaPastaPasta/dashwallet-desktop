// The adapters over the real Rust static library. Offline: regtest with
// loopback endpoints nothing listens on.
import Foundation
import Testing
@testable import WalletRuntime

@MainActor
@Suite(.serialized) struct RealEngineTests {
    nonisolated static let regtestOptions = NetworkOptions(
        dapiAddresses: ["http://127.0.0.1:1"], quorumURL: "http://127.0.0.1:1", spvPeers: ["127.0.0.1:1"])

    static func live(_ dir: TempDir) throws -> WalletRuntimeServices {
        try WalletRuntimeServices.live(
            dataRoot: dir.url, workerThreads: 2, networkOptions: { _ in Self.regtestOptions }, fallbackNetwork: .regtest)
    }

    /// Runs `body` on live services and always shuts the engine down after.
    static func withLive(_ body: (WalletRuntimeServices) async throws -> Void) async throws {
        let dir = TempDir()
        let services = try live(dir)
        do {
            try await body(services)
        } catch {
            try? await services.shutdown()
            throw error
        }
        try await services.shutdown()
    }

    @Test func amountsFormatThroughRustUnits() async throws {
        try await Self.withLive { services in
            let amounts = services.amounts
            // No session yet: names come from the fallback network (regtest → tDASH).
            #expect(amounts.unitName(.dash) == "tDASH")
            #expect(
                amounts.format(Amount(duffs: 123_456_789), unit: .dash, style: .withUnit(plusSign: false, separators: .never))
                    == "1.23456789 tDASH")
            let parsed = try amounts.parse("1.5", unit: .dash)
            #expect(parsed == Amount(duffs: 150_000_000))
            do throws(ServiceError) {
                _ = try amounts.parse("1.2.3", unit: .dash)
                Issue.record("unparsable text must fail")
            } catch {
                #expect(error.code == .unitsUnparsable)
            }
            // Out-of-range digit counts clamp instead of trapping (review M-8).
            let floored = amounts.format(
                Amount(duffs: 123_456_789), unit: .dash, style: .floored(plusSign: false, separators: .never, digits: 99))
            #expect(floored == "1.23456789 tDASH")
        }
    }

    @Test func urisAndQRThroughRust() async throws {
        try await Self.withLive { services in
            let uri = services.uri
            do throws(ServiceError) {
                _ = try uri.parsePaymentURI("bitcoin:abc")
                Issue.record("a bitcoin: URI must fail")
            } catch {
                #expect(error.code.rawValue.hasPrefix("uri."))
            }
            let matrix = try uri.qrMatrix(for: "dash:test")
            #expect(matrix.size > 0)
            #expect(matrix.modules.count == matrix.size * matrix.size)
            if case .invalid = uri.classifyAddress("not an address") {} else {
                Issue.record("garbage must classify as invalid")
            }
            do throws(ServiceError) {
                try services.messages.verify(address: "nope", message: "m", signature: "s")
                Issue.record("verify must fail")
            } catch {
                #expect(error.code.rawValue.hasPrefix("message."))
            }
        }
    }

    /// Start → vault → import → lock/unlock → authorize → shutdown through
    /// the composition root, as an app would drive it.
    @Test func liveServicesRunTheM1Flow() async throws {
        try await Self.withLive { services in
            try await services.launch(defaultNetwork: .regtest)
            #expect(await services.host.activeNetwork == .regtest)
            #expect(services.auth.lockState == .noVault)
            // wallet_infos may still be an engine stub; either way nothing is invented.
            let state = services.walletState
            #expect(state.wallets != nil || state.lastError?.code == .notImplemented)

            let vault = services.vault
            _ = try await vault.create(passphrase: vault.makeSecret(utf8: "runtime test passphrase"))
            #expect(services.auth.lockState == .unlocked)

            let phrase = try await vault.generateMnemonic(wordCount: 12, language: .english)
            let check = try await vault.checkMnemonic(phrase)
            #expect(check.wordCount == 12)
            #expect(check.checksum == .valid)
            let id = try await services.lifecycle.importWallet(
                mnemonic: phrase, bip39Passphrase: vault.makeSecret(utf8: ""), options: WalletImportOptions(birthHeight: 0))
            #expect(id.hex.count == 64)
            let status = try await vault.status()
            #expect(status.walletsWithSecrets.contains(id))

            // DashKit validates the page size before the engine (review M-8).
            do throws(ServiceError) {
                _ = try await services.history.page(wallet: id, query: HistoryQuery(limit: 10_000))
                Issue.record("limit 10000 must fail")
            } catch {
                #expect(error.code == .invalidArgument)
            }

            try await services.auth.lock()
            #expect(services.auth.lockState == .locked)
            do throws(ServiceError) {
                try await services.auth.unlock(passphrase: vault.makeSecret(utf8: "wrong"), scope: .full)
                Issue.record("wrong passphrase must fail")
            } catch {
                #expect(error.code == .vaultWrongPassphrase)
            }
            try await services.auth.unlock(passphrase: vault.makeSecret(utf8: "runtime test passphrase"), scope: .full)
            #expect(services.auth.lockState == .unlocked)

            let grant = try await services.auth.authorize(
                .revealSecret, credential: .passphrase(vault.makeSecret(utf8: "runtime test passphrase")))
            let revealed = try await vault.revealMnemonic(wallet: id, grant: grant)
            #expect(revealed.phrase.count == phrase.count)

            try await services.lifecycle.stop()
            #expect(await services.host.activeNetwork == nil)
            #expect(services.settings.lastNetwork == .regtest)
        }
    }
}
