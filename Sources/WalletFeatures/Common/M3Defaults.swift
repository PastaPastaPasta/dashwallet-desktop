// Dash Core constants the M3 engine call `coinjoin_limits` returns
// (m3-engine.md §2.1; dw-coinjoin `denoms`/`settings`). Used where no engine
// adapter answers (the placeholder services and demo mode), so those values
// never drift from Core's.
import Foundation
import WalletRuntime

public enum M3Defaults {
    private static let coin = Amount.duffsPerDash

    /// `coinjoin_limits()`: denominations largest first, 0.00140001 DASH
    /// minimum, the Options ranges and dash-qt's defaults.
    public static let coinJoinLimits = CoinJoinLimits(
        denominations: [
            Amount(duffs: 10 * coin + 10_000), Amount(duffs: coin + 1_000), Amount(duffs: coin / 10 + 100),
            Amount(duffs: coin / 100 + 10), Amount(duffs: coin / 1_000 + 1),
        ],
        minimumMixingBalance: Amount(duffs: coin / 1_000 + 1 + 40_000), rounds: 2...16, sessions: 1...10,
        targetAmountDash: 2...21_000_000, denoms: 10...100_000, defaults: .dashQtDefaults)

    /// The `set_coinjoin_settings` range check (dw-coinjoin
    /// `CoinJoinSettings::validate`): the field that is out of range, or
    /// `nil`. The goal must not exceed the hard cap.
    public static func invalidCoinJoinField(_ settings: CoinJoinSettings, limits: CoinJoinLimits = coinJoinLimits)
        -> String?
    {
        if !limits.sessions.contains(settings.maxSessions) { return "max_sessions" }
        if !limits.rounds.contains(settings.rounds) { return "rounds" }
        if !limits.targetAmountDash.contains(settings.targetAmountDash) { return "target_amount_dash" }
        if !limits.denoms.contains(settings.denomsHardCap) { return "denoms_hard_cap" }
        if settings.denomsGoal < limits.denoms.lowerBound || settings.denomsGoal > settings.denomsHardCap {
            return "denoms_goal"
        }
        return nil
    }

    /// `set_coinjoin_salt`'s check: 64 lowercase hexadecimal characters.
    public static func isValidSalt(_ salt: String) -> Bool {
        salt.utf8.count == 64 && salt.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }
}
