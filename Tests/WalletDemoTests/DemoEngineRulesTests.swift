// The demo services must answer like the engine (docs/contracts/m1-engine.md):
// valid addresses and phrases, the grant rules of §2.2 and the send rules of
// §2.7, so a screen that works in demo mode works against the engine.
import Foundation
import Testing
import WalletDemo
import WalletFeatures
import WalletRuntime

@MainActor
struct DemoEngineRulesTests {
    /// A valid testnet address that is not one of the demo wallet's own.
    static let payTo = "yPgfYhP6PwdZd8xn1TKDps27nL6kLpvh98"

    static func environment(_ scenario: DemoScenario, network: DashNetwork = .testnet) -> AppEnvironment {
        DemoEnvironment.make(scenario: scenario, network: network)
    }

    static func wallet(_ env: AppEnvironment) throws -> WalletID {
        try #require(env.walletState.selectedWalletID)
    }

    /// The code `body` fails with; `nil` when it succeeds or throws something else.
    static func code(_ body: () async throws -> Void) async -> ServiceErrorCode? {
        do {
            try await body()
            return nil
        } catch {
            return (error as? ServiceError)?.code
        }
    }

    // MARK: Data

    @Test(arguments: [DashNetwork.testnet, .mainnet])
    func sampleAddressesAreValidForTheEngine(_ network: DashNetwork) async throws {
        let env = Self.environment(.funded, network: network)
        let wallet = try Self.wallet(env)
        let addresses = try await env.receive.addresses(wallet: wallet, filter: AddressFilter())
        #expect(addresses.count == 60)
        for info in addresses {
            #expect(env.uri.classifyAddress(info.address) == .core(scriptHash: false), "\(info.address)")
        }
        let book = try await env.addressBook.entries(wallet: wallet, purpose: nil, search: nil)
        #expect(book.allSatisfy { env.uri.classifyAddress($0.address) == .core(scriptHash: false) })
        let current = try await env.receive.currentAddress(wallet: wallet)
        #expect(try env.uri.buildPaymentURI(address: current.address, amount: nil, label: nil, message: nil)
            == "dash:\(current.address)")
    }

    @Test func demoPhrasePassesTheEngineChecksum() async throws {
        let env = Self.environment(.fresh)
        for count in [12, 24] {
            let phrase = try await env.vault.generateMnemonic(wordCount: count, language: .english)
            let check = try await env.vault.checkMnemonic(phrase)
            #expect(check.wordCount == count)
            #expect(check.checksum == .valid)
        }
        let typo = env.vault.makeSecret(utf8: DemoEnvironment.phrase.prefix(11).joined(separator: " ") + " abandon")
        #expect(try await env.vault.checkMnemonic(typo).checksum == .invalid)
    }

    @Test func balancesAreTheSumOfTheCoins() async throws {
        let env = Self.environment(.funded)
        let wallet = try Self.wallet(env)
        let coins = try await env.coinControl.utxos(wallet: wallet, filter: UtxoFilter())
        let balances = try #require(env.walletState.balances)
        #expect(coins.reduce(Int64(0)) { $0 + $1.amount.duffs } == balances.total.duffs)
        let page = try await env.history.page(wallet: wallet, query: HistoryQuery(limit: 500))
        #expect(page.records.count == 25)
        #expect(page.records.reduce(Int64(0)) { $0 + $1.amount.duffs } == balances.total.duffs)
    }

    // MARK: Grants (§2.2)

    @Test func lockedVaultFollowsTheCredentialTable() async throws {
        let env = Self.environment(.locked)
        let wallet = try Self.wallet(env)
        let auth = env.auth
        #expect(await Self.code { _ = try await auth.authorize(.spend(max: Amount(duffs: 1)), wallet: wallet, credential: .unencrypted) }
            == .vaultLocked)
        #expect(await Self.code { _ = try await auth.authorize(.revealSecret, wallet: wallet, credential: .unencrypted) }
            == .vaultCredentialRequired)
        #expect(await Self.code { _ = try await auth.authorize(.revealSecret, wallet: nil, credential: .unencrypted) }
            == .invalidArgument)
        let wrong = env.vault.makeSecret(utf8: "wrong")
        #expect(await Self.code { _ = try await auth.authorize(.revealSecret, wallet: wallet, credential: .passphrase(wrong)) }
            == .vaultWrongPassphrase)
        // A passphrase grant acts while the vault stays locked (review M4).
        let grant = try await auth.authorize(
            .revealSecret, wallet: wallet, credential: .passphrase(env.vault.makeSecret(utf8: DemoEnvironment.passphrase)))
        #expect(auth.lockState == .locked)
        let revealed = try await env.vault.revealMnemonic(wallet: wallet, grant: grant)
        #expect(revealed.phrase.count > 0)
        // Single use.
        #expect(await Self.code { _ = try await env.vault.revealMnemonic(wallet: wallet, grant: grant) } == .vaultGrantInvalid)
    }

    @Test func grantsAreBoundToPurposeAndWallet() async throws {
        let env = Self.environment(.funded)
        let wallet = try Self.wallet(env)
        let other = WalletID(hex: String(repeating: "ab", count: 32))!
        let grant = try await env.auth.authorize(.revealSecret, wallet: other, credential: .unencrypted)
        #expect(await Self.code { _ = try await env.vault.revealMnemonic(wallet: wallet, grant: grant) }
            == .vaultGrantPurposeMismatch)
        let sign = try await env.auth.authorize(.signMessage, wallet: wallet, credential: .unencrypted)
        #expect(await Self.code { _ = try await env.vault.revealMnemonic(wallet: wallet, grant: sign) }
            == .vaultGrantPurposeMismatch)
        // Refused grants stay issued; a revoked one is gone.
        env.auth.revoke(grant)
        #expect(await Self.code { _ = try await env.vault.revealMnemonic(wallet: other, grant: grant) } == .vaultGrantInvalid)
    }

    // MARK: Sending (§2.7)

    @Test func duplicateRecipientsAreRefused() async throws {
        let env = Self.environment(.funded)
        let draft = try await env.sender.makeDraft(wallet: try Self.wallet(env))
        let recipient = PaymentRecipient(address: Self.payTo, amount: Amount(duffs: 100_000))
        do {
            try await draft.setRecipients([recipient, recipient])
            Issue.record("duplicates accepted")
        } catch {
            #expect(error.code == .sendDuplicateAddress)
            #expect(error.recipientIndex == 1)
        }
        await #expect(throws: ServiceError.self) {
            try await draft.setRecipients([PaymentRecipient(address: Self.payTo, amount: Amount(duffs: 545))])
        }
        await #expect(throws: ServiceError.self) {
            try await draft.setRecipients([PaymentRecipient(address: "yNotAnAddress", amount: Amount(duffs: 100_000))])
        }
    }

    @Test func aPaymentSpendsCoinsAndAddsTheRecipient() async throws {
        let env = Self.environment(.funded)
        let wallet = try Self.wallet(env)
        let before = try #require(env.walletState.balances).total.duffs
        let draft = try await env.sender.makeDraft(wallet: wallet)
        try await draft.setRecipients([PaymentRecipient(address: Self.payTo, amount: Amount(duffs: 25_000_000))])
        let estimate = try await draft.estimate()
        // The cap covers the fee as well (as the engine's).
        let tooLow = try await env.auth.authorize(.spend(max: Amount(duffs: 25_000_000)), wallet: wallet, credential: .unencrypted)
        #expect(await Self.code { _ = try await draft.prepare(grant: tooLow) } == .sendGrantExceeded)
        let grant = try await env.auth.authorize(
            .spend(max: Amount(duffs: 25_000_000 + estimate.fee.duffs)), wallet: wallet, credential: .unencrypted)
        let prepared = try await draft.prepare(grant: grant)
        #expect(prepared.summary.fee == estimate.fee)
        let reserved = try await env.coinControl.utxos(wallet: wallet, filter: UtxoFilter()).filter(\.reserved)
        #expect(!reserved.isEmpty)
        // The grant was used.
        let draft2 = try await env.sender.makeDraft(wallet: wallet)
        try await draft2.setRecipients([PaymentRecipient(address: Self.payTo, amount: Amount(duffs: 1_000_000))])
        #expect(await Self.code { _ = try await draft2.prepare(grant: grant) } == .sendGrantInvalid)

        let result = try await draft.broadcast(prepared)
        #expect(result.txid == prepared.summary.txid)
        #expect(try #require(env.walletState.balances).total.duffs == before - 25_000_000 - estimate.fee.duffs)
        let book = try await env.addressBook.entries(wallet: wallet, purpose: .send, search: nil)
        #expect(book.contains { $0.address == Self.payTo && $0.label.isEmpty })
        let detail = try await env.history.detail(wallet: wallet, txid: result.txid)
        #expect(detail.records.first?.amount.duffs == -(25_000_000 + estimate.fee.duffs))
        #expect(await Self.code { _ = try await draft.broadcast(prepared) } == .sendPreparedTxUnknown)
    }

    @Test func theCapAndTheFeeAreChecked() async throws {
        let env = Self.environment(.funded)
        let wallet = try Self.wallet(env)
        let total = try #require(env.walletState.balances).confirmed.duffs
        let draft = try await env.sender.makeDraft(wallet: wallet)
        try await draft.setRecipients([PaymentRecipient(address: Self.payTo, amount: Amount(duffs: total))])
        do {
            _ = try await draft.estimate()
            Issue.record("no fee error")
        } catch {
            #expect(error.code == .sendAmountWithFeeExceedsBalance)
            #expect((error.parameters["fee"] ?? 0) > 0)
        }
        try await draft.setRecipients([PaymentRecipient(address: Self.payTo, amount: Amount(duffs: 2_000_000))])
        let grant = try await env.auth.authorize(.spend(max: Amount(duffs: 1_999_999)), wallet: wallet, credential: .unencrypted)
        #expect(await Self.code { _ = try await draft.prepare(grant: grant) } == .sendGrantExceeded)
        // Subtract-fee keeps the payment within the amounts.
        try await draft.setRecipients([
            PaymentRecipient(address: Self.payTo, amount: Amount(duffs: total), subtractFeeFromAmount: true),
        ])
        let max = try await env.sender.maxSpendable(wallet: wallet, source: .any, fee: .recommended(targetBlocks: 6))
        #expect(max.duffs == total)
        let estimate = try await draft.estimate()
        #expect(estimate.totalSent.duffs == total - estimate.fee.duffs)
    }

    @Test func noPeersReleasesTheCoinsAndSpendsThePreparedTransaction() async throws {
        let env = Self.environment(.offline)
        let wallet = try Self.wallet(env)
        let max = try await env.sender.maxSpendable(wallet: wallet, source: .any, fee: .recommended(targetBlocks: 6))
        let draft = try await env.sender.makeDraft(wallet: wallet)
        try await draft.setRecipients([PaymentRecipient(address: Self.payTo, amount: Amount(duffs: 10_000_000))])
        let fee = try await draft.estimate().fee.duffs
        let grant = try await env.auth.authorize(
            .spend(max: Amount(duffs: 10_000_000 + fee)), wallet: wallet, credential: .unencrypted)
        let prepared = try await draft.prepare(grant: grant)
        let reserved = try await env.sender.maxSpendable(wallet: wallet, source: .any, fee: .recommended(targetBlocks: 6))
        #expect(reserved.duffs < max.duffs)
        #expect(await Self.code { _ = try await draft.broadcast(prepared) } == .sendNoPeers)
        #expect(await Self.code { _ = try await draft.broadcast(prepared) } == .sendPreparedTxUnknown)
        let released = try await env.sender.maxSpendable(wallet: wallet, source: .any, fee: .recommended(targetBlocks: 6))
        #expect(released == max)
    }

    /// The engine's `install_key`: an unlock that changes the lock state
    /// drops every grant (review L7).
    @Test func unlockWithAnotherScopeDropsEveryGrant() async throws {
        let env = Self.environment(.locked)
        let wallet = try Self.wallet(env)
        func secret() -> any SecretBuffer { env.vault.makeSecret(utf8: DemoEnvironment.passphrase) }
        try await env.auth.unlock(passphrase: secret(), scope: .full)
        // Unlocking again with the same scope changes nothing: the grant stays.
        let kept = try await env.auth.authorize(.revealSecret, wallet: wallet, credential: .passphrase(secret()))
        try await env.auth.unlock(passphrase: secret(), scope: .full)
        _ = try await env.vault.revealMnemonic(wallet: wallet, grant: kept)
        // Another scope drops it.
        let dropped = try await env.auth.authorize(.revealSecret, wallet: wallet, credential: .passphrase(secret()))
        try await env.auth.unlock(passphrase: secret(), scope: .mixingOnly)
        #expect(env.auth.lockState == .unlockedMixingOnly)
        #expect(await Self.code { _ = try await env.vault.revealMnemonic(wallet: wallet, grant: dropped) } == .vaultGrantInvalid)
    }

    /// Like the engine, `changePassphrase` rejects an empty new passphrase
    /// before it checks the old one, so no failed attempt is counted (L7).
    @Test func changePassphraseValidatesTheNewPassphraseFirst() async throws {
        let env = Self.environment(.locked)
        let before = try await env.vault.status().failedAttempts
        let rejected = await Self.code {
            _ = try await env.vault.changePassphrase(
                old: env.vault.makeSecret(utf8: "wrong"), new: env.vault.makeSecret(utf8: ""))
        }
        #expect(rejected == .vaultPassphraseRejected)
        #expect(try await env.vault.status().failedAttempts == before)
        let wrong = await Self.code {
            _ = try await env.vault.changePassphrase(
                old: env.vault.makeSecret(utf8: "wrong"), new: env.vault.makeSecret(utf8: "new one"))
        }
        #expect(wrong == .vaultWrongPassphrase)
        #expect(try await env.vault.status().failedAttempts == before + 1)
    }

    @Test func lockingDropsEveryGrant() async throws {
        let env = Self.environment(.locked)
        let wallet = try Self.wallet(env)
        // A grant without its own key cannot be issued on a locked vault, so
        // unlock, take one, lock again: prepare refuses before using it.
        try await env.auth.unlock(passphrase: env.vault.makeSecret(utf8: DemoEnvironment.passphrase), scope: .full)
        let grant = try await env.auth.authorize(.spend(max: Amount(duffs: 1_000_000)), wallet: wallet, credential: .unencrypted)
        try await env.auth.lock()
        let draft = try await env.sender.makeDraft(wallet: wallet)
        try await draft.setRecipients([PaymentRecipient(address: Self.payTo, amount: Amount(duffs: 1_000_000))])
        // Locking dropped every grant.
        #expect(await Self.code { _ = try await draft.prepare(grant: grant) } == .sendGrantInvalid)
    }
}
