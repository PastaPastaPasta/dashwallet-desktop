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

    /// What the real vault answers to `authorize` without and with the
    /// passphrase.
    private struct EngineAnswer {
        /// `nil`: a grant was issued.
        let withoutCredential: ServiceErrorCode?
        let withPassphrase: ServiceErrorCode?
    }

    /// Asks the real engine for a grant; a grant it issues is revoked again.
    private static func answer(
        _ auth: AuthenticationGate, _ purpose: GrantPurpose, wallet: WalletID?, credential: Credential
    ) async -> ServiceErrorCode? {
        do {
            let grant = try await auth.authorize(purpose, wallet: wallet, credential: credential)
            auth.revoke(grant)
            return nil
        } catch {
            return error.code
        }
    }

    /// Review L4: `AuthenticationGate.requirement(for:)` against dw-vault's
    /// credential table, lock state × purpose × credential, with "require
    /// authentication for every payment" on and off:
    /// - `.none` is only answered when the vault issues the grant without a
    ///   credential; with the setting off it is answered exactly then;
    /// - whenever the passphrase is asked for, the vault issues the grant for
    ///   it and the lock state does not change;
    /// - a grant refused without a credential is refused for the lock state
    ///   or the purpose (`vault.credential_required`, `vault.locked`,
    ///   `vault.mixing_only`).
    /// The unencrypted states are not covered here: creating an unencrypted
    /// vault writes the data key to the login keychain (dw-vault's
    /// `passphrase_operations_on_an_unencrypted_vault` covers them).
    @Test func L4_requirementMatchesTheEngineCredentialTable() async throws {
        try await Self.withLive { services in
            try await services.launch(defaultNetwork: .regtest)
            let auth = services.auth
            let vault = services.vault
            let passphrase = "matrix test passphrase"
            let anyWallet = WalletID(hex: String(repeating: "ab", count: 32))!
            #expect(auth.lockState == .noVault)
            for purpose in [GrantPurpose.spend(max: Amount(duffs: 1)), .signMessage, .revealSecret, .wipe] {
                #expect(auth.requirement(for: purpose) == .none)
                let none = await Self.answer(auth, purpose, wallet: anyWallet, credential: .unencrypted)
                #expect(none == .vaultNoVault, "\(purpose)")
            }

            _ = try await vault.create(passphrase: vault.makeSecret(utf8: passphrase))
            let phrase = try await vault.generateMnemonic(wordCount: 12, language: .english)
            let wallet = try await services.lifecycle.importWallet(
                mnemonic: phrase, bip39Passphrase: vault.makeSecret(utf8: ""), options: WalletImportOptions(birthHeight: 0))

            let purposes: [GrantPurpose] = [.spend(max: Amount(duffs: 1)), .signMessage, .revealSecret, .wipe, .changeCredential]
            let states: [VaultLockState] = [.unlocked, .unlockedMixingOnly, .locked]
            for state in states {
                try await auth.lock()
                switch state {
                case .unlocked: try await auth.unlock(passphrase: vault.makeSecret(utf8: passphrase), scope: .full)
                case .unlockedMixingOnly:
                    try await auth.unlock(passphrase: vault.makeSecret(utf8: passphrase), scope: .mixingOnly)
                default: break
                }
                #expect(auth.lockState == state)
                for purpose in purposes {
                    let bound = purpose == .changeCredential ? nil : wallet
                    let answer = EngineAnswer(
                        withoutCredential: await Self.answer(auth, purpose, wallet: bound, credential: .unencrypted),
                        withPassphrase: await Self.answer(
                            auth, purpose, wallet: bound, credential: .passphrase(vault.makeSecret(utf8: passphrase))))
                    let label = "\(state) \(purpose)"
                    #expect(answer.withPassphrase == nil, "\(label): the passphrase must be accepted")
                    #expect(auth.lockState == state, "\(label): a passphrase grant keeps the lock state")
                    if let refused = answer.withoutCredential {
                        #expect(
                            [.vaultCredentialRequired, .vaultLocked, .vaultMixingOnly].contains(refused),
                            "\(label): \(refused)")
                    }
                    for everyPayment in [true, false] {
                        try services.settings.setRequireAuthenticationForEveryPayment(everyPayment)
                        let requirement = auth.requirement(for: purpose)
                        if requirement == .none {
                            #expect(answer.withoutCredential == nil, "\(label) every=\(everyPayment): asks for nothing")
                        }
                        if !everyPayment {
                            #expect(
                                (requirement == .none) == (answer.withoutCredential == nil),
                                "\(label): requirement \(requirement), engine \(String(describing: answer.withoutCredential))")
                        }
                    }
                }
            }
            try services.settings.setRequireAuthenticationForEveryPayment(true)
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
                .revealSecret, wallet: id, credential: .passphrase(vault.makeSecret(utf8: "runtime test passphrase")))
            let revealed = try await vault.revealMnemonic(wallet: id, grant: grant)
            #expect(revealed.phrase.count == phrase.count)

            try await services.lifecycle.stop()
            #expect(await services.host.activeNetwork == nil)
            #expect(services.settings.lastNetwork == .regtest)
        }
    }
    /// The R1/R2 adapters reach the Rust engine: SPV-honest node info, the
    /// minimum-relay fee policy, wallet close/open, CSV, console, xpub and
    /// the peer-moderation stub (U2).
    @Test func liveM2AdaptersAnswerThroughRust() async throws {
        try await Self.withLive { services in
            try await services.launch(defaultNetwork: .regtest)
            let desktop = DesktopRuntimeServices(runtime: services, platform: .fake(), onQuit: {})
            let vault = services.vault
            // A passphrase vault: an unencrypted one keeps its key in the OS
            // secret store, which a Linux container does not have.
            _ = try await vault.create(passphrase: vault.makeSecret(utf8: "runtime test passphrase"))
            let phrase = try await vault.generateMnemonic(wordCount: 12, language: .english)
            let id = try await services.lifecycle.importWallet(
                mnemonic: phrase, bip39Passphrase: vault.makeSecret(utf8: ""), options: WalletImportOptions(birthHeight: 0))

            let info = try await desktop.nodeInformation.information()
            #expect(info.network == .regtest && info.mempoolTransactionCount == nil && info.tipHash == nil)
            let fees = try await desktop.fees.feePolicy()
            #expect(fees.source == .minimumRelay && fees.minimumRelayPerKB == 1000)
            #expect(try await desktop.walletLifecycle.existingNetworks().contains { $0.network == .regtest })

            try await desktop.walletLifecycle.unload(id)
            #expect(try await desktop.walletLifecycle.loadStates().first { $0.walletID == id }?.loaded == false)
            #expect(services.walletState.wallets?.contains { $0.id == id } != true)
            try await desktop.walletLifecycle.load(id)
            #expect(try await desktop.walletLifecycle.loadStates().first { $0.walletID == id }?.loaded == true)

            let xpub = try await desktop.walletLifecycle.accountXpub(wallet: id, account: 0)
            #expect(xpub.xpub.hasPrefix("tpub"))
            let csv = try await desktop.transactionActions.exportCSV(
                wallet: id, filter: HistoryFilter(), sort: .newestFirst, options: HistoryCSVOptions(unit: .dash))
            #expect(String(decoding: csv, as: UTF8.self).hasPrefix("\"Confirmed\""))
            let commands = try await desktop.console.commands()
            #expect(commands.contains { $0.name == "getblockcount" && $0.available })

            do throws(ServiceError) {
                _ = try await desktop.peerModeration.bannedPeers()
                Issue.record("peer moderation is a typed stub until U2")
            } catch {
                #expect(error.code == .notImplemented)
            }
        }
    }
}
