import DashWalletCore
import Foundation

// DashKit wrappers of the masternode keychain (IOS-083,
// docs/contracts/m3-engine.md §2.5; owner R3): a wallet's DIP3 provider keys
// and the reveal of one private key. Revealed keys arrive in zeroing buffers.

// MARK: Models

/// A provider key family (engine `MasternodeKeyRole`).
public enum MNKeyRole: Sendable, Hashable { case owner, voting, `operator`, platformNode }

/// One derived provider key (public data).
public struct MNKeyInfo: Sendable, Hashable {
    public let role: MNKeyRole
    public let index: UInt32
    public let derivationPath: String
    public let address: String?
    public let publicKeyHex: String
    public let legacyPublicKeyHex: String?
    public let platformNodeID: String?
}

/// A revealed provider private key.
public struct MNRevealedKey: Sendable {
    public let privateKeyHex: SecretBytes
    public let wif: SecretBytes?
    public let tenderdashKey: SecretBytes?
}

extension MNKeyRole {
    init(_ r: DashWalletCore.MasternodeKeyRole) {
        switch r {
        case .owner: self = .owner
        case .voting: self = .voting
        case .operator: self = .operator
        case .platformNode: self = .platformNode
        }
    }

    var ffi: DashWalletCore.MasternodeKeyRole {
        switch self {
        case .owner: .owner
        case .voting: .voting
        case .operator: .operator
        case .platformNode: .platformNode
        }
    }
}

// MARK: Calls

extension EngineClient {
    /// `masternode_keys`: keys of `role` for `start..<start+count` (≤ 100).
    public func masternodeKeys(on network: DashNetwork, wallet: WalletID, role: MNKeyRole, start: UInt32, count: UInt32)
        async throws(DashKitError) -> [MNKeyInfo]
    {
        let session = try session(network)
        let ffiRole = role.ffi
        return try await mapped {
            try await session.masternodeKeys(walletId: wallet.hex, role: ffiRole, start: start, count: count)
        }.map {
            MNKeyInfo(
                role: MNKeyRole($0.role), index: $0.index, derivationPath: $0.derivationPath, address: $0.address,
                publicKeyHex: $0.publicKeyHex, legacyPublicKeyHex: $0.legacyPublicKeyHex,
                platformNodeID: $0.platformNodeId)
        }
    }

    /// `Vault.reveal_masternode_key` with a `RevealSecret` grant for `wallet`.
    public func revealMasternodeKey(
        on network: DashNetwork, wallet: WalletID, role: MNKeyRole, index: UInt32, grantID: String
    ) async throws(DashKitError) -> MNRevealedKey {
        let vault = try session(network).vault()
        let ffiRole = role.ffi
        let r = try await mapped {
            try await vault.revealMasternodeKey(walletId: wallet.hex, role: ffiRole, index: index, grantId: grantID)
        }
        // Copy into zeroing buffers and wipe the binding's Data at once (M5).
        func take(_ data: Data?) -> SecretBytes? {
            guard var data else { return nil }
            return SecretBytes(consuming: &data)
        }
        var key = r.privateKeyHex
        return MNRevealedKey(
            privateKeyHex: SecretBytes(consuming: &key), wif: take(r.wif), tenderdashKey: take(r.tenderdashKey))
    }
}
