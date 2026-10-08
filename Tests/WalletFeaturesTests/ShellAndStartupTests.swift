// App shell, startup, data-directory chooser and shutdown (QT-004, QT-005,
// QT-007, QT-008, QT-011, QT-012, QT-015…018, QT-021, QT-022, QT-153).
import Foundation
import PlatformServices
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Shell and startup view models")
struct ShellAndStartupTests {
    let world = FakeWorld()
    let m2 = FakeM2World()

    func makeShell(features: FeatureFlags = .m1) -> ShellModel {
        ShellModel(env: world.environment(), m2: m2.services, features: features)
    }

    func item(_ shell: ShellModel, _ id: String) -> MenuItemModel? {
        shell.menus.flatMap(\.items).first { $0.id == id }
    }

    // MARK: Window title, sidebar, icons

    @Test func QT011_windowTitleAppendsWindowTitleOptionWalletAndNetwork() async {
        m2.launchOptions = LaunchOptions(windowTitleSuffix: "Office")
        let shell = makeShell()
        await shell.refresh()
        #expect(shell.windowTitle == "Dash Wallet - Office - Main - [testnet]")
    }

    @Test func QT011_geometryIsSavedPerWindow() {
        let shell = makeShell()
        let frame = WindowGeometry(x: 10, y: 20, width: 980, height: 640)
        shell.saveGeometry(frame, for: .main)
        #expect(shell.geometry(for: .main) == frame)
        #expect(shell.geometry(for: .tools) == nil)
        #expect(m2.desktopPreferences.desktop.windows["main"] == frame)
        shell.saveGeometry(frame, for: .main)
        #expect(m2.desktopPreferences.updates == 1)
    }

    @Test func QT012_sectionsFollowFeatures() async {
        #expect(makeShell(features: .m1).sections == [.overview, .send, .receive, .transactions])
        let shell = makeShell(features: FeatureFlags(contacts: true))
        #expect(shell.sections == [.overview, .send, .receive, .transactions, .contacts])
        await shell.selectShortcut(5)
        #expect(shell.selection == .contacts)
    }

    @Test func QT021_QT022_hdIconAndLockIcon() async {
        let shell = makeShell()
        await shell.start()
        #expect(shell.hdIconVisible)
        #expect(shell.hdTooltip == "HD key generation is enabled")
        #expect(shell.lockIcon == nil)  // unencrypted
        world.auth.publish(.locked)
        await eventually { shell.lockIcon == .locked }
        #expect(shell.lockIcon?.tooltip == "Wallet is encrypted and currently locked")
        world.auth.publish(.unlockedMixingOnly)
        await eventually { shell.lockIcon == .unlockedMixingOnly }
        shell.stop()
    }

    // MARK: Menus

    @Test func QT015_fileMenuHasDashQtEntriesAndOpenWalletSubmenu() async {
        m2.walletLifecycle.states.withLock {
            $0 = [
                WalletLoadState(walletID: walletA, name: "Main", loaded: true, loadOnStartup: true, watchOnly: false),
                WalletLoadState(walletID: walletB, name: "Savings", loaded: false, loadOnStartup: false, watchOnly: false),
            ]
        }
        let shell = makeShell()
        await shell.refresh()
        let file = shell.menus[0]
        #expect(file.title == "File")
        #expect(file.items.filter { !$0.isSeparator }.map(\.title) == [
            "Create Wallet…", "Open Wallet", "Close Wallet…", "Close All Wallets…", "Migrate Wallet", "Backup Wallet…",
            "Restore Wallet…", "Open URI…", "Sign message…", "Verify message…", "Load PSBT from file…",
            "Load PSBT from clipboard…", "Open debug log file", "Open wallet configuration file",
            "Show Automatic Backups", "Exit",
        ])
        let open = item(shell, "file.open")!
        #expect(open.children.map(\.title) == ["Main", "Savings"])
        // Loaded wallets are disabled in the submenu.
        #expect(open.children.map(\.isEnabled) == [false, true])
        // Not-ready entries are present, disabled and say why.
        let migrate = item(shell, "file.migrate")!
        #expect(!migrate.isEnabled && migrate.helpText == L10n.Shell.migrateWalletUnavailable)
        #expect(item(shell, "file.exit")?.shortcut == KeyShortcut("q"))
    }

    @Test func QT015_openWalletWithNoneRegisteredShowsNoWalletsAvailable() async {
        m2.walletLifecycle.states.withLock { $0 = [] }
        let shell = makeShell()
        await shell.refresh()
        #expect(item(shell, "file.open")?.children.map(\.title) == ["No wallets available"])
    }

    @Test func QT101_openAndCloseWalletFromTheMenu() async {
        m2.walletLifecycle.states.withLock {
            $0 = [WalletLoadState(walletID: walletB, name: "Savings", loaded: false, loadOnStartup: false, watchOnly: false)]
        }
        let shell = makeShell()
        await shell.refresh()
        await shell.perform(.openWallet(walletB))
        #expect(m2.walletLifecycle.states.current?.first?.loaded == true)
        world.walletState.selectedWalletID = walletA
        await shell.perform(.closeWallet)
        #expect(shell.confirmation == .closeWallet(walletA, name: "Main"))
        #expect(shell.confirmation?.message == "Are you sure you wish to close the wallet Main?")
        m2.walletLifecycle.states.withLock {
            $0 = [WalletLoadState(walletID: walletA, name: "Main", loaded: true, loadOnStartup: true, watchOnly: false)]
        }
        await shell.confirm()
        // Close also drops the wallet from load-on-startup (dash-qt).
        #expect(m2.walletLifecycle.states.current?.first?.loaded == false)
        #expect(m2.walletLifecycle.states.current?.first?.loadOnStartup == false)
    }

    @Test func QT101_openWalletWhileEngineStubIsNotImplementedListsNothing() async {
        let shell = makeShell()
        await shell.refresh()
        #expect(shell.loadStates.isEmpty)
        #expect(shell.errorMessage == nil)
    }

    @Test func QT016_settingsMenuFollowsTheLockState() async {
        let shell = makeShell()
        await shell.refresh()
        #expect(item(shell, "settings.encrypt")?.isEnabled == true)
        #expect(item(shell, "settings.changePassphrase")?.isEnabled == false)
        #expect(item(shell, "settings.unlock") == nil && item(shell, "settings.lock") == nil)
        world.auth.lockState = .locked
        await shell.refresh()
        #expect(item(shell, "settings.encrypt")?.isEnabled == false)
        #expect(item(shell, "settings.unlock") != nil && item(shell, "settings.lock") == nil)
        world.auth.lockState = .unlocked
        await shell.refresh()
        #expect(item(shell, "settings.unlock") == nil && item(shell, "settings.lock") != nil)
        await shell.perform(.lockWallet)
        #expect(world.auth.lockCount == 1)
        #expect(item(shell, "settings.discreet")?.shortcut == KeyShortcut("d", [.command, .shift]))
    }

    @Test func QT016_discreetModeTogglesAndIsChecked() async {
        let shell = makeShell()
        await shell.perform(.toggleDiscreetMode)
        #expect(world.settings.display.hideBalances)
        #expect(item(shell, "settings.discreet")?.isChecked == true)
    }

    @Test func QT017_windowMenuHasToolsTabsWithShortcuts() {
        let shell = makeShell()
        let window = shell.menus[2]
        #expect(window.items.filter { !$0.isSeparator }.map(\.title) == [
            "Minimize", "Sending addresses", "Receiving addresses", "Information", "Console", "Network Traffic", "Peers",
            "Repair",
        ])
        #expect(item(shell, "window.console")?.shortcut == KeyShortcut("c", [.command, .shift]))
        #expect(item(shell, "window.traffic")?.isEnabled == false)
    }

    /// M3: the item needs the CoinJoin feature and "Enable CoinJoin features"
    /// (see ShellM3Tests for the engine-backed case).
    @Test func QT018_helpMenuShowsCoinJoinInformationOnlyWithCoinJoin() {
        #expect(makeShell().menus[3].items.map(\.title) == ["Command-line options", "About Dash Wallet"])
        let withCoinJoin = makeShell(features: FeatureFlags(coinJoin: true))
        #expect(withCoinJoin.menus[3].items.map(\.title) == ["Command-line options", "About Dash Wallet"])
        withCoinJoin.coinJoinOptionChanged(enabled: true)
        #expect(withCoinJoin.menus[3].items.map(\.title) == [
            "Command-line options", "CoinJoin information", "About Dash Wallet",
        ])
    }

    @Test func QT015_uiCommandsBecomePresentationsAndDisabledOnesDoNothing() async {
        let shell = makeShell()
        await shell.perform(.tools(.console))
        #expect(shell.pendingPresentation == .tools(.console))
        shell.presentationHandled()
        await shell.perform(.migrateWallet)
        #expect(shell.pendingPresentation == nil)
    }

    @Test func QT116_showAutomaticBackupsRevealsTheBackupFolder() async {
        let folder = URL(fileURLWithPath: "/data/testnet/backups")
        m2.backups.policyValue.withLock { $0 = BackupPolicy(keep: 10, directory: folder) }
        let shell = makeShell()
        await shell.perform(.showAutomaticBackups)
        #expect(m2.fileRevealer.revealed.current == [folder])
    }

    @Test func QT116_showAutomaticBackupsWhileNotImplementedSaysSo() async {
        let shell = makeShell()
        await shell.perform(.showAutomaticBackups)
        #expect(shell.errorMessage == L10n.Common.notAvailableYet)
    }

    // MARK: Startup (QT-005, QT-007)

    @Test func QT005_splashFollowsPhasesAndNeverMovesBackwards() async {
        let model = StartupViewModel(startup: m2.startup, optionsReset: m2.optionsReset, launchOptions: LaunchOptions())
        model.start()
        m2.startup.publish(.openingNetwork(.testnet), progress: 0.3)
        await eventually { model.phase == .openingNetwork(.testnet) }
        #expect(model.statusText == "Opening Testnet…")
        m2.startup.publish(.loadingWallets, progress: 0.2)
        await eventually { model.phase == .loadingWallets }
        #expect(model.progress == 0.3)
        m2.startup.publish(.ready, progress: 1)
        await eventually { model.stage == .ready }
        model.stop()
    }

    @Test func QT005_qQuitsDuringStartup() {
        let model = StartupViewModel(startup: m2.startup, optionsReset: m2.optionsReset, launchOptions: LaunchOptions())
        model.quit()
        model.quit()
        #expect(model.stage == .quitting)
        #expect(m2.startup.quitRequests == 1)
    }

    @Test func QT006_minHidesTheSplash() {
        let model = StartupViewModel(
            startup: m2.startup, optionsReset: m2.optionsReset, launchOptions: LaunchOptions(startMinimized: true))
        #expect(!model.showsSplash)
    }

    @Test func QT007_corruptSettingsAskResetOrAbort() {
        let bad = URL(fileURLWithPath: "/data/settings.json")
        m2.optionsReset.recoveredFromCorruption = [bad]
        let model = StartupViewModel(startup: m2.startup, optionsReset: m2.optionsReset, launchOptions: LaunchOptions())
        #expect(model.stage == .settingsUnreadable([bad]))
        #expect(L10n.Shell.settingsUnreadable == "Settings file could not be read.")
        model.resetSettings()
        #expect(model.stage == .starting)

        let aborting = StartupViewModel(startup: m2.startup, optionsReset: m2.optionsReset, launchOptions: LaunchOptions())
        aborting.abortSettings()
        #expect(aborting.stage == .quitting)
        // Abort writes nothing.
        #expect(m2.optionsReset.resets == 0)
    }

    // MARK: Data directory (QT-004)

    @Test func QT004_chooserShowsDashQtStatusAndCreatesOnOK() async {
        let defaultDirectory = URL(fileURLWithPath: "/home/u/.dashwallet")
        let model = DataDirectoryChooserViewModel(defaultDirectory: defaultDirectory, inspector: m2.dataDirectories)
        await model.load()
        #expect(model.statusText == "A new data directory will be created.")
        #expect(model.freeSpaceText == "123 GB of space available")
        let file = URL(fileURLWithPath: "/home/u/file.txt")
        m2.dataDirectories.states.withLock { $0[file] = .notADirectory }
        await model.setCustomDirectory(file)
        #expect(model.statusText == "Path already exists, and is not a directory.")
        #expect(!model.canAccept)
        await model.choose(.defaultDirectory)
        await model.accept()
        #expect(model.outcome == .accepted(defaultDirectory))
        #expect(m2.dataDirectories.created.current == [defaultDirectory])
    }

    @Test func QT004_cancelQuits() {
        let model = DataDirectoryChooserViewModel(defaultDirectory: URL(fileURLWithPath: "/x"), inspector: m2.dataDirectories)
        model.cancel()
        #expect(model.outcome == .cancelled)
    }

    // MARK: Shutdown (QT-008)

    @Test func QT008_shutdownWindowCannotCloseWhileRunning() async {
        let gate = Gate()
        m2.shutdown.gate = gate
        let model = ShutdownViewModel(coordinator: m2.shutdown)
        let task = Task { await model.quit() }
        await eventually { model.isVisible }
        #expect(!model.canClose)
        #expect(model.title == "Dash Wallet is shutting down…")
        await model.quit()  // idempotent
        gate.open()
        await task.value
        #expect(model.state == .finished)
        #expect(m2.shutdown.shutdowns == 1)
    }

    // MARK: About (QT-153, IOS-107, IOS-112)

    @Test func QT153_commandLineOptionsHaveDashQtDescriptions() {
        let about = AboutViewModel(env: world.environment(), m2: m2.services)
        let options = about.commandLineOptions
        #expect(options.first == CommandLineOption(name: "--min", text: "Start minimized"))
        #expect(options.contains { $0.name == "--windowtitle=<name>" && $0.text.contains("appended") })
    }

    @Test func IOS107_aboutShowsVersionNetworkAndDataDirectory() async {
        m2.nodeInformation.info.withLock { $0 = FakeNodeInformation.sample() }
        let about = AboutViewModel(env: world.environment(), m2: m2.services)
        await about.load()
        #expect(about.versionText == "Version v0.2.0")
        #expect(about.networkName == "Testnet")
        #expect(about.dataDirectory == URL(fileURLWithPath: "/data"))
    }

    @Test func IOS112_logExportIsAStateMachineAndReportsNotImplemented() async {
        let about = AboutViewModel(env: world.environment(), m2: m2.services)
        let file = URL(fileURLWithPath: "/tmp/logs.zip")
        await about.exportLogs(to: file)
        #expect(about.logExport == .failed(L10n.Common.notAvailableYet))
        m2.logs.configured.withLock { $0 = true }
        await about.exportLogs(to: file)
        #expect(about.logExport == .exported(file))
    }
}
