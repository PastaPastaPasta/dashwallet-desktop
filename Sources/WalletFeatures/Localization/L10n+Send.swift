import Foundation

extension L10n {
    /// dash-qt `SendCoinsDialog` copy (QT-059, QT-060, QT-062, QT-067) plus
    /// iOS send guards (IOS-051).
    public enum Send {
        public static let invalidAddress = "The recipient address is not valid. Please recheck."
        public static let invalidAmount = "The amount to pay must be larger than 0."
        public static let amountExceedsBalance = "The amount exceeds your balance."
        public static let creationFailed = "Transaction creation failed!"
        public static let insufficientMixedFunds =
            "Unable to locate enough mixed funds for this transaction. CoinJoin uses exact denominated amounts to send funds, you might simply need to mix some more coins."
        public static let dustAmount = "Transaction amount too small"
        public static let amountTooLarge = "The amount is larger than 21 million DASH."
        public static let unparsableAmount = "The amount is not a valid number."
        public static let platformAddress = "This is a Dash Platform address, not a Dash Core address"
        public static let shieldedAddress = "Sending to shielded addresses is not available yet."
        public static let txTooLarge = "Transaction too large"
        public static let preselectedCoinsInsufficient =
            "The preselected coins total amount does not cover the transaction target. Please allow other inputs to be selected automatically or include more coins manually."
        public static let invalidChangeAddress = "Warning: Invalid Dash address"
        public static let watchOnly = "This wallet cannot sign transactions."
        public static let preparedTxSpent =
            "The coins of this transaction were spent meanwhile. Review the payment again."
        public static let noPeers = "No connected peers. The transaction was not sent."
        public static let broadcastRejected = "The transaction was rejected by the network."
        public static let grantExceeded = "The amount is larger than this authorization allows. Enter your passphrase."
        public static let syncing = "Please wait until the wallet has finished synchronizing."
        public static let offline = "The wallet is offline. Connect to the network to send."
        public static let noRecipients = "Add a recipient."
        public static let duplicateTitle = "Confirm duplicate recipients"
        public static let duplicateText = "Duplicate address found: addresses should only be used once each."
        public static let confirmTitle = "Confirm send coins"
        public static let confirmQuestion = "Do you want to create this transaction?"
        public static let confirmReview = "Please, review your transaction."
        public static let usingAnyFunds = "using any available funds"
        public static let usingCoinJoinFunds = "using CoinJoin funds only"
        public static let coinJoinFeeNote =
            "(CoinJoin transactions have higher fees usually due to no change output being allowed)"
        public static let send = "Send"
        public static let sendMixedFunds = "Send mixed funds"
        public static let customFeeTooLow =
            "A too low fee might result in a never confirming transaction (read the tooltip)"

        public static func networkMismatch(_ network: String) -> String {
            "This address is not a \(network) address."
        }

        /// `fee` `nil` when no estimate exists yet.
        public static func amountWithFeeExceedsBalance(_ fee: String?) -> String {
            guard let fee else { return "The total exceeds your balance when the transaction fee is included." }
            return "The total exceeds your balance when the \(fee) transaction fee is included."
        }

        public static func absurdFee(_ maximum: String) -> String {
            "A fee higher than \(maximum) is considered an absurdly high fee."
        }

        public static func sendCountdown(_ seconds: Int) -> String { "Send (\(seconds))" }

        public static func recipientLine(amount: String, label: String?, address: String) -> String {
            if let label, !label.isEmpty { return "\(amount) to '\(label)' (\(address))" }
            return "\(amount) to \(address)"
        }

        public static func entriesDisplayed(_ shown: Int, of total: Int) -> String {
            "(\(shown) of \(total) entries displayed)"
        }

        public static func transactionFee(_ fee: String) -> String { "Transaction fee: \(fee)" }

        public static func sizeAndRate(kilobytes: String, rate: String) -> String {
            "Transaction size: \(kilobytes) kB, fee rate: \(rate)/kB"
        }

        public static func totalAmount(_ amount: String) -> String { "Total Amount: \(amount)" }

        public static func inputCount(_ count: Int) -> String {
            count == 1 ? "This transaction will consume 1 input" : "This transaction will consume \(count) inputs"
        }
    }
}
