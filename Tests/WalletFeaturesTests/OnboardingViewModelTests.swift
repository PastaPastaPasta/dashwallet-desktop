// Create and restore flows (QT-102…105, QT-111, IOS-002…007, IOS-010, review H-5).
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

/// Deterministic generator for the verify challenge.
struct SeededGenerator: RandomNumberGenerator {
    var state: UInt64

    mutating func next() -> UInt64 {
        state = state &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
        return state
    }
}

@MainActor
@Suite("Onboarding view model")
struct OnboardingViewModelTests {
    let world = FakeWorld(wallets: [], selected: nil)
    static let phrase = "abandon ability able about above absent absorb abstract absurd abuse access accident"

    func makeModel(developerMode: Bool = false, withAuth: Bool = true) -> OnboardingViewModel {
        OnboardingViewModel(
            vault: world.vault, lifecycle: world.lifecycle, host: world.host, screenCapture: world.screenCapture,
            timing: world.timing, developerMode: developerMode, auth: withAuth ? world.auth : nil,
            randomNumberGenerator: SeededGenerator(state: 42))
    }

    /// Picks the correct chips for the whole challenge.
    func solveChallenge(_ model: OnboardingViewModel) async {
        let words = Self.phrase.split(separator: " ").map(String.init)
        for position in model.verifyChallenge {
            let chip = model.verifyChips.first { $0.word == words[position] && !$0.used }!
            await model.select(chip: chip.id)
        }
    }

    // MARK: Create

    @Test func IOS002_createShowsAFreshPhraseAndStoresNothing() async {
        let model = makeModel()
        await model.startCreate()
        #expect(model.step == .showPhrase)
        #expect(model.flow == .create)
        #expect(model.phraseWords == Self.phrase.split(separator: " ").map(String.init))
        #expect(world.vault.state.current.generateCalls == [12])
        #expect(world.vault.state.current.creates.isEmpty)
        #expect(world.lifecycle.imports.current.isEmpty)
    }

    @Test func IOS002_offers12And24Words() async {
        let model = makeModel()
        model.setWordCount(15)
        #expect(model.wordCount == 12)
        model.setWordCount(24)
        await model.startCreate()
        #expect(world.vault.state.current.generateCalls == [24])
    }

    @Test func IOS006_phraseScreenIsProtectedFromCapture() async {
        let model = makeModel()
        await model.startCreate()
        #expect(model.captureProtected)
        #expect(world.screenCapture.visibleCalls == [true])
        await eventually { world.screenCapture.broadcast.subscriberCount == 1 }
        world.screenCapture.broadcast.send(.screenshotTaken)
        await eventually { model.captureWarning }
        model.confirmWrittenDown()
        #expect(model.phraseWords.isEmpty)
        #expect(world.screenCapture.visibleCalls == [true, false])
        #expect(!model.captureProtected)
    }

    @Test func IOS004_verifyChallengeNeedsTheRightWordsInOrder() async {
        let model = makeModel()
        await model.startCreate()
        model.confirmWrittenDown()
        #expect(model.step == .verifyPhrase)
        #expect(model.verifyChallenge.count == 4)
        #expect(model.verifyChallenge == model.verifyChallenge.sorted())
        #expect(model.verifyChips.count == 6)
        let words = Self.phrase.split(separator: " ").map(String.init)
        // A word that is not the next challenge word is a mistake and does not advance.
        let wrong = model.verifyChips.first { $0.word != words[model.verifyChallenge[0]] }!
        await model.select(chip: wrong.id)
        #expect(model.verifyMistake)
        #expect(model.verifiedCount == 0)
        await solveChallenge(model)
        #expect(model.isVerified)
        #expect(!model.verifyMistake)
        // No vault yet: the encryption passphrase comes next.
        #expect(model.step == .choosePassphrase)
    }

    @Test func verifyWordChecksPositions() async {
        let model = makeModel()
        await model.startCreate()
        #expect(model.verify(word: "abandon", at: 0))
        #expect(!model.verify(word: "ability", at: 0))
        #expect(!model.verify(word: "abandon", at: 99))
    }

    @Test func QT111_passphraseRules() async {
        let model = makeModel()
        #expect(!model.setPassphrase("", confirmation: ""))
        #expect(model.passphraseError == L10n.Onboarding.passphraseEmpty)
        #expect(!model.setPassphrase("one", confirmation: "two"))
        #expect(model.passphraseError == L10n.Onboarding.passphraseMismatch)
        let long = String(repeating: "x", count: 1025)
        #expect(!model.setPassphrase(long, confirmation: long))
        #expect(model.passphraseError == L10n.Onboarding.passphraseTooLong)
        let max = String(repeating: "x", count: 1024)
        #expect(model.setPassphrase(max, confirmation: max))
        #expect(model.passphraseError == nil)
        model.updatePassphraseDraft("Tr0ub4dor&3xyz")
        #expect(model.passphraseStrength >= .good)
    }

    @Test func IOS010_createEncryptsTheNewVaultAndImportsAsANewWallet() async {
        let model = makeModel()
        await model.startCreate()
        model.confirmWrittenDown()
        await solveChallenge(model)
        #expect(model.setPassphrase("vault pass", confirmation: "vault pass"))
        await model.finish()
        #expect(model.step == .done(world.lifecycle.nextWalletID))
        #expect(world.vault.state.current.creates == ["vault pass"])
        let call = world.lifecycle.imports.current.first
        #expect(call?.mnemonic == Self.phrase)
        #expect(call?.bip39Passphrase == "")
        // H-5: a new wallet lets the engine start at its tip.
        #expect(call?.options.birthHeight == nil)
        #expect(call?.options.coreCompatible == false)
        #expect(call?.options.lookahead == nil)
        #expect(model.phraseWords.isEmpty && model.verifyChips.isEmpty)
    }

    @Test func skippingEncryptionCreatesAnUnencryptedVault() async {
        let model = makeModel()
        await model.startCreate()
        model.confirmWrittenDown()
        await solveChallenge(model)
        #expect(model.setPassphrase(nil, confirmation: nil))
        await model.finish()
        #expect(world.vault.state.current.creates == [nil])
    }

    @Test func secondWalletSkipsVaultCreation() async {
        world.vault.state.withLock {
            $0.status = VaultStatus(
                state: .unlocked, encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0,
                retryAfterSeconds: nil, walletsWithSecrets: [walletA])
        }
        let model = makeModel()
        await model.startCreate()
        model.confirmWrittenDown()
        await solveChallenge(model)
        #expect(model.step == .done(world.lifecycle.nextWalletID))
        #expect(world.vault.state.current.creates.isEmpty)
    }

    @Test func lockedVaultIsUnlockedBeforeAddingAWallet() async {
        world.vault.state.withLock {
            $0.status = VaultStatus(
                state: .locked, encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0,
                retryAfterSeconds: nil, walletsWithSecrets: [walletA])
        }
        world.auth.unlockErrors = [ServiceError(code: .vaultWrongPassphrase)]
        let model = makeModel()
        model.startRestore()
        await model.updateRestoreText(Self.phrase)
        model.continueRestore()
        await model.finishRestore()
        #expect(model.step == .unlockVault)
        await model.unlockVault(passphrase: "wrong")
        #expect(model.step == .unlockVault)
        #expect(model.unlockError == L10n.Common.wrongPassphrase)
        await model.unlockVault(passphrase: "right")
        #expect(world.auth.unlockCalls.map(\.0) == ["wrong", "right"])
        #expect(world.auth.unlockCalls.last?.1 == .full)
        #expect(model.step == .done(world.lifecycle.nextWalletID))
    }

    @Test func importRefusedAsLockedAsksForTheVaultPassphrase() async {
        world.vault.state.withLock {
            $0.status = VaultStatus(
                state: .unlocked, encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0,
                retryAfterSeconds: nil, walletsWithSecrets: [])
        }
        world.lifecycle.importErrors.withLock { $0 = [ServiceError(code: .init(rawValue: "wallet.vault_locked"))] }
        let model = makeModel()
        model.startRestore()
        await model.updateRestoreText(Self.phrase)
        model.continueRestore()
        await model.finishRestore()
        #expect(model.step == .unlockVault)
        await model.unlockVault(passphrase: "pw")
        #expect(model.step == .done(world.lifecycle.nextWalletID))
    }

    @Test func failureAfterUnlockReturnsToTheStepBeforeIt() async {
        world.vault.state.withLock {
            $0.status = VaultStatus(
                state: .locked, encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0,
                retryAfterSeconds: nil, walletsWithSecrets: [walletA])
        }
        world.lifecycle.importErrors.withLock { $0 = [ServiceError(code: .walletAlreadyExists)] }
        let model = makeModel()
        model.startRestore()
        await model.updateRestoreText(Self.phrase)
        model.continueRestore()
        await model.finishRestore()
        await model.unlockVault(passphrase: "pw")
        #expect(model.step == .failed(OnboardingFailure(code: .walletAlreadyExists, message: L10n.Onboarding.walletAlreadyExists)))
        model.back()
        #expect(model.step == .restoreOptions)
    }

    @Test func withoutAnAuthGateALockedVaultFails() async {
        world.vault.state.withLock {
            $0.status = VaultStatus(
                state: .unlocked, encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0,
                retryAfterSeconds: nil, walletsWithSecrets: [])
        }
        world.lifecycle.importErrors.withLock { $0 = [ServiceError(code: .vaultLocked)] }
        let model = makeModel(withAuth: false)
        model.startRestore()
        await model.updateRestoreText(Self.phrase)
        model.continueRestore()
        await model.finishRestore()
        #expect(model.step == .failed(OnboardingFailure(code: .vaultLocked, message: L10n.Onboarding.vaultLocked)))
    }

    // MARK: Restore

    @Test func QT104_restoreDefaultsScanFromGenesisWithoutLookahead() {
        let model = makeModel()
        model.startRestore()
        #expect(model.step == .restorePhrase)
        #expect(model.options.birthHeight == 0)
        #expect(model.options.lookahead == nil)
        #expect(!model.options.coreCompatible)
    }

    @Test func IOS007_liveValidationAndSuggestions() async {
        let model = makeModel()
        model.startRestore()
        await model.updateRestoreText("aban")
        #expect(model.wordSuggestions == ["abandon"])
        await model.updateRestoreText("abandon ")
        #expect(model.wordSuggestions.isEmpty)
        #expect(model.restoreProblem == L10n.Onboarding.unsupportedWordCount)
        #expect(!model.canContinueRestore)
        await model.updateRestoreText("  ABANDON   ability able about above absent absorb abstract absurd abuse access accident ")
        #expect(model.restoreCheck?.wordCount == 12)
        #expect(model.restoreProblem == nil)
        #expect(model.canContinueRestore)
        model.continueRestore()
        #expect(model.step == .restoreOptions)
    }

    @Test func IOS007_unknownWordsAndBadChecksumAreReported() async {
        let unknown = "abandon xyzzy able about above absent absorb abstract absurd abuse access accident"
        let badSum = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon"
        world.vault.state.withLock {
            $0.checks[unknown] = MnemonicCheck(wordCount: 12, unknownWordIndices: [1], language: nil, checksum: .invalid)
            $0.checks[badSum] = MnemonicCheck(wordCount: 12, unknownWordIndices: [], language: .english, checksum: .invalid)
        }
        let model = makeModel()
        model.startRestore()
        await model.updateRestoreText(unknown)
        #expect(model.restoreProblem == "Word 2 is not in the word list.")
        await model.updateRestoreText(badSum)
        #expect(model.restoreProblem == L10n.Onboarding.invalidPhrase)
        #expect(!model.canContinueRestore)
    }

    @Test func QT104_coreOnlyChecksumForcesCoreCompatibility() async {
        world.vault.state.withLock {
            $0.checks[Self.phrase] = MnemonicCheck(wordCount: 12, unknownWordIndices: [], language: .english, checksum: .coreOnly)
        }
        let model = makeModel()
        model.startRestore()
        await model.updateRestoreText(Self.phrase)
        #expect(model.coreOnlyWarning)
        #expect(model.options.coreCompatible)
        #expect(model.canContinueRestore)
        model.setCoreCompatible(false)
        #expect(model.options.coreCompatible)
    }

    @Test func QT104_restoreWithBIP39PassphraseCoreCompatAndBirthHeight() async {
        world.vault.state.withLock {
            $0.status = VaultStatus(
                state: .unencrypted, encrypted: false, quickUnlockEnrolled: false, failedAttempts: 0,
                retryAfterSeconds: nil, walletsWithSecrets: [])
        }
        let model = makeModel()
        model.startRestore()
        await model.updateRestoreText(Self.phrase)
        model.continueRestore()
        model.setBIP39Passphrase("25th word")
        model.setCoreCompatible(true)
        model.setBirthHeight(1_000_000)
        await model.finishRestore()
        let call = world.lifecycle.imports.current.first
        #expect(call?.mnemonic == Self.phrase)
        #expect(call?.bip39Passphrase == "25th word")
        #expect(call?.options.coreCompatible == true)
        #expect(call?.options.birthHeight == 1_000_000)
        #expect(model.step == .done(world.lifecycle.nextWalletID))
    }

    @Test func QT105_birthDateBecomesAConservativeHeight() async {
        try? await world.host.start(network: .mainnet, options: NetworkOptions())
        let model = makeModel()
        await model.load()
        model.startRestore()
        model.setBirthDate(world.clock.now.addingTimeInterval(86_400))
        #expect(model.birthDateError == L10n.Onboarding.birthDateInFuture)
        let date = Date(timeIntervalSince1970: 1_700_000_000)
        model.setBirthDate(date)
        #expect(model.birthDateError == nil)
        #expect(model.options.birthHeight == BirthHeightEstimator.height(for: date, network: .mainnet))
        model.setBirthDate(nil)
        #expect(model.options.birthHeight == 0)
        model.setBirthHeight(nil)
        #expect(model.options.birthHeight == 0)
    }

    @Test func birthHeightEstimateIsBelowTheRealHeight() {
        // Mainnet block 1,900,000 was mined around 2023-08-22 (2.5 min blocks).
        let estimate = BirthHeightEstimator.height(for: Date(timeIntervalSince1970: 1_692_700_000), network: .mainnet)
        #expect(estimate > 1_000_000 && estimate < 1_900_000)
        #expect(BirthHeightEstimator.height(for: Date(timeIntervalSince1970: 0), network: .mainnet) == 0)
        #expect(BirthHeightEstimator.height(for: Date(), network: .regtest) == 0)
        #expect(BirthHeightEstimator.height(for: Date(), network: .devnet(name: "a")) == 0)
    }

    @Test func importFailureCanBeRetriedFromTheSameStep() async {
        world.vault.state.withLock {
            $0.status = VaultStatus(
                state: .unencrypted, encrypted: false, quickUnlockEnrolled: false, failedAttempts: 0,
                retryAfterSeconds: nil, walletsWithSecrets: [])
        }
        world.lifecycle.importErrors.withLock { $0 = [ServiceError(code: .walletAlreadyExists)] }
        let model = makeModel()
        model.startRestore()
        await model.updateRestoreText(Self.phrase)
        model.continueRestore()
        await model.finishRestore()
        #expect(model.step == .failed(OnboardingFailure(code: .walletAlreadyExists, message: L10n.Onboarding.walletAlreadyExists)))
        model.back()
        #expect(model.step == .restoreOptions)
        await model.finishRestore()
        #expect(model.step == .done(world.lifecycle.nextWalletID))
    }

    @Test func createAfterAbandonedRestoreUsesNewWalletOptions() async {
        let model = makeModel()
        model.startRestore()
        model.setBirthHeight(5)
        model.back()
        #expect(model.step == .welcome)
        await model.startCreate()
        #expect(model.options.birthHeight == nil)
    }

    // MARK: Network (IOS-106)

    @Test func IOS106_networkChoiceOnTheWelcomeScreen() async {
        let model = makeModel()
        await model.load()
        #expect(model.network == .testnet)
        #expect(model.availableNetworks == [.mainnet, .testnet])
        await model.chooseNetwork(.mainnet)
        #expect(model.network == .mainnet)
        #expect(world.lifecycle.switches.current == [.mainnet])
        #expect(makeModel(developerMode: true).availableNetworks.contains(.regtest))
    }

    @Test func passphraseStrengthScale() {
        #expect(PassphraseStrength.evaluate("") == .none)
        #expect(PassphraseStrength.evaluate("short") == .weak)
        #expect(PassphraseStrength.evaluate("lowercase") == .weak)
        #expect(PassphraseStrength.evaluate("Lower1234") == .fair)
        #expect(PassphraseStrength.evaluate("Correct-Horse-Battery-9-Staple") == .strong)
    }
}
