// Security settings (IOS-011, IOS-014…016, IOS-108, IOS-109): quick unlock
// and its spending limit, auto-lock, "require authentication for every
// payment", autohide balance, the recovery-phrase picker, forgot passphrase
// and wipe.
import Foundation
import Observation
import WalletRuntime

/// A quick-unlock change waiting for the passphrase (it needs a
/// `.changeCredential` grant).
public enum QuickUnlockChange: Sendable, Hashable {
    case enroll
    case setSpendLimit(Amount)
}

public enum QuickUnlockFlow: Sendable, Hashable {
    case idle
    case needsPassphrase(QuickUnlockChange)
    case working
    case failed(String)
}

/// Forgot passphrase (IOS-014): phrase → wallet check → new passphrase.
public enum ForgotPassphraseStep: Sendable, Hashable {
    case idle
    case enteringPhrase
    case choosingPassphrase(wallet: WalletID)
    case recovering
    case done(VaultRecoveryResult)
    case failed(String)
}

/// Wipe (IOS-109): typed sentence, passphrase when encrypted, then every
/// wallet removed and the vault destroyed.
public enum WipeStep: Sendable, Hashable {
    case idle
    case confirming
    case wiping
    case done
    case failed(String)
}

@MainActor
@Observable
public final class SecurityViewModel {
    public private(set) var vaultStatus: VaultStatus?
    public private(set) var policy: QuickUnlockPolicy?
    public private(set) var quickUnlockFlow: QuickUnlockFlow = .idle
    public private(set) var autoLockInterval: AutoLockInterval
    public private(set) var requireAuthenticationForEveryPayment: Bool
    public private(set) var autohideBalance: Bool
    public private(set) var forgotPassphrase: ForgotPassphraseStep = .idle
    public private(set) var wipeStep: WipeStep = .idle
    public private(set) var errorMessage: String?
    /// A reveal needs the passphrase (encrypted vault).
    public private(set) var revealNeedsPassphrase = false

    public var provider: QuickUnlockProvider { quickUnlock.provider }
    /// Touch ID / Windows Hello rows are shown only where the OS has them and
    /// the vault is encrypted (slot B wraps the passphrase key).
    public var showsQuickUnlock: Bool { provider != .unavailable && (vaultStatus?.encrypted ?? false) }
    public var providerName: String? {
        switch provider {
        case .touchID: L10n.Security.touchID
        case .windowsHello: L10n.Security.windowsHello
        case .unavailable: nil
        }
    }
    public var spendLimitOptions: [Amount] { QuickUnlockPolicy.spendLimitOptions }
    public var autoLockOptions: [AutoLockInterval] { AutoLockInterval.allCases }
    /// The wallet picker for the recovery phrase and forgot passphrase (IOS-108).
    public var wallets: [WalletInfo] { walletState.wallets ?? [] }

    /// Names of the wallets left without secrets after a recovery.
    public var walletsWithoutSecretsText: String? {
        guard case .done(let result) = forgotPassphrase, !result.walletsWithoutSecrets.isEmpty else { return nil }
        let names = result.walletsWithoutSecrets.map { id in wallets.first { $0.id == id }?.name ?? id.hex }
        return L10n.Security.walletsWithoutSecrets(names)
    }

    public func spendLimitText(_ limit: Amount) -> String {
        amounts.format(limit, unit: .dash, style: .withUnit(plusSign: false, separators: .standard))
    }

    private let quickUnlock: any QuickUnlockManaging
    private let autoLock: any AutoLockControlling
    private let auth: any AuthenticationGating
    private let vault: any VaultProviding
    private let recovery: any VaultRecovering
    private let lifecycle: any LifecycleQueueing
    private let walletLifecycle: any WalletLifecycleManaging
    private let walletState: any WalletStateProviding
    private let settings: any SettingsProviding
    private let paymentAuthentication: any PaymentAuthenticationSetting
    private let desktopPreferences: any DesktopPreferencesStoring
    private let amounts: any AmountFormatting
    /// The recovery phrase between the forgot-passphrase steps; zeroed when dropped.
    private var pendingPhrase: (any SecretBuffer)?
    private var pendingBIP39Passphrase: (any SecretBuffer)?

    public init(
        quickUnlock: any QuickUnlockManaging, autoLock: any AutoLockControlling, auth: any AuthenticationGating,
        vault: any VaultProviding, recovery: any VaultRecovering, lifecycle: any LifecycleQueueing,
        walletLifecycle: any WalletLifecycleManaging, walletState: any WalletStateProviding,
        settings: any SettingsProviding, paymentAuthentication: any PaymentAuthenticationSetting,
        desktopPreferences: any DesktopPreferencesStoring, amounts: any AmountFormatting
    ) {
        self.quickUnlock = quickUnlock
        self.autoLock = autoLock
        self.auth = auth
        self.vault = vault
        self.recovery = recovery
        self.lifecycle = lifecycle
        self.walletLifecycle = walletLifecycle
        self.walletState = walletState
        self.settings = settings
        self.paymentAuthentication = paymentAuthentication
        self.desktopPreferences = desktopPreferences
        self.amounts = amounts
        autoLockInterval = autoLock.interval
        requireAuthenticationForEveryPayment = paymentAuthentication.requireAuthenticationForEveryPayment
        autohideBalance = settings.display.hideBalances
    }

    public convenience init(env: AppEnvironment, m2: M2Services) {
        self.init(
            quickUnlock: m2.quickUnlock, autoLock: m2.autoLock, auth: env.auth, vault: env.vault,
            recovery: m2.vaultRecovery, lifecycle: env.lifecycle, walletLifecycle: m2.walletLifecycle,
            walletState: env.walletState, settings: env.settings, paymentAuthentication: m2.paymentAuthentication,
            desktopPreferences: m2.desktopPreferences, amounts: env.amounts)
    }

    public func load() async {
        do {
            vaultStatus = try await vault.status()
        } catch {
            errorMessage = ErrorText.m2(error.code)
        }
        guard provider != .unavailable else {
            policy = nil
            return
        }
        do {
            policy = try await quickUnlock.policy()
        } catch {
            policy = nil
            if error.code != .notImplemented { errorMessage = ErrorText.m2(error.code) }
        }
    }

    // MARK: Quick unlock (IOS-011, IOS-016)

    /// Turning it on enrolls with a `.changeCredential` grant (default limit 0.5 DASH).
    public func enableQuickUnlock(passphrase: String? = nil) async {
        await changeQuickUnlock(.enroll, passphrase: passphrase)
    }

    public func disableQuickUnlock() async {
        quickUnlockFlow = .working
        do {
            policy = try await quickUnlock.remove()
            quickUnlockFlow = .idle
        } catch {
            quickUnlockFlow = .failed(ErrorText.m2(error.code))
        }
    }

    /// One of `spendLimitOptions`; needs the passphrase like enrolment.
    public func setSpendLimit(_ limit: Amount, passphrase: String? = nil) async {
        guard spendLimitOptions.contains(limit) else { return }
        await changeQuickUnlock(.setSpendLimit(limit), passphrase: passphrase)
    }

    /// The passphrase for the change in `.needsPassphrase`.
    public func provideQuickUnlockPassphrase(_ passphrase: String) async {
        guard case .needsPassphrase(let change) = quickUnlockFlow else { return }
        await changeQuickUnlock(change, passphrase: passphrase)
    }

    public func cancelQuickUnlockChange() {
        quickUnlockFlow = .idle
    }

    private func changeQuickUnlock(_ change: QuickUnlockChange, passphrase: String?) async {
        guard let credential = makeCredential(for: .changeCredential, passphrase: passphrase, auth: auth, vault: vault)
        else {
            quickUnlockFlow = .needsPassphrase(change)
            return
        }
        quickUnlockFlow = .working
        do {
            let grant = try await auth.authorize(.changeCredential, wallet: nil, credential: credential)
            switch change {
            case .enroll: policy = try await quickUnlock.enroll(grant: grant)
            case .setSpendLimit(let limit): policy = try await quickUnlock.setSpendLimit(limit, grant: grant)
            }
            quickUnlockFlow = .idle
        } catch {
            quickUnlockFlow = error.code == .vaultWrongPassphrase
                ? .needsPassphrase(change) : .failed(ErrorText.m2(error.code))
            if error.code == .vaultWrongPassphrase { errorMessage = ErrorText.m2(error.code) }
        }
    }

    // MARK: Auto-lock, payments, balance (IOS-015, IOS-016, IOS-108)

    public func setAutoLock(_ interval: AutoLockInterval) {
        do {
            try autoLock.setInterval(interval)
            autoLockInterval = interval
            errorMessage = nil
        } catch {
            errorMessage = ErrorText.m2(error.code)
        }
    }

    public func setRequireAuthenticationForEveryPayment(_ value: Bool) {
        do {
            try paymentAuthentication.setRequireAuthenticationForEveryPayment(value)
            requireAuthenticationForEveryPayment = value
            errorMessage = nil
        } catch {
            errorMessage = ErrorText.m2(error.code)
        }
    }

    /// iOS "Autohide balance" = discreet mode (QT-039).
    public func setAutohideBalance(_ value: Bool) {
        var display = settings.display
        display.hideBalances = value
        do {
            try settings.update(display)
            autohideBalance = value
            errorMessage = nil
        } catch {
            errorMessage = L10n.Settings.settingsNotSaved
        }
    }

    // MARK: Recovery phrase (IOS-108)

    /// Reveals `wallet`'s phrase behind a `.revealSecret` grant and records
    /// the backup for the IOS-005 reminder.
    public func revealPhrase(wallet: WalletID, passphrase: String? = nil) async -> RevealedMnemonic? {
        guard let credential = makeCredential(for: .revealSecret, passphrase: passphrase, auth: auth, vault: vault)
        else {
            revealNeedsPassphrase = true
            return nil
        }
        revealNeedsPassphrase = false
        do {
            let grant = try await auth.authorize(.revealSecret, wallet: wallet, credential: credential)
            let revealed = try await vault.revealMnemonic(wallet: wallet, grant: grant)
            BackupReminderViewModel.recordBackup(wallet, in: desktopPreferences)
            errorMessage = nil
            return revealed
        } catch {
            errorMessage = ErrorText.m2(error.code)
            return nil
        }
    }

    // MARK: Forgot passphrase (IOS-014)

    public func startForgotPassphrase() {
        dropPendingPhrase()
        forgotPassphrase = .enteringPhrase
    }

    /// Checks the phrase offline; the engine checks that it derives `wallet`
    /// when the vault is replaced (`vault.recovery_mismatch`).
    public func submitRecoveryPhrase(_ phrase: String, bip39Passphrase: String = "", wallet: WalletID) async {
        guard forgotPassphrase == .enteringPhrase else { return }
        let normalized = phrase.lowercased().split(whereSeparator: \.isWhitespace).joined(separator: " ")
        let secret = vault.makeSecret(utf8: normalized)
        do {
            let check = try await vault.checkMnemonic(secret)
            guard check.checksum != .invalid, check.unknownWordIndices.isEmpty else {
                errorMessage = L10n.Security.invalidPhrase
                return
            }
        } catch {
            errorMessage = ErrorText.m2(error.code)
            return
        }
        errorMessage = nil
        pendingPhrase = secret
        pendingBIP39Passphrase = vault.makeSecret(utf8: bip39Passphrase)
        forgotPassphrase = .choosingPassphrase(wallet: wallet)
    }

    /// Replaces the vault with one encrypted by the new passphrase.
    public func chooseNewPassphrase(_ passphrase: String, confirmation: String) async {
        guard case .choosingPassphrase(let wallet) = forgotPassphrase, let phrase = pendingPhrase,
            let bip39 = pendingBIP39Passphrase
        else { return }
        if passphrase.isEmpty {
            errorMessage = L10n.Settings.passphraseEmpty
            return
        }
        guard passphrase == confirmation else {
            errorMessage = L10n.Settings.passphraseMismatch
            return
        }
        guard passphrase.count <= 1024 else {
            errorMessage = L10n.Onboarding.passphraseTooLong
            return
        }
        errorMessage = nil
        forgotPassphrase = .recovering
        do {
            let result = try await recovery.recover(
                wallet: wallet, mnemonic: phrase, bip39Passphrase: bip39,
                newPassphrase: vault.makeSecret(utf8: passphrase))
            vaultStatus = result.status
            forgotPassphrase = .done(result)
        } catch {
            forgotPassphrase = error.code == .vaultRecoveryMismatch
                ? .failed(L10n.M2Errors.recoveryMismatch) : .failed(ErrorText.m2(error.code))
        }
        dropPendingPhrase()
    }

    public func cancelForgotPassphrase() {
        dropPendingPhrase()
        forgotPassphrase = .idle
    }

    private func dropPendingPhrase() {
        pendingPhrase = nil
        pendingBIP39Passphrase = nil
    }

    // MARK: Wipe (IOS-109)

    public func requestWipe() {
        wipeStep = .confirming
    }

    public func cancelWipe() {
        if wipeStep == .confirming { wipeStep = .idle }
    }

    /// The typed sentence must match; an encrypted vault needs `passphrase`.
    public func wipe(confirmation: String, passphrase: String? = nil) async {
        guard wipeStep == .confirming else { return }
        guard confirmation.trimmingCharacters(in: .whitespacesAndNewlines) == L10n.Wallets.wipeAcceptPhrase else {
            errorMessage = L10n.Wallets.acceptPhraseMismatch
            return
        }
        guard makeCredential(for: .wipe, passphrase: passphrase, auth: auth, vault: vault) != nil else {
            errorMessage = L10n.Common.passphraseRequired
            return
        }
        errorMessage = nil
        wipeStep = .wiping
        let wiper = WalletWiper(
            walletLifecycle: walletLifecycle, lifecycle: lifecycle, walletState: walletState, auth: auth, vault: vault,
            recovery: recovery)
        do {
            vaultStatus = try await wiper.wipeAll(passphrase: passphrase)
            wipeStep = .done
        } catch {
            wipeStep = .failed(ErrorText.m2(error.code))
        }
    }
}
