// The demo M2 services answer like the engine (docs/contracts/m2-engine.md):
// dash-qt's abandon/resend rules, the coin-control formula, multiwallet
// load/unload, recovery and destroy, and typed `not_implemented` wherever the
// demo would have to read or write files. The M2 view models run against it.
import Foundation
import Testing
import WalletDemo
import WalletFeatures
import WalletRuntime

@MainActor
struct DemoM2RulesTests {
    static func make(_ scenario: DemoScenario = .funded) -> (env: AppEnvironment, m2: M2Services) {
        DemoEnvironment.makeWithM2(scenario: scenario, desktopPlatform: .linux)
    }

    static func code(_ body: () async throws -> Void) async -> ServiceErrorCode? {
        do {
            try await body()
            return nil
        } catch {
            return (error as? ServiceError)?.code
        }
    }

    // MARK: Transactions (QT-091, QT-093, IOS-034)

    @Test func QT091_receivedUnconfirmedCannotBeAbandonedOrResent() async throws {
        let (env, m2) = Self.make()
        let wallet = try #require(env.walletState.selectedWalletID)
        let page = try await env.history.page(wallet: wallet, query: HistoryQuery(limit: 500))
        let pending = try #require(page.records.first { $0.status.kind == .unconfirmed && !$0.status.instantLocked })
        let extras = try await m2.transactionActions.extras(wallet: wallet, txid: pending.id.txid)
        #expect(!extras.canAbandon && !extras.canResend)
        let error = await Self.code { try await m2.transactionActions.abandon(wallet: wallet, txid: pending.id.txid) }
        #expect(error == .txActionRefused)
        let confirmed = try #require(page.records.first { $0.status.kind == .confirmed })
        do {
            try await m2.transactionActions.resend(wallet: wallet, txid: confirmed.id.txid)
            Issue.record("resend of a confirmed transaction succeeded")
        } catch {
            #expect(error.code == .txActionRefused)
            #expect(error.parameters["refusal"] == TransactionActionRefusal.confirmed.rawValue)
        }
        #expect(await Self.code { try await m2.transactionActions.abandon(wallet: wallet, txid: "xyz") } == .invalidArgument)
        #expect(await Self.code {
            try await m2.transactionActions.abandon(wallet: wallet, txid: String(repeating: "0", count: 64))
        } == .txActionTxNotFound)
    }

    @Test func IOS034_dropUnconfirmedHasNothingEligibleInTheSample() async throws {
        let (_, m2) = Self.make()
        #expect(try await m2.transactionActions.dropUnconfirmed(wallet: nil) == 0)
    }

    @Test func QT093_csvHasDashQtColumnsAndRefusesBadTypeNames() async throws {
        let (env, m2) = Self.make()
        let wallet = try #require(env.walletState.selectedWalletID)
        let data = try await m2.transactionActions.exportCSV(
            wallet: wallet, filter: HistoryFilter(), sort: .newestFirst, options: HistoryCSVOptions(unit: .dash))
        let text = String(decoding: data, as: UTF8.self)
        #expect(text.hasPrefix("\"Confirmed\",\"Date\",\"Type\",\"Label\",\"Address\",\"Amount (tDASH)\",\"ID\"\n"))
        #expect(text.split(separator: "\n").count == 26)
        let bad = await Self.code {
            _ = try await m2.transactionActions.exportCSV(
                wallet: wallet, filter: HistoryFilter(), sort: .newestFirst,
                options: HistoryCSVOptions(unit: .dash, typeNames: ["x"]))
        }
        #expect(bad == .historyInvalidQuery)
    }

    // MARK: Fees and coin control (QT-057, QT-072, QT-074)

    @Test func QT057_feePolicyIsMinimumRelayWithDashQtTargets() async throws {
        let (_, m2) = Self.make()
        let policy = try await m2.fees.feePolicy()
        #expect(policy.source == .minimumRelay)
        #expect(policy.targets.map(\.targetBlocks) == [2, 4, 6, 12, 24, 48, 144, 504, 1008])
        #expect(policy.maximumBroadcastRatePerKB == 10_000_000)
    }

    @Test func QT072_QT074_summaryFormulaAndUnavailableCoins() async throws {
        let (env, m2) = Self.make()
        let wallet = try #require(env.walletState.selectedWalletID)
        let coins = try await env.coinControl.utxos(wallet: wallet, filter: UtxoFilter())
        let picked = Array(coins.prefix(2)).map(\.outpoint)
        let gone = OutPoint(txid: String(repeating: "1", count: 64), vout: 0)
        let summary = try await m2.fees.summary(
            wallet: wallet, outpoints: picked + [gone], payAmounts: [Amount(duffs: 1_000_000)],
            fee: .recommended(targetBlocks: 6), allChangeToFee: false)
        #expect(summary.quantity == 2)
        #expect(summary.bytes == 148 * 2 + 34 * 2 + 10)
        #expect(summary.unavailable == [gone])
        #expect(!summary.insufficientFunds)
    }

    // MARK: Multiwallet (QT-101, IOS-009)

    @Test func QT101_unloadKeepsTheWalletRegisteredAndHidesIt() async throws {
        let (env, m2) = Self.make()
        let wallet = try #require(env.walletState.selectedWalletID)
        try await m2.walletLifecycle.unload(wallet)
        #expect(env.walletState.wallets?.isEmpty == true)
        #expect(try await m2.walletLifecycle.loadStates().map(\.loaded) == [false])
        // Wallet-scoped calls answer wallet_not_found for an unloaded wallet.
        #expect(await Self.code { _ = try await env.history.page(wallet: wallet, query: HistoryQuery()) } == .walletNotFound)
        try await m2.walletLifecycle.unload(wallet)  // idempotent
        try await m2.walletLifecycle.load(wallet)
        #expect(env.walletState.wallets?.count == 1)
        try await m2.walletLifecycle.setLoadOnStartup(wallet, false)
        #expect(try await m2.walletLifecycle.loadStates().first?.loadOnStartup == false)
    }

    @Test func IOS009_existingNetworksListTheSampleWallet() async throws {
        let (_, m2) = Self.make()
        let networks = try await m2.walletLifecycle.existingNetworks()
        #expect(networks.map(\.network) == [.testnet])
        #expect(networks.first?.hasWalletState == true && networks.first?.hasOSStoreKey == true)
    }

    @Test func QT114_IOS111_watchOnlyAndXpubAreNotImplementedInDemo() async throws {
        let (env, m2) = Self.make()
        let wallet = try #require(env.walletState.selectedWalletID)
        #expect(await Self.code { _ = try await m2.walletLifecycle.importWatchOnly(xpub: "tpub", options: WatchOnlyImportOptions()) } == .notImplemented)
        #expect(await Self.code { _ = try await m2.walletLifecycle.accountXpub(wallet: wallet, account: 0) } == .notImplemented)
    }

    // MARK: Files (QT-106…110, QT-076…079, IOS-112)

    @Test func QT106_fileCallsAreTypedNotImplemented() async throws {
        let (env, m2) = Self.make()
        let wallet = try #require(env.walletState.selectedWalletID)
        let file = URL(fileURLWithPath: "/tmp/none")
        #expect(await Self.code { _ = try await m2.fileImporter.inspect(file) } == .notImplemented)
        #expect(await Self.code { _ = try await m2.backups.backup(wallet: wallet, to: file, passphrase: nil) } == .notImplemented)
        #expect(await Self.code { _ = try await m2.logs.exportLogs(to: file) } == .notImplemented)
        #expect(try await m2.backups.automaticBackups(wallet: nil).isEmpty)
        #expect(await Self.code { _ = try await m2.backups.setKeep(11) } == .invalidArgument)
        #expect(try await m2.backups.setKeep(3).keep == 3)
    }

    @Test func QT078_psbtSizeLimitThenNotImplemented() {
        let (_, m2) = Self.make()
        do {
            _ = try m2.psbt.load(Data(count: 100 * 1024 * 1024 + 1))
            Issue.record("oversized PSBT accepted")
        } catch {
            #expect(error.code == .psbtTooLarge)
        }
        do {
            _ = try m2.psbt.load(Data("cHNidP8B".utf8))
            Issue.record("demo parsed a PSBT")
        } catch {
            #expect(error.code == .notImplemented)
        }
    }

    // MARK: Tools (QT-143, QT-145, QT-147, IOS-113)

    @Test func QT143_informationComesFromSyncAndLeavesFullNodeFieldsUnknown() async throws {
        let (_, m2) = Self.make()
        let info = try await m2.nodeInformation.information()
        #expect(info.connectionsOut == 8 && info.tipHeight != nil)
        #expect(info.mempoolTransactionCount == nil && info.masternodes == nil)
        let (_, offline) = Self.make(.offline)
        #expect(try await offline.nodeInformation.warnings() == [.syncStalled])
    }

    @Test func QT147_banNeedsAConnectedPeer() async throws {
        let (env, m2) = Self.make()
        let peer = try #require(try await env.sync.peers().first)
        #expect(await Self.code { try await m2.peerModeration.ban(address: "198.51.100.1:9999", for: .seconds(3600)) } == .syncPeerNotFound)
        try await m2.peerModeration.ban(address: peer.address, for: .seconds(3600))
        let banned = try await m2.peerModeration.bannedPeers()
        #expect(banned.count == 1)
        try await m2.peerModeration.unban(subnet: banned[0].subnet)
        #expect(try await m2.peerModeration.bannedPeers().isEmpty)
    }

    @Test func IOS113_birthHeightAboveTipIsOutOfRange() async throws {
        let (env, m2) = Self.make()
        let wallet = try #require(env.walletState.selectedWalletID)
        #expect(await Self.code { try await m2.repair.setBirthHeight(wallet: wallet, height: 9_999_999) } == .syncHeightOutOfRange)
        try await m2.repair.setBirthHeight(wallet: wallet, height: 100)
    }

    @Test func QT145_consoleRedactsAndAnswersLikeTheEngine() async throws {
        let (env, m2) = Self.make()
        let vault = env.vault
        #expect(try m2.console.redact(vault.makeSecret(utf8: "walletpassphrase \"pw\" 60")) == "walletpassphrase(…)")
        #expect(await Self.code { _ = try m2.console.redact(vault.makeSecret(utf8: "getblock \"abc")) } == .consoleParseError)
        let result = try await m2.console.execute(vault.makeSecret(utf8: "getblockcount"), wallet: nil, grant: nil)
        #expect(result == .output(text: "1234567", isJSON: false))
        #expect(await Self.code { _ = try await m2.console.execute(vault.makeSecret(utf8: "getbalance"), wallet: nil, grant: nil) } == .consoleWalletRequired)
        #expect(await Self.code { _ = try await m2.console.execute(vault.makeSecret(utf8: "getmempoolinfo"), wallet: nil, grant: nil) } == .consoleNotAvailable)
        #expect(await Self.code { _ = try await m2.console.execute(vault.makeSecret(utf8: "nosuch"), wallet: nil, grant: nil) } == .consoleRPCError)
    }

    // MARK: Security (IOS-011, IOS-014, IOS-109)

    @Test func IOS011_quickUnlockIsUnavailableInDemo() async throws {
        let (_, m2) = Self.make()
        #expect(m2.quickUnlock.provider == .unavailable)
        #expect(try await m2.quickUnlock.policy().spendLimit == QuickUnlockPolicy.defaultSpendLimit)
    }

    @Test func IOS014_recoveryNeedsTheWalletsPhrase() async throws {
        let (env, m2) = Self.make(.locked)
        let wallet = try #require(env.walletState.selectedWalletID)
        let wrong = env.vault.makeSecret(utf8: "abandon abandon abandon")
        #expect(await Self.code {
            _ = try await m2.vaultRecovery.recover(
                wallet: wallet, mnemonic: wrong, bip39Passphrase: env.vault.makeSecret(utf8: ""),
                newPassphrase: env.vault.makeSecret(utf8: "new"))
        } == .vaultRecoveryMismatch)
        let result = try await m2.vaultRecovery.recover(
            wallet: wallet, mnemonic: env.vault.makeSecret(utf8: DemoEnvironment.sampleWalletPhrase),
            bip39Passphrase: env.vault.makeSecret(utf8: ""), newPassphrase: env.vault.makeSecret(utf8: "new"))
        #expect(result.status.state == .unlocked && result.walletsWithoutSecrets.isEmpty)
        // The new passphrase now unlocks it.
        try await env.auth.lock()
        try await env.auth.unlock(passphrase: env.vault.makeSecret(utf8: "new"), scope: .full)
    }

    @Test func IOS109_destroyRefusesWhileAWalletRemainsThenDeletesTheVault() async throws {
        let (env, m2) = Self.make()
        let wallet = try #require(env.walletState.selectedWalletID)
        #expect(await Self.code { _ = try await m2.vaultRecovery.destroy(credential: .unencrypted) } == .vaultNotEmpty)
        let grant = try await env.auth.authorize(.wipe, wallet: wallet, credential: .unencrypted)
        try await env.lifecycle.removeWallet(wallet, grant: grant)
        let status = try await m2.vaultRecovery.destroy(credential: .unencrypted)
        #expect(status.state == .noVault)
    }

    // MARK: View models over the demo (V1)

    @Test func viewModelsRunAgainstTheDemo() async throws {
        let (env, m2) = Self.make()
        let shell = ShellModel(env: env, m2: m2)
        await shell.refresh()
        #expect(shell.windowTitle == "Dash Wallet - Demo wallet - [testnet]")
        #expect(shell.loadStates.count == 1)

        let wallets = WalletManagementViewModel(env: env, m2: m2)
        await wallets.load()
        #expect(!wallets.loadStatesUnavailable && wallets.wallets.count == 1)
        await wallets.importFile(URL(fileURLWithPath: "/tmp/dump.txt"))
        #expect(wallets.flow == .failed(L10n.Common.notAvailableYet))

        let options = OptionsViewModel(env: env, m2: m2)
        await options.load()
        #expect(options.dustProtectionAvailable && options.automaticBackupsAvailable)
        options.wallet.dustProtectionEnabled = true
        try await options.apply()
        #expect(try await m2.dustProtection.threshold() == Amount(duffs: 10_000))

        let coins = CoinControlViewModel(env: env, m2: m2)
        await coins.load()
        await coins.selectAll()
        #expect(coins.summary != nil && coins.summaryText?.bytes.hasPrefix("≈") == true)

        let info = InformationViewModel(env: env, m2: m2)
        await info.load()
        #expect(info.sections.flatMap(\.rows).first { $0.title == "Current number of transactions" }?.note
            == L10n.Tools.requiresFullNode)

        let console = ConsoleViewModel(env: env, m2: m2)
        await console.run(line: "getblockcount")
        #expect(console.entries.last?.text == "1234567")
    }
}
