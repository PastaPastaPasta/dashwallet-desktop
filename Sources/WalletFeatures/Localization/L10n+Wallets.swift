import Foundation
import WalletRuntime

extension L10n {
    /// Wallet management copy: dash-qt File-menu flows (research 02 §12.6, §18)
    /// and iOS Wallets / reinstall detection (research 03 §1.1, §1.16;
    /// QT-101, QT-106…110, QT-114, QT-116, IOS-009, IOS-110, IOS-111).
    public enum Wallets {
        public static let openWalletFailed = "Open wallet failed"
        public static let walletRestored = "Wallet restored successfully"
        public static let backupSuccessful = "Backup Successful"
        public static func backupSaved(_ path: String) -> String { "The wallet data was successfully saved to \(path)." }
        public static let backupFailed = "Backup Failed"
        public static let removeTitle = "Remove wallet"
        public static func removeQuestion(_ name: String) -> String {
            "Remove the wallet \(name) from this computer? Its keys are deleted; you can only restore it from its recovery phrase or a backup."
        }
        public static let berkeleyDBUnavailable =
            "This is a legacy Berkeley DB wallet.dat. Importing it is not available yet; export it from Dash Core with dumpwallet and import that file."
        public static let psbtFile = "This file is a partially signed transaction."
        public static let unknownFile = "This file is not a wallet backup, dumpwallet file or wallet.dat."
        public static func imported(labels: Int) -> String {
            labels == 1 ? "The wallet was imported with 1 label." : "The wallet was imported with \(labels) labels."
        }
        public static func keysNotImported(_ keys: Int, scripts: Int) -> String {
            "\(keys) loose keys and \(scripts) scripts were not imported. Sweep their funds into this wallet instead."
        }
        public static func exported(keys: Int) -> String {
            keys == 1 ? "1 key was exported." : "\(keys) keys were exported."
        }
        public static let mnemonicNotCoreCompatible =
            "Dash Core cannot rebuild this wallet from its recovery phrase with upgradetohd; use the exported file instead."
        public static let coinJoinNotScanned =
            "Legacy Dash Core wallets do not scan the CoinJoin account; mixed funds may not show there."
        public static let invalidSeedHex = "The HD seed must be 32 to 128 hexadecimal characters."
        public static let xpubTitle = "Extended public key"

        public static let existingTitle = "Wallets found on this device"
        public static let existingMessage =
            "Wallet data from a previous installation is still stored on this device. Keep using these wallets, or delete all wallet data and start fresh? Make sure every recovery phrase is backed up before deleting."
        public static let keepWallets = "Keep Wallets"
        public static let deleteAll = "Delete All"
        public static let deleteAllTitle = "Delete All Wallets?"
        public static let deleteAllMessage =
            "This permanently removes all wallets, private keys, and recovery phrases stored by this app on this device. They can only be restored from backups. This cannot be undone."
        /// The sentence the user types to confirm a wipe (iOS `wipeAcceptPhrase`).
        public static let wipeAcceptPhrase =
            "I accept that I will lose my coins if I no longer possess the recovery phrase"
        public static let acceptPhraseMismatch = "Type the sentence exactly as shown."
        public static let typeSentence = "Type the sentence above"
    }

    /// Security copy (research 03 §1.2, §1.16; IOS-011, IOS-014…016, IOS-108/109).
    public enum Security {
        public static let touchID = "Touch ID"
        public static let windowsHello = "Windows Hello"
        public static let quickUnlockReason = "Unlock your Dash wallet"
        public static let spendingLimit = "Quick-unlock spending limit"
        public static let autoLock = "Auto Lock"
        public static let requireAuthentication = "Require authentication for every payment"
        public static let autohideBalance = "Autohide balance"
        public static let forgotPassphrase = "Forgot passphrase?"
        public static let recoverTitle = "Reset passphrase"
        public static let recoverPrompt = "Enter the recovery phrase of one of your wallets."
        public static let invalidPhrase = "This is not a valid recovery phrase."
        public static func walletsWithoutSecrets(_ names: [String]) -> String {
            "These wallets are watch-only until you import their recovery phrases again: \(names.joined(separator: ", "))."
        }
        public static let passphraseReset = "Your passphrase was reset."
        public static let wipeTitle = "Wipe Wallet"
        public static let wiped = "All wallet data was deleted."

        public static func autoLockName(_ interval: AutoLockInterval) -> String {
            switch interval {
            case .immediately: "Immediately"
            case .oneMinute: "1 minute"
            case .fiveMinutes: "5 minutes"
            case .oneHour: "1 hour"
            case .oneDay: "24 hours"
            case .never: "Never"
            }
        }
    }
}
