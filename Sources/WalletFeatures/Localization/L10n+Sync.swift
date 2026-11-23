import Foundation

extension L10n {
    /// dash-qt `ModalOverlay` copy (QT-027), shared by both apps.
    public enum SyncOverlay {
        public static let title = "Recent transactions may not yet be visible"
        public static let body =
            "The wallet is still synchronizing with the Dash network, so its balance may be incorrect. Spending coins from transactions not shown yet will not be accepted by the network."
        public static let status = "Status"
        public static let blocksLeft = "Number of blocks left"
        public static let lastBlockTime = "Last block time"
        public static let progress = "Progress"
        public static let progressPerHour = "Progress increase per hour"
        public static let timeLeft = "Estimated time left until synced"
        public static let hide = "Hide"
        public static let show = "Show synchronization details"
    }

    /// Connected peers (QT-024 "Show Peers", QT-147).
    public enum Peers {
        public static let title = "Peers"
        public static let show = "Show Peers…"
        public static let address = "Address"
        public static let userAgent = "User Agent"
        public static let height = "Height"
        public static let ping = "Ping"
        public static let direction = "Direction"
        public static let inbound = "Inbound"
        public static let outbound = "Outbound"
        public static let none = "Not connected to any peer."
        public static let changePeers = "Change Peers"
        public static let changePeersHelp = "Disconnect from the current peers and connect to others."
    }
}
