import Foundation
import WalletRuntime

extension L10n {
    /// dash-qt PSBT copy (`psbtoperationsdialog.cpp`, `sendcoinsdialog.cpp`;
    /// research 02 §5.8; QT-076…079).
    public enum PSBT {
        public static let dialogTitle = "PSBT Operations"
        public static let signTx = "Sign Tx"
        public static let broadcastTx = "Broadcast Tx"
        public static let copyToClipboard = "Copy to Clipboard"
        public static let save = "Save…"
        public static let close = "Close"
        public static let createUnsigned = "Create Unsigned"

        public static func sends(_ amount: String, to address: String) -> String { " * Sends \(amount) to \(address)" }
        public static let ownAddress = "own address"
        public static let unknownAddress = "unknown address"
        public static let feeUnknown = "Unable to calculate transaction fee or total transaction amount."
        public static func paysFee(_ fee: String) -> String { "Pays transaction fee: \(fee)" }
        public static let totalAmount = "Total Amount"
        public static func unsignedInputs(_ count: Int) -> String { "Transaction has \(count) unsigned inputs." }

        public static let missingInputInfo = "Transaction is missing some information about inputs."
        public static let needsSignatures = "Transaction still needs signature(s)."
        public static let noWallet = "(But no wallet is loaded.)"
        public static let cannotSign = "(But this wallet cannot sign transactions.)"
        public static let noMatchingKeys = "(But this wallet does not have the right keys.)"
        public static let complete = "Transaction is fully signed and ready for broadcast."

        public static let signedComplete = "Signed transaction successfully. Transaction is ready to broadcast."
        public static func signedPartially(_ count: Int) -> String {
            "Signed \(count) inputs, but more signatures are still required."
        }
        public static let couldNotSign = "Could not sign any more inputs."
        public static func broadcastSucceeded(_ txid: String) -> String {
            "Transaction broadcast successfully! Transaction ID: \(txid)"
        }
        public static func broadcastFailed(_ reason: String) -> String { "Transaction broadcast failed: \(reason)" }
        public static let copied = "PSBT copied to clipboard."
        public static let saved = "PSBT saved to disk."
        public static func loadFailed(_ reason: String) -> String { "Failed to load transaction: \(reason)" }
        public static let clipboardInvalid = "Unable to decode PSBT from clipboard (invalid base64)"
        public static let unsignedTitle = "Unsigned Transaction"
        public static let unsignedCopied = "The PSBT has been copied to the clipboard. You can also save it."
        public static let saveTitle = "Save Transaction Data"
        public static let fileFilter = "Partially Signed Transaction (Binary) (*.psbt)"
        public static let noExternalAmount = "The amount this transaction sends could not be determined."
    }
}
