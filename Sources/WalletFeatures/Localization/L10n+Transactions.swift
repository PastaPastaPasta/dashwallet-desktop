import Foundation
import WalletRuntime

extension L10n {
    /// dash-qt `TransactionTableModel` / `TransactionView` copy (QT-086…093).
    public enum Transactions {
        public static let searchPlaceholder = "Enter address, transaction id, or label to search"
        public static let minAmountPlaceholder = "Min amount"
        public static let exportTitle = "Export Transaction History"
        public static let selectedAmount = "Selected amount:"
        public static let invalidMinAmount = "The minimum amount is not a valid number."
        public static let exportFailed = "There was an error trying to save the transaction history."

        /// Display strings for each record type (QT-086, research 02 §4.1).
        public static func typeName(_ type: TxType) -> String {
            switch type {
            case .other: ""
            case .generated: "Mined"
            case .sendToAddress, .sendToOther: "Sent to"
            case .recvWithAddress: "Received with"
            case .recvFromOther: "Received from"
            case .sendToSelf: "Payment to yourself"
            case .recvWithCoinJoin: "Received via CoinJoin"
            case .coinJoinMixing: "CoinJoin Mixing"
            case .coinJoinCollateralPayment: "CoinJoin Collateral Payment"
            case .coinJoinMakeCollaterals: "CoinJoin Make Collateral Inputs"
            case .coinJoinCreateDenominations: "CoinJoin Create Denominations"
            case .coinJoinSend: "CoinJoin Send"
            case .platformTransfer: "Platform Transfer"
            case .dustReceive: "Dust Receive"
            case .dataTransaction: "Data Transaction"
            case .masternodeRegistration: "Masternode Registration"
            case .masternodeUpdate: "Masternode Update"
            case .assetLock: "Asset Lock"
            }
        }

        /// Type filter menu entries (QT-089).
        public static func typeFilterName(_ preset: TypeFilterPreset) -> String {
            switch preset {
            case .all: "All"
            case .mostCommon: "Most Common"
            case .receivedWith: "Received with"
            case .sentTo: "Sent to"
            case .coinJoinSend: "CoinJoin Send"
            case .coinJoinMakeCollaterals: "CoinJoin Make Collateral Inputs"
            case .coinJoinCreateDenominations: "CoinJoin Create Denominations"
            case .coinJoinMixing: "CoinJoin Mixing"
            case .coinJoinCollateralPayment: "CoinJoin Collateral Payment"
            case .toYourself: "To yourself"
            case .mined: "Mined"
            case .masternode: "Masternode"
            case .platformTransfer: "Platform Transfer"
            case .assetLock: "Asset Lock"
            case .dataTransaction: "Data Transaction"
            case .dustReceive: "Dust Receive"
            case .other: "Other"
            }
        }

        public static func dateFilterName(_ preset: DateFilterPreset) -> String {
            switch preset {
            case .all: "All"
            case .today: "Today"
            case .thisWeek: "This week"
            case .thisMonth: "This month"
            case .lastMonth: "Last month"
            case .thisYear: "This year"
            case .range: "Range…"
            }
        }

        /// Status column tooltip (QT-087).
        public static func statusText(_ status: TxStatus) -> String {
            var text: String
            switch status.kind {
            case .unconfirmed: text = "Unconfirmed"
            case .confirming: text = "Confirming (\(status.confirmations) of 6 recommended confirmations)"
            case .confirmed: text = "Confirmed (\(status.confirmations) confirmations)"
            case .conflicted: text = "Conflicted"
            case .abandoned: text = "Abandoned"
            case .immature:
                if let left = status.maturesIn {
                    text = "Immature (\(status.confirmations) confirmations, will be available after \(left) more blocks)"
                } else {
                    text = "Immature (\(status.confirmations) confirmations)"
                }
            case .notAccepted: text = "Generated but not accepted"
            }
            if status.instantLocked { text += ", verified via InstantSend" }
            if status.chainLocked { text += ", locked via ChainLocks" }
            return text
        }
    }
}
