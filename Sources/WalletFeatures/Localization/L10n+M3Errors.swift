import Foundation
import WalletRuntime

extension L10n {
    /// Copy for the M3 error codes (m3-engine.md §4), chosen by code and the
    /// numeric `parameters`, never from `ServiceError.detail`.
    public enum M3Errors {
        public static let coinJoinDisabled = "CoinJoin is disabled in the options."
        public static let coinJoinWatchOnly = "Watch-only wallets cannot mix."
        public static func coinJoinMinimum(_ amount: String) -> String { "CoinJoin requires at least \(amount) to use." }
        public static let coinJoinNothingToMove = "There are no mixed coins to move."
        public static let spvNotRunning = "Sync is not running. Start it and try again."
        public static let noPeers = "No peers are connected. Try again when the wallet is connected to the network."
        public static let broadcastRejected = "The network rejected the transaction."
        public static let grantInvalid = "The authorization expired. Please try again."
        public static let watchOnly = "Watch-only wallets have no private keys."
    }
}

extension ErrorText {
    /// Copy for an M3 error (any domain), falling back to the M2 and common
    /// tables. `amount` formats the duff parameters in the display unit.
    public static func m3(_ error: ServiceError, amount: (Amount) -> String) -> String {
        typealias E = L10n.M3Errors
        switch error.code {
        case .coinjoinDisabled: return E.coinJoinDisabled
        case .coinjoinWatchOnly: return E.coinJoinWatchOnly
        case .coinjoinInsufficientFunds:
            let minimum = error.parameters["min_duffs"].map { Amount(duffs: $0) }
            return E.coinJoinMinimum(amount(minimum ?? M3Defaults.coinJoinLimits.minimumMixingBalance))
        case .coinjoinVaultLocked, .masternodeVaultLocked: return L10n.Common.vaultLocked
        case .coinjoinGrantInvalid, .masternodeGrantInvalid: return E.grantInvalid
        case .coinjoinNothingToMove: return E.coinJoinNothingToMove
        case .coinjoinSpvNotRunning: return E.spvNotRunning
        case .coinjoinNoPeers: return E.noPeers
        case .coinjoinBroadcastRejected: return E.broadcastRejected
        case .masternodeWatchOnly: return E.watchOnly
        default: return m2(error.code)
        }
    }
}
