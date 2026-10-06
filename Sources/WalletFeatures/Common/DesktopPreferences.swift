// M2 desktop preferences the view models keep in `settings.json` (section
// "desktop"), and the small Swift-side seams the M2 view models need that the
// WalletRuntime contracts do not name (m2-engine.md §1: `OptionsProviding`,
// `BackupReminderTracking`; M1 `set_dust_protection`).
import Foundation
import Observation
import WalletRuntime

/// A window frame in screen points (QT-011: geometry saved per window).
public struct WindowGeometry: Sendable, Hashable, Codable {
    public var x: Double
    public var y: Double
    public var width: Double
    public var height: Double

    public init(x: Double, y: Double, width: Double, height: Double) {
        self.x = x
        self.y = y
        self.width = width
        self.height = height
    }
}

/// dash-qt coin selection dialog mode (`nCoinControlMode`; list is the default).
public enum CoinControlMode: String, Sendable, Hashable, CaseIterable, Codable {
    case list, tree
}

/// Coin selection dialog columns, in dash-qt's order (QT-069).
public enum CoinColumn: String, Sendable, Hashable, CaseIterable, Codable {
    case amount, label, address, mixingRounds, date, confirmations
}

/// `nCoinControlSortColumn` / `nCoinControlSortOrder`; default Amount, descending.
public struct CoinSort: Sendable, Hashable, Codable {
    public var column: CoinColumn
    public var ascending: Bool

    public init(column: CoinColumn = .amount, ascending: Bool = false) {
        self.column = column
        self.ascending = ascending
    }
}

/// Options-dialog values kept by the Swift side (QT-137, QT-139, IOS-104).
public struct DesktopOptions: Sendable, Hashable, Codable {
    /// `SubFeeFromAmount` (default off).
    public var subtractFeeByDefault: Bool
    /// `fCoinControlFeatures` (default off).
    public var coinControl: Bool
    /// `enable_psbt_controls` (default off).
    public var psbtControls: Bool
    /// `fKeepChangeAddress` (default off).
    public var keepCustomChangeAddress: Bool
    /// `sCustomChangeAddress`, kept while `keepCustomChangeAddress` is on.
    public var customChangeAddress: String?
    /// `fShowMasternodesTab` (default off).
    public var showMasternodesTab: Bool
    /// `fShowGovernanceTab` (default off).
    public var showGovernanceTab: Bool
    /// `show_governance_clock` (default off).
    public var showGovernanceClock: Bool
    /// `strThirdPartyTxUrls`: `|`-separated, `%s` = txid (default empty).
    public var thirdPartyTxURLs: String
    /// ISO 4217 code; `nil` follows the OS locale (IOS-104).
    public var localCurrency: String?

    public init(
        subtractFeeByDefault: Bool = false, coinControl: Bool = false, psbtControls: Bool = false,
        keepCustomChangeAddress: Bool = false, customChangeAddress: String? = nil, showMasternodesTab: Bool = false,
        showGovernanceTab: Bool = false, showGovernanceClock: Bool = false, thirdPartyTxURLs: String = "",
        localCurrency: String? = nil
    ) {
        self.subtractFeeByDefault = subtractFeeByDefault
        self.coinControl = coinControl
        self.psbtControls = psbtControls
        self.keepCustomChangeAddress = keepCustomChangeAddress
        self.customChangeAddress = customChangeAddress
        self.showMasternodesTab = showMasternodesTab
        self.showGovernanceTab = showGovernanceTab
        self.showGovernanceClock = showGovernanceClock
        self.thirdPartyTxURLs = thirdPartyTxURLs
        self.localCurrency = localCurrency
    }

    // Missing keys take their defaults, so older files load.
    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let d = DesktopOptions()
        subtractFeeByDefault = try c.decodeIfPresent(Bool.self, forKey: .subtractFeeByDefault) ?? d.subtractFeeByDefault
        coinControl = try c.decodeIfPresent(Bool.self, forKey: .coinControl) ?? d.coinControl
        psbtControls = try c.decodeIfPresent(Bool.self, forKey: .psbtControls) ?? d.psbtControls
        keepCustomChangeAddress =
            try c.decodeIfPresent(Bool.self, forKey: .keepCustomChangeAddress) ?? d.keepCustomChangeAddress
        customChangeAddress = try c.decodeIfPresent(String.self, forKey: .customChangeAddress)
        showMasternodesTab = try c.decodeIfPresent(Bool.self, forKey: .showMasternodesTab) ?? d.showMasternodesTab
        showGovernanceTab = try c.decodeIfPresent(Bool.self, forKey: .showGovernanceTab) ?? d.showGovernanceTab
        showGovernanceClock = try c.decodeIfPresent(Bool.self, forKey: .showGovernanceClock) ?? d.showGovernanceClock
        thirdPartyTxURLs = try c.decodeIfPresent(String.self, forKey: .thirdPartyTxURLs) ?? d.thirdPartyTxURLs
        localCurrency = try c.decodeIfPresent(String.self, forKey: .localCurrency)
    }
}

/// IOS-005 per wallet: when funds first arrived and whether the reminder or
/// a phrase backup happened.
public struct BackupReminderState: Sendable, Hashable, Codable {
    /// First time a positive balance was seen while the phrase was not backed up.
    public var firstFundsAt: Date?
    /// The reminder was shown once (iOS `walletBackupReminderWasShown`).
    public var reminderShown: Bool
    /// The phrase was revealed (Show Recovery Phrase) or exported for dash-qt.
    public var backedUp: Bool

    public init(firstFundsAt: Date? = nil, reminderShown: Bool = false, backedUp: Bool = false) {
        self.firstFundsAt = firstFundsAt
        self.reminderShown = reminderShown
        self.backedUp = backedUp
    }
}

/// Everything M2 view models persist next to `UIPreferences`.
public struct DesktopPreferences: Sendable, Hashable, Codable {
    public var options: DesktopOptions
    /// Keyed by `ShellWindow.rawValue`.
    public var windows: [String: WindowGeometry]
    public var coinControlMode: CoinControlMode
    public var coinSort: CoinSort
    /// Keyed by wallet id hex.
    public var backupReminders: [String: BackupReminderState]
    /// The customised shortcut bar; `nil` = state-dependent defaults (IOS-025).
    public var shortcuts: [ShortcutAction]?
    /// `consoleFontSize`, 4...40 pt.
    public var consoleFontSize: Int

    public init(
        options: DesktopOptions = DesktopOptions(), windows: [String: WindowGeometry] = [:],
        coinControlMode: CoinControlMode = .list, coinSort: CoinSort = CoinSort(),
        backupReminders: [String: BackupReminderState] = [:], shortcuts: [ShortcutAction]? = nil,
        consoleFontSize: Int = ConsoleViewModel.defaultFontSize
    ) {
        self.options = options
        self.windows = windows
        self.coinControlMode = coinControlMode
        self.coinSort = coinSort
        self.backupReminders = backupReminders
        self.shortcuts = shortcuts
        self.consoleFontSize = consoleFontSize
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        options = try c.decodeIfPresent(DesktopOptions.self, forKey: .options) ?? DesktopOptions()
        windows = try c.decodeIfPresent([String: WindowGeometry].self, forKey: .windows) ?? [:]
        coinControlMode = try c.decodeIfPresent(CoinControlMode.self, forKey: .coinControlMode) ?? .list
        coinSort = try c.decodeIfPresent(CoinSort.self, forKey: .coinSort) ?? CoinSort()
        backupReminders = try c.decodeIfPresent([String: BackupReminderState].self, forKey: .backupReminders) ?? [:]
        shortcuts = try c.decodeIfPresent([ShortcutAction].self, forKey: .shortcuts)
        consoleFontSize = try c.decodeIfPresent(Int.self, forKey: .consoleFontSize) ?? ConsoleViewModel.defaultFontSize
    }
}

/// Persists `DesktopPreferences` (live: the "desktop" section of `settings.json`).
@MainActor
public protocol DesktopPreferencesStoring: AnyObject {
    var desktop: DesktopPreferences { get }
    func update(_ desktop: DesktopPreferences) throws(ServiceError)
}

/// `DesktopPreferencesStoring` as the `"desktop"` section of the runtime's
/// `settings.json`, next to the `"ui"` section.
@MainActor
@Observable
public final class SettingsDesktopPreferencesStore: DesktopPreferencesStoring {
    public static let sectionKey = "desktop"

    public private(set) var desktop: DesktopPreferences
    @ObservationIgnored private let settings: SettingsStore

    public init(settings: SettingsStore) {
        self.settings = settings
        desktop = settings.section(Self.sectionKey, as: DesktopPreferences.self) ?? DesktopPreferences()
    }

    public func update(_ desktop: DesktopPreferences) throws(ServiceError) {
        try settings.setSection(Self.sectionKey, desktop)
        self.desktop = desktop
    }
}

/// dash-qt dust attack protection (QT-075) over the M1 engine calls
/// `dust_protection` / `set_dust_protection` (owner S1 adapter): `nil` = off,
/// otherwise 1...1,000,000 duffs (`invalid_argument` outside).
public protocol DustProtectionControlling: AnyObject, Sendable {
    func threshold() async throws(ServiceError) -> Amount?
    func setThreshold(_ threshold: Amount?) async throws(ServiceError)
}

/// Options ▸ Reset Options (QT-141) and the corrupt-settings Reset (QT-007):
/// backs up `settings.json` and `global.json` to `.bak`, restores defaults
/// and returns the backup files (owner S1 adapter over `SettingsStore`).
@MainActor
public protocol OptionsResetting: AnyObject {
    /// Files that were unreadable at load and moved to `.bak` (QT-007).
    var recoveredFromCorruption: [URL] { get }
    func resetOptions() throws(ServiceError) -> [URL]
}

/// iOS "Require authentication for every payment" (`SettingsStore`, M1).
@MainActor
public protocol PaymentAuthenticationSetting: AnyObject {
    var requireAuthenticationForEveryPayment: Bool { get }
    func setRequireAuthenticationForEveryPayment(_ value: Bool) throws(ServiceError)
}

extension SettingsStore: PaymentAuthenticationSetting {}

/// The desktop OS the app runs on; selects options dash-qt hides on macOS
/// (start on login, tray settings).
public enum DesktopPlatform: Sendable, Hashable {
    case macOS, windows, linux

    public static var current: DesktopPlatform {
        #if os(macOS)
        .macOS
        #elseif os(Windows)
        .windows
        #else
        .linux
        #endif
    }
}
