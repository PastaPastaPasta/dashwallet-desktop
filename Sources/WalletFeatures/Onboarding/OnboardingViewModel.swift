// Create and restore flows (QT-102…105, IOS-002…007, IOS-010).
import Foundation
import Observation
import PlatformServices
import WalletRuntime

public enum OnboardingStep: Sendable, Equatable {
    case welcome
    /// The new phrase is on screen (IOS-003, IOS-006).
    case showPhrase
    /// Word-chip verification (IOS-004).
    case verifyPhrase
    /// Encryption passphrase for a new vault (QT-111, IOS-010).
    case choosePassphrase
    case restorePhrase
    /// BIP39 passphrase, Core compatibility, birth date (QT-104/105).
    case restoreOptions
    case working
    case done(WalletID)
    case failed(OnboardingFailure)
}

public enum OnboardingFlow: Sendable, Hashable {
    case create
    case restore
}

public struct OnboardingFailure: Sendable, Equatable {
    public let code: ServiceErrorCode
    public let message: String
}

/// One word chip on the verify screen.
public struct VerifyChip: Sendable, Hashable, Identifiable {
    public let id: Int
    public let word: String
    public internal(set) var used: Bool
}

@MainActor
@Observable
public final class OnboardingViewModel {
    public private(set) var step: OnboardingStep = .welcome
    public private(set) var flow: OnboardingFlow?
    public private(set) var network: DashNetwork?
    public let availableNetworks: [DashNetwork]

    // Create.
    public private(set) var wordCount = 12
    /// Display only; filled while `step == .showPhrase` and cleared on leaving it.
    public private(set) var phraseWords: [String] = []
    /// Phrase positions (0-based) to confirm, in order (IOS-004).
    public private(set) var verifyChallenge: [Int] = []
    public private(set) var verifyChips: [VerifyChip] = []
    /// How many challenge words were picked correctly.
    public private(set) var verifiedCount = 0
    public private(set) var verifyMistake = false
    /// The OS reported a screenshot or recording while the phrase was shown.
    public private(set) var captureWarning = false
    /// The OS excludes the window from capture while the phrase is shown.
    public private(set) var captureProtected = false

    // Passphrase.
    public private(set) var passphraseStrength: PassphraseStrength = .none
    public private(set) var passphraseError: String?

    // Restore.
    public private(set) var restoreCheck: MnemonicCheck?
    public private(set) var restoreProblem: String?
    /// The phrase only passes Dash Core's legacy checksum; Core compatibility
    /// was switched on automatically (QT-104).
    public private(set) var coreOnlyWarning = false
    public private(set) var wordSuggestions: [String] = []
    public private(set) var birthDate: Date?
    public private(set) var birthDateError: String?
    public var options = WalletImportOptions()

    public var isVerified: Bool { !verifyChallenge.isEmpty && verifiedCount == verifyChallenge.count }

    public var canContinueRestore: Bool {
        guard let check = restoreCheck, check.unknownWordIndices.isEmpty,
            MnemonicWords.restoreCounts.contains(check.wordCount)
        else { return false }
        return check.checksum == .valid || check.checksum == .coreOnly
    }

    private let vault: any VaultProviding
    private let lifecycle: any LifecycleQueueing
    private let host: any WalletHosting
    private let screenCapture: (any ScreenCaptureGuard)?
    private let timing: Timing
    private var rng: AnyRandomNumberGenerator
    private var mnemonic: (any SecretBuffer)?
    private var bip39Passphrase: (any SecretBuffer)?
    private var vaultPassphrase: (any SecretBuffer)?
    private var skipEncryption = false
    private var restoreGeneration = 0
    /// Where `back()` returns after a failed finish.
    private var retryStep: OnboardingStep = .welcome
    private var captureTask: Task<Void, Never>?

    public init(
        vault: any VaultProviding, lifecycle: any LifecycleQueueing, host: any WalletHosting,
        screenCapture: (any ScreenCaptureGuard)?, timing: Timing, developerMode: Bool,
        randomNumberGenerator: any RandomNumberGenerator = SystemRandomNumberGenerator()
    ) {
        self.vault = vault
        self.lifecycle = lifecycle
        self.host = host
        self.screenCapture = screenCapture
        self.timing = timing
        self.rng = AnyRandomNumberGenerator(base: randomNumberGenerator)
        self.availableNetworks = developerMode ? [.mainnet, .testnet, .regtest] : [.mainnet, .testnet]
    }

    public convenience init(env: AppEnvironment) {
        self.init(
            vault: env.vault, lifecycle: env.lifecycle, host: env.host, screenCapture: env.screenCapture,
            timing: env.timing, developerMode: env.developerMode)
    }

    public func load() async {
        network = await host.activeNetwork
    }

    /// Switches the network before a wallet is created (IOS-106 on the welcome screen).
    public func chooseNetwork(_ network: DashNetwork) async {
        guard step == .welcome, network != self.network else { return }
        do {
            try await lifecycle.switchNetwork(to: network)
            self.network = network
        } catch {
            step = .failed(failure(error))
        }
    }

    // MARK: Create

    public func setWordCount(_ count: Int) {
        guard MnemonicWords.createCounts.contains(count) else { return }
        wordCount = count
    }

    /// Generates a phrase and shows it. Nothing is stored until `finish()`.
    public func startCreate() async {
        flow = .create
        do {
            let phrase = try await vault.generateMnemonic(wordCount: wordCount, language: .english)
            mnemonic = phrase
            phraseWords = Self.words(of: phrase)
            enterShowPhrase()
        } catch {
            step = .failed(failure(error))
        }
    }

    /// The user wrote the phrase down: hide it and build the verify challenge.
    public func confirmWrittenDown() {
        guard step == .showPhrase, let mnemonic else { return }
        leaveShowPhrase()
        let words = Self.words(of: mnemonic)
        let challengeSize = min(4, words.count)
        var positions = Array(words.indices)
        positions.shuffle(using: &rng)
        verifyChallenge = positions.prefix(challengeSize).sorted()
        let challengeWords = Set(verifyChallenge.map { words[$0] })
        let decoys = positions.dropFirst(challengeSize).map { words[$0] }
            .filter { !challengeWords.contains($0) }
        var chipWords = verifyChallenge.map { words[$0] } + Array(Set(decoys).sorted().prefix(2))
        chipWords.shuffle(using: &rng)
        verifyChips = chipWords.enumerated().map { VerifyChip(id: $0.offset, word: $0.element, used: false) }
        verifiedCount = 0
        verifyMistake = false
        step = .verifyPhrase
    }

    /// Whether `word` is the phrase word at `index` (0-based).
    public func verify(word: String, at index: Int) -> Bool {
        guard let mnemonic else { return false }
        let words = Self.words(of: mnemonic)
        return words.indices.contains(index) && words[index] == word
    }

    /// Picks a chip for the next challenge position. A wrong pick flags
    /// `verifyMistake` and does not advance (iOS behaviour).
    public func select(chip id: VerifyChip.ID) async {
        guard step == .verifyPhrase, !isVerified,
            let chipIndex = verifyChips.firstIndex(where: { $0.id == id && !$0.used })
        else { return }
        if verify(word: verifyChips[chipIndex].word, at: verifyChallenge[verifiedCount]) {
            verifyChips[chipIndex].used = true
            verifiedCount += 1
            verifyMistake = false
            if isVerified { await advanceAfterPhrase() }
        } else {
            verifyMistake = true
        }
    }

    // MARK: Passphrase

    /// Live strength while typing.
    public func updatePassphraseDraft(_ text: String) {
        passphraseStrength = PassphraseStrength.evaluate(text)
        passphraseError = nil
    }

    /// Sets the vault passphrase; `nil` creates an unencrypted vault ("skip
    /// encryption" under Advanced, DESIGN-opus §7.3). Returns `false` and
    /// sets `passphraseError` when the input is rejected.
    @discardableResult
    public func setPassphrase(_ text: String?, confirmation: String?) -> Bool {
        guard let text else {
            vaultPassphrase = nil
            skipEncryption = true
            passphraseError = nil
            return true
        }
        if text.isEmpty {
            passphraseError = L10n.Onboarding.passphraseEmpty
        } else if text != confirmation {
            passphraseError = L10n.Onboarding.passphraseMismatch
        } else if text.count > 1024 {
            passphraseError = L10n.Onboarding.passphraseTooLong
        } else {
            vaultPassphrase = vault.makeSecret(utf8: text)
            skipEncryption = false
            passphraseError = nil
            return true
        }
        return false
    }

    // MARK: Restore

    public func startRestore() {
        flow = .restore
        // Scan from genesis by default (review H-5). No lookahead: the engine
        // has no per-wallet gap limit yet (DESIGN-fable U12) and refuses one
        // with not_implemented rather than ignoring it.
        options = WalletImportOptions(birthHeight: 0, coreCompatible: false, lookahead: nil)
        step = .restorePhrase
    }

    /// Validates the typed phrase. The text goes into a zeroing buffer at
    /// once; only suggestions for the last word are derived from the string.
    public func updateRestoreText(_ text: String) async {
        let endsWithSpace = text.last?.isWhitespace ?? true
        let lastWord = text.split(whereSeparator: { $0.isWhitespace }).last.map(String.init) ?? ""
        wordSuggestions = endsWithSpace ? [] : MnemonicWords.suggestions(for: lastWord)
        restoreGeneration += 1
        let generation = restoreGeneration
        let normalized = MnemonicWords.normalize(text)
        guard !normalized.isEmpty else {
            mnemonic = nil
            restoreCheck = nil
            restoreProblem = nil
            coreOnlyWarning = false
            return
        }
        let secret = vault.makeSecret(utf8: normalized)
        do {
            let check = try await vault.checkMnemonic(secret)
            guard generation == restoreGeneration else { return }
            mnemonic = secret
            applyRestoreCheck(check)
        } catch {
            guard generation == restoreGeneration else { return }
            restoreCheck = nil
            restoreProblem = failure(error).message
        }
    }

    public func continueRestore() {
        guard step == .restorePhrase, canContinueRestore else { return }
        step = .restoreOptions
    }

    /// Optional BIP39 passphrase ("25th word"). Empty means none.
    public func setBIP39Passphrase(_ text: String) {
        bip39Passphrase = text.isEmpty ? nil : vault.makeSecret(utf8: text)
    }

    /// Core compatibility (QT-104). Cannot be switched off for a phrase that
    /// only passes Core's checksum.
    public func setCoreCompatible(_ on: Bool) {
        options.coreCompatible = on || restoreCheck?.checksum == .coreOnly
    }

    /// Scan start from the wallet's creation date; `nil` scans from genesis.
    public func setBirthDate(_ date: Date?) {
        birthDateError = nil
        guard let date else {
            birthDate = nil
            options.birthHeight = 0
            return
        }
        if date > timing.now() {
            birthDateError = L10n.Onboarding.birthDateInFuture
            return
        }
        birthDate = date
        options.birthHeight = BirthHeightEstimator.height(for: date, network: network ?? .mainnet)
    }

    /// Scan start as a block height (advanced); `nil` scans from genesis.
    public func setBirthHeight(_ height: UInt32?) {
        birthDate = nil
        birthDateError = nil
        options.birthHeight = height ?? 0
    }

    public func finishRestore() async {
        guard step == .restoreOptions, birthDateError == nil else { return }
        await advanceAfterPhrase()
    }

    // MARK: Finish

    /// Creates the vault when there is none, then adds the wallet through the
    /// lifecycle queue.
    public func finish() async {
        guard let mnemonic else { return }
        if step != .working { retryStep = step }
        step = .working
        do {
            let status = try await vault.status()
            if status.state == .noVault {
                _ = try await vault.create(passphrase: skipEncryption ? nil : vaultPassphrase)
            }
            let id = try await lifecycle.importWallet(
                mnemonic: mnemonic, bip39Passphrase: bip39Passphrase ?? vault.makeSecret(utf8: ""),
                options: options)
            clearSecrets()
            step = .done(id)
        } catch {
            step = .failed(failure(error))
        }
    }

    /// Returns to the previous screen; from a failure, to where the user can retry.
    public func back() {
        switch step {
        case .showPhrase, .restorePhrase:
            leaveShowPhrase()
            clearSecrets()
            flow = nil
            step = .welcome
        case .verifyPhrase:
            if let mnemonic {
                phraseWords = Self.words(of: mnemonic)
                enterShowPhrase()
            }
        case .restoreOptions:
            step = .restorePhrase
        case .choosePassphrase:
            step = flow == .restore ? .restoreOptions : .verifyPhrase
        case .failed:
            if mnemonic == nil {
                flow = nil
                step = .welcome
            } else {
                step = retryStep
            }
        case .welcome, .working, .done:
            break
        }
    }

    public func dismissCaptureWarning() {
        captureWarning = false
    }

    // MARK: Private

    private func advanceAfterPhrase() async {
        retryStep = step
        do {
            let status = try await vault.status()
            if status.state == .noVault {
                passphraseStrength = .none
                step = .choosePassphrase
            } else {
                await finish()
            }
        } catch {
            step = .failed(failure(error))
        }
    }

    private func applyRestoreCheck(_ check: MnemonicCheck) {
        restoreCheck = check
        coreOnlyWarning = check.checksum == .coreOnly
        if coreOnlyWarning { options.coreCompatible = true }
        if !check.unknownWordIndices.isEmpty {
            restoreProblem = L10n.Onboarding.unknownWords(check.unknownWordIndices)
        } else if !MnemonicWords.restoreCounts.contains(check.wordCount) {
            restoreProblem = L10n.Onboarding.unsupportedWordCount
        } else if check.checksum == .invalid {
            restoreProblem = L10n.Onboarding.invalidPhrase
        } else {
            restoreProblem = nil
        }
    }

    private func enterShowPhrase() {
        captureWarning = false
        captureProtected = screenCapture?.setSecretContentVisible(true) ?? false
        if let screenCapture {
            let events = screenCapture.events()
            captureTask?.cancel()
            captureTask = Task { [weak self] in
                for await event in events {
                    guard let self else { return }
                    if event != .recordingStopped { self.captureWarning = true }
                }
            }
        }
        step = .showPhrase
    }

    private func leaveShowPhrase() {
        phraseWords = []
        captureTask?.cancel()
        captureTask = nil
        if screenCapture != nil {
            screenCapture?.setSecretContentVisible(false)
            captureProtected = false
        }
    }

    private func clearSecrets() {
        mnemonic = nil
        bip39Passphrase = nil
        vaultPassphrase = nil
        skipEncryption = false
        phraseWords = []
        verifyChips = []
        verifyChallenge = []
        verifiedCount = 0
    }

    private func failure(_ error: ServiceError) -> OnboardingFailure {
        let message: String
        switch error.code {
        case .walletInvalidMnemonic: message = L10n.Onboarding.invalidPhrase
        case .walletUnsupportedWordCount: message = L10n.Onboarding.unsupportedWordCount
        case .walletAlreadyExists: message = L10n.Onboarding.walletAlreadyExists
        case .walletWatchOnlyExists: message = L10n.Onboarding.watchOnlyExists
        case .vaultLocked, EngineCode.walletVaultLocked: message = L10n.Onboarding.vaultLocked
        case .vaultPassphraseRejected: message = L10n.Onboarding.passphraseRejected
        default: message = ErrorText.common(error.code)
        }
        return OnboardingFailure(code: error.code, message: message)
    }

    /// Splits a phrase buffer into words for display. The strings live only
    /// as long as the screen shows them.
    private static func words(of phrase: any SecretBuffer) -> [String] {
        phrase.withUnsafeBytes { bytes in
            String(decoding: bytes, as: UTF8.self).split(separator: " ").map(String.init)
        }
    }
}
