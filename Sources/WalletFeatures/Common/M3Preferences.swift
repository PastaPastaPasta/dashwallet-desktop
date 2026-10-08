// M3 values the view models keep in `settings.json` (the "desktop" section,
// `DesktopPreferences.m3`): dash-qt's QSettings keys for CoinJoin and the
// iOS per-wallet CoinJoin tags. Missing keys take their defaults, so older files load.
import Foundation
import WalletRuntime

public struct M3Preferences: Sendable, Hashable, Codable {
    /// `hasMixed`: the "Most Common" hint was shown once (QT-044).
    public var coinJoinHintShown: Bool
    /// `fShowAdvancedCJUI` (default off, QT-047).
    public var showAdvancedCoinJoinUI: Bool
    /// `fLowKeysWarning` (default on). Kept for import parity; our HD
    /// wallets have no keypool, so it never fires (m3-engine.md §8).
    public var lowKeysWarning: Bool
    /// IOS-057 "Later": the CoinJoin balance (duffs) the move prompt was
    /// dismissed at, per wallet id hex. It shows again once the balance changes.
    public var mixedCoinsDismissedAt: [String: Int64]
    /// IOS-030: txids of the app's own "move mixed coins" sweeps, per wallet
    /// id hex (iOS `CoinJoinWithdrawalStore`).
    public var coinJoinWithdrawals: [String: [String]]

    public init(
        coinJoinHintShown: Bool = false, showAdvancedCoinJoinUI: Bool = false, lowKeysWarning: Bool = true,
        mixedCoinsDismissedAt: [String: Int64] = [:], coinJoinWithdrawals: [String: [String]] = [:]
    ) {
        self.coinJoinHintShown = coinJoinHintShown
        self.showAdvancedCoinJoinUI = showAdvancedCoinJoinUI
        self.lowKeysWarning = lowKeysWarning
        self.mixedCoinsDismissedAt = mixedCoinsDismissedAt
        self.coinJoinWithdrawals = coinJoinWithdrawals
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let d = M3Preferences()
        coinJoinHintShown = try c.decodeIfPresent(Bool.self, forKey: .coinJoinHintShown) ?? d.coinJoinHintShown
        showAdvancedCoinJoinUI =
            try c.decodeIfPresent(Bool.self, forKey: .showAdvancedCoinJoinUI) ?? d.showAdvancedCoinJoinUI
        lowKeysWarning = try c.decodeIfPresent(Bool.self, forKey: .lowKeysWarning) ?? d.lowKeysWarning
        mixedCoinsDismissedAt =
            try c.decodeIfPresent([String: Int64].self, forKey: .mixedCoinsDismissedAt) ?? d.mixedCoinsDismissedAt
        coinJoinWithdrawals =
            try c.decodeIfPresent([String: [String]].self, forKey: .coinJoinWithdrawals) ?? d.coinJoinWithdrawals
    }
}

extension DesktopPreferencesStoring {
    /// Changes the M3 section and writes it; `false` when the write failed.
    @discardableResult
    func updateM3(_ change: (inout M3Preferences) -> Void) -> Bool {
        var stored = desktop
        change(&stored.m3)
        guard stored != desktop else { return true }
        do {
            try update(stored)
            return true
        } catch {
            return false
        }
    }
}
