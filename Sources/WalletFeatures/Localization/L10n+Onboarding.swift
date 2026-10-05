import Foundation

extension L10n {
    public enum Onboarding {
        public static let welcomeTitle = "Welcome to Dash Wallet"
        public static let createWallet = "Create a new wallet"
        public static let restoreWallet = "Restore wallet"
        public static let showPhraseWarning =
            "Write down these words in order and keep them somewhere safe. Anyone with them can spend your funds."
        public static let screenCaptureWarning =
            "A screenshot or recording of your recovery phrase was detected. Anyone who sees it can take your funds. Delete it, or start again to get a new phrase."
        public static let verifyPrompt = "Tap the words in the order shown."
        public static let verifyWrongWord = "That is not the right word. Try again."
        public static let verifiedSuccessfully = "Verified Successfully"
        public static let passphraseEmpty = "Enter a passphrase."
        public static let passphraseMismatch = "The supplied passphrases do not match."
        public static let passphraseTooLong = "The passphrase is longer than 1024 characters."
        public static let passphraseRejected = "The wallet rejected this passphrase. Choose another one."
        public static let encryptWarning =
            "Warning: If you encrypt your wallet and lose your passphrase, you will LOSE ALL OF YOUR DASH! Your recovery phrase still restores the wallet."
        public static let invalidPhrase = "The recovery phrase is not valid. Check every word."
        public static let unsupportedWordCount = "Recovery phrases have 12, 15, 18, 21 or 24 words."
        public static let coreOnlyChecksumWarning =
            "This recovery phrase only passes Dash Core's legacy checksum. It will be restored with Dash Core compatibility."
        public static let walletAlreadyExists = "This wallet has already been added."
        public static let watchOnlyExists = "A watch-only copy of this wallet exists. Remove it first."
        public static let vaultLocked = "Unlock the wallet before adding another wallet."
        public static let birthDateInFuture = "The wallet creation date cannot be in the future."

        public static func unknownWords(_ positions: [Int]) -> String {
            let list = positions.map { String($0 + 1) }.joined(separator: ", ")
            return positions.count == 1
                ? "Word \(list) is not in the word list." : "Words \(list) are not in the word list."
        }
    }

    public enum PassphraseStrength {
        public static let weak = "Weak"
        public static let fair = "Fair"
        public static let good = "Good"
        public static let strong = "Strong"
    }
}
