import Foundation
import WalletRuntime

extension L10n {
    /// dash-qt transaction context menu and details dialog copy (research 02
    /// §4.5–4.6; QT-075, QT-090…092) and iOS history copy (IOS-027…032).
    public enum TransactionsM2 {
        public static let copyAddress = "Copy address"
        public static let copyLabel = "Copy label"
        public static let copyAmount = "Copy amount"
        public static let copyTransactionID = "Copy transaction ID"
        public static let copyRawTransaction = "Copy raw transaction"
        public static let copyFullDetails = "Copy full transaction details"
        public static let showDetails = "Show transaction details"
        public static let abandon = "Abandon transaction"
        public static let resend = "Resend transaction"
        public static let unlockDust = "Unlock dust UTXO"
        public static let editLabel = "Edit address label"
        public static let showQRCode = "Show address QR code"
        public static func showIn(_ host: String) -> String { "Show in \(host)" }
        public static let noLabel = "(no label)"

        public static let abandonTitle = "Abandon transaction"
        public static let abandonQuestion =
            "Abandon this transaction? Its inputs become spendable again and its amount no longer counts toward the balance."
        public static let abandonMayConfirm =
            "This wallet cannot see whether the network still holds the transaction, so it may still confirm."
        public static let abandoned = "The transaction was abandoned."
        public static let resent = "The transaction was announced to the connected peers again."
        public static let dustUnlocked = "The dust outputs were unlocked."

        public static func refusal(_ refusal: TransactionActionRefusal) -> String {
            switch refusal {
            case .confirmed: "This transaction is already confirmed."
            case .instantLocked: "This transaction is locked by InstantSend."
            case .alreadyAbandoned: "This transaction is already abandoned."
            case .coinbase: "Mined transactions cannot be abandoned or resent."
            case .inMempool: "The network still has this transaction."
            case .notSentByWallet: "This transaction was not sent by this wallet."
            }
        }

        // Details dialog (QT-092)
        public static func detailsTitle(_ txid: String) -> String { "Details for \(txid)" }
        public static let status = "Status"
        public static let date = "Date"
        public static let type = "Type"
        public static let source = "Source"
        public static let generated = "Generated"
        public static let platformTransfer = "Platform Transfer"
        public static let from = "From"
        public static let to = "To"
        public static let unknown = "unknown"
        public static let ownAddress = "own address"
        public static let watchOnly = "watch-only"
        public static let label = "label"
        public static let credit = "Credit"
        public static func maturesIn(_ blocks: UInt32) -> String {
            blocks == 1 ? "matures in 1 more block" : "matures in \(blocks) more blocks"
        }
        public static let debit = "Debit"
        public static let totalDebit = "Total debit"
        public static let totalCredit = "Total credit"
        public static let fee = "Transaction fee"
        public static let net = "Net amount"
        public static let message = "Message"
        public static let comment = "Comment"
        public static let transactionID = "Transaction ID"
        public static let totalSize = "Transaction total size"
        public static let payload = "Payload"
        public static func bytes(_ count: UInt32) -> String { "\(count) bytes" }
        public static func conflicted() -> String { "conflicted with a transaction" }
        public static let inMempool = "0/unconfirmed, in memory pool"
        public static let notInMempool = "0/unconfirmed, not in memory pool"
        public static let unconfirmed = "0/unconfirmed"
        public static let abandonedSuffix = ", abandoned"
        public static func depthUnconfirmed(_ depth: UInt32) -> String { "\(depth)/unconfirmed" }
        public static func confirmations(_ depth: UInt32) -> String { "\(depth) confirmations" }
        public static let chainLockedSuffix = ", locked via ChainLocks"
        public static let instantSendSuffix = ", verified via InstantSend"
        public static let maturityNote =
            "Generated coins must mature 100 blocks before they can be spent. When you generated this block, it was broadcast to the network to be added to the block chain. If it fails to get into the chain, its state will change to \"not accepted\" and it won't be spendable. This may occasionally happen if another node generates a block within a few seconds of yours."

        // iOS history (IOS-027…032)
        public static let chipSent = "Sent"
        public static let chipReceived = "Received"
        public static let chipRewards = "Rewards"
        public static let chipMasternode = "Masternode"
        public static let all = "All"
        public static let only = "Only"
        public static let dateUnknown = "Date unknown"
        public static let mixingTransactions = "Mixing Transactions"
        public static func mixingCount(_ count: Int) -> String {
            count == 1 ? "1 transaction" : "\(count) transactions"
        }
        public static let insight = "Insight"
        public static let blockchair = "Blockchair"
    }
}
