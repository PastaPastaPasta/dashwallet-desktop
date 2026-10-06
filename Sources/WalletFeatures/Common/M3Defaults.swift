// Dash Core constants the M3 engine calls `coinjoin_limits`,
// `governance_params` and `masternode_network_defaults` return (m3-engine.md
// §2; dw-coinjoin `denoms`/`settings`, dw-governance `params`, dw-protx
// `params`). Used where no engine adapter answers yet (the placeholder
// services and demo mode), so those values never drift from Core's.
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

    /// `governance_params(network)`.
    public static func governanceParameters(_ network: DashNetwork) -> GovernanceParameters {
        let (start, cycle, window, quorum): (UInt32, UInt32, UInt32, Int)
        switch network {
        case .mainnet: (start, cycle, window, quorum) = (614_820, 16_616, 1_662, 10)
        case .testnet, .devnet: (start, cycle, window, quorum) = (4_200, 24, 8, 1)
        case .regtest: (start, cycle, window, quorum) = (1_500, 20, 10, 1)
        }
        return GovernanceParameters(
            superblockStartHeight: start, superblockCycle: cycle, maturityWindow: window, minQuorum: quorum,
            proposalFee: Amount(duffs: coin), feeConfirmations: 6, maxNameLength: 40, maxPayloadBytes: 512,
            maxPayments: 12, evonodeVoteWeight: 4, voteUpdateMinimum: .seconds(3_600), targetSpacing: .seconds(150))
    }

    /// `masternode_network_defaults(network)`.
    public static func masternodeDefaults(_ network: DashNetwork) -> MasternodeNetworkDefaults {
        let (core, p2p, https): (UInt16, UInt16, UInt16)
        switch network {
        case .mainnet: (core, p2p, https) = (9_999, 26_656, 443)
        case .testnet: (core, p2p, https) = (19_999, 22_000, 22_001)
        case .devnet: (core, p2p, https) = (19_799, 22_100, 22_101)
        case .regtest: (core, p2p, https) = (19_899, 22_200, 22_201)
        }
        return MasternodeNetworkDefaults(
            coreP2PPort: core, platformP2PPort: p2p, platformHTTPSPort: https,
            masternodeCollateral: Amount(duffs: 1_000 * coin), evonodeCollateral: Amount(duffs: 4_000 * coin),
            shares: 2...8, minimumShareAmount: Amount(duffs: 100 * coin), maxEarlyPeriodBlocks: 420_480,
            maxEnvelopeBytes: 2 * 1_024 * 1_024, maxOperatorRewardX100: 10_000)
    }

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
