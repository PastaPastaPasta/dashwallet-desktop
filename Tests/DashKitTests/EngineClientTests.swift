import DashKit
import Foundation
import Testing

/// End-to-end through the real Rust static library: EngineClient → UniFFI →
/// dw-engine → platform-wallet + SqlitePersister. Offline: regtest with
/// loopback endpoints nothing listens on.
@Suite(.serialized) struct EngineClientTests {
    static let regtestOptions = SessionOptions(
        dapiAddresses: ["http://127.0.0.1:1"],
        quorumURL: "http://127.0.0.1:1",
        spvPeers: ["127.0.0.1:1"])

    static let abandon12 =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"

    /// A fresh directory under the system temp dir, removed when the test ends.
    final class TempDir {
        let url: URL
        init() throws {
            url = FileManager.default.temporaryDirectory
                .appendingPathComponent("dashkit-tests-\(UUID().uuidString)", isDirectory: true)
            try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        }
        deinit { try? FileManager.default.removeItem(at: url) }
    }

    @Test func coreVersionComesFromRust() {
        #expect(EngineClient.coreVersion.hasPrefix("dashwallet_core "))
    }

    @Test func createWalletDeliversEventsAndPersistsAcrossRestart() async throws {
        let dir = try TempDir()
        let client = try EngineClient(dataRoot: dir.url, workerThreads: 2)
        let stream = client.events.subscribe()
        let collector = Task {
            var seen: [EngineEvent] = []
            for await e in stream { seen.append(e) }
            return seen
        }

        try await client.open(.regtest, options: Self.regtestOptions)
        #expect(await client.isOpen(.regtest))
        let created = try await client.createWallet(on: .regtest)
        #expect(created.mnemonic.utf8String()?.split(separator: " ").count == 12)

        let wallets = try await client.wallets(on: .regtest)
        #expect(wallets.map(\.walletID) == [created.walletID])
        #expect(wallets.first?.balances == .zero)
        #expect(try await client.balances(on: .regtest, wallet: created.walletID) == .zero)
        #expect(FileManager.default.fileExists(
            atPath: client.directory(for: .regtest).appendingPathComponent("wallet.sqlite").path))

        // shutdown() closes the session (emitting sessionClosed) and finishes the bus.
        try await client.shutdown()
        let seen = await collector.value
        #expect(seen.first == .sessionOpened(.regtest))
        #expect(seen.contains(.walletCreated(.regtest, created.walletID)))
        #expect(seen.last == .sessionClosed(.regtest))

        // A new engine on the same directory sees the wallet.
        let reopened = try EngineClient(dataRoot: dir.url, workerThreads: 2)
        try await reopened.open(.regtest, options: Self.regtestOptions)
        #expect(try await reopened.wallets(on: .regtest).map(\.walletID) == [created.walletID])
        try await reopened.shutdown()
    }

    @Test func importIsDeterministic() async throws {
        let dir = try TempDir()
        let client = try EngineClient(dataRoot: dir.url, workerThreads: 2)
        try await client.open(.regtest, options: Self.regtestOptions)
        let id = try await client.importWallet(
            on: .regtest, mnemonic: SecretBytes(utf8: Self.abandon12), birthHeight: 0)
        try await client.close(.regtest)

        let other = try TempDir()
        let second = try EngineClient(dataRoot: other.url, workerThreads: 2)
        try await second.open(.regtest, options: Self.regtestOptions)
        let again = try await second.importWallet(
            on: .regtest, mnemonic: SecretBytes(utf8: Self.abandon12), birthHeight: 0)
        #expect(again == id)
        #expect(WalletID(hex: id.hex.uppercased()) == id)
        try await second.shutdown()
        try await client.shutdown()
    }

    @Test func errorsCrossTheBoundaryTyped() async throws {
        let dir = try TempDir()
        let client = try EngineClient(dataRoot: dir.url, workerThreads: 2)

        await #expect(throws: DashKitError.networkNotOpen(detail: "regtest")) {
            try await client.createWallet(on: .regtest)
        }
        // Regtest has no default DAPI endpoints.
        do {
            try await client.open(.regtest)
            Issue.record("open without DAPI addresses should fail")
        } catch {
            #expect(error.code == "invalid_config")
        }

        try await client.open(.regtest, options: Self.regtestOptions)
        do {
            _ = try await client.createWallet(on: .regtest, wordCount: 13)
            Issue.record("13 words should be rejected")
        } catch {
            #expect(error.code == "invalid_argument")
        }
        do {
            _ = try await client.importWallet(on: .regtest, mnemonic: SecretBytes(utf8: "not a phrase"))
            Issue.record("invalid phrase should be rejected")
        } catch {
            #expect(error.code == "wallet.invalid_mnemonic")
        }
        // A second import of the same phrase is a distinct error (review M6).
        _ = try await client.importWallet(on: .regtest, mnemonic: SecretBytes(utf8: Self.abandon12), birthHeight: 0)
        do {
            _ = try await client.importWallet(on: .regtest, mnemonic: SecretBytes(utf8: Self.abandon12), birthHeight: 0)
            Issue.record("duplicate import should be rejected")
        } catch {
            #expect(error.code == "wallet.already_exists")
        }
        // A BIP39 passphrase needs the vault, which does not exist yet.
        do {
            _ = try await client.importWallet(
                on: .regtest, mnemonic: SecretBytes(utf8: Self.abandon12), bip39Passphrase: SecretBytes(utf8: "x"))
            Issue.record("passphrase import should be not implemented")
        } catch {
            #expect(error.code == "not_implemented")
        }
        do {
            _ = try await client.balances(on: .regtest, wallet: WalletID(hex: String(repeating: "0", count: 64))!)
            Issue.record("unknown wallet should be rejected")
        } catch {
            #expect(error.code == "wallet_not_found")
        }
        try await client.shutdown()
    }

    @Test func spvStartStopEmitsStateEvents() async throws {
        let dir = try TempDir()
        let client = try EngineClient(dataRoot: dir.url, workerThreads: 2)
        let stream = client.events.subscribe(network: .regtest)
        try await client.open(.regtest, options: Self.regtestOptions)
        try await client.startSPV(on: .regtest)
        #expect(try await client.isSPVRunning(on: .regtest))
        try await client.stopSPV(on: .regtest)
        #expect(try await client.isSPVRunning(on: .regtest) == false)
        try await client.shutdown()

        var states: [Bool] = []
        for await e in stream {
            if case .spvStateChanged(_, let running) = e { states.append(running) }
        }
        #expect(states == [true, false])
    }
}
