// Settings (QT-016, QT-020, QT-039, QT-111, QT-113, QT-135…141 subset, IOS-104/106).
import Foundation
import Observation
import WalletRuntime

@MainActor
@Observable
public final class SettingsViewModel {
    public static let decimalDigitsRange = 2...8

    public private(set) var network: DashNetwork?
    public let availableNetworks: [DashNetwork]
    public private(set) var transition: LifecycleTransition = .idle
    public private(set) var display: DisplaySettings
    public private(set) var theme: AppTheme
    /// `nil` follows the system language. Only English ships in M1; the list
    /// grows when translations are imported.
    public private(set) var languageCode: String?
    public let availableLanguages: [String?] = [nil, "en"]
    public private(set) var vault: VaultStatus?
    /// A credential is needed for the pending reveal (QT-113).
    public private(set) var needsPassphrase = false
    public private(set) var errorMessage: String?
    public private(set) var infoMessage: String?

    public var isSwitchingNetwork: Bool {
        if case .switchingNetwork = transition { return true }
        return false
    }

    private let host: any WalletHosting
    private let lifecycle: any LifecycleQueueing
    private let walletState: any WalletStateProviding
    private let vaultService: any VaultProviding
    private let auth: any AuthenticationGating
    private let settings: any SettingsProviding
    private let preferences: any UIPreferencesStoring
    private var transitionTask: Task<Void, Never>?

    public init(
        host: any WalletHosting, lifecycle: any LifecycleQueueing, walletState: any WalletStateProviding,
        vault: any VaultProviding, auth: any AuthenticationGating, settings: any SettingsProviding,
        preferences: any UIPreferencesStoring, developerMode: Bool
    ) {
        self.host = host
        self.lifecycle = lifecycle
        self.walletState = walletState
        self.vaultService = vault
        self.auth = auth
        self.settings = settings
        self.preferences = preferences
        self.availableNetworks = developerMode ? [.mainnet, .testnet, .regtest] : [.mainnet, .testnet]
        self.display = settings.display
        self.theme = preferences.preferences.theme
        self.languageCode = preferences.preferences.languageCode
        self.network = settings.lastNetwork
    }

    public convenience init(env: AppEnvironment) {
        self.init(
            host: env.host, lifecycle: env.lifecycle, walletState: env.walletState, vault: env.vault, auth: env.auth,
            settings: env.settings, preferences: env.preferences, developerMode: env.developerMode)
    }

    public func load() async {
        network = await host.activeNetwork ?? settings.lastNetwork
        transition = await lifecycle.transition
        display = settings.display
        do {
            vault = try await vaultService.status()
        } catch {
            errorMessage = ErrorText.common(error.code)
        }
    }

    /// Follows lifecycle transitions (the switching overlay) until `stop()`.
    public func start() {
        stop()
        let transitions = lifecycle.transitions()
        transitionTask = Task { [weak self] in
            for await transition in transitions {
                guard let self else { return }
                self.transition = transition
            }
        }
    }

    public func stop() {
        transitionTask?.cancel()
        transitionTask = nil
    }

    // MARK: Network

    /// Stops the current network and starts `network` through the lifecycle
    /// queue (IOS-106, QT-002).
    public func switchNetwork(to network: DashNetwork) async {
        guard network != self.network, availableNetworks.contains(network) else { return }
        errorMessage = nil
        do {
            try await lifecycle.switchNetwork(to: network)
            self.network = network
        } catch {
            errorMessage = ErrorText.common(error.code)
        }
    }

    // MARK: Display

    public func update(_ display: DisplaySettings) {
        var clamped = display
        clamped.decimalDigits = min(max(display.decimalDigits, Self.decimalDigitsRange.lowerBound),
                                    Self.decimalDigitsRange.upperBound)
        do {
            try settings.update(clamped)
            self.display = clamped
            errorMessage = nil
        } catch {
            errorMessage = L10n.Settings.settingsNotSaved
        }
    }

    /// Display unit everywhere (QT-020).
    public func setUnit(_ unit: DisplayUnit) {
        var next = display
        next.unit = unit
        update(next)
    }

    /// dash-qt "decimal digits", 2–8 (QT-036).
    public func setDecimalDigits(_ digits: Int) {
        var next = display
        next.decimalDigits = digits
        update(next)
    }

    /// Discreet mode, persisted (QT-039).
    public func setDiscreet(_ on: Bool) {
        var next = display
        next.hideBalances = on
        update(next)
    }

    public func setTheme(_ theme: AppTheme) {
        var stored = preferences.preferences
        stored.theme = theme
        savePreferences(stored) { self.theme = theme }
    }

    public func setLanguage(_ code: String?) {
        guard availableLanguages.contains(code) else { return }
        var stored = preferences.preferences
        stored.languageCode = code
        savePreferences(stored) { self.languageCode = code }
    }

    // MARK: Security

    /// Encrypts an unencrypted vault (QT-111): the seed stays the same.
    public func encryptWallet(passphrase: String, confirmation: String) async {
        guard validateNew(passphrase, confirmation: confirmation) else { return }
        do {
            let grant = try await auth.authorize(.changeCredential, credential: .unencrypted)
            vault = try await vaultService.encrypt(newPassphrase: vaultService.makeSecret(utf8: passphrase), grant: grant)
            infoMessage = L10n.Settings.walletEncrypted
        } catch {
            errorMessage = securityText(error.code)
        }
    }

    public func changePassphrase(old: String, new: String, confirmation: String) async {
        guard validateNew(new, confirmation: confirmation) else { return }
        do {
            vault = try await vaultService.changePassphrase(
                old: vaultService.makeSecret(utf8: old), new: vaultService.makeSecret(utf8: new))
            infoMessage = L10n.Settings.passphraseChanged
        } catch {
            errorMessage = securityText(error.code)
        }
    }

    /// Show Recovery Phrase (QT-113): authorizes `.revealSecret` first. On an
    /// encrypted vault without `passphrase` it sets `needsPassphrase` and
    /// returns `nil`. The BIP39 passphrase is part of the result (DESIGN.md R1).
    public func revealPhrase(passphrase: String? = nil) async -> RevealedMnemonic? {
        errorMessage = nil
        guard let wallet = walletState.selectedWalletID else {
            errorMessage = L10n.Common.noWallet
            return nil
        }
        let credential: Credential
        switch auth.requirement(for: .revealSecret) {
        case .none:
            credential = .unencrypted
        case .passphrase, .quickUnlockOrPassphrase:
            guard let passphrase else {
                needsPassphrase = true
                return nil
            }
            credential = .passphrase(vaultService.makeSecret(utf8: passphrase))
        }
        needsPassphrase = false
        do {
            let grant = try await auth.authorize(.revealSecret, credential: credential)
            return try await vaultService.revealMnemonic(wallet: wallet, grant: grant)
        } catch {
            errorMessage = securityText(error.code)
            return nil
        }
    }

    // MARK: Private

    private func validateNew(_ passphrase: String, confirmation: String) -> Bool {
        errorMessage = nil
        infoMessage = nil
        if passphrase.isEmpty {
            errorMessage = L10n.Settings.passphraseEmpty
        } else if passphrase != confirmation {
            errorMessage = L10n.Settings.passphraseMismatch
        } else if passphrase.count > 1024 {
            errorMessage = L10n.Onboarding.passphraseTooLong
        } else {
            return true
        }
        return false
    }

    private func savePreferences(_ stored: UIPreferences, onSuccess: () -> Void) {
        do {
            try preferences.update(stored)
            onSuccess()
            errorMessage = nil
        } catch {
            errorMessage = L10n.Settings.settingsNotSaved
        }
    }

    private func securityText(_ code: ServiceErrorCode) -> String {
        switch code {
        case EngineCode.vaultAlreadyEncrypted: L10n.Settings.alreadyEncrypted
        case EngineCode.vaultNotEncrypted: L10n.Settings.notEncrypted
        case .vaultNoSecret: L10n.Settings.noRecoveryPhrase
        case .vaultPassphraseRejected: L10n.Onboarding.passphraseRejected
        default: ErrorText.common(code)
        }
    }
}
