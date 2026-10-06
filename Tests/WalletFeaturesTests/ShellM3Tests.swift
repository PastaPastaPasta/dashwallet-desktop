// The M3 parts of the shell, Tools and history (QT-012, QT-016, QT-018,
// QT-022, QT-029, QT-144, QT-153, IOS-030) and the live placeholder services.
import Foundation
import PlatformServices
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("M3 shell, tools and history")
struct ShellM3Tests {
    let world = FakeWorld()
    let m2 = FakeM2World()
    let m3 = FakeM3World()

    init() {
        m3.coinJoin.settingsValue.withLock { $0 = .dashQtDefaults }
    }

    func makeShell(platform: DesktopPlatform = .linux) -> ShellModel {
        ShellModel(
            walletState: world.walletState, auth: world.auth, settings: world.settings, host: world.host,
            walletLifecycle: m2.walletLifecycle, backups: m2.backups, fileRevealer: m2.fileRevealer,
            desktopPreferences: m2.desktopPreferences, launchOptions: LaunchOptions(), features: .m3,
            coinJoin: m3.coinJoin, platform: platform)
    }

    @Test func QT012_coinJoinSectionFollowsTheEnableOption() async {
        let shell = makeShell()
        await shell.refresh()
        #expect(shell.sections == [.overview, .send, .receive, .transactions, .coinJoin])
        m2.desktopPreferences.desktop.options.showMasternodesTab = true
        m2.desktopPreferences.desktop.options.showGovernanceTab = true
        #expect(shell.sections == [.overview, .send, .receive, .transactions, .coinJoin, .masternodes, .governance])
        await shell.selectShortcut(5)
        #expect(shell.selection == .coinJoin)
        shell.coinJoinOptionChanged(enabled: false)
        #expect(!shell.sections.contains(.coinJoin))
        #expect(shell.selection == .overview)
    }

    @Test func QT012_engineWithoutCoinJoinHidesTheSection() async {
        let shell = ShellModel(
            env: world.environment(), m2: m2.services, features: .m3, m3: M3Services.unavailable(network: .testnet))
        await shell.refresh()
        #expect(!shell.sections.contains(.coinJoin))
        #expect(!shell.coinJoinOn)
    }

    @Test func QT016_settingsMenuOffersUnlockForMixingOnly() async {
        world.auth.lockState = .locked
        let shell = makeShell()
        await shell.refresh()
        let items = shell.menus[1].items
        let mixing = try! #require(items.first { $0.id == "settings.unlockMixing" })
        #expect(mixing.title == "Unlock Wallet for mixing only…")
        #expect(mixing.unlockScope == .mixingOnly)
        await shell.perform(item: mixing)
        #expect(shell.pendingPresentation == .unlockWallet)
        #expect(shell.pendingUnlockScope == .mixingOnly)
        shell.presentationHandled()
        #expect(shell.pendingUnlockScope == .full)
        let full = try! #require(items.first { $0.id == "settings.unlock" })
        await shell.perform(item: full)
        #expect(shell.pendingUnlockScope == .full)
        // While mixing only, the full unlock stays and the mixing one goes.
        world.auth.lockState = .unlockedMixingOnly
        await shell.refresh()
        #expect(shell.menus[1].items.contains { $0.id == "settings.unlock" })
        #expect(!shell.menus[1].items.contains { $0.id == "settings.unlockMixing" })
    }

    @Test func QT018_helpShowsCoinJoinInformationWhileCoinJoinIsOn() async {
        let shell = makeShell()
        await shell.refresh()
        #expect(shell.menus[3].items.map(\.title) == ["Command-line options", "CoinJoin information", "About Dash Wallet"])
        #expect(shell.isEnabled(.coinJoinInformation))
        await shell.perform(.coinJoinInformation)
        #expect(shell.pendingPresentation == .coinJoinInformation)
        shell.coinJoinOptionChanged(enabled: false)
        #expect(!shell.menus[3].items.map(\.title).contains("CoinJoin information"))
    }

    @Test func QT022_mixingOnlyLockIconIsOrange() async {
        world.auth.lockState = .unlockedMixingOnly
        let shell = makeShell()
        await shell.refresh()
        #expect(shell.lockIcon == .unlockedMixingOnly)
        #expect(shell.lockIcon?.isMixingOnly == true)
        #expect(shell.lockIcon?.tooltip == "Wallet is encrypted and currently unlocked for mixing only")
        #expect(LockIcon.locked.isMixingOnly == false)
    }

    @Test func QT029_trayMenuFollowsDashQtWithCoinJoin() async {
        let shell = makeShell()
        await shell.refresh()
        let titles = shell.trayMenu.filter { !$0.isSeparator }.map(\.title)
        #expect(Array(titles.prefix(4)) == ["Show / Hide", "Send", "CoinJoin", "Receive"])
        #expect(titles.last == "Exit")
        let coinJoin = shell.trayMenu.first { $0.id == "tray.coinJoin" }
        #expect(coinJoin?.command == .section(.coinJoin))
        let mac = makeShell(platform: .macOS)
        await mac.refresh()
        let macTitles = mac.trayMenu.filter { !$0.isSeparator }.map(\.title)
        #expect(macTitles.first == "Send")
        #expect(!macTitles.contains("Exit"))
        mac.coinJoinOptionChanged(enabled: false)
        #expect(!mac.trayMenu.contains { $0.id == "tray.coinJoin" })
    }

    @Test func QT153_coinJoinInformationExplainsWithoutTheBackupWarning() {
        let text = L10n.CoinJoin.informationSections.map(\.body).joined()
        #expect(L10n.CoinJoin.informationTitle == "CoinJoin information")
        #expect(text.contains("0.001 DASH, 0.01 DASH, 0.1 DASH, 1 DASH and 10 DASH"))
        // HD wallets need no keypool backups (DESIGN-opus §7.5).
        #expect(!text.contains("automatic backups enabled"))
        #expect(L10n.CoinJoin.documentationURL.scheme == "https")
    }

    // MARK: Tools ▸ Information ▸ Network (QT-144)

    @Test func QT144_networkTabShowsSPVValuesAndHonestDashes() async {
        m3.coinJoin.networkStatistics.withLock {
            $0 = NetworkStatistics(
                creditPool: nil, instantSend: nil, masternodes: MasternodeCount(total: 3200, enabled: 3000),
                evonodes: MasternodeCount(total: 450, enabled: 440),
                bestChainLock: ChainLockInfo(height: 1_234_567, blockHash: "00ab", blockDate: nil),
                quorums: [QuorumSummary(name: "llmq_50_60", type: 1, active: 24, healthPercent: 98.5, rotated: false)])
        }
        let model = NetworkInformationViewModel(env: world.environment(), m3: m3.services)
        await model.reload()
        let rows = model.sections.flatMap(\.rows)
        func row(_ title: String) -> InformationRow? { rows.first { $0.title == title } }
        #expect(row("Masternode Count")?.value == "3200 (3000 enabled)")
        #expect(row("EvoNode Count")?.value == "450 (440 enabled)")
        #expect(row("Best ChainLock height")?.value == "1234567")
        #expect(row("llmq_50_60")?.value == "24 active (98.5% health)")
        #expect(row("Total locked")?.value == "—")
        #expect(row("Total locked")?.note == "Requires full-node data source")
        #expect(row("Verified locks")?.note == "Requires full-node data source")
    }

    @Test func QT144_unavailableEngineSaysSo() async {
        let model = NetworkInformationViewModel(env: world.environment(), m3: M3Services.unavailable(network: .testnet))
        await model.reload()
        #expect(!model.available)
        #expect(model.sections.isEmpty)
    }

    // MARK: History (IOS-030)

    @Test func IOS030_coinJoinWithdrawalsAreOneCombinedRow() async {
        world.history.state.withLock {
            $0.records = [
                record(txid(1), type: .sendToSelf, amount: -500, date: Date(timeIntervalSince1970: 1_760_000_000)),
                record(txid(2), type: .sendToSelf, amount: -700, date: Date(timeIntervalSince1970: 1_760_003_600)),
                record(txid(3), amount: 1_00000000, date: Date(timeIntervalSince1970: 1_760_007_200)),
            ]
        }
        m2.desktopPreferences.desktop.m3.coinJoinWithdrawals[walletA.hex] = [txid(1), txid(2)]
        let model = TransactionsViewModel(env: world.environment(), m2: m2.services, network: .testnet)
        await model.reload()
        let group = try! #require(model.coinJoinWithdrawals)
        #expect(group.records.count == 2)
        #expect(group.total == Amount(duffs: -1_200))
        #expect(group.title == "CoinJoin Withdrawals")
        // The day list keeps them until the UI draws the group.
        #expect(model.dayGroups.flatMap(\.items).count == 3)
        model.groupsCoinJoinWithdrawals = true
        #expect(model.dayGroups.flatMap(\.items).count == 1)
    }

    // MARK: Placeholders

    @Test func unavailableServicesAnswerNotImplementedAndKeepCoreConstants() async {
        let services = M3Services.unavailable(network: .mainnet)
        #expect(services.coinJoin.limits().minimumMixingBalance == Amount(duffs: 140_001))
        #expect(services.governance.parameters().superblockCycle == 16_616)
        #expect(services.masternodes.defaults().coreP2PPort == 9_999)
        do {
            _ = try await services.coinJoin.status(wallet: walletA)
            Issue.record("expected not_implemented")
        } catch {
            #expect(error.code == .notImplemented)
        }
        do {
            try await services.coinJoin.setSalt("zz", wallet: walletA)
            Issue.record("expected invalid_argument")
        } catch {
            #expect(error.code == .invalidArgument)
        }
    }
}
