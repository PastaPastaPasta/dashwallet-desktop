// dash-qt's Options dialog (QT-135…141, QT-033, IOS-104/105): the user edits
// copies of every tab; OK (`apply`) writes what changed, Cancel (`discard`)
// restores the stored values.
import Foundation
import Observation
import PlatformServices
import WalletRuntime

public enum OptionsTab: String, Sendable, Hashable, CaseIterable {
    case main, wallet, network, display, appearance, notifications

    public var title: String {
        switch self {
        case .main: L10n.Options.mainTab
        case .wallet: L10n.Options.walletTab
        case .network: L10n.Options.networkTab
        case .display: L10n.Options.displayTab
        case .appearance: L10n.Options.appearanceTab
        case .notifications: L10n.Options.notificationsTab
        }
    }
}

/// Options ▸ Main (QT-136). dash-qt hides all four on macOS.
public struct MainOptions: Sendable, Hashable {
    public var startOnLogin: Bool
    public var showTrayIcon: Bool
    public var minimizeToTray: Bool
    public var minimizeOnClose: Bool
}

/// Options ▸ Wallet (QT-137, QT-075, QT-116).
public struct WalletOptions: Sendable, Hashable {
    public var subtractFeeByDefault: Bool
    public var coinControl: Bool
    public var psbtControls: Bool
    public var keepCustomChangeAddress: Bool
    /// `dustprotectionthreshold`: off, or the threshold in duffs.
    public var dustProtectionEnabled: Bool
    /// 1...1,000,000 duffs; dash-qt's default is 10,000.
    public var dustThreshold: Int64
    /// 0...10 newest automatic backups (`-createwalletbackups`).
    public var automaticBackups: Int
}

/// Options ▸ Network (QT-138): shown, not editable until the engine has
/// proxy support (U1); the numeric-IP check runs anyway.
public struct NetworkOptionsView: Sendable, Hashable {
    public var proxyEnabled = false
    public var proxyIP = "127.0.0.1"
    public var proxyPort = "9050"
    public var onionEnabled = false
    public var onionIP = "127.0.0.1"
    public var onionPort = "9050"
    public let isEditable = false
    public let disabledReason = L10n.Options.proxyRequiresEngine
}

/// Options ▸ Display (QT-139, IOS-104).
public struct DisplayOptions: Sendable, Hashable {
    public var languageCode: String?
    public var unit: DisplayUnit
    public var decimalDigits: Int
    public var showMasternodesTab: Bool
    public var showGovernanceTab: Bool
    public var showGovernanceClock: Bool
    public var thirdPartyTxURLs: String
    /// ISO 4217; `nil` follows the OS locale. A list only until rates (M5).
    public var localCurrency: String?
}

/// Notifications (IOS-105, QT-033).
public struct NotificationOptions: Sendable, Hashable {
    public var enabled: Bool
    public var showCoinJoinNotifications: Bool
}

/// Options ▸ CoinJoin plus the Wallet tab's "Enable CoinJoin features"
/// (QT-046, QT-047, QT-135). The engine values are global for the network;
/// the interface toggles are host settings (dash-qt QSettings).
public struct CoinJoinOptions: Sendable, Hashable {
    public var enabled: Bool
    public var multiSession: Bool
    public var maxSessions: Int
    public var rounds: Int
    /// Whole DASH.
    public var targetAmountDash: Int
    public var denomsGoal: Int
    public var denomsHardCap: Int
    /// `fShowAdvancedCJUI`.
    public var showAdvancedInterface: Bool
    /// `fLowKeysWarning`; shown for import parity, never fires for HD wallets.
    public var lowKeysWarning: Bool

    init(_ settings: CoinJoinSettings, preferences: M3Preferences) {
        enabled = settings.enabled
        multiSession = settings.multiSession
        maxSessions = settings.maxSessions
        rounds = settings.rounds
        targetAmountDash = settings.targetAmountDash
        denomsGoal = settings.denomsGoal
        denomsHardCap = settings.denomsHardCap
        showAdvancedInterface = preferences.showAdvancedCoinJoinUI
        lowKeysWarning = preferences.lowKeysWarning
    }

    var engineSettings: CoinJoinSettings {
        CoinJoinSettings(
            enabled: enabled, multiSession: multiSession, maxSessions: maxSessions, rounds: rounds,
            targetAmountDash: targetAmountDash, denomsGoal: denomsGoal, denomsHardCap: denomsHardCap)
    }
}

/// Reset Options (QT-141): confirm, back up, reset, quit.
public enum OptionsResetFlow: Sendable, Hashable {
    case idle
    case confirming
    /// Done: the app quits; `backups` are the saved files.
    case done(backups: [URL])
    case failed(String)
}

@MainActor
@Observable
public final class OptionsViewModel {
    public static let dustThresholdRange: ClosedRange<Int64> = 1...1_000_000
    public static let defaultDustThreshold: Int64 = 10_000
    public static let automaticBackupsRange = 0...10
    public static let decimalDigitsRange = 2...8

    public var main: MainOptions
    public var wallet: WalletOptions
    public var network = NetworkOptionsView()
    public var display: DisplayOptions
    public var appearance: AppTheme
    public var notifications: NotificationOptions
    /// Options ▸ CoinJoin; edits apply on OK like every tab (QT-046).
    public var coinJoin: CoinJoinOptions
    /// The ranges of the CoinJoin spin boxes (`coinjoin_limits`).
    public let coinJoinLimits: CoinJoinLimits
    /// `false` until the engine answers `coinjoin_settings` (no adapter yet:
    /// the tab says "Not available yet").
    public private(set) var coinJoinAvailable = false

    public private(set) var notificationAuthorization: NotificationAuthorization?
    /// "Client restart required to activate changes." stays once set.
    public private(set) var restartRequired = false
    public private(set) var resetFlow: OptionsResetFlow = .idle
    public private(set) var errorMessage: String?
    /// The engine answered `not_implemented` for these values; their controls
    /// show "Not available yet." instead of a made-up value.
    public private(set) var dustProtectionAvailable = true
    public private(set) var automaticBackupsAvailable = true
    public let availableLanguages: [String?] = [nil, "en"]
    public let platform: DesktopPlatform
    /// Called after an apply turned "Enable coin control features" on or off
    /// (dash-qt's `coinControlFeatureChanged` signal); the composition root
    /// routes it to `MainViewModel.coinControlFeatureChanged()`.
    @ObservationIgnored public var onCoinControlFeatureChanged: (@MainActor () -> Void)?

    public var tabs: [OptionsTab] {
        OptionsTab.allCases.filter { $0 != .main || showsStartOnLogin || showsTrayOptions }
    }

    /// dash-qt shows the CoinJoin tab while "Enable CoinJoin features" is on.
    public var showsCoinJoinTab: Bool { coinJoinService != nil && coinJoin.enabled }

    /// dash-qt hides start-on-login on macOS.
    public var showsStartOnLogin: Bool { launchAtLogin.isSupported }
    /// Tray settings exist on Windows and Linux only.
    public var showsTrayOptions: Bool { platform != .macOS }

    public var hasChanges: Bool { snapshot() != stored }

    public var notificationStatusText: String? {
        switch notificationAuthorization {
        case .denied: L10n.Options.notificationsDenied
        case .unavailable: L10n.Options.notificationsUnavailable
        case .authorized, .notDetermined, nil: nil
        }
    }

    public var proxyError: String? {
        guard network.proxyEnabled else { return nil }
        return Self.validateProxy(ip: network.proxyIP, port: network.proxyPort) ? nil : L10n.Options.proxyInvalid
    }

    private struct Snapshot: Equatable {
        var main: MainOptions
        var wallet: WalletOptions
        var display: DisplayOptions
        var appearance: AppTheme
        var notifications: NotificationOptions
        var coinJoin: CoinJoinOptions
    }

    private var stored: Snapshot
    private let settings: any SettingsProviding
    private let preferences: any UIPreferencesStoring
    private let desktopPreferences: any DesktopPreferencesStoring
    private let shellSettings: any ShellSettingsProviding
    private let launchAtLogin: any LaunchAtLoginManaging
    private let notifier: any SystemNotifying
    private let dustProtection: any DustProtectionControlling
    private let backups: any BackupProviding
    private let optionsReset: any OptionsResetting
    private let host: any WalletHosting
    private let coinJoinService: (any CoinJoinControlling)?

    public init(
        settings: any SettingsProviding, preferences: any UIPreferencesStoring,
        desktopPreferences: any DesktopPreferencesStoring, shellSettings: any ShellSettingsProviding,
        launchAtLogin: any LaunchAtLoginManaging, notifier: any SystemNotifying,
        dustProtection: any DustProtectionControlling, backups: any BackupProviding,
        optionsReset: any OptionsResetting, host: any WalletHosting, platform: DesktopPlatform = .current,
        coinJoin coinJoinService: (any CoinJoinControlling)? = nil
    ) {
        self.settings = settings
        self.preferences = preferences
        self.desktopPreferences = desktopPreferences
        self.shellSettings = shellSettings
        self.launchAtLogin = launchAtLogin
        self.notifier = notifier
        self.dustProtection = dustProtection
        self.backups = backups
        self.optionsReset = optionsReset
        self.host = host
        self.platform = platform
        self.coinJoinService = coinJoinService
        let limits = coinJoinService?.limits() ?? M3Defaults.coinJoinLimits
        coinJoinLimits = limits
        let shell = shellSettings.shell
        let options = desktopPreferences.desktop.options
        let snapshot = Snapshot(
            main: MainOptions(
                startOnLogin: false, showTrayIcon: shell.showTrayIcon, minimizeToTray: shell.minimizeToTray,
                minimizeOnClose: shell.minimizeOnClose),
            wallet: WalletOptions(
                subtractFeeByDefault: options.subtractFeeByDefault, coinControl: options.coinControl,
                psbtControls: options.psbtControls, keepCustomChangeAddress: options.keepCustomChangeAddress,
                dustProtectionEnabled: false, dustThreshold: Self.defaultDustThreshold, automaticBackups: 10),
            display: DisplayOptions(
                languageCode: preferences.preferences.languageCode, unit: settings.display.unit,
                decimalDigits: settings.display.decimalDigits, showMasternodesTab: options.showMasternodesTab,
                showGovernanceTab: options.showGovernanceTab, showGovernanceClock: options.showGovernanceClock,
                thirdPartyTxURLs: options.thirdPartyTxURLs, localCurrency: options.localCurrency),
            appearance: preferences.preferences.theme,
            notifications: NotificationOptions(
                enabled: shell.notificationsEnabled, showCoinJoinNotifications: shell.showCoinJoinNotifications),
            coinJoin: CoinJoinOptions(limits.defaults, preferences: desktopPreferences.desktop.m3))
        stored = snapshot
        main = snapshot.main
        wallet = snapshot.wallet
        display = snapshot.display
        appearance = snapshot.appearance
        notifications = snapshot.notifications
        coinJoin = snapshot.coinJoin
    }

    public convenience init(env: AppEnvironment, m2: M2Services, m3: M3Services? = nil) {
        self.init(
            settings: env.settings, preferences: env.preferences, desktopPreferences: m2.desktopPreferences,
            shellSettings: m2.shellSettings, launchAtLogin: m2.launchAtLogin, notifier: m2.notifications,
            dustProtection: m2.dustProtection, backups: m2.backups, optionsReset: m2.optionsReset, host: env.host,
            platform: m2.platform, coinJoin: m3?.coinJoin)
    }

    /// Reads the values that live in the engine and the OS (dialog open).
    public func load() async {
        errorMessage = nil
        if launchAtLogin.isSupported {
            stored.main.startOnLogin = (try? launchAtLogin.isEnabled()) ?? false
        }
        do {
            let threshold = try await dustProtection.threshold()
            stored.wallet.dustProtectionEnabled = threshold != nil
            stored.wallet.dustThreshold = threshold?.duffs ?? Self.defaultDustThreshold
            dustProtectionAvailable = true
        } catch {
            dustProtectionAvailable = false
            if error.code != .notImplemented { errorMessage = ErrorText.m2(error.code) }
        }
        do {
            stored.wallet.automaticBackups = try await backups.policy().keep
            automaticBackupsAvailable = true
        } catch {
            automaticBackupsAvailable = false
            if error.code != .notImplemented { errorMessage = ErrorText.m2(error.code) }
        }
        if let coinJoinService {
            do {
                stored.coinJoin = CoinJoinOptions(
                    try await coinJoinService.settings(), preferences: desktopPreferences.desktop.m3)
                coinJoinAvailable = true
            } catch {
                coinJoinAvailable = false
                if !error.isNotImplemented { errorMessage = ErrorText.m3(error, amount: { "\($0.duffs)" }) }
            }
        }
        notificationAuthorization = await notifier.authorization()
        discard()
    }

    /// Cancel: every tab back to the stored values.
    public func discard() {
        main = stored.main
        wallet = stored.wallet
        display = stored.display
        appearance = stored.appearance
        notifications = stored.notifications
        coinJoin = stored.coinJoin
        network = NetworkOptionsView()
    }

    /// OK: validates, then writes each changed value to its store. Stops at
    /// the first failure; values written before it stay written.
    public func apply() async throws(ServiceError) {
        errorMessage = nil
        clampCoinJoin()
        let next = snapshot()
        if next.wallet.dustProtectionEnabled, !Self.dustThresholdRange.contains(next.wallet.dustThreshold) {
            errorMessage = L10n.Options.dustThresholdInvalid
            throw ServiceError(code: .invalidArgument, detail: "dust threshold")
        }
        guard Self.automaticBackupsRange.contains(next.wallet.automaticBackups) else {
            errorMessage = L10n.Options.automaticBackupsInvalid
            throw ServiceError(code: .invalidArgument, detail: "automatic backups")
        }
        if coinJoinAvailable, let field = M3Defaults.invalidCoinJoinField(next.coinJoin.engineSettings, limits: coinJoinLimits) {
            errorMessage = L10n.CoinJoin.invalidSetting
            throw ServiceError(code: .invalidArgument, detail: field)
        }
        do {
            try await write(next)
        } catch {
            errorMessage = ErrorText.m2(error.code)
            throw error
        }
    }

    private func write(_ next: Snapshot) async throws(ServiceError) {
        if next.main.startOnLogin != stored.main.startOnLogin, launchAtLogin.isSupported {
            let arguments = await autostartArguments()
            do {
                try launchAtLogin.setEnabled(next.main.startOnLogin, arguments: arguments)
            } catch {
                throw ServiceError(error)
            }
            stored.main.startOnLogin = next.main.startOnLogin
        }
        var shell = shellSettings.shell
        shell.showTrayIcon = next.main.showTrayIcon
        shell.minimizeToTray = next.main.minimizeToTray
        shell.minimizeOnClose = next.main.minimizeOnClose
        shell.notificationsEnabled = next.notifications.enabled
        shell.showCoinJoinNotifications = next.notifications.showCoinJoinNotifications
        if shell != shellSettings.shell { try shellSettings.update(shell) }
        stored.main = next.main
        stored.notifications = next.notifications
        if next.notifications.enabled, notificationAuthorization == .notDetermined {
            notificationAuthorization = await notifier.requestAuthorization()
        }

        var desktop = desktopPreferences.desktop
        desktop.options.subtractFeeByDefault = next.wallet.subtractFeeByDefault
        desktop.options.coinControl = next.wallet.coinControl
        desktop.options.psbtControls = next.wallet.psbtControls
        desktop.options.keepCustomChangeAddress = next.wallet.keepCustomChangeAddress
        if !next.wallet.keepCustomChangeAddress { desktop.options.customChangeAddress = nil }
        desktop.options.showMasternodesTab = next.display.showMasternodesTab
        desktop.options.showGovernanceTab = next.display.showGovernanceTab
        desktop.options.showGovernanceClock = next.display.showGovernanceClock
        desktop.options.thirdPartyTxURLs = next.display.thirdPartyTxURLs
        desktop.options.localCurrency = next.display.localCurrency
        desktop.m3.showAdvancedCoinJoinUI = next.coinJoin.showAdvancedInterface
        desktop.m3.lowKeysWarning = next.coinJoin.lowKeysWarning
        let coinControlChanged = desktop.options.coinControl != desktopPreferences.desktop.options.coinControl
        if desktop != desktopPreferences.desktop { try desktopPreferences.update(desktop) }
        if coinControlChanged { onCoinControlFeatureChanged?() }
        stored.coinJoin.showAdvancedInterface = next.coinJoin.showAdvancedInterface
        stored.coinJoin.lowKeysWarning = next.coinJoin.lowKeysWarning

        // CoinJoin options apply live (dash-qt), only when they changed.
        if let coinJoinService, coinJoinAvailable, next.coinJoin.engineSettings != stored.coinJoin.engineSettings {
            try await coinJoinService.setSettings(next.coinJoin.engineSettings)
            stored.coinJoin = next.coinJoin
        }

        if dustProtectionAvailable,
            next.wallet.dustProtectionEnabled != stored.wallet.dustProtectionEnabled
                || (next.wallet.dustProtectionEnabled && next.wallet.dustThreshold != stored.wallet.dustThreshold)
        {
            try await dustProtection.setThreshold(
                next.wallet.dustProtectionEnabled ? Amount(duffs: next.wallet.dustThreshold) : nil)
        }
        if automaticBackupsAvailable, next.wallet.automaticBackups != stored.wallet.automaticBackups {
            _ = try await backups.setKeep(next.wallet.automaticBackups)
        }
        stored.wallet = next.wallet

        var displaySettings = settings.display
        displaySettings.unit = next.display.unit
        displaySettings.decimalDigits = min(
            max(next.display.decimalDigits, Self.decimalDigitsRange.lowerBound), Self.decimalDigitsRange.upperBound)
        if displaySettings != settings.display { try settings.update(displaySettings) }

        var ui = preferences.preferences
        ui.theme = next.appearance
        if next.display.languageCode != ui.languageCode {
            ui.languageCode = next.display.languageCode
            restartRequired = true
        }
        if ui != preferences.preferences { try preferences.update(ui) }
        stored.display = next.display
        stored.display.decimalDigits = displaySettings.decimalDigits
        stored.appearance = next.appearance
        display = stored.display
    }

    // MARK: Validation and lists

    /// dash-qt's proxy check: numeric IPv4 or IPv6 (no host names), port 1–65535.
    public nonisolated static func validateProxy(ip: String, port: String) -> Bool {
        guard let number = Int(port), (1...65_535).contains(number) else { return false }
        return isIPv4(ip) || isIPv6(ip)
    }

    nonisolated static func isIPv4(_ text: String) -> Bool {
        let parts = text.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 4 else { return false }
        return parts.allSatisfy { part in
            !part.isEmpty && part.count <= 3 && part.allSatisfy(\.isASCII) && part.allSatisfy(\.isNumber)
                && (Int(part) ?? 256) <= 255
        }
    }

    nonisolated static func isIPv6(_ text: String) -> Bool {
        var text = Substring(text)
        if text.hasPrefix("[") && text.hasSuffix("]") { text = text.dropFirst().dropLast() }
        guard text.contains(":"), text.count >= 2 else { return false }
        let doubleColons = text.components(separatedBy: "::").count - 1
        guard doubleColons <= 1 else { return false }
        let groups = text.split(separator: ":", omittingEmptySubsequences: true)
        var count = 0
        for (index, group) in groups.enumerated() {
            if index == groups.count - 1, group.contains(".") {
                guard isIPv4(String(group)) else { return false }
                count += 2
                continue
            }
            guard (1...4).contains(group.count), group.allSatisfy({ $0.isHexDigit }) else { return false }
            count += 1
        }
        return doubleColons == 1 ? count < 8 : count == 8
    }

    /// ISO 4217 codes, filtered by code or English name (IOS-104 search).
    public func currencies(matching search: String = "") -> [String] {
        let all = Locale.commonISOCurrencyCodes.sorted()
        let query = search.trimmingCharacters(in: .whitespaces).lowercased()
        guard !query.isEmpty else { return all }
        return all.filter { L10n.Options.currencyName($0).lowercased().contains(query) }
    }

    /// The currency shown while none is chosen: the OS locale's, else USD.
    public var defaultCurrency: String { Locale.current.currency?.identifier ?? "USD" }

    // MARK: Reset (QT-141)

    public func requestReset() {
        resetFlow = .confirming
    }

    public func cancelReset() {
        if resetFlow == .confirming { resetFlow = .idle }
    }

    /// Backs up and resets the settings files, removes autostart, then the
    /// app quits (dash-qt does not restart).
    public func confirmReset() {
        guard resetFlow == .confirming else { return }
        do {
            let backups = try optionsReset.resetOptions()
            if launchAtLogin.isSupported { try? launchAtLogin.setEnabled(false, arguments: []) }
            resetFlow = .done(backups: backups)
        } catch {
            resetFlow = .failed(ErrorText.m2(error.code))
        }
    }

    // MARK: Private

    private func snapshot() -> Snapshot {
        Snapshot(
            main: main, wallet: wallet, display: display, appearance: appearance, notifications: notifications,
            coinJoin: coinJoin)
    }

    /// dash-qt's spin boxes: every value in its range and Target ≤ Maximum.
    public func clampCoinJoin() {
        let l = coinJoinLimits
        var c = coinJoin
        c.maxSessions = min(max(c.maxSessions, l.sessions.lowerBound), l.sessions.upperBound)
        c.rounds = min(max(c.rounds, l.rounds.lowerBound), l.rounds.upperBound)
        c.targetAmountDash = min(max(c.targetAmountDash, l.targetAmountDash.lowerBound), l.targetAmountDash.upperBound)
        c.denomsHardCap = min(max(c.denomsHardCap, l.denoms.lowerBound), l.denoms.upperBound)
        c.denomsGoal = min(max(c.denomsGoal, l.denoms.lowerBound), c.denomsHardCap)
        if c != coinJoin { coinJoin = c }
    }

    /// dash-qt autostart arguments: start minimized on the active network.
    private func autostartArguments() async -> [String] {
        var arguments = ["--min"]
        switch await host.activeNetwork {
        case .testnet: arguments.append("--testnet")
        case .regtest: arguments.append("--regtest")
        case .devnet(let name): arguments.append("--devnet=\(name)")
        case .mainnet, nil: break
        }
        return arguments
    }
}
