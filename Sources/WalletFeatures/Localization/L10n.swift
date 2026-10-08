// English source strings, one table per feature (DESIGN-opus §5.3 rule 3).
// dash-qt strings are copied exactly where a checklist item asks for them.
// The translation pipeline (Transifex import) replaces these tables later.
import Foundation

public enum L10n {}

extension L10n {
    public enum Common {
        public static let unknown = "Unknown"
        public static let notAvailableYet = "This feature is not available yet."
        public static let networkNotOpen = "The wallet is not open yet. Please wait and try again."
        public static let walletNotFound = "The selected wallet could not be found."
        public static let storage = "The wallet data could not be read or written."
        public static let unexpected = "An unexpected error occurred."
        public static let noWallet = "No wallet loaded."
        public static let wrongPassphrase = "The passphrase entered for the wallet decryption was incorrect."
        public static let unlockCancelled = "Wallet unlock was cancelled."
        public static let passphraseRequired = "Enter your wallet passphrase."
        public static let vaultLocked = "The wallet is locked. Unlock it first."
        public static let mixingOnly = "The wallet is unlocked for mixing only. Unlock it fully first."
        public static let credentialRequired = "Enter your passphrase."
        public static let throttled = "Too many incorrect passphrase attempts. Please wait and try again."
    }

    public enum Navigation {
        public static let overview = "Overview"
        public static let send = "Send"
        public static let receive = "Receive"
        public static let transactions = "Transactions"
        public static let coinJoin = "CoinJoin"
        public static let contacts = "Contacts"
        public static let explore = "Explore"
        public static let appName = "Dash Wallet"
    }
}
