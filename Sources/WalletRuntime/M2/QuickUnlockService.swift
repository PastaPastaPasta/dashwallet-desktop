// Quick unlock (vault slot B, IOS-011/016) and the forgot-passphrase / wipe
// steps of the vault (IOS-009/014/109), over the engine vault and the OS
// biometric key store.
import DashKit
import Foundation
import PlatformServices

extension QuickUnlockPolicy {
    init(_ kit: DashKit.QuickUnlockPolicy) {
        self.init(
            enrolled: kit.enrolled, spendLimit: Amount(kit.spendLimit),
            passphraseMaxAge: .seconds(Int64(clamping: kit.passphraseMaxAgeSeconds)),
            lastPassphraseEntry: kit.lastPassphraseAt)
    }
}

extension QuickUnlockProvider {
    init(_ kind: BiometricKind) {
        switch kind {
        case .touchID: self = .touchID
        case .windowsHello: self = .windowsHello
        case .none: self = .unavailable
        }
    }
}

/// `QuickUnlockManaging` over the engine vault and a `BiometricKeyStoring`.
///
/// The vault (Rust) creates the wrap key and enforces every rule: the
/// spending limit, the 7-day passphrase age and the purposes quick unlock
/// may authorize. This type only moves the key between the vault and the
/// OS store, keyed by network:
/// - `enroll` stores the key the vault returns; if the OS store refuses,
///   slot B is removed again so no slot exists that nothing can open;
/// - `remove` deletes slot B, then the OS item;
/// - `credential(reason:)` shows the OS prompt and returns the released
///   key as a zeroing buffer.
public final class QuickUnlockService: QuickUnlockManaging {
    private let context: EngineContext
    private let keyStore: any BiometricKeyStoring
    private let statusObserver: (@Sendable (VaultStatus) async -> Void)?

    init(
        context: EngineContext, keyStore: any BiometricKeyStoring,
        statusObserver: (@Sendable (VaultStatus) async -> Void)? = nil
    ) {
        self.context = context
        self.keyStore = keyStore
        self.statusObserver = statusObserver
    }

    public var provider: QuickUnlockProvider {
        QuickUnlockProvider(keyStore.kind)
    }

    public func policy() async throws(ServiceError) -> QuickUnlockPolicy {
        QuickUnlockPolicy(try await context.call { engine, network throws(DashKitError) in
            try await engine.quickUnlockPolicy(on: network)
        })
    }

    public func enroll(grant: AuthGrant) async throws(ServiceError) -> QuickUnlockPolicy {
        guard case .changeCredential = grant.purpose else {
            throw ServiceError(code: .vaultGrantPurposeMismatch, detail: "enrolling needs a changeCredential grant")
        }
        guard provider != .unavailable else {
            throw ServiceError(code: .vaultQuickUnlockUnavailable, detail: "no biometric key store on this host")
        }
        let network = try context.network()
        let grantID = grant.id
        let wrapKey = try await context.call { engine, network throws(DashKitError) in
            try await engine.enrollQuickUnlock(on: network, grantID: grantID)
        }
        let key = wrapKey.withUnsafeBytes { BiometricKey(copying: $0) }
        do {
            try keyStore.store(key, network: network.description)
        } catch {
            // Slot B without a stored key would be a way in nobody holds.
            _ = try? await context.call { engine, network throws(DashKitError) in
                try await engine.removeQuickUnlock(on: network)
            }
            await reportStatus()
            throw ServiceError(error)
        }
        await reportStatus()
        return try await policy()
    }

    public func remove() async throws(ServiceError) -> QuickUnlockPolicy {
        let network = try context.network()
        let status = try await context.call { engine, network throws(DashKitError) in
            try await engine.removeQuickUnlock(on: network)
        }
        await statusObserver?(VaultStatus(status))
        try deleteStoredKey(network)
        return try await policy()
    }

    public func setSpendLimit(_ limit: Amount, grant: AuthGrant) async throws(ServiceError) -> QuickUnlockPolicy {
        guard QuickUnlockPolicy.spendLimitOptions.contains(limit) else {
            throw ServiceError(code: .invalidArgument, detail: "\(limit.duffs) is not a spending-limit option")
        }
        guard case .changeCredential = grant.purpose else {
            throw ServiceError(
                code: .vaultGrantPurposeMismatch, detail: "the spending limit needs a changeCredential grant")
        }
        let grantID = grant.id
        let kitLimit = limit.kit
        return QuickUnlockPolicy(try await context.call { engine, network throws(DashKitError) in
            try await engine.setQuickUnlockSpendLimit(on: network, grantID: grantID, limit: kitLimit)
        })
    }

    public func credential(reason: String) async throws(ServiceError) -> Credential {
        let network = try context.network()
        let keyStore = keyStore
        let key = try await platformCall { () async throws(PlatformServiceError) in
            try await keyStore.retrieve(network: network.description, reason: reason)
        }
        return .quickUnlock(wrapKey: key.withUnsafeBytes { DashKit.SecretBytes(copying: $0) })
    }

    /// Deletes the OS item of `network`; a missing item is not an error.
    fileprivate func deleteStoredKey(_ network: DashKit.DashNetwork) throws(ServiceError) {
        guard provider != .unavailable else { return }
        let keyStore = keyStore
        try platformCall { () throws(PlatformServiceError) in try keyStore.delete(network: network.description) }
    }

    private func reportStatus() async {
        guard let statusObserver,
            let status = try? await context.call({ engine, network throws(DashKitError) in
                try await engine.vaultStatus(on: network)
            })
        else { return }
        await statusObserver(VaultStatus(status))
    }
}

/// `VaultRecovering` over the engine vault. Both calls end slot B, so the
/// OS item is deleted afterwards (through `quickUnlock`).
public final class VaultRecoveryService: VaultRecovering {
    private let context: EngineContext
    private let quickUnlock: QuickUnlockService
    private let statusObserver: (@Sendable (VaultStatus) async -> Void)?

    init(
        context: EngineContext, quickUnlock: QuickUnlockService,
        statusObserver: (@Sendable (VaultStatus) async -> Void)? = nil
    ) {
        self.context = context
        self.quickUnlock = quickUnlock
        self.statusObserver = statusObserver
    }

    public func recover(
        wallet: WalletID, mnemonic: any SecretBuffer, bip39Passphrase: any SecretBuffer,
        newPassphrase: any SecretBuffer
    ) async throws(ServiceError) -> VaultRecoveryResult {
        let id = try wallet.kit
        let phrase = secretBytes(mnemonic)
        let passphrase = secretBytes(bip39Passphrase)
        let new = secretBytes(newPassphrase)
        let network = try context.network()
        let recovery = try await context.call { engine, network throws(DashKitError) in
            try await engine.recoverVault(
                on: network, wallet: id, mnemonic: phrase, bip39Passphrase: passphrase, newPassphrase: new)
        }
        let status = VaultStatus(recovery.status)
        await statusObserver?(status)
        // The vault no longer has slot B; a stale OS item would only fail.
        try? quickUnlock.deleteStoredKey(network)
        return VaultRecoveryResult(status: status, walletsWithoutSecrets: recovery.walletsWithoutSecrets.map(WalletID.init))
    }

    public func destroy(credential: Credential) async throws(ServiceError) -> VaultStatus {
        let kitCredential = credential.kit
        let network = try context.network()
        let status = VaultStatus(try await context.call { engine, network throws(DashKitError) in
            try await engine.destroyVault(on: network, credential: kitCredential)
        })
        await statusObserver?(status)
        try? quickUnlock.deleteStoredKey(network)
        return status
    }
}
