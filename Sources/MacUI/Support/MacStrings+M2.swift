// English copy for the macOS chrome of the M2 windows (buttons, headers,
// help tags). Feature copy, including every dash-qt string, lives in
// WalletFeatures `L10n`.
#if os(macOS)
import Foundation

extension MacStrings.Common {
    static let choose = "Choose…"
    static let next = "Next"
    static let saveEllipsis = "Save…"
    static let refresh = "Refresh"
    static let apply = "Apply"
    static let yes = "Yes"
    static let no = "No"
}

extension MacStrings {
    enum Shell {
        static let reset = "Reset"
        static let abort = "Abort"
    }

    enum Dock {
        static let showHide = "Show / Hide"
        static let receive = "Receive…"
    }

    enum About {
        static let logsFileName = "dash-wallet-logs.zip"
    }

    enum Options {
        static let general = "General"
        static let security = "Security"
        static let thirdPartyHelp =
            "Third-party URLs (e.g. a block explorer) that appear in the transactions tab as context menu items. %s in the URL is replaced by the transaction hash. Multiple URLs are separated by a vertical bar |."
    }

    enum Security {
        static let quickUnlockUnavailable = "Not available on this Mac or with an unencrypted wallet"
    }

    enum Wallets {
        static let windowTitle = "Wallets"
        static let walletsHeader = "Wallets"
        static let none = "No wallets on this network yet."
        static let loadStatesUnavailable = "Closed wallets cannot be listed yet; only open wallets are shown."
        static let add = "Add Wallet"
        static let importFile = "Import Wallet File…"
        static let importKeyMaterial = "Import Keys from Dash Core…"
        static let addWatchOnly = "Add Watch-Only Wallet…"
        static let open = "Open"
        static let close = "Close"
        static let closeAll = "Close All"
        static let more = "Wallet"
        static let rename = "Rename…"
        static let remove = "Remove…"
        static let exportForCore = "Export for Dash Core…"
        static let unnamed = "Unnamed wallet"
        static let watchOnly = "Watch-only"
        static let loaded = "Open"
        static let notLoaded = "Closed"
        static let loadOnStartup = "Open at startup"
        static let working = "Working…"
        static let name = "Name"
        static let kind = "Kind"
        static let hdSeed = "HD seed (hex)"
        static let xprv = "Extended private key"
        static let descriptors = "Descriptors"
        static let importAction = "Import"
        static let format = "Format"
        static let dumpWallet = "dumpwallet text file"
        static let descriptorsJSON = "importdescriptors JSON"
        static let exportWarning = "The exported file holds private keys. Anyone with it can take your funds."
        static let backupPassphrase = "Backup passphrase (optional)"
        static let backupPassphraseHelp =
            "The backup holds the wallet's keys. Without a passphrase it is protected only by where you keep it."
        static let passphrasesDiffer = "The passphrases do not match."
    }

    enum PeerTools {
        static let ban = "Ban"
    }

    enum Console {
        static let placeholder = "Enter a command (type help for a list)"
        static let bigger = "Increase font size"
        static let smaller = "Decrease font size"
        static let clear = "Clear console"
    }

    enum CoinControl {
        static let empty = "No coins."
        static let reserved = "Reserved by a payment in progress"
        static let locked = "Locked"
        static let customChangeUnavailable =
            "Sending to a custom change address is not available yet; change returns to this wallet."
    }

    enum PSBT {
        static let empty =
            "No transaction loaded. Use File ▸ Load PSBT from file or Load PSBT from clipboard, or drop a .psbt file here."
    }
}

extension MacStrings.Transactions {
    static let tableLayout = "dash-qt table"
    static let historyLayout = "History by day"
    /// Segment titles of the layout switch (UX-SPEC §4.9).
    static let listLayout = "List"
    static let tableLayoutShort = "Table"
    static let actionResult = "Transaction"
}

extension MacStrings.Overview {
    static let replaceShortcut = "Replace With"
    static let resetShortcuts = "Reset Shortcuts"
}

extension MacStrings.MenuBar {
    static let payFromClipboard = "Pay from Clipboard"
    static let requestAmount = "Request amount (optional)"
    static let copyRequest = "Copy Request"
}
#endif
