import Foundation
import WalletRuntime

extension L10n {
    /// dash-qt Options dialog copy (research 02 §13; QT-135…141, IOS-104/105).
    public enum Options {
        public static let mainTab = "Main"
        public static let walletTab = "Wallet"
        public static let networkTab = "Network"
        public static let displayTab = "Display"
        public static let appearanceTab = "Appearance"
        public static let notificationsTab = "Notifications"

        public static let startOnLogin = "Start Dash Wallet on system login"
        public static let showTrayIcon = "Show tray icon"
        public static let minimizeToTray = "Minimize to the tray instead of the taskbar"
        public static let minimizeOnClose = "Minimize on close"

        public static let subtractFeeByDefault = "Subtract fee from amount by default"
        public static let coinControl = "Enable coin control features"
        public static let psbtControls = "Enable PSBT controls"
        public static let keepCustomChangeAddress = "Keep custom change address"
        public static let dustProtection = "Enable dust attack protection"
        public static let dustThreshold = "Dust threshold [duffs]"
        public static let dustThresholdInvalid = "The dust threshold must be between 1 and 1,000,000 duffs."
        public static let automaticBackups = "Automatic backups to keep"
        public static let automaticBackupsInvalid = "Keep between 0 and 10 automatic backups."

        public static let proxy = "Connect through SOCKS5 proxy (default proxy):"
        public static let proxyIP = "Proxy IP:"
        public static let proxyPort = "Port:"
        public static let onionProxy = "Use separate SOCKS5 proxy to reach peers via Tor onion services:"
        public static let proxyInvalid = "The supplied proxy address is invalid."
        public static let proxyRequiresEngine = "Proxy support requires an engine update and is not available yet."

        public static let language = "User Interface language:"
        public static let languageDefault = "(Default)"
        public static let unit = "Unit to show amounts in:"
        public static let decimalDigits = "Decimal digits"
        public static let showMasternodesTab = "Show Masternodes Tab"
        public static let showGovernanceTab = "Show Governance Tab"
        public static let showGovernanceClock = "Show governance clock"
        public static let thirdPartyTxURLs = "Third-party transaction URLs"
        public static let localCurrency = "Local currency"

        public static let notificationsEnabled = "Transaction notifications"
        public static let notificationsDenied = "Turned off in system settings"
        public static let notificationsUnavailable = "Notifications are not available on this system."
        public static let showCoinJoinNotifications = "Show popups for CoinJoin transactions"

        public static let restartRequired = "Client restart required to activate changes."
        public static let spvFootnote = "Not applicable to this wallet (SPV):"
        /// dash-qt node options the SPV wallet does not offer (DESIGN-opus §1.14).
        public static let spvOnlyOptions = [
            "Prune block storage", "Size of database cache", "Number of script verification threads",
            "Enable RPC server", "Map port using UPnP", "Map port using NAT-PMP", "Allow incoming connections",
        ]

        public static let resetOptions = "Reset Options"
        public static let resetTitle = "Confirm options reset"
        public static let resetQuestion =
            "Client restart required to activate changes.\n\nCurrent settings will be backed up next to the settings file.\n\nClient will be shut down. Do you want to proceed?"
        public static let unavailable = "Not available yet."

        public static func languageName(_ code: String?) -> String {
            guard let code else { return languageDefault }
            let name = Locale(identifier: "en").localizedString(forIdentifier: code) ?? code
            return "\(name) (\(code))"
        }

        public static func currencyName(_ code: String) -> String {
            let name = Locale(identifier: "en").localizedString(forCurrencyCode: code) ?? code
            return "\(name) (\(code))"
        }
    }
}
