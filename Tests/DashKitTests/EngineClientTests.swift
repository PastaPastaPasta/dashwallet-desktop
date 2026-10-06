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

    static let vaultPassphrase = "dashkit test passphrase"

    /// Opens regtest on `client` and creates an encrypted vault (no OS
    /// keyring access in tests).
    static func openWithVault(_ client: EngineClient) async throws {
        try await client.open(.regtest, options: regtestOptions)
        _ = try await client.createVault(on: .regtest, passphrase: SecretBytes(utf8: vaultPassphrase))
    }

    /// Review H-1/H-2: a new wallet's phrase goes into the vault before the
    /// wallet is registered, and after a restart the vault still holds it.
    @Test func newWalletKeepsItsKeysAcrossRestart() async throws {
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
        let phrase = try client.generateMnemonic(wordCount: 12, language: .english)
        #expect(phrase.utf8String()?.split(separator: " ").count == 12)
        // No vault yet: nothing is registered.
        do {
            _ = try await client.importWallet(on: .regtest, mnemonic: phrase)
            Issue.record("import without a vault should fail")
        } catch {
            #expect(error.code == "wallet.no_vault")
        }
        #expect(try await client.walletInfos(on: .regtest).isEmpty)

        let status = try await client.createVault(on: .regtest, passphrase: SecretBytes(utf8: Self.vaultPassphrase))
        #expect(status.state == .unlocked)
        let walletID = try await client.importWallet(on: .regtest, mnemonic: phrase, birthHeight: 0)

        let wallets = try await client.walletInfos(on: .regtest)
        #expect(wallets.map(\.walletID) == [walletID])
        #expect(wallets.first?.name == "Wallet 1")
        #expect(wallets.first?.hasMnemonic == true)
        // Not scanned yet: unknown, not zero (review M-3).
        #expect(wallets.first?.balances == nil)
        #expect(try await client.balances(on: .regtest, wallet: walletID) == nil)
        #expect(try await client.vaultStatus(on: .regtest).walletsWithSecrets == [walletID])
        #expect(FileManager.default.fileExists(
            atPath: client.directory(for: .regtest).appendingPathComponent("wallet.sqlite").path))

        // shutdown() closes the session (emitting sessionClosed) and finishes the bus.
        try await client.shutdown()
        let seen = await collector.value
        #expect(seen.first == .sessionOpened(.regtest))
        #expect(seen.contains(.lockStateChanged(.regtest)))
        #expect(seen.contains(.walletCreated(.regtest, walletID)))
        #expect(seen.last == .sessionClosed(.regtest))

        // A new engine on the same directory sees the wallet and, after the
        // passphrase, reveals the same phrase.
        let reopened = try EngineClient(dataRoot: dir.url, workerThreads: 2)
        try await reopened.open(.regtest, options: Self.regtestOptions)
        #expect(try await reopened.walletInfos(on: .regtest).map(\.walletID) == [walletID])
        let locked = try await reopened.vaultStatus(on: .regtest)
        #expect(locked.state == .locked)
        #expect(locked.walletsWithSecrets == [walletID])
        let grant = try await reopened.authorize(
            on: .regtest, purpose: .revealSecret,
            credential: .passphrase(SecretBytes(utf8: Self.vaultPassphrase)))
        let revealed = try await reopened.revealMnemonic(on: .regtest, wallet: walletID, grantID: grant.id)
        #expect(revealed.phrase.utf8String() == phrase.utf8String())
        #expect(revealed.bip39Passphrase.utf8String() == "")
        try await reopened.shutdown()
    }

    @Test func importIsDeterministic() async throws {
        let dir = try TempDir()
        let client = try EngineClient(dataRoot: dir.url, workerThreads: 2)
        try await Self.openWithVault(client)
        let id = try await client.importWallet(
            on: .regtest, mnemonic: SecretBytes(utf8: Self.abandon12), birthHeight: 0)
        try await client.close(.regtest)

        let other = try TempDir()
        let second = try EngineClient(dataRoot: other.url, workerThreads: 2)
        try await Self.openWithVault(second)
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
            try await client.importWallet(on: .regtest, mnemonic: SecretBytes(utf8: Self.abandon12))
        }
        // Regtest has no default DAPI endpoints.
        do {
            try await client.open(.regtest)
            Issue.record("open without DAPI addresses should fail")
        } catch {
            #expect(error.code == "invalid_config")
        }

        try await Self.openWithVault(client)
        do {
            _ = try client.generateMnemonic(wordCount: 13, language: .english)
            Issue.record("13 words should be rejected")
        } catch {
            #expect(error.code == "wallet.unsupported_word_count")
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
        // A BIP39 passphrase gives a different wallet.
        let withPassphrase = try await client.importWallet(
            on: .regtest, mnemonic: SecretBytes(utf8: Self.abandon12), bip39Passphrase: SecretBytes(utf8: "x"),
            birthHeight: 0)
        #expect(try await client.vaultStatus(on: .regtest).walletsWithSecrets.contains(withPassphrase))
        // Lookahead is 1..=1000 (key-wallet's gap-limit ceiling).
        do {
            _ = try await client.importWallet(
                on: .regtest, mnemonic: SecretBytes(utf8: Self.abandon12), bip39Passphrase: SecretBytes(utf8: "y"),
                options: ImportOptions(birthHeight: 0, lookahead: 1001))
            Issue.record("lookahead above 1000 should be rejected")
        } catch {
            #expect(error.code == "invalid_argument")
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
