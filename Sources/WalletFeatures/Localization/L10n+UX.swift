import Foundation

extension L10n {
    /// List-mode transaction titles and status chips (UX-SPEC §5.4). English from
    /// dashwallet-iOS where it has the string; `new` marks strings neither iOS
    /// nor dash-qt has.
    public enum TxTitle {
        public static let received = "Received"
        public static let sent = "Sent"
        public static let sentToYourself = "Sent to yourself"
        public static let internalTransfer = "Internal transfer"
        public static let mixing = "Mixing"
        /// dash-qt `TransactionTableModel` type name.
        public static let coinJoinSend = "CoinJoin Send"
        public static let masternodeReward = "Masternode reward"
        public static let mined = "Mined"
        /// new
        public static let providerTransaction = "Provider transaction (ProTx)"
        /// new
        public static let assetLock = "Asset lock"

        public static let chipPending = "Pending"
        public static let chipInstantSend = "InstantSend"
        public static let chipConflicted = "Conflicted"
        public static let chipAbandoned = "Abandoned"
        /// new
        public static let chipNotAccepted = "Not accepted"
        /// iOS: an immature coinbase.
        public static let locked = "Locked"

        public static let today = "Today"
        public static let yesterday = "Yesterday"
    }

    /// Copy of the iOS-style shell surfaces (UX-SPEC §4). iOS strings unless
    /// marked `new`.
    public enum UX {
        public static let history = "History"
        public static let filter = "Filter"
        /// new
        public static let seeAllTransactions = "See all transactions"
        public static let syncingBalance = "Syncing Balance"
        public static let balanceUnavailable = "Balance unavailable"
        /// iOS "Tap to hide balance", adapted.
        public static let clickToHideBalance = "Click to hide balance"
        /// iOS "Tap to show balance", adapted.
        public static let clickToShowBalance = "Click to show balance"
        public static let noTransactions = "There are no transactions to display"
        public static let loadingTransactions = "Loading transactions"
        public static let copied = "Copied"
        public static let receiveDash = "Receive Dash"
        /// new: one-line explanations under the breakdown strip's titles.
        public static let availableHint = "Spendable now"
        public static let pendingHint = "Awaiting confirmation"
        public static let immatureHint = "Mining/masternode rewards maturing"
        /// Breakdown strip titles: dash-qt's labels without the colon.
        public static let available = "Available"
        public static let pending = "Pending"
        public static let immature = "Immature"
        /// new: shown for a value only a full node has.
        public static let requiresFullNode = "Requires full-node data source"
        /// new
        public static let notAvailableUntilSynced = "Not available until sync completes"
        public static let quickReceive = "Quick Receive"
        public static let scanToSend = "Scan to Send"
        public static func syncingPercent(_ percent: Double) -> String {
            String(format: "Syncing %.1f%%", percent * 100)
        }
    }
}
