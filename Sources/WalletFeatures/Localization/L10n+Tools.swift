import Foundation
import WalletRuntime

extension L10n {
    /// dash-qt Tools window copy (research 02 §14; QT-040, QT-143…148, IOS-113).
    public enum Tools {
        public static let windowTitle = "Tools window"
        public static let none = "—"
        public static let requiresFullNode = "Requires full-node data source"

        // Information
        public static let general = "General"
        public static let clientVersion = "Client version"
        public static let userAgent = "User Agent"
        public static let datadir = "Datadir"
        public static let startupTime = "Startup time"
        public static let network = "Network"
        public static let name = "Name"
        public static let connections = "Number of connections"
        public static let localAddresses = "Local Addresses"
        public static let blockChain = "Block chain"
        public static let blockHeight = "Current block height"
        public static let lastBlockTime = "Last block time"
        public static let lastBlockHash = "Last block hash"
        public static let memoryPool = "Memory Pool"
        public static let mempoolCount = "Current number of transactions"
        public static let mempoolUsage = "Memory usage"
        public static let masternodes = "Masternodes"
        public static let evonodes = "EvoNodes"
        public static let chainLocks = "ChainLocks"
        public static func connectionCount(total: Int, inbound: Int, outbound: Int) -> String {
            "\(total) (In: \(inbound) / Out: \(outbound))"
        }
        public static func masternodeCount(total: Int, enabled: Int) -> String { "Total: \(total) (Enabled: \(enabled))" }
        public static func networkName(_ network: DashNetwork) -> String {
            switch network {
            case .mainnet: "main"
            case .testnet: "test"
            case .devnet(let name): "devnet-\(name)"
            case .regtest: "regtest"
            }
        }

        // Alert banner (QT-040)
        public static let prereleaseBuild =
            "This is a pre-release test build - use at your own risk - do not use for mining or merchant applications"
        public static let uncleanShutdown = "The wallet was not shut down cleanly last time. It is checking its data."
        public static let syncStalled = "Sync has not made progress for a while. Try changing peers."
        public static let clockSkew =
            "Please check that your computer's date and time are correct! If your clock is wrong, Dash Wallet will not work properly."
        public static let platformContextUnavailable = "Dash Platform data cannot be verified right now."

        // Console (QT-145)
        public static func welcome(clear: String, bigger: String, smaller: String) -> String {
            """
            Welcome to the Dash Wallet RPC console.
            Use up and down arrows to navigate history, and \(clear) to clear screen.
            Use \(bigger) and \(smaller) to increase or decrease the font size.
            Type help for an overview of available commands.
            For more information on using this console, type help-console.
            """
        }
        public static let scamWarning =
            "WARNING: Scammers have been active, telling users to type commands here, stealing their wallet contents. Do not use this console without fully understanding the ramifications of a command."
        public static let executing = "Executing…"
        public static func executingWithWallet(_ name: String) -> String { "Executing command using \"\(name)\" wallet" }
        public static let executingWithoutWallet = "Executing command without any wallet"
        public static let invalidCommandLine = "Error: Invalid command line"
        public static let notAvailableInSPV = "Not available in SPV mode."
        public static let walletRequired =
            "Wallet file not specified (must request wallet RPC through /wallet/<filename> uri-path)."
        public static let noWalletSelection = "(none)"
        public static let authorizationCancelled = "Authorization cancelled."

        // Peers (QT-147)
        public static let copyAddress = "Copy address"
        public static let disconnect = "Disconnect"
        public static let banHour = "Ban for 1 hour"
        public static let banDay = "Ban for 1 day"
        public static let banWeek = "Ban for 1 week"
        public static let banYear = "Ban for 1 year"
        public static let bannedSubnet = "IP/Netmask"
        public static let bannedUntil = "Banned Until"
        public static let copySubnet = "Copy IP/Netmask"
        public static let unban = "Unban"

        // Repair (QT-117, QT-148, IOS-034, IOS-113)
        public static let rescan = "Rescan Chain"
        public static let rescanFull = "Rescan Chain (full)"
        public static let resetChainData = "Reset chain data and resync"
        public static let resetChainDataQuestion =
            "This deletes the downloaded block headers and filters and syncs them again. Your wallets, keys and settings are kept. Continue?"
        public static let rescanUnavailable = "Rescan unavailable"
        public static let rescanFailed = "Rescan failed. Potentially corrupted data files."
        public static let cancelRescan = "Cancel"
        public static let dropUnconfirmed = "Remove unconfirmed transactions"
        public static let dropUnconfirmedQuestion =
            "Abandon every unconfirmed transaction that the network does not have and rescan? A transaction that reached the network may still confirm."
        public static func droppedUnconfirmed(_ count: Int) -> String {
            count == 1 ? "1 transaction was removed; a rescan was scheduled."
                : "\(count) transactions were removed; a rescan was scheduled."
        }
        public static let birthHeight = "Wallet birth height"
        public static let birthHeightInvalid = "Enter a block height."
        public static func rescanProgress(current: UInt32, target: UInt32) -> String {
            "Rescanning… \(current) / \(target)"
        }
    }
}
