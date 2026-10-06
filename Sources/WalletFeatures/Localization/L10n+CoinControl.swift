import Foundation
import WalletRuntime

extension L10n {
    /// dash-qt coin control copy (`coincontroldialog.cpp`, `sendcoinsdialog.cpp`;
    /// research 02 §5.6–5.7; QT-068…074).
    public enum CoinControl {
        public static let title = "Coin Selection"
        public static let featuresHeader = "Coin Control Features"
        public static let inputs = "Inputs…"
        public static let automaticallySelected = "automatically selected"
        public static let insufficientFunds = "Insufficient funds!"
        public static let quantity = "Quantity:"
        public static let bytes = "Bytes:"
        public static let amount = "Amount:"
        public static let fee = "Fee:"
        public static let afterFee = "After Fee:"
        public static let change = "Change:"
        public static let selectAll = "(un)select all"
        public static let lockAll = "(un)lock all"
        public static let treeMode = "Tree mode"
        public static let listMode = "List mode"
        public static let showAllCoins = "Show all coins"
        public static let hideCoinJoinCoins = "Hide CoinJoin coins"
        public static func lockedCount(_ count: Int) -> String { "(\(count) locked)" }

        public static let columnAmount = "Amount"
        public static let columnLabel = "Received with label"
        public static let columnAddress = "Received with address"
        public static let columnMixingRounds = "Mixing Rounds"
        public static let columnDate = "Date"
        public static let columnConfirmations = "Confirmations"
        public static let noLabel = "(no label)"
        public static let changeLabel = "(change)"

        public static let copyQuantity = "Copy quantity"
        public static let copyAmount = "Copy amount"
        public static let copyFee = "Copy fee"
        public static let copyAfterFee = "Copy after fee"
        public static let copyBytes = "Copy bytes"
        public static let copyChange = "Copy change"
        public static let copyAddress = "Copy address"
        public static let copyLabel = "Copy label"
        public static let copyOutpoint = "Copy transaction ID and output index"
        public static let lockUnspent = "Lock unspent"
        public static let unlockUnspent = "Unlock unspent"

        public static func tolerance(_ duffs: Int64) -> String {
            duffs == 1 ? "Can vary +/- 1 duff per input." : "Can vary +/- \(duffs) duffs per input."
        }
        public static let coinsUnselected = "Some coins were unselected because they were spent."

        public static let customChange = "Custom change address"
        public static let invalidChangeAddress = "Warning: Invalid Dash address"
        public static let unknownChangeAddress = "Warning: Unknown change address"
        public static let confirmChangeTitle = "Confirm custom change address"
        public static let confirmChangeQuestion =
            "The address you selected for change is not part of this wallet. Any or all funds in your wallet may be sent to this address. Are you sure?"

        public static func columnTitle(_ column: CoinColumn) -> String {
            switch column {
            case .amount: columnAmount
            case .label: columnLabel
            case .address: columnAddress
            case .mixingRounds: columnMixingRounds
            case .date: columnDate
            case .confirmations: columnConfirmations
            }
        }
    }
}
