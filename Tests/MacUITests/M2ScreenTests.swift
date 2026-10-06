// Renders every M2 MacUI screen over the demo services (with the demo M2
// services of DemoEnvironment.makeWithM2) in light and dark, and checks the
// demo data reached the view models. With DWD_WRITE_SCREENSHOTS=1 the PNGs
// go to docs/screenshots/m2/. Tests are named after the checklist ids.
#if os(macOS)
import AppKit
import DashUIMac
import Foundation
import PlatformServices
import SwiftUI
import Testing
import WalletDemo
import WalletFeatures
import WalletRuntime

@testable import MacUI

@MainActor
@Suite(.serialized)
struct M2ScreenTests {
    static let windowSize = CGSize(width: 900, height: 600)

    // MARK: Options (QT-135…141, IOS-011/015/016)

    @Test(arguments: [ColorScheme.light, .dark])
    func QT135_optionsTabs(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let features = try #require(model.features)
        let main = try #require(model.main)
        let options = features.options
        await options.load()
        // macOS has neither start-on-login nor a tray: dash-qt hides Main.
        #expect(!options.tabs.contains(.main))
        #expect(options.tabs.contains(.wallet) && options.tabs.contains(.display))
        for tab in [MacOptionsTab.options(.wallet), .options(.network), .options(.display), .security] {
            let name: String
            switch tab {
            case .options(let item): name = "options-\(item.rawValue)"
            case .security: name = "options-security"
            case .general: name = "options-general"
            }
            try await Self.capture(
                OptionsView(
                    model: model, options: options, settings: main.settings, security: features.security, tab: tab),
                CGSize(width: 600, height: 520), scheme, name)
        }
    }

    @Test func QT136_optionsApplyAndCancel() async throws {
        let model = try await Self.model(.funded, .light)
        let features = try #require(model.features)
        let options = features.options
        await options.load()
        #expect(!options.hasChanges)
        options.wallet.coinControl = true
        options.display.thirdPartyTxURLs = "https://explorer.example/tx/%s"
        #expect(options.hasChanges)
        options.discard()
        #expect(!options.wallet.coinControl)
        options.wallet.coinControl = true
        try await options.apply()
        #expect(features.m2.desktopPreferences.desktop.options.coinControl)
        // Proxy fields stay disabled until the engine supports a proxy.
        #expect(!options.network.isEditable)
    }

    // MARK: Coin control (QT-068…074)

    @Test(arguments: [ColorScheme.light, .dark])
    func QT069_coinSelectionListAndTree(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let coinControl = try #require(model.main?.coinControl)
        // Send reads the selection only with coin control features on.
        let features = try #require(model.features)
        features.options.wallet.coinControl = true
        try await features.options.apply()
        await coinControl.load()
        #expect(!coinControl.coins.isEmpty)
        #expect(coinControl.isAutomatic)
        let first = try #require(coinControl.coins.first(where: coinControl.isSelectable))
        await coinControl.toggle(first.outpoint)
        #expect(coinControl.selected == [first.outpoint])
        #expect(coinControl.summaryText?.quantity == "1")
        coinControl.setMode(.list)
        try await Self.capture(
            CoinControlView(coinControl: coinControl, formatAmount: model.formatAmount, onDone: {}),
            CGSize(width: 900, height: 560), scheme, "coin-control-list")
        coinControl.setMode(.tree)
        try await Self.capture(
            CoinControlView(coinControl: coinControl, formatAmount: model.formatAmount, onDone: {}),
            CGSize(width: 900, height: 560), scheme, "coin-control-tree")
        coinControl.setMode(.list)
        #expect(coinControl.source() == .outpoints([first.outpoint]))
        // The window is still open: Send already pays from the ticked coin.
        #expect(model.main?.send?.source == .outpoints([first.outpoint]))
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func QT068_sendCoinControlPanel(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let features = try #require(model.features)
        let send = try #require(model.main?.send)
        features.options.wallet.coinControl = true
        features.options.wallet.psbtControls = true
        try await features.options.apply()
        await send.coinControl?.load()
        model.main?.selection = .send
        send.entries[0].address = ScreenTests.payTo
        send.entries[0].amountText = "0.1"
        try await ScreenTests.capture(
            ScreenTests.chrome(model, SendView(model: model, send: send)), ScreenTests.mainSize, scheme,
            "send-coin-control", milestone: "m2")
    }

    // MARK: PSBT (QT-076…079)

    @Test(arguments: [ColorScheme.light, .dark])
    func QT077_psbtDialog(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let psbt = try #require(model.features?.psbt)
        #expect(psbt.step == .empty)
        try await Self.capture(PSBTView(psbt: psbt, onClose: {}), CGSize(width: 620, height: 420), scheme, "psbt-empty")
        // Not base64: the dialog says so instead of loading.
        model.features?.m2.clipboard.setString("not a psbt")
        await psbt.loadFromClipboard()
        #expect(psbt.errorMessage == L10n.PSBT.clipboardInvalid)
        // The demo has no PSBT codec: the engine's not_implemented, shown as such.
        model.features?.m2.clipboard.setString(Data("psbt\u{ff}demo".utf8).base64EncodedString())
        await psbt.loadFromClipboard()
        #expect(psbt.step == .empty)
        #expect(psbt.errorMessage != nil)
        try await Self.capture(PSBTView(psbt: psbt, onClose: {}), CGSize(width: 620, height: 420), scheme, "psbt-load-error")
    }

    // MARK: Tools (QT-143…148)

    @Test(arguments: [ColorScheme.light, .dark])
    func QT143_toolsInformation(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let information = try #require(model.features?.information)
        await information.load()
        #expect(!information.sections.isEmpty)
        // SPV cannot know the mempool: the rows say so instead of a number.
        let mempool = try #require(information.sections.first { $0.title == L10n.Tools.memoryPool })
        #expect(mempool.rows.allSatisfy { $0.value == L10n.Tools.none && $0.note == L10n.Tools.requiresFullNode })
        try await Self.capture(InformationView(information: information), Self.windowSize, scheme, "tools-information")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func QT145_toolsConsole(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let console = try #require(model.features?.console)
        console.clear()
        await console.load()
        await console.run(line: "getblockcount")
        #expect(console.entries.contains { $0.kind == .reply })
        #expect(console.history == ["getblockcount"])
        try await Self.capture(ConsoleView(console: console), Self.windowSize, scheme, "tools-console")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func QT147_toolsPeers(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let peers = try #require(model.features?.peers)
        await peers.load()
        #expect(peers.canModerate)
        #expect((peers.peers ?? []).count == 8)
        try await Self.capture(PeersToolView(peers: peers), Self.windowSize, scheme, "tools-peers")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func QT148_toolsRepair(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let repair = try #require(model.features?.repair)
        repair.requestResetChainData()
        #expect(repair.state == .confirming(.resetChainData))
        repair.cancelConfirmation()
        try await Self.capture(RepairView(repair: repair), Self.windowSize, scheme, "tools-repair")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func QT143_toolsWindow(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let features = try #require(model.features)
        model.toolsTab = .information
        try await Self.capture(
            ToolsView(features: features, tab: .constant(.information)), Self.windowSize, scheme, "tools-window")
    }

    // MARK: Transactions (QT-090…092, IOS-027…030)

    @Test(arguments: [ColorScheme.light, .dark])
    func IOS027_historyLayout(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let transactions = try #require(model.main?.transactions)
        model.main?.selection = .transactions
        await transactions.reload()
        await transactions.refreshChips()
        #expect(!transactions.dayGroups.isEmpty)
        try await ScreenTests.capture(
            ScreenTests.chrome(
                model,
                TransactionsView(
                    transactions: transactions, unitName: model.unitName, formatAmount: model.formatAmount,
                    layout: .history)),
            ScreenTests.mainSize, scheme, "transactions-history", milestone: "m2")
        await transactions.selectOnlyChip(.sent)
        #expect(transactions.rows.allSatisfy { $0.amount.duffs <= 0 })
        await transactions.selectAllChips()
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func QT092_transactionDetailsWithActions(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let transactions = try #require(model.main?.transactions)
        await transactions.reload()
        let first = try #require(transactions.rows.first)
        await transactions.select(first.id)
        let detail = try #require(transactions.detail)
        #expect(transactions.detailFields.contains { $0.title == L10n.TransactionsM2.transactionID })
        #expect(transactions.copyRawTransaction() == detail.rawHex)
        #expect(transactions.copyFullDetails(first).contains(first.id.txid) || first.address != nil)
        try await Self.capture(
            TransactionDetailView(detail: detail, transactions: transactions, formatAmount: model.formatAmount, onClose: {}),
            CGSize(width: 620, height: 640), scheme, "transaction-detail")
    }

    @Test func QT091_abandonAsksFirst() async throws {
        let model = try await Self.model(.funded, .light)
        let transactions = try #require(model.main?.transactions)
        await transactions.reload()
        // The first record the engine lets go of.
        for record in transactions.rows {
            await transactions.select(record.id)
            guard transactions.canAbandon else { continue }
            transactions.requestAbandon()
            guard case .confirmingAbandon(let txid, _) = transactions.actionState else {
                Issue.record("abandon did not ask first")
                return
            }
            #expect(txid == record.id.txid)
            await transactions.confirmAbandon()
            #expect(transactions.actionState == .done(L10n.TransactionsM2.abandoned))
            return
        }
        // The demo history has nothing abandonable: the action stays disabled.
        #expect(transactions.actionState == .idle)
    }

    // MARK: Wallets, security (IOS-110, IOS-011…016)

    @Test(arguments: [ColorScheme.light, .dark])
    func IOS110_walletsWindow(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let wallets = try #require(model.features?.wallets)
        await wallets.load()
        #expect(!wallets.wallets.isEmpty)
        #expect(wallets.wallets.allSatisfy { $0.loaded })
        try await Self.capture(
            WalletsView(wallets: wallets, selectedWalletID: model.main?.selectedWalletID),
            CGSize(width: 720, height: 520), scheme, "wallets")
        // Close, then open again (QT-101).
        let id = try #require(wallets.wallets.first?.walletID)
        await wallets.close(id)
        #expect(wallets.wallets.first { $0.walletID == id }?.loaded == false)
        await wallets.open(id)
        #expect(wallets.wallets.first { $0.walletID == id }?.loaded == true)
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func IOS015_securityTab(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.locked, scheme)
        let features = try #require(model.features)
        let main = try #require(model.main)
        let security = features.security
        await security.load()
        #expect(security.vaultStatus?.encrypted == true)
        security.setAutoLock(.fiveMinutes)
        #expect(security.autoLockInterval == .fiveMinutes)
        try await Self.capture(
            SecurityOptionsTab(security: security, settings: main.settings), CGSize(width: 600, height: 520), scheme,
            "security")
    }

    // MARK: Shell (QT-004, QT-005, QT-007, QT-008, QT-153, IOS-005, IOS-025)

    @Test(arguments: [ColorScheme.light, .dark])
    func QT005_splash(_ scheme: ColorScheme) async throws {
        let startup = StartupViewModel(
            startup: SplashStub(), optionsReset: ResetStub(corrupt: []), launchOptions: RuntimeLaunchOptions())
        #expect(startup.stage == .starting)
        #expect(startup.statusText == L10n.Shell.loadingWallets)
        try await Self.capture(SplashView(startup: startup), CGSize(width: 520, height: 360), scheme, "splash")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func QT007_corruptSettings(_ scheme: ColorScheme) async throws {
        let file = URL(fileURLWithPath: "/Users/me/Library/Application Support/org.dashfoundation.DashWallet/settings.json")
        let startup = StartupViewModel(
            startup: SplashStub(), optionsReset: ResetStub(corrupt: [file]), launchOptions: RuntimeLaunchOptions())
        #expect(startup.stage == .settingsUnreadable([file]))
        try await Self.capture(
            SettingsUnreadableView(startup: startup, files: [file]), CGSize(width: 640, height: 360), scheme,
            "settings-unreadable")
        startup.resetSettings()
        #expect(startup.stage == .starting)
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func QT004_dataDirectoryChooser(_ scheme: ColorScheme) async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("dwd-chooser-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        let chooser = DataDirectoryChooserViewModel(
            defaultDirectory: directory, inspector: FileSystemDataDirectoryInspector())
        await chooser.load()
        #expect(chooser.statusText == L10n.Shell.willCreate)
        let model = MacAppModel(chooser: chooser, launch: LaunchOptions()) { (_: URL) throws(ServiceError) -> MacAppServices in
            throw ServiceError(code: .notImplemented)
        }
        try await Self.capture(
            DataDirectoryChooserView(model: model, chooser: chooser), CGSize(width: 640, height: 420), scheme,
            "data-directory")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func QT008_shutdownWindow(_ scheme: ColorScheme) async throws {
        let coordinator = SlowShutdown()
        let shutdown = ShutdownViewModel(coordinator: coordinator)
        let quitting = Task { await shutdown.quit() }
        try await ScreenTests.settle { shutdown.isVisible }
        #expect(!shutdown.canClose)
        try await Self.capture(ShutdownView(shutdown: shutdown), CGSize(width: 380, height: 180), scheme, "shutdown")
        coordinator.finish()
        await quitting.value
        #expect(shutdown.state == .finished)
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func QT153_aboutAndCommandLine(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let about = try #require(model.features?.about)
        await about.load()
        #expect(about.networkName != nil)
        #expect(about.commandLineOptions.contains { $0.name.contains("windowtitle") })
        try await Self.capture(AboutView(about: about), CGSize(width: 480, height: 420), scheme, "about")
        try await Self.capture(
            CommandLineOptionsView(about: about), CGSize(width: 560, height: 420), scheme, "command-line-options")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func IOS025_overviewShortcutsAndReminder(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let features = try #require(model.features)
        let main = try #require(model.main)
        let home = try #require(main.home)
        let network = try #require(main.network)
        let bar = features.shortcutBar(for: network)
        bar.start()
        #expect(bar.slots.count == ShortcutBarViewModel.slotCount)
        try await ScreenTests.capture(
            ScreenTests.chrome(model, OverviewPage(model: model, main: main, home: home)), ScreenTests.mainSize, scheme, "overview-shortcuts", milestone: "m2")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func IOS117_menuBarCompanion(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let companion = try #require(model.features?.companion)
        await companion.load()
        companion.setRequestAmount("0.5")
        #expect(companion.requestURI?.contains("amount=0.5") == true)
        try await Self.capture(MenuBarContentView(model: model), CGSize(width: 280, height: 720), scheme, "menu-bar")
    }

    // MARK: Behaviour without rendering

    /// QT-015…018: dash-qt's menus, with the app-menu items moved out.
    @Test func QT015_menusFollowTheShell() async throws {
        let model = try await Self.model(.funded, .light)
        let shell = try #require(model.shell)
        let titles = shell.menus.map(\.title)
        #expect(titles == [L10n.Shell.fileMenu, L10n.Shell.settingsMenu, L10n.Shell.windowMenu, L10n.Shell.helpMenu])
        let file = try #require(shell.menus.first)
        let open = try #require(file.items.first { $0.id == "file.open" })
        #expect(open.children.count == 1)
        #expect(!shell.isEnabled(.migrateWallet))
        #expect(shell.isEnabled(.backupWallet))
        // Window ▸ Console opens the Tools window on its Console tab.
        await model.performNow(.tools(.console))
        #expect(model.toolsTab == .console)
        await model.performNow(.unlockWallet)
        #expect(!model.isUnlockPresented, "unlock is disabled for an unencrypted vault")
        await model.performNow(.backupWallet)
        #expect(model.isBackupPresented)
        await model.performNow(.closeWallet)
        #expect(shell.confirmation != nil)
        shell.cancelConfirmation()
    }

    /// QT-011: the window title carries `--windowtitle` and the network tag.
    @Test func QT011_windowTitle() async throws {
        let launch = LaunchOptions.parse(["app", "--demo", "-windowtitle=Lab"])
        #expect(launch.argumentError == nil)
        #expect(launch.runtime.windowTitleSuffix == "Lab")
        let (env, m2) = DemoEnvironment.makeWithM2(scenario: .funded, launchOptions: launch.runtime)
        let model = MacAppModel(environment: env, m2: m2, launch: launch)
        await model.start()
        #expect(model.windowTitle.hasPrefix("Dash Wallet - Lab - "))
        #expect(model.windowTitle.contains("[testnet]"))
    }

    /// QT-006: dash-qt's GUI options next to the app's own switches.
    @Test func QT006_launchOptions() {
        let options = LaunchOptions.parse(["app", "--demo", "-min", "-splash=0", "-choosedatadir", "--testnet"])
        #expect(options.argumentError == nil)
        #expect(options.runtime.startMinimized)
        #expect(!options.runtime.showSplash)
        #expect(options.runtime.chooseDataDirectory)
        #expect(options.network == .testnet)
        // `--datadir <path>` stays the app's own switch.
        let data = LaunchOptions.parse(["app", "--datadir", "/tmp/dwd"])
        #expect(data.dataDirectory?.path == "/tmp/dwd")
        #expect(data.argumentError == nil)
        let help = LaunchOptions.parse(["app", "-help"])
        #expect(help.runtime.showHelp)
    }

    /// QT-029: the Dock menu's entries.
    @Test func QT029_dockMenu() async throws {
        let model = try await Self.model(.funded, .light)
        let titles = model.dockMenuItems.filter { !$0.isSeparator }.map(\.title)
        #expect(titles.first == MacStrings.Dock.showHide)
        #expect(titles.contains(L10n.Shell.signMessage))
        #expect(titles.contains(L10n.Shell.console))
    }

    /// QT-095/096: the address book's CSV and QR code.
    @Test func QT096_addressBookCSVAndQR() async throws {
        let model = try await Self.model(.funded, .light)
        let book = try #require(model.makeAddressBook(purpose: .receive))
        await book.load()
        let entry = try #require(book.entries.first)
        #expect(book.exportCSV().contains(entry.address))
        book.showQR(for: entry)
        #expect(book.qr != nil)
        #expect(book.qrURI?.hasPrefix("dash:\(entry.address)") == true)
    }

    // MARK: Helpers

    static func model(_ scenario: DemoScenario, _ scheme: ColorScheme) async throws -> MacAppModel {
        let launch = LaunchOptions(demoScenario: scenario, appearance: scheme == .dark ? .dark : .light, menuBarExtra: false)
        let (env, m2) = DemoEnvironment.makeWithM2(scenario: scenario, launchOptions: launch.runtime)
        let model = MacAppModel(environment: env, m2: m2, launch: launch)
        await model.start()
        await model.main?.receive?.load()
        return model
    }

    @discardableResult
    static func capture<V: View>(_ view: V, _ size: CGSize, _ scheme: ColorScheme, _ name: String) async throws
        -> NSBitmapImageRep
    {
        try await ScreenTests.capture(view, size, scheme, name, milestone: "m2")
    }
}

/// A startup halfway through: wallets loading.
@MainActor
private final class SplashStub: StartupProgressing {
    var phase: StartupPhase = .loadingWallets
    var progress: Double = 0.4
    func changes() -> AsyncStream<StartupPhase> { AsyncStream { $0.yield(.loadingWallets) } }
    func requestEmergencyQuit() {}
}

@MainActor
private final class ResetStub: OptionsResetting {
    let recoveredFromCorruption: [URL]
    init(corrupt: [URL]) { recoveredFromCorruption = corrupt }
    func resetOptions() throws(ServiceError) -> [URL] { recoveredFromCorruption }
}

/// A shutdown that waits until the test lets it finish.
@MainActor
private final class SlowShutdown: ShutdownCoordinating {
    private(set) var isShuttingDown = false
    private var continuation: CheckedContinuation<Void, Never>?

    func shutdown() async {
        isShuttingDown = true
        await withCheckedContinuation { continuation = $0 }
        isShuttingDown = false
    }

    func finish() {
        continuation?.resume()
        continuation = nil
    }
}
#endif
