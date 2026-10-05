// UI preferences that `SettingsProviding.display` does not carry yet: theme,
// language and the persisted transaction filters (QT-089, QT-139/140).
import Foundation
import WalletRuntime

/// Light / Dark / System (DESIGN.md R2; dash-qt "Traditional" is not offered).
public enum AppTheme: String, Sendable, Hashable, CaseIterable, Codable {
    case light, dark, system
}

/// dash-qt transaction date filter (QT-089). `range` uses
/// `UIPreferences.transactionDateFrom`/`transactionDateTo`.
public enum DateFilterPreset: String, Sendable, Hashable, CaseIterable, Codable {
    case all, today, thisWeek, thisMonth, lastMonth, thisYear, range
}

/// dash-qt transaction type filter entries, in dash-qt's menu order (QT-089).
public enum TypeFilterPreset: String, Sendable, Hashable, CaseIterable, Codable {
    case all, mostCommon, receivedWith, sentTo, coinJoinSend, coinJoinMakeCollaterals, coinJoinCreateDenominations,
        coinJoinMixing, coinJoinCollateralPayment, toYourself, mined, masternode, platformTransfer, assetLock,
        dataTransaction, dustReceive, other

    /// The five entries dash-qt hides while CoinJoin is disabled.
    public var isCoinJoin: Bool {
        switch self {
        case .coinJoinSend, .coinJoinMakeCollaterals, .coinJoinCreateDenominations, .coinJoinMixing,
            .coinJoinCollateralPayment:
            true
        default:
            false
        }
    }
}

public struct UIPreferences: Sendable, Hashable, Codable {
    public var theme: AppTheme
    /// BCP 47 code; `nil` follows the system language. Only English ships in M1.
    public var languageCode: String?
    public var transactionDate: DateFilterPreset
    public var transactionDateFrom: Date?
    /// Exclusive end (dash-qt).
    public var transactionDateTo: Date?
    public var transactionType: TypeFilterPreset

    public init(
        theme: AppTheme = .system, languageCode: String? = nil, transactionDate: DateFilterPreset = .all,
        transactionDateFrom: Date? = nil, transactionDateTo: Date? = nil, transactionType: TypeFilterPreset = .all
    ) {
        self.theme = theme
        self.languageCode = languageCode
        self.transactionDate = transactionDate
        self.transactionDateFrom = transactionDateFrom
        self.transactionDateTo = transactionDateTo
        self.transactionType = transactionType
    }
}

/// Persists `UIPreferences` (the app's settings store, next to `settings.json`).
@MainActor
public protocol UIPreferencesStoring: AnyObject {
    var preferences: UIPreferences { get }
    func update(_ preferences: UIPreferences) throws(ServiceError)
}
