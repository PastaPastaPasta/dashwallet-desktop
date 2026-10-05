// M1 service contracts: vault, mnemonics and authorization (DESIGN-opus §1.8).
import Foundation

public enum VaultLockState: Sendable, Hashable {
    case noVault
    case noKeys
    case unencrypted
    case locked
    case unlockedMixingOnly
    case unlocked
}

public enum UnlockScope: Sendable, Hashable {
    case full
    case mixingOnly
}

public struct VaultStatus: Sendable, Hashable {
    public let state: VaultLockState
    public let encrypted: Bool
    public let quickUnlockEnrolled: Bool
    public let failedAttempts: UInt32
    public let retryAfterSeconds: UInt64?
    public let walletsWithSecrets: [WalletID]

    public init(
        state: VaultLockState, encrypted: Bool, quickUnlockEnrolled: Bool, failedAttempts: UInt32,
        retryAfterSeconds: UInt64?, walletsWithSecrets: [WalletID]
    ) {
        self.state = state
        self.encrypted = encrypted
        self.quickUnlockEnrolled = quickUnlockEnrolled
        self.failedAttempts = failedAttempts
        self.retryAfterSeconds = retryAfterSeconds
        self.walletsWithSecrets = walletsWithSecrets
    }
}

public enum GrantPurpose: Sendable, Hashable {
    case spend(max: Amount)
    case revealSecret
    case signMessage
    case changeCredential
    case wipe
    /// M3/M4 purposes, listed so the enum matches the engine's.
    case masternodeOperation
    case governance
    case platformOperation
}

/// An engine-issued authorization, passed to the call it authorizes.
public struct AuthGrant: Sendable, Hashable {
    public let id: String
    public let purpose: GrantPurpose
    public let expiresAt: Date
    public let singleUse: Bool

    public init(id: String, purpose: GrantPurpose, expiresAt: Date, singleUse: Bool) {
        self.id = id
        self.purpose = purpose
        self.expiresAt = expiresAt
        self.singleUse = singleUse
    }
}

/// What the gate needs before it can issue a grant for a purpose.
public enum CredentialRequirement: Sendable, Hashable {
    /// Unencrypted vault with "require authentication for every payment" off.
    case none
    case passphrase
    /// Touch ID / Windows Hello, falling back to the passphrase (M2).
    case quickUnlockOrPassphrase
}

public enum Credential: Sendable {
    case passphrase(any SecretBuffer)
    /// Unencrypted vault, no prompt (see `CredentialRequirement.none`).
    case unencrypted
}

public enum MnemonicLanguage: Sendable, Hashable, CaseIterable {
    case english, chineseSimplified, chineseTraditional, czech, french, italian, japanese, korean, portuguese,
        spanish
}

public enum MnemonicChecksum: Sendable, Hashable {
    case valid
    /// Accepted only with Dash Core compatibility (QT-104).
    case coreOnly
    case invalid
}

public struct MnemonicCheck: Sendable, Hashable {
    public let wordCount: Int
    public let unknownWordIndices: [Int]
    public let language: MnemonicLanguage?
    public let checksum: MnemonicChecksum

    public init(wordCount: Int, unknownWordIndices: [Int], language: MnemonicLanguage?, checksum: MnemonicChecksum) {
        self.wordCount = wordCount
        self.unknownWordIndices = unknownWordIndices
        self.language = language
        self.checksum = checksum
    }
}

public struct RevealedMnemonic: Sendable {
    public let phrase: any SecretBuffer
    /// Empty when the wallet has no BIP39 passphrase.
    public let bip39Passphrase: any SecretBuffer

    public init(phrase: any SecretBuffer, bip39Passphrase: any SecretBuffer) {
        self.phrase = phrase
        self.bip39Passphrase = bip39Passphrase
    }
}

/// Vault and mnemonic operations of the active network (owner: B).
public protocol VaultProviding: AnyObject, Sendable {
    func status() async throws(ServiceError) -> VaultStatus
    /// `passphrase` `nil` creates an unencrypted vault (OS secret store).
    func create(passphrase: (any SecretBuffer)?) async throws(ServiceError) -> VaultStatus
    func encrypt(newPassphrase: any SecretBuffer, grant: AuthGrant) async throws(ServiceError) -> VaultStatus
    func changePassphrase(old: any SecretBuffer, new: any SecretBuffer) async throws(ServiceError) -> VaultStatus
    func revealMnemonic(wallet: WalletID, grant: AuthGrant) async throws(ServiceError) -> RevealedMnemonic
    /// A fresh phrase; nothing is stored until `LifecycleQueueing.importWallet`.
    func generateMnemonic(wordCount: Int, language: MnemonicLanguage) async throws(ServiceError) -> any SecretBuffer
    func checkMnemonic(_ phrase: any SecretBuffer) async throws(ServiceError) -> MnemonicCheck
    /// Copies user-typed text into a zeroing buffer. Call as soon as the text
    /// leaves the text field.
    func makeSecret(utf8 text: String) -> any SecretBuffer
}

/// The single authorization primitive (iOS `AuthenticationGate`). Owns the
/// lock state the lock screen and status bar show.
@MainActor
public protocol AuthenticationGating: AnyObject {
    var lockState: VaultLockState? { get }
    /// Current lock state followed by every change.
    func lockStateChanges() -> AsyncStream<VaultLockState>
    func requirement(for purpose: GrantPurpose) -> CredentialRequirement
    func authorize(_ purpose: GrantPurpose, credential: Credential) async throws(ServiceError) -> AuthGrant
    func unlock(passphrase: any SecretBuffer, scope: UnlockScope) async throws(ServiceError)
    func lock() async throws(ServiceError)
}
