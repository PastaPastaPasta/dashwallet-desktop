// M3 service contract of the masternode keychain (IOS-083; engine
// masternode_keys.rs; m3-swift.md §2.3, m3-engine.md §2.5): a wallet's DIP3
// provider keys and the reveal of one private key. Masternode list, ProTx,
// tracked masternodes and evonode tools stay in Dash Core (repo CLAUDE.md
// "Product scope"; parked on the branches m3/r2-governance, m3/r3-protx).
import Foundation

/// A provider key family (DIP3 paths under DIP9 feature 3'). Payout keys
/// are BIP44 addresses and live in Receive.
public enum MasternodeKeyRole: Sendable, Hashable, CaseIterable {
    case owner, voting, `operator`, platformNode
}

/// One derived provider key, public data only (IOS-083).
public struct MasternodeKeyInfo: Sendable, Hashable, Identifiable {
    public let role: MasternodeKeyRole
    public let index: UInt32
    public let derivationPath: String
    public let address: String?
    public let publicKeyHex: String
    public let legacyPublicKeyHex: String?
    public let platformNodeID: String?

    public var id: String { derivationPath }

    public init(
        role: MasternodeKeyRole, index: UInt32, derivationPath: String, address: String?, publicKeyHex: String,
        legacyPublicKeyHex: String?, platformNodeID: String?
    ) {
        self.role = role
        self.index = index
        self.derivationPath = derivationPath
        self.address = address
        self.publicKeyHex = publicKeyHex
        self.legacyPublicKeyHex = legacyPublicKeyHex
        self.platformNodeID = platformNodeID
    }
}

/// A revealed provider private key, held transiently.
public struct RevealedMasternodeKey: Sendable {
    public let privateKeyHex: any SecretBuffer
    public let wif: (any SecretBuffer)?
    public let tenderdashKey: (any SecretBuffer)?

    public init(privateKeyHex: any SecretBuffer, wif: (any SecretBuffer)?, tenderdashKey: (any SecretBuffer)?) {
        self.privateKeyHex = privateKeyHex
        self.wif = wif
        self.tenderdashKey = tenderdashKey
    }
}

/// Masternode keychain (IOS-083). Errors: `masternode.*`.
public protocol MasternodeKeychainProviding: AnyObject, Sendable {
    /// `range.count ≤ 100`.
    func keys(wallet: WalletID, role: MasternodeKeyRole, range: Range<UInt32>) async throws(ServiceError)
        -> [MasternodeKeyInfo]
    /// `grant`: `.revealSecret`.
    func reveal(wallet: WalletID, role: MasternodeKeyRole, index: UInt32, grant: AuthGrant)
        async throws(ServiceError) -> RevealedMasternodeKey
}
