import Foundation

extension L10n {
    public enum Lock {
        public static let title = "Unlock wallet"
        public static let prompt = "This operation needs your wallet passphrase to unlock the wallet."
        public static let disabled =
            "Too many incorrect attempts. Restore the wallet with your recovery phrase to set a new passphrase."
        public static let restoreWithPhrase = "Restore with recovery phrase"

        public static func attemptsRemaining(_ count: Int) -> String {
            count == 1 ? "1 attempt remaining" : "\(count) attempts remaining"
        }

        /// iOS lockout copy with the remaining wait, e.g. "5 minutes".
        public static func tryAgainIn(_ wait: String) -> String {
            "Wallet disabled, please try again in \(wait)"
        }

        public static func minutes(_ count: Int) -> String { count == 1 ? "1 minute" : "\(count) minutes" }
        public static func hours(_ count: Int) -> String { count == 1 ? "1 hour" : "\(count) hours" }
    }
}
