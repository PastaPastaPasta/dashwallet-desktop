import Foundation
import WalletRuntime

extension L10n {
    /// iOS's Masternode Keys tool (IOS-083). Masternode list and ProTx copy
    /// stays with Dash Core (repo CLAUDE.md "Product scope").
    public enum Masternodes {
        public static let keychainTitle = "Masternode Keys"
        public static func keyRole(_ role: MasternodeKeyRole) -> String {
            switch role {
            case .owner: "Owner keys"
            case .voting: "Voting keys"
            case .operator: "Operator keys (BLS)"
            case .platformNode: "Platform node keys (Ed25519)"
            }
        }
        public static func keyIndex(_ index: UInt32) -> String { "Key \(index)" }
        public static let derivationPath = "Derivation path"
        public static let address = "Address"
        public static let publicKey = "Public key"
        public static let legacyPublicKey = "Public key (legacy scheme)"
        public static let platformNodeID = "Platform node ID"
        public static let privateKey = "Private key"
        public static let wif = "WIF"
        public static let tenderdashKey = "Tenderdash private key"
        public static let reveal = "Reveal private key"
        public static let hide = "Hide"
        public static let loadMore = "Show more keys"
        public static let notAvailableYet = "Not available yet"
    }
}
