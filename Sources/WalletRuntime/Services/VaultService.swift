// Vault and mnemonic operations of the open network (DESIGN-opus §1.8).
import DashKit
import Foundation

/// `VaultProviding` over the engine `Vault` and the pure mnemonic functions.
///
/// Secrets: every `SecretBuffer` this type hands out is a DashKit
/// `SecretBytes`, which zeroes its single allocation on deinit (review M5,
/// L3). Buffers from elsewhere are copied into one for the engine call, and
/// that copy is zeroed when it is released.
public final class VaultService: VaultProviding {
    private let context: EngineContext
    /// Receives the status every vault call returns, so the authentication
    /// gate's lock state follows without waiting for the engine event.
    private let statusObserver: (@Sendable (VaultStatus) async -> Void)?

    init(context: EngineContext, statusObserver: (@Sendable (VaultStatus) async -> Void)? = nil) {
        self.context = context
        self.statusObserver = statusObserver
    }

    public func status() async throws(ServiceError) -> VaultStatus {
        await report(VaultStatus(try await context.call { engine, network throws(DashKitError) in
            try await engine.vaultStatus(on: network)
        }))
    }

    public func create(passphrase: (any SecretBuffer)?) async throws(ServiceError) -> VaultStatus {
        let secret = passphrase.map(secretBytes)
        return await report(VaultStatus(try await context.call { engine, network throws(DashKitError) in
            try await engine.createVault(on: network, passphrase: secret)
        }))
    }

    public func encrypt(newPassphrase: any SecretBuffer, grant: AuthGrant) async throws(ServiceError) -> VaultStatus {
        guard case .changeCredential = grant.purpose else {
            throw ServiceError(code: .vaultGrantPurposeMismatch, detail: "encrypting needs a changeCredential grant")
        }
        let secret = secretBytes(newPassphrase)
        let grantID = grant.id
        return await report(VaultStatus(try await context.call { engine, network throws(DashKitError) in
            try await engine.encryptVault(on: network, newPassphrase: secret, grantID: grantID)
        }))
    }

    public func changePassphrase(old: any SecretBuffer, new: any SecretBuffer) async throws(ServiceError)
        -> VaultStatus
    {
        let oldSecret = secretBytes(old)
        let newSecret = secretBytes(new)
        return await report(VaultStatus(try await context.call { engine, network throws(DashKitError) in
            try await engine.changeVaultPassphrase(on: network, old: oldSecret, new: newSecret)
        }))
    }

    public func revealMnemonic(wallet: WalletID, grant: AuthGrant) async throws(ServiceError) -> RevealedMnemonic {
        guard case .revealSecret = grant.purpose else {
            throw ServiceError(code: .vaultGrantPurposeMismatch, detail: "revealing needs a revealSecret grant")
        }
        let id = try wallet.kit
        let grantID = grant.id
        let revealed = try await context.call { engine, network throws(DashKitError) in
            try await engine.revealMnemonic(on: network, wallet: id, grantID: grantID)
        }
        return RevealedMnemonic(phrase: revealed.phrase, bip39Passphrase: revealed.bip39Passphrase)
    }

    /// Word counts outside `UInt8` fail with `wallet.unsupported_word_count`
    /// rather than trapping (review M-8).
    public func generateMnemonic(wordCount: Int, language: MnemonicLanguage) async throws(ServiceError)
        -> any SecretBuffer
    {
        let engine = context.engine
        let kitLanguage = language.kit
        return try serviceCall { () throws(DashKitError) in
            try engine.generateMnemonic(wordCount: wordCount, language: kitLanguage)
        }
    }

    public func checkMnemonic(_ phrase: any SecretBuffer) async throws(ServiceError) -> MnemonicCheck {
        let engine = context.engine
        let secret = secretBytes(phrase)
        return MnemonicCheck(try serviceCall { () throws(DashKitError) in try engine.checkMnemonic(secret) })
    }

    public func makeSecret(utf8 text: String) -> any SecretBuffer {
        DashKit.SecretBytes(utf8: text)
    }

    private func report(_ status: VaultStatus) async -> VaultStatus {
        await statusObserver?(status)
        return status
    }
}
