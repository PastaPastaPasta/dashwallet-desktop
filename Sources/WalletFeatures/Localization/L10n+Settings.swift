import Foundation
import WalletRuntime

extension L10n {
    public enum Settings {
        public static let mainnet = "Mainnet"
        public static let testnet = "Testnet"
        public static let regtest = "Regtest"
        public static let themeLight = "Light"
        public static let themeDark = "Dark"
        public static let themeSystem = "System"
        public static let languageSystem = "(Default)"
        public static let passphraseChanged = "Wallet passphrase was successfully changed."
        public static let walletEncrypted = "Wallet encrypted"
        public static let alreadyEncrypted = "The wallet is already encrypted."
        public static let notEncrypted = "The wallet is not encrypted."
        public static let passphraseMismatch = "The supplied passphrases do not match."
        public static let passphraseEmpty = "Enter a passphrase."
        public static let noRecoveryPhrase = "No Recovery Phrase"
        public static let settingsNotSaved = "The settings could not be saved."

        public static func devnet(_ name: String) -> String { "Devnet (\(name))" }

        public static func networkName(_ network: DashNetwork) -> String {
            switch network {
            case .mainnet: mainnet
            case .testnet: testnet
            case .regtest: regtest
            case .devnet(let name): devnet(name)
            }
        }
    }
}
