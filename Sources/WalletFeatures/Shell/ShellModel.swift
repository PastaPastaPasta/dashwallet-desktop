// The app shell: window title, sidebar sections, dash-qt's menus, status
// icons and window geometry (QT-011, QT-012, QT-015…018, QT-021, QT-022).
// Menu, tray and Dock items all route through `perform(_:)`.
import Foundation
import Observation
import PlatformServices
import WalletRuntime

/// The Tools window tabs (research 02 §14).
public enum ToolsTab: String, Sendable, Hashable, CaseIterable {
    case information, console, networkTraffic, peers, repair

    public var title: String {
        switch self {
        case .information: L10n.Shell.information
        case .console: L10n.Shell.console
        case .networkTraffic: L10n.Shell.networkTraffic
        case .peers: L10n.Shell.peers
        case .repair: L10n.Shell.repair
        }
    }
}

/// Windows whose geometry the shell remembers (QT-011).
public enum ShellWindow: String, Sendable, Hashable, CaseIterable {
    case main, tools, options, addressBook, psbt
}

/// Every dash-qt menu entry (research 02 §2.1), plus the sidebar and the
/// tray's Show/Hide.
public enum ShellCommand: Sendable, Hashable {
    // File
    case createWallet, openWallet(WalletID), closeWallet, closeAllWallets, migrateWallet, backupWallet,
        restoreWallet, openURI, signMessage, verifyMessage, loadPSBTFromFile, loadPSBTFromClipboard, openDebugLog,
        openConfigurationFile, showAutomaticBackups, exit
    // Settings
    case encryptWallet, changePassphrase, showRecoveryPhrase, unlockWallet, lockWallet, toggleDiscreetMode, options
    // Window
    case minimize, sendingAddresses, receivingAddresses, tools(ToolsTab)
    // Help
    case commandLineOptions, coinJoinInformation, about
    // Sidebar, tray
    case section(SidebarItem), showHideWindow
}

/// Modifier keys of a shortcut; `.command` is Cmd on macOS and Ctrl elsewhere.
public enum KeyModifier: Sendable, Hashable, CaseIterable {
    case command, shift, option
}

public struct KeyShortcut: Sendable, Hashable {
    public let key: String
    public let modifiers: Set<KeyModifier>

    public init(_ key: String, _ modifiers: Set<KeyModifier> = [.command]) {
        self.key = key
        self.modifiers = modifiers
    }
}

public struct MenuItemModel: Sendable, Hashable, Identifiable {
    public let id: String
    public let title: String
    public let command: ShellCommand?
    public let shortcut: KeyShortcut?
    public let isEnabled: Bool
    /// `nil` for items that are not checkable.
    public let isChecked: Bool?
    /// dash-qt's status tip, or why the item is disabled.
    public let helpText: String?
    public let children: [MenuItemModel]
    public let isSeparator: Bool
    /// Unlock items: which unlock they open (QT-016 "Unlock Wallet for
    /// mixing only"); run them with `ShellModel.perform(item:)`.
    public let unlockScope: UnlockScope?

    public init(
        id: String, title: String, command: ShellCommand?, shortcut: KeyShortcut? = nil, isEnabled: Bool = true,
        isChecked: Bool? = nil, helpText: String? = nil, children: [MenuItemModel] = [], isSeparator: Bool = false,
        unlockScope: UnlockScope? = nil
    ) {
        self.id = id
        self.title = title
        self.command = command
        self.shortcut = shortcut
        self.isEnabled = isEnabled
        self.isChecked = isChecked
        self.helpText = helpText
        self.children = children
        self.isSeparator = isSeparator
        self.unlockScope = unlockScope
    }

    static func separator(_ id: String) -> MenuItemModel {
        MenuItemModel(id: id, title: "", command: nil, isEnabled: false, isSeparator: true)
    }
}

public enum MenuID: String, Sendable, Hashable {
    case file, settings, window, help
}

public struct MenuModel: Sendable, Hashable, Identifiable {
    public let id: MenuID
    public let title: String
    public let items: [MenuItemModel]
}

/// The status-bar lock icon (QT-022). An unencrypted vault and a vault
/// without keys show none.
public enum LockIcon: Sendable, Hashable {
    case unlocked, unlockedMixingOnly, locked

    /// dash-qt's orange open lock: unlocked for mixing only (QT-022).
    public var isMixingOnly: Bool { self == .unlockedMixingOnly }

    public var tooltip: String {
        switch self {
        case .unlocked: L10n.Shell.unlockedTooltip
        case .unlockedMixingOnly: L10n.Shell.mixingOnlyTooltip
        case .locked: L10n.Shell.lockedTooltip
        }
    }
}

/// A question the shell asks before acting (dash-qt confirm boxes).
public enum ShellConfirmation: Sendable, Hashable {
    case closeWallet(WalletID, name: String)
    case closeAllWallets

    public var title: String {
        switch self {
        case .closeWallet: L10n.Shell.closeWalletTitle
        case .closeAllWallets: L10n.Shell.closeAllWalletsTitle
        }
    }

    public var message: String {
        switch self {
        case .closeWallet(_, let name): L10n.Shell.closeWalletQuestion(name)
        case .closeAllWallets: L10n.Shell.closeAllWalletsQuestion
        }
    }
}

@MainActor
@Observable
public final class ShellModel {
    public let features: FeatureFlags
    public private(set) var network: DashNetwork?
    public private(set) var wallets: [WalletInfo]?
    public private(set) var selectedWalletID: WalletID?
    public private(set) var loadStates: [WalletLoadState] = []
    public private(set) var lockState: VaultLockState?
    public private(set) var discreet: Bool
    public private(set) var selection: SidebarItem = .overview
    /// A command the UI performs (opens a window, sheet or dialog, quits).
    /// The UI clears it with `presentationHandled()`.
    public private(set) var pendingPresentation: ShellCommand?
    public private(set) var confirmation: ShellConfirmation?
    public private(set) var errorMessage: String?
    /// Options ▸ Wallet ▸ "Enable CoinJoin features" (engine setting).
    public private(set) var coinJoinEnabled = false
    /// The unlock the pending `.unlockWallet` presentation asks for: full,
    /// or for mixing only (QT-016, QT-112).
    public private(set) var pendingUnlockScope: UnlockScope = .full

    /// CoinJoin is on: the feature is built in and enabled in the options.
    public var coinJoinOn: Bool { features.coinJoin && coinJoinEnabled }

    /// `Dash Wallet - <--windowtitle> - <wallet> - [network]` (QT-011).
    public var windowTitle: String {
        var parts = [L10n.Navigation.appName]
        if let suffix = launchOptions.windowTitleSuffix, !suffix.isEmpty { parts.append(suffix) }
        if let wallet = selectedWallet, !wallet.name.isEmpty { parts.append(wallet.name) }
        if let tag = network.flatMap(MainViewModel.networkTag) { parts.append(tag) }
        return parts.joined(separator: " - ")
    }

    /// Visible sidebar sections in order; Cmd/Alt+1…N follow it (QT-012).
    public var sections: [SidebarItem] {
        SidebarItem.visible(with: features).filter { item in
            item != .coinJoin || coinJoinEnabled
        }
    }

    /// HD icon for HD wallets (QT-021).
    public var hdIconVisible: Bool { selectedWallet?.hd ?? false }
    public var hdTooltip: String { L10n.Shell.hdEnabled }

    /// `nil` while unencrypted, without keys or unknown (QT-022).
    public var lockIcon: LockIcon? {
        switch lockState {
        case .locked: .locked
        case .unlocked: .unlocked
        case .unlockedMixingOnly: .unlockedMixingOnly
        case .noVault, .noKeys, .unencrypted, nil: nil
        }
    }

    public var menus: [MenuModel] { [fileMenu, settingsMenu, windowMenu, helpMenu] }

    private var selectedWallet: WalletInfo? {
        guard let id = selectedWalletID else { return nil }
        return wallets?.first { $0.id == id }
    }

    private var hasWallet: Bool { !(wallets ?? []).isEmpty }

    private let walletState: any WalletStateProviding
    private let auth: any AuthenticationGating
    private let settings: any SettingsProviding
    private let host: any WalletHosting
    private let walletLifecycle: any WalletLifecycleManaging
    private let backups: any BackupProviding
    private let fileRevealer: any FileRevealing
    private let desktopPreferences: any DesktopPreferencesStoring
    private let launchOptions: LaunchOptions
    private let coinJoin: (any CoinJoinControlling)?
    private let platform: DesktopPlatform
    private var tasks: [Task<Void, Never>] = []

    public init(
        walletState: any WalletStateProviding, auth: any AuthenticationGating, settings: any SettingsProviding,
        host: any WalletHosting, walletLifecycle: any WalletLifecycleManaging, backups: any BackupProviding,
        fileRevealer: any FileRevealing, desktopPreferences: any DesktopPreferencesStoring,
        launchOptions: LaunchOptions, features: FeatureFlags = .m1, coinJoin: (any CoinJoinControlling)? = nil,
        platform: DesktopPlatform = .current
    ) {
        self.walletState = walletState
        self.auth = auth
        self.settings = settings
        self.host = host
        self.walletLifecycle = walletLifecycle
        self.backups = backups
        self.fileRevealer = fileRevealer
        self.desktopPreferences = desktopPreferences
        self.launchOptions = launchOptions
        self.features = features
        self.coinJoin = coinJoin
        self.platform = platform
        self.discreet = settings.display.hideBalances
        self.lockState = auth.lockState
        self.wallets = walletState.wallets
        self.selectedWalletID = walletState.selectedWalletID
    }

    public convenience init(
        env: AppEnvironment, m2: M2Services, features: FeatureFlags = .m1, m3: M3Services? = nil
    ) {
        self.init(
            walletState: env.walletState, auth: env.auth, settings: env.settings, host: env.host,
            walletLifecycle: m2.walletLifecycle, backups: m2.backups, fileRevealer: m2.fileRevealer,
            desktopPreferences: m2.desktopPreferences, launchOptions: m2.launchOptions, features: features,
            coinJoin: m3?.coinJoin, platform: m2.platform)
    }

    // MARK: Observation

    /// Loads the state the menus need and follows wallet, lock, display and
    /// load-state changes until `stop()`.
    public func start() async {
        stop()
        await refresh()
        let walletChanges = walletState.changes()
        let lockChanges = auth.lockStateChanges()
        let displayChanges = settings.changes()
        let loadChanges = walletLifecycle.loadStateChanges()
        tasks.append(Task { [weak self] in
            for await _ in walletChanges {
                guard let self else { return }
                self.wallets = self.walletState.wallets
                self.selectedWalletID = self.walletState.selectedWalletID
            }
        })
        tasks.append(Task { [weak self] in
            for await state in lockChanges {
                self?.lockState = state
            }
        })
        tasks.append(Task { [weak self] in
            for await display in displayChanges {
                self?.discreet = display.hideBalances
            }
        })
        tasks.append(Task { [weak self] in
            for await _ in loadChanges {
                await self?.reloadLoadStates()
            }
        })
    }

    public func stop() {
        tasks.forEach { $0.cancel() }
        tasks = []
    }

    public func refresh() async {
        network = await host.activeNetwork
        wallets = walletState.wallets
        selectedWalletID = walletState.selectedWalletID
        lockState = auth.lockState
        discreet = settings.display.hideBalances
        await reloadLoadStates()
        await reloadCoinJoinEnabled()
        if !sections.contains(selection) { selection = .overview }
    }

    /// Reads "Enable CoinJoin features"; an engine without CoinJoin keeps it off.
    private func reloadCoinJoinEnabled() async {
        guard features.coinJoin, let coinJoin else {
            coinJoinEnabled = false
            return
        }
        coinJoinEnabled = (try? await coinJoin.settings().enabled) ?? false
    }

    /// Options OK changed "Enable CoinJoin features": the CoinJoin section,
    /// tray entry and Help item follow it (dash-qt).
    public func coinJoinOptionChanged(enabled: Bool) {
        coinJoinEnabled = enabled
        if !sections.contains(selection) { selection = .overview }
    }

    // MARK: Commands

    /// Runs a menu, tray or Dock command. Commands that open UI become
    /// `pendingPresentation`; disabled ones do nothing.
    public func perform(_ command: ShellCommand) async {
        guard isEnabled(command) else { return }
        errorMessage = nil
        switch command {
        case .section(let item):
            guard sections.contains(item) else { return }
            selection = item
        case .openWallet(let id):
            await run { () async throws(ServiceError) in try await self.walletLifecycle.load(id) }
        case .closeWallet:
            guard let wallet = selectedWallet else { return }
            confirmation = .closeWallet(wallet.id, name: wallet.name)
        case .closeAllWallets:
            confirmation = .closeAllWallets
        case .lockWallet:
            await run { () async throws(ServiceError) in try await self.auth.lock() }
        case .unlockWallet:
            pendingUnlockScope = .full
            pendingPresentation = command
        case .toggleDiscreetMode:
            var display = settings.display
            display.hideBalances.toggle()
            do {
                try settings.update(display)
                discreet = display.hideBalances
            } catch {
                errorMessage = L10n.Settings.settingsNotSaved
            }
        case .showAutomaticBackups:
            await revealBackups()
        default:
            pendingPresentation = command
        }
    }

    public func presentationHandled() {
        pendingPresentation = nil
        pendingUnlockScope = .full
    }

    /// Runs a menu item; unlock items carry their scope (QT-016 "Unlock
    /// Wallet for mixing only" presents `.unlockWallet` with
    /// `pendingUnlockScope == .mixingOnly`).
    public func perform(item: MenuItemModel) async {
        guard let command = item.command else { return }
        await perform(command)
        if command == .unlockWallet, pendingPresentation == .unlockWallet, let scope = item.unlockScope {
            pendingUnlockScope = scope
        }
    }

    /// Answers `confirmation` with Yes. Closing a wallet also drops it from
    /// the load-on-startup list (dash-qt).
    public func confirm() async {
        guard let confirmation else { return }
        self.confirmation = nil
        switch confirmation {
        case .closeWallet(let id, _):
            await close([id])
        case .closeAllWallets:
            await close(loadStates.filter(\.loaded).map(\.walletID))
        }
    }

    public func cancelConfirmation() {
        confirmation = nil
    }

    /// Cmd/Alt+N, numbered by the visible sections (QT-012).
    public func selectShortcut(_ number: Int) async {
        let items = sections
        guard (1...max(items.count, 1)).contains(number), number <= items.count else { return }
        await perform(.section(items[number - 1]))
    }

    /// Re-checks the selection after an option hid its section: a hidden
    /// active tab jumps to Overview (dash-qt).
    public func optionsChanged() {
        if !sections.contains(selection) { selection = .overview }
    }

    // MARK: Window geometry

    public func geometry(for window: ShellWindow) -> WindowGeometry? {
        desktopPreferences.desktop.windows[window.rawValue]
    }

    public func saveGeometry(_ geometry: WindowGeometry, for window: ShellWindow) {
        var stored = desktopPreferences.desktop
        guard stored.windows[window.rawValue] != geometry else { return }
        stored.windows[window.rawValue] = geometry
        do {
            try desktopPreferences.update(stored)
        } catch {
            errorMessage = L10n.Settings.settingsNotSaved
        }
    }

    // MARK: Menus

    /// Whether `command` can run now (the menu enablement rules of research 02 §2.1).
    public func isEnabled(_ command: ShellCommand) -> Bool {
        switch command {
        case .closeWallet, .backupWallet, .openURI, .signMessage, .verifyMessage, .sendingAddresses,
            .receivingAddresses:
            hasWallet && selectedWalletID != nil
        case .closeAllWallets:
            hasWallet
        case .openWallet(let id):
            loadStates.contains { $0.walletID == id && !$0.loaded }
        case .migrateWallet, .openDebugLog, .openConfigurationFile, .tools(.networkTraffic):
            false
        case .coinJoinInformation:
            coinJoinOn
        case .encryptWallet:
            lockState == .unencrypted
        case .changePassphrase:
            lockState == .locked || lockState == .unlocked || lockState == .unlockedMixingOnly
        case .showRecoveryPhrase:
            selectedWallet?.hasMnemonic ?? false
        case .unlockWallet:
            lockState == .locked || lockState == .unlockedMixingOnly
        case .lockWallet:
            lockState == .unlocked || lockState == .unlockedMixingOnly
        case .section(let item):
            sections.contains(item)
        default:
            true
        }
    }

    private var fileMenu: MenuModel {
        let open = loadStates.isEmpty
            ? [MenuItemModel(id: "file.open.none", title: L10n.Shell.noWalletsAvailable, command: nil, isEnabled: false)]
            : loadStates.map { state in
                item("file.open.\(state.walletID.hex)", state.name, .openWallet(state.walletID))
            }
        return MenuModel(id: .file, title: L10n.Shell.fileMenu, items: [
            item("file.create", L10n.Shell.createWallet, .createWallet, tip: L10n.Shell.createWalletTip),
            MenuItemModel(
                id: "file.open", title: L10n.Shell.openWallet, command: nil, helpText: L10n.Shell.openWalletTip,
                children: open),
            item("file.close", L10n.Shell.closeWallet, .closeWallet, tip: L10n.Shell.closeWalletTip),
            item("file.closeAll", L10n.Shell.closeAllWallets, .closeAllWallets, tip: L10n.Shell.closeAllWalletsTip),
            .separator("file.sep1"),
            item("file.migrate", L10n.Shell.migrateWallet, .migrateWallet, tip: L10n.Shell.migrateWalletUnavailable),
            item("file.backup", L10n.Shell.backupWallet, .backupWallet, tip: L10n.Shell.backupWalletTip),
            item("file.restore", L10n.Shell.restoreWallet, .restoreWallet, tip: L10n.Shell.restoreWalletTip),
            .separator("file.sep2"),
            item("file.openURI", L10n.Shell.openURI, .openURI, tip: L10n.Shell.openURITip),
            item("file.sign", L10n.Shell.signMessage, .signMessage, tip: L10n.Shell.signMessageTip),
            item("file.verify", L10n.Shell.verifyMessage, .verifyMessage, tip: L10n.Shell.verifyMessageTip),
            .separator("file.sep3"),
            item("file.psbtFile", L10n.Shell.loadPSBTFromFile, .loadPSBTFromFile, tip: L10n.Shell.loadPSBTFromFileTip),
            item(
                "file.psbtClipboard", L10n.Shell.loadPSBTFromClipboard, .loadPSBTFromClipboard,
                tip: L10n.Shell.loadPSBTFromClipboardTip),
            .separator("file.sep4"),
            item("file.debugLog", L10n.Shell.openDebugLog, .openDebugLog, tip: L10n.Shell.openDebugLogUnavailable),
            item(
                "file.config", L10n.Shell.openConfigurationFile, .openConfigurationFile,
                tip: L10n.Shell.openConfigurationFileUnavailable),
            item(
                "file.backups", L10n.Shell.showAutomaticBackups, .showAutomaticBackups,
                tip: L10n.Shell.showAutomaticBackupsTip),
            .separator("file.sep5"),
            item("file.exit", L10n.Shell.exit, .exit, shortcut: KeyShortcut("q"), tip: L10n.Shell.exitTip),
        ])
    }

    private var settingsMenu: MenuModel {
        var items = [
            item("settings.encrypt", L10n.Shell.encryptWallet, .encryptWallet, tip: L10n.Shell.encryptWalletTip),
            item(
                "settings.changePassphrase", L10n.Shell.changePassphrase, .changePassphrase,
                tip: L10n.Shell.changePassphraseTip),
            item(
                "settings.recoveryPhrase", L10n.Shell.showRecoveryPhrase, .showRecoveryPhrase,
                tip: hasWallet && !(selectedWallet?.hasMnemonic ?? false)
                    ? L10n.Shell.noRecoveryPhrase : L10n.Shell.showRecoveryPhraseTip),
        ]
        // dash-qt shows Unlock or Lock depending on the state, not both.
        if lockState == .locked || lockState == .unlockedMixingOnly {
            items.append(item(
                "settings.unlock", L10n.Shell.unlockWallet, .unlockWallet, tip: L10n.Shell.unlockWalletTip,
                unlockScope: .full))
        }
        // dash-qt offers the mixing-only unlock while CoinJoin is on.
        if coinJoinOn, lockState == .locked {
            items.append(item(
                "settings.unlockMixing", L10n.Shell.unlockWalletForMixing, .unlockWallet,
                tip: L10n.Shell.unlockWalletForMixingTip, unlockScope: .mixingOnly))
        }
        if lockState == .unlocked || lockState == .unlockedMixingOnly {
            items.append(item("settings.lock", L10n.Shell.lockWallet, .lockWallet, tip: L10n.Shell.lockWalletTip))
        }
        items += [
            .separator("settings.sep1"),
            item(
                "settings.discreet", L10n.Shell.discreetMode, .toggleDiscreetMode,
                shortcut: KeyShortcut("d", [.command, .shift]), checked: discreet, tip: L10n.Shell.discreetModeTip),
            item("settings.options", L10n.Shell.options, .options, shortcut: KeyShortcut(","), tip: L10n.Shell.optionsTip),
        ]
        return MenuModel(id: .settings, title: L10n.Shell.settingsMenu, items: items)
    }

    private var windowMenu: MenuModel {
        MenuModel(id: .window, title: L10n.Shell.windowMenu, items: [
            item("window.minimize", L10n.Shell.minimize, .minimize, shortcut: KeyShortcut("m")),
            .separator("window.sep1"),
            item("window.sending", L10n.Shell.sendingAddresses, .sendingAddresses),
            item("window.receiving", L10n.Shell.receivingAddresses, .receivingAddresses),
            .separator("window.sep2"),
            item(
                "window.information", L10n.Shell.information, .tools(.information),
                shortcut: KeyShortcut("i", [.command, .shift])),
            item("window.console", L10n.Shell.console, .tools(.console), shortcut: KeyShortcut("c", [.command, .shift])),
            item(
                "window.traffic", L10n.Shell.networkTraffic, .tools(.networkTraffic),
                shortcut: KeyShortcut("g", [.command, .shift]), tip: L10n.Shell.networkTrafficUnavailable),
            item("window.peers", L10n.Shell.peers, .tools(.peers), shortcut: KeyShortcut("p", [.command, .shift])),
            item("window.repair", L10n.Shell.repair, .tools(.repair), shortcut: KeyShortcut("r", [.command, .shift])),
        ])
    }

    private var helpMenu: MenuModel {
        var items = [
            item(
                "help.commandLine", L10n.Shell.commandLineOptions, .commandLineOptions,
                tip: L10n.Shell.commandLineOptionsTip),
        ]
        // Shown only while CoinJoin is enabled (dash-qt).
        if coinJoinOn {
            items.append(item("help.coinJoin", L10n.Shell.coinJoinInformation, .coinJoinInformation))
        }
        items.append(item("help.about", L10n.Shell.about, .about, tip: L10n.Shell.aboutTip))
        return MenuModel(id: .help, title: L10n.Shell.helpMenu, items: items)
    }

    private func item(
        _ id: String, _ title: String, _ command: ShellCommand, shortcut: KeyShortcut? = nil, checked: Bool? = nil,
        tip: String? = nil, unlockScope: UnlockScope? = nil
    ) -> MenuItemModel {
        let enabled = isEnabled(command)
        let needsWallet: Bool
        switch command {
        case .closeWallet, .closeAllWallets, .backupWallet, .openURI, .signMessage, .verifyMessage,
            .sendingAddresses, .receivingAddresses:
            needsWallet = true
        default:
            needsWallet = false
        }
        let help = !enabled && needsWallet && !hasWallet ? L10n.Shell.walletRequired : tip
        return MenuItemModel(
            id: id, title: title, command: command, shortcut: shortcut, isEnabled: enabled, isChecked: checked,
            helpText: help, unlockScope: unlockScope)
    }

    /// dash-qt's tray / Dock menu (QT-029, research 02 §2.4): Show/Hide and
    /// Exit are left out on macOS; "CoinJoin" follows the CoinJoin option.
    public var trayMenu: [MenuItemModel] {
        var items: [MenuItemModel] = []
        if platform != .macOS {
            items += [item("tray.showHide", L10n.Shell.showHide, .showHideWindow), .separator("tray.sep0")]
        }
        items.append(item("tray.send", L10n.Navigation.send, .section(.send)))
        if coinJoinOn {
            items.append(item("tray.coinJoin", L10n.Navigation.coinJoin, .section(.coinJoin)))
        }
        items += [
            item("tray.receive", L10n.Navigation.receive, .section(.receive)),
            .separator("tray.sep1"),
            item("tray.sign", L10n.Shell.signMessage, .signMessage, tip: L10n.Shell.signMessageTip),
            item("tray.verify", L10n.Shell.verifyMessage, .verifyMessage, tip: L10n.Shell.verifyMessageTip),
            .separator("tray.sep2"),
            item("tray.options", L10n.Shell.options, .options, tip: L10n.Shell.optionsTip),
            .separator("tray.sep3"),
            item("tray.information", L10n.Shell.information, .tools(.information)),
            item("tray.console", L10n.Shell.console, .tools(.console)),
            item(
                "tray.traffic", L10n.Shell.networkTraffic, .tools(.networkTraffic),
                tip: L10n.Shell.networkTrafficUnavailable),
            item("tray.peers", L10n.Shell.peers, .tools(.peers)),
            item("tray.repair", L10n.Shell.repair, .tools(.repair)),
            .separator("tray.sep4"),
            item("tray.debugLog", L10n.Shell.openDebugLog, .openDebugLog, tip: L10n.Shell.openDebugLogUnavailable),
            item(
                "tray.config", L10n.Shell.openConfigurationFile, .openConfigurationFile,
                tip: L10n.Shell.openConfigurationFileUnavailable),
            item(
                "tray.backups", L10n.Shell.showAutomaticBackups, .showAutomaticBackups,
                tip: L10n.Shell.showAutomaticBackupsTip),
        ]
        if platform != .macOS {
            items += [.separator("tray.sep5"), item("tray.exit", L10n.Shell.exit, .exit, tip: L10n.Shell.exitTip)]
        }
        return items
    }

    // MARK: Private

    private func reloadLoadStates() async {
        do {
            loadStates = try await walletLifecycle.loadStates()
        } catch {
            loadStates = []
            if error.code != .notImplemented { errorMessage = ErrorText.m2(error.code) }
        }
    }

    private func close(_ ids: [WalletID]) async {
        for id in ids {
            let ok = await run { () async throws(ServiceError) in
                try await self.walletLifecycle.unload(id)
                try await self.walletLifecycle.setLoadOnStartup(id, false)
            }
            if !ok { break }
        }
        await reloadLoadStates()
    }

    private func revealBackups() async {
        do {
            let policy = try await backups.policy()
            try fileRevealer.reveal(policy.directory)
        } catch let error as ServiceError {
            errorMessage = ErrorText.m2(error.code)
        } catch let error as PlatformServiceError {
            errorMessage = ErrorText.m2(ServiceError(error).code)
        } catch {
            errorMessage = L10n.Common.unexpected
        }
    }

    @discardableResult
    private func run(_ body: () async throws(ServiceError) -> Void) async -> Bool {
        do {
            try await body()
            return true
        } catch {
            errorMessage = ErrorText.m2(error.code)
            return false
        }
    }
}
