import Foundation

extension L10n {
    /// dash-qt Overview copy (QT-034, QT-037, QT-039) and sync row (QT-025, IOS-023).
    public enum Home {
        public static let available = "Available:"
        public static let pending = "Pending:"
        public static let immature = "Immature:"
        public static let total = "Total:"
        public static let availableTooltip = "Your current spendable balance"
        public static let pendingTooltip =
            "Total of transactions that have yet to be confirmed, and do not yet count toward the spendable balance"
        public static let immatureTooltip = "Mined balance that has not yet matured"
        public static let outOfSync = "(out of sync)"
        public static let outOfSyncTooltip =
            "The displayed information may be out of date. Your wallet automatically synchronizes with the Dash network after a connection is established, but this process has not completed yet."
        public static let discreetModeTip =
            "Discreet mode activated for the Overview tab. To unmask the values, uncheck Settings->Discreet mode."
        public static let connectingToPeers = "Connecting to peers…"
        public static let synchronizing = "Synchronizing with network…"
        public static let synced = "Up to date"
        public static let notConnected = "Not connected"
        public static let stalled = "Sync has stalled. Try other peers."
        public static let changePeers = "Change peers"
        public static let headers = "Headers"
        public static let filterHeaders = "Filter Headers"
        public static let filters = "Filters"
        public static let masternodes = "Masternode List"

        public static func syncingPhase(_ phase: String, percent: Int) -> String {
            "Syncing \(phase) (\(percent)%)…"
        }
    }
}
