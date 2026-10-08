// Adapter for the masternode keychain contract (m3-swift.md §2.3, owner R3):
// `MasternodeKeychainProviding` over the `EngineClient` wrappers of
// `EngineClient+M3MasternodeKeys.swift`. Each call reads the open network.
import DashKit
import Foundation

/// A wallet's DIP3 provider keys and the reveal of one private key (IOS-083).
public final class MasternodeKeychainService: MasternodeKeychainProviding {
    private let engine: EngineClient
    private let active: ActiveNetwork

    public init(engine: EngineClient, active: ActiveNetwork) {
        self.engine = engine
        self.active = active
    }

    public func keys(wallet: WalletID, role: MasternodeKeyRole, range: Range<UInt32>) async throws(ServiceError)
        -> [MasternodeKeyInfo]
    {
        let network = try active.require()
        let id = try wallet.kit
        let engine = engine
        let kitRole = role.kit
        let rows = try await serviceCall { () async throws(DashKitError) in
            try await engine.masternodeKeys(
                on: network, wallet: id, role: kitRole, start: range.lowerBound, count: UInt32(range.count))
        }
        return rows.map {
            MasternodeKeyInfo(
                role: MasternodeKeyRole($0.role), index: $0.index, derivationPath: $0.derivationPath,
                address: $0.address, publicKeyHex: $0.publicKeyHex, legacyPublicKeyHex: $0.legacyPublicKeyHex,
                platformNodeID: $0.platformNodeID)
        }
    }

    /// `grant`: `.revealSecret` for `wallet`.
    public func reveal(wallet: WalletID, role: MasternodeKeyRole, index: UInt32, grant: AuthGrant)
        async throws(ServiceError) -> RevealedMasternodeKey
    {
        guard grant.purpose == .revealSecret else {
            throw ServiceError(code: .vaultGrantPurposeMismatch, detail: "revealing a key needs a revealSecret grant")
        }
        let network = try active.require()
        let id = try wallet.kit
        let engine = engine
        let kitRole = role.kit
        let grantID = grant.id
        let key = try await serviceCall { () async throws(DashKitError) in
            try await engine.revealMasternodeKey(on: network, wallet: id, role: kitRole, index: index, grantID: grantID)
        }
        return RevealedMasternodeKey(privateKeyHex: key.privateKeyHex, wif: key.wif, tenderdashKey: key.tenderdashKey)
    }
}

extension MasternodeKeyRole {
    init(_ kit: MNKeyRole) {
        switch kit {
        case .owner: self = .owner
        case .voting: self = .voting
        case .operator: self = .operator
        case .platformNode: self = .platformNode
        }
    }

    var kit: MNKeyRole {
        switch self {
        case .owner: .owner
        case .voting: .voting
        case .operator: .operator
        case .platformNode: .platformNode
        }
    }
}
