import Foundation
import WalletRuntime

extension L10n {
    /// dash-qt main-window menus, status icons, splash, data-directory chooser
    /// and shutdown window (research 02 §1, §2; QT-004…018, QT-021/022).
    public enum Shell {
        public static let fileMenu = "File"
        public static let settingsMenu = "Settings"
        public static let windowMenu = "Window"
        public static let helpMenu = "Help"

        public static let createWallet = "Create Wallet…"
        public static let createWalletTip = "Create a new wallet"
        public static let openWallet = "Open Wallet"
        public static let openWalletTip = "Open a wallet"
        public static let noWalletsAvailable = "No wallets available"
        public static let closeWallet = "Close Wallet…"
        public static let closeWalletTip = "Close wallet"
        public static let closeAllWallets = "Close All Wallets…"
        public static let closeAllWalletsTip = "Close all wallets"
        public static let migrateWallet = "Migrate Wallet"
        public static let migrateWalletUnavailable = "Migrating legacy wallets is not available yet."
        public static let backupWallet = "Backup Wallet…"
        public static let backupWalletTip = "Backup wallet to another location"
        public static let restoreWallet = "Restore Wallet…"
        public static let restoreWalletTip = "Restore a wallet from a backup file"
        public static let openURI = "Open URI…"
        public static let openURITip = "Open a dash: URI"
        public static let signMessage = "Sign message…"
        public static let signMessageTip = "Sign messages with your Dash addresses to prove you own them"
        public static let verifyMessage = "Verify message…"
        public static let verifyMessageTip = "Verify messages to ensure they were signed with specified Dash addresses"
        public static let loadPSBTFromFile = "Load PSBT from file…"
        public static let loadPSBTFromFileTip = "Load Partially Signed Dash Transaction"
        public static let loadPSBTFromClipboard = "Load PSBT from clipboard…"
        public static let loadPSBTFromClipboardTip = "Load Partially Signed Dash Transaction from clipboard"
        public static let openDebugLog = "Open debug log file"
        public static let openDebugLogUnavailable =
            "The log files are collected by Help ▸ About ▸ Export Logs; there is no single debug log to open."
        public static let openConfigurationFile = "Open wallet configuration file"
        public static let openConfigurationFileUnavailable =
            "Not applicable to this wallet (SPV): there is no dash.conf configuration file."
        public static let showAutomaticBackups = "Show Automatic Backups"
        public static let showAutomaticBackupsTip = "Show automatically created wallet backups"
        public static let exit = "Exit"
        public static let exitTip = "Quit application"

        public static let encryptWallet = "Encrypt Wallet…"
        public static let encryptWalletTip = "Encrypt the private keys that belong to your wallet"
        public static let changePassphrase = "Change Passphrase…"
        public static let changePassphraseTip = "Change the passphrase used for wallet encryption"
        public static let showRecoveryPhrase = "Show Recovery Phrase…"
        public static let showRecoveryPhraseTip = "Show the recovery phrase of the current wallet"
        public static let unlockWallet = "Unlock Wallet…"
        public static let unlockWalletTip = "Unlock wallet"
        public static let unlockWalletForMixing = "Unlock Wallet for mixing only…"
        public static let unlockWalletForMixingTip = "Unlock wallet for mixing only; sending stays locked"
        public static let showHide = "Show / Hide"
        public static let lockWallet = "Lock Wallet"
        public static let lockWalletTip = "Lock wallet"
        public static let discreetMode = "Discreet mode"
        public static let discreetModeTip = "Mask the values in the Overview tab"
        public static let options = "Options…"
        public static let optionsTip = "Modify configuration options for Dash Wallet"

        public static let minimize = "Minimize"
        public static let sendingAddresses = "Sending addresses"
        public static let receivingAddresses = "Receiving addresses"
        public static let information = "Information"
        public static let console = "Console"
        public static let networkTraffic = "Network Traffic"
        public static let networkTrafficUnavailable = "The network traffic graph is not available yet."
        public static let peers = "Peers"
        public static let repair = "Repair"

        public static let commandLineOptions = "Command-line options"
        public static let commandLineOptionsTip = "Show the Dash Wallet help message to get a list with possible command-line options"
        public static let coinJoinInformation = "CoinJoin information"
        public static let about = "About Dash Wallet"
        public static let aboutTip = "Show information about Dash Wallet"
        public static let walletRequired = "Open or create a wallet first."
        public static let noRecoveryPhrase = "This wallet has no recovery phrase."

        public static let closeWalletTitle = "Close wallet"
        public static func closeWalletQuestion(_ name: String) -> String {
            "Are you sure you wish to close the wallet \(name)?"
        }
        public static let closeAllWalletsTitle = "Close all wallets"
        public static let closeAllWalletsQuestion = "Are you sure you wish to close all wallets?"

        public static let hdEnabled = "HD key generation is enabled"
        public static let lockedTooltip = "Wallet is encrypted and currently locked"
        public static let unlockedTooltip = "Wallet is encrypted and currently unlocked"
        public static let mixingOnlyTooltip = "Wallet is encrypted and currently unlocked for mixing only"

        // Splash (QT-005).
        public static let loadingSettings = "Loading settings…"
        public static func openingNetwork(_ network: DashNetwork) -> String {
            "Opening \(L10n.Settings.networkName(network))…"
        }
        public static let loadingWallets = "Loading wallet…"
        public static let startingSync = "Connecting to peers…"
        public static let doneLoading = "Done loading"
        public static let startupFailed = "The wallet could not be started."
        public static let pressQToQuit = "Press Q to quit"

        // Corrupt settings (QT-007, dash-qt `InitSettings`).
        public static let settingsUnreadable = "Settings file could not be read."
        public static let settingsResetQuestion =
            "Do you want to reset settings to default values, or to abort without making changes?"
        public static let settingsUnwritable = "Settings file could not be written."
        public static let settingsUnwritableDetail = "A fatal error occurred. Check that settings file is writable."

        // Data directory chooser (QT-004, `intro.cpp`).
        public static let welcomeTitle = "Welcome"
        public static let welcome = "Welcome to Dash Wallet."
        public static let welcomeDetail =
            "As this is the first time the program is launched, you can choose where Dash Wallet will store its data."
        public static let useDefaultDirectory = "Use the default data directory"
        public static let useCustomDirectory = "Use a custom data directory:"
        public static let willCreate = "A new data directory will be created."
        public static let exists = "Directory already exists. Add /name if you intend to create a new directory here."
        public static let notADirectory = "Path already exists, and is not a directory."
        public static let cannotCreate = "Cannot create data directory here."
        public static func spaceAvailable(_ gigabytes: Int64) -> String {
            gigabytes == 1 ? "1 GB of space available" : "\(gigabytes) GB of space available"
        }
        public static let errorTitle = "Error"
        public static func cannotCreateDirectory(_ path: String) -> String {
            "Error: Specified data directory \"\(path)\" cannot be created."
        }

        // Shutdown window (QT-008).
        public static let shuttingDown = "Dash Wallet is shutting down…"
        public static let doNotShutDown = "Do not shut down the computer until this window disappears."
    }
}
