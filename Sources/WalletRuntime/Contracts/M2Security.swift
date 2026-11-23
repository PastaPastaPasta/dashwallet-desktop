// M2 service contracts: quick unlock (vault slot B), auto-lock, forgot
// passphrase and wipe (engine vault_m2.rs; m2-swift.md §2.8).
import Foundation

/// The OS biometric provider for quick unlock on this host.
public enum QuickUnlockProvider: Sendable, Hashable {
    case touchID
    /// M6 (m2-engine.md §2.10).
    case windowsHello
    /// Linux, or no biometrics enrolled: quick unlock is hidden.
    case unavailable
}

/// Biometric quick-unlock rules the vault enforces (IOS-011/016).
public struct QuickUnlockPolicy: Sendable, Hashable {
    /// The iOS spending-limit options, in duffs: 0, 0.1, 0.5, 1 and 5 DASH.
    public static let spendLimitOptions: [Amount] = [0, 10_000_000, 50_000_000, 100_000_000, 500_000_000]
        .map { Amount(duffs: $0) }
    public static let defaultSpendLimit = Amount(duffs: 50_000_000)

    public let enrolled: Bool
    public let spendLimit: Amount
    /// 7 days: after that a passphrase is required again.
    public let passphraseMaxAge: Duration
    public let lastPassphraseEntry: Date?

    public init(enrolled: Bool, spendLimit: Amount, passphraseMaxAge: Duration, lastPassphraseEntry: Date?) {
        self.enrolled = enrolled
        self.spendLimit = spendLimit
        self.passphraseMaxAge = passphraseMaxAge
        self.lastPassphraseEntry = lastPassphraseEntry
    }
}

/// Touch ID / Windows Hello (owner S1). Enrolling stores the vault's slot B
/// wrap key in the OS biometric store; `credential(reason:)` prompts and
/// returns `Credential.quickUnlock` for `AuthenticationGating.authorize`.
/// The vault refuses quick-unlock grants for reveal, wipe and credential
/// changes and above the spending limit (`vault.quick_unlock_limit_exceeded`).
public protocol QuickUnlockManaging: AnyObject, Sendable {
    var provider: QuickUnlockProvider { get }
    func policy() async throws(ServiceError) -> QuickUnlockPolicy
    /// `.changeCredential` grant issued with the passphrase.
    func enroll(grant: AuthGrant) async throws(ServiceError) -> QuickUnlockPolicy
    func remove() async throws(ServiceError) -> QuickUnlockPolicy
    /// One of `QuickUnlockPolicy.spendLimitOptions`; `.changeCredential` grant.
    func setSpendLimit(_ limit: Amount, grant: AuthGrant) async throws(ServiceError) -> QuickUnlockPolicy
    /// Shows the OS biometric prompt with `reason`.
    func credential(reason: String) async throws(ServiceError) -> Credential
}

/// iOS auto-lock choices (IOS-015).
public enum AutoLockInterval: String, Sendable, Hashable, CaseIterable, Codable {
    case immediately, oneMinute, fiveMinutes, oneHour, oneDay, never

    public var duration: Duration? {
        switch self {
        case .immediately: .zero
        case .oneMinute: .seconds(60)
        case .fiveMinutes: .seconds(300)
        case .oneHour: .seconds(3600)
        case .oneDay: .seconds(86_400)
        case .never: nil
        }
    }
}

/// Locks the vault after inactivity, on sleep and on screen lock (owner S1).
/// dash-qt never auto-locks: the default is `.never`.
@MainActor
public protocol AutoLockControlling: AnyObject {
    var interval: AutoLockInterval { get }
    func setInterval(_ interval: AutoLockInterval) throws(ServiceError)
    /// User input in any window resets the timer.
    func noteActivity()
}

/// Outcome of the forgot-passphrase flow (IOS-014).
public struct VaultRecoveryResult: Sendable, Hashable {
    public let status: VaultStatus
    /// Wallets whose secrets were lost with the old vault: they are
    /// watch-only until their phrases are imported again.
    public let walletsWithoutSecrets: [WalletID]

    public init(status: VaultStatus, walletsWithoutSecrets: [WalletID]) {
        self.status = status
        self.walletsWithoutSecrets = walletsWithoutSecrets
    }
}

/// Forgot passphrase and "Delete All" (owner S1 adapter over the vault).
public protocol VaultRecovering: AnyObject, Sendable {
    /// The phrase must derive `wallet` (`vault.recovery_mismatch`). Replaces
    /// the vault with one encrypted by `newPassphrase`.
    func recover(
        wallet: WalletID, mnemonic: any SecretBuffer, bip39Passphrase: any SecretBuffer,
        newPassphrase: any SecretBuffer
    ) async throws(ServiceError) -> VaultRecoveryResult
    /// Deletes the vault once every wallet was removed (`vault.not_empty`).
    func destroy(credential: Credential) async throws(ServiceError) -> VaultStatus
}
