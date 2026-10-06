import Foundation
import WalletRuntime

extension L10n {
    /// iOS shortcut bar, backup reminder, About and faucet copy (research 03
    /// §1.1, §1.3, §1.16; IOS-005, IOS-025, IOS-107, IOS-121, QT-153).
    public enum HomeM2 {
        public static let backup = "Backup"
        public static let receive = "Receive"
        public static let send = "Send"
        public static let scanQR = "Scan QR"
        public static let sendToAddress = "Send to Address"
        public static let buySell = "Buy & Sell"
        public static let explore = "Explore"
        public static let spend = "Spend"
        public static let atm = "ATM"
        public static let coinbase = "Coinbase"
        public static let uphold = "Uphold"
        public static let topper = "Topper"
        public static let dashDEX = "Dash DEX"
        public static let crowdNode = "CrowdNode"
        public static let testDash = "1 tDash"
        public static let switchWallet = "Switch Wallet"
        public static let nodes = "Nodes"
        public static let laterRelease = "Available in a later release."
        public static let inAppFaucetUnavailable =
            "The in-app faucet is not available yet; the shortcut opens the testnet faucet website."

        public static let backupReminderTitle = "Back up your recovery phrase"
        public static let backupReminderMessage =
            "Your wallet received funds a day ago and its recovery phrase was never backed up. Without it you lose access to your funds if this computer fails."
        public static let backupNow = "Back up now"
        public static let later = "Later"

        public static let aboutTitle = "About Dash Wallet"
        public static func version(_ version: String) -> String { "Version \(version)" }
        public static let network = "Network"
        public static let dataDirectory = "Data directory"
        public static let github = "GitHub"
        public static let support = "Contact support"
        public static let exportLogs = "Export Logs"
        public static let logsExported = "Logs exported."
        public static let license =
            "Distributed under the MIT software license, see the accompanying file COPYING or https://opensource.org/licenses/MIT."
        public static let commandLineTitle = "Command-line options"
        public static let commandLineUsage = "Usage: dash-wallet [command-line options] [URI]"
        public static let coinJoinInfoTitle = "CoinJoin information"

        /// dash-qt `-help` descriptions for the options the desktop wallet keeps (QT-006).
        public static func optionDescription(_ name: String) -> String {
            switch name {
            case "min": "Start minimized"
            case "splash": "Show splash screen on startup (default: 1)"
            case "resetguisettings": "Reset all settings changed in the GUI"
            case "choosedatadir": "Choose data directory on startup (default: 0)"
            case "datadir": "Specify data directory"
            case "testnet": "Use the test chain. Equivalent to -chain=test."
            case "regtest": "Use the regression test chain. Equivalent to -chain=regtest."
            case "devnet": "Use devnet chain with provided name"
            case "chain": "Use the chain <chain> (default: main). Allowed values: main, test, regtest, devnet"
            case "lang": "Set language, for example \"de_DE\" (default: system locale)"
            case "windowtitle": "Sets a window title which is appended to \"Dash Wallet\""
            default: ""
            }
        }
    }
}
