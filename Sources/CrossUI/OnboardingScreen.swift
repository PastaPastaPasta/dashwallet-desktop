// Onboarding (create / restore) and the lock screen (QT-102…105, QT-111,
// IOS-002…007, IOS-010, IOS-012/013).
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct OnboardingScreen: View {
    let model: OnboardingViewModel

    @State var passphrase = ""
    @State var confirmation = ""
    @State var restoreText = ""
    @State var bip39Passphrase = ""
    @State var birthHeightText = ""

    var body: some View {
        let model = model
        Page(L10n.Onboarding.welcomeTitle) {
            step(model)
        }
    }

    @ViewBuilder
    private func step(_ model: OnboardingViewModel) -> some View {
        switch model.step {
        case .welcome:
            DashCard {
                if !model.availableNetworks.isEmpty {
                    DashPicker(
                        CrossStrings.network,
                        options: model.availableNetworks.map { PickerOption($0, L10n.Settings.networkName($0)) },
                        selection: bind(
                            { model.network ?? model.availableNetworks[0] },
                            { network in Task { await model.chooseNetwork(network) } }))
                }
                DashPicker(
                    CrossStrings.wordCount,
                    options: MnemonicWords.createCounts.map { PickerOption($0, CrossStrings.words($0)) },
                    selection: bind({ model.wordCount }, { model.setWordCount($0) }))
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(L10n.Onboarding.createWallet, size: .large) { Task { await model.startCreate() } }
                    DashButton(L10n.Onboarding.restoreWallet, style: .tintedBlue, size: .large) {
                        restoreText = ""
                        model.startRestore()
                    }
                }
            }
        case .showPhrase:
            DashCard {
                Toast(L10n.Onboarding.showPhraseWarning, kind: .warning)
                if model.captureWarning {
                    Toast(L10n.Onboarding.screenCaptureWarning, kind: .error, actionTitle: CrossStrings.close) {
                        model.dismissCaptureWarning()
                    }
                }
                PhraseGrid(words: model.phraseWords)
                navigation(model) {
                    DashButton(CrossStrings.wroteItDown) { model.confirmWrittenDown() }
                }
            }
        case .verifyPhrase:
            DashCard {
                Text(L10n.Onboarding.verifyPrompt).dashFont(.footnote)
                if model.verifiedCount < model.verifyChallenge.count {
                    SectionHeader(CrossStrings.selectWord(model.verifyChallenge[model.verifiedCount] + 1), style: .subheadMedium)
                }
                HStack(spacing: Int(DashSpacing.s)) {
                    ForEach(model.verifyChips) { chip in
                        DashButton(chip.word, style: .tintedGray, size: .small, isEnabled: !chip.used) {
                            Task { await model.select(chip: chip.id) }
                        }
                    }
                }
                if model.verifyMistake {
                    Text(L10n.Onboarding.verifyWrongWord).dashFont(.caption1).dashForeground(.errorText)
                }
                if model.isVerified {
                    Text(L10n.Onboarding.verifiedSuccessfully).dashFont(.caption1).dashForeground(.green)
                }
                navigation(model) { EmptyView() }
            }
        case .choosePassphrase:
            DashCard {
                Text(L10n.Onboarding.encryptWarning).dashFont(.footnote)
                DashSecureField(
                    CrossStrings.newPassphrase,
                    text: bind({ passphrase }, { text in
                        passphrase = text
                        model.updatePassphraseDraft(text)
                    }),
                    error: model.passphraseError)
                DashSecureField(CrossStrings.repeatPassphrase, text: $confirmation)
                if model.passphraseStrength != .none {
                    Text("\(CrossStrings.strength) \(model.passphraseStrength.title)").dashFont(.caption1)
                }
                navigation(model) {
                    DashButton(CrossStrings.encryptWithPassphrase, isEnabled: !passphrase.isEmpty) {
                        let (text, repeated) = (passphrase, confirmation)
                        if model.setPassphrase(text, confirmation: repeated) {
                            passphrase = ""
                            confirmation = ""
                            Task { await model.finish() }
                        }
                    }
                    DashButton(CrossStrings.skipEncryption, style: .plainBlue) {
                        passphrase = ""
                        confirmation = ""
                        model.setPassphrase(nil, confirmation: nil)
                        Task { await model.finish() }
                    }
                }
            }
        case .restorePhrase:
            DashCard {
                DashTextField(
                    CrossStrings.recoveryPhrase, placeholder: CrossStrings.recoveryPhrasePlaceholder,
                    text: bind({ restoreText }, { text in
                        restoreText = text
                        Task { await model.updateRestoreText(text) }
                    }),
                    error: model.restoreProblem)
                if !model.wordSuggestions.isEmpty {
                    Text("\(CrossStrings.suggestions) \(model.wordSuggestions.joined(separator: ", "))")
                        .dashFont(.caption1).dashForeground(.secondaryText)
                }
                if model.coreOnlyWarning {
                    Toast(L10n.Onboarding.coreOnlyChecksumWarning, kind: .warning)
                }
                navigation(model) {
                    DashButton(CrossStrings.continueTitle, isEnabled: model.canContinueRestore) {
                        restoreText = ""
                        model.continueRestore()
                    }
                }
            }
        case .restoreOptions:
            DashCard {
                DashSecureField(
                    CrossStrings.bip39Passphrase,
                    text: bind({ bip39Passphrase }, { text in
                        bip39Passphrase = text
                        model.setBIP39Passphrase(text)
                    }))
                Toggle(
                    CrossStrings.coreCompatible,
                    isOn: bind({ model.options.coreCompatible }, { model.setCoreCompatible($0) })
                )
                .toggleStyle(.switch)
                DashTextField(
                    CrossStrings.birthHeight,
                    text: bind({ birthHeightText }, { text in
                        birthHeightText = text
                        model.setBirthHeight(UInt32(text))
                    }),
                    error: model.birthDateError, width: 200)
                navigation(model) {
                    DashButton(CrossStrings.continueTitle) {
                        bip39Passphrase = ""
                        Task { await model.finishRestore() }
                    }
                }
            }
        case .unlockVault:
            DashCard {
                Text(CrossStrings.unlockVaultPrompt).dashFont(.footnote)
                DashSecureField(CrossStrings.passphrase, placeholder: CrossStrings.walletPassphrase, text: $passphrase)
                if let error = model.unlockError {
                    Text(error).dashFont(.caption1).dashForeground(.errorText)
                }
                navigation(model) {
                    DashButton(CrossStrings.unlock, isEnabled: !passphrase.isEmpty) {
                        let text = passphrase
                        passphrase = ""
                        Task { await model.unlockVault(passphrase: text) }
                    }
                }
            }
        case .working:
            CenteredMessage(text: CrossStrings.creatingWallet, busy: true)
        case .done:
            CenteredMessage(text: CrossStrings.walletReady, busy: false)
        case .failed(let failure):
            DashCard {
                Toast(failure.message, kind: .error)
                DashButton(CrossStrings.back, style: .strokeGray) { model.back() }
            }
        }
    }

    /// Back plus the step's own buttons.
    private func navigation(_ model: OnboardingViewModel, @ViewBuilder _ actions: () -> some View) -> some View {
        HStack(spacing: Int(DashSpacing.s)) {
            DashButton(CrossStrings.back, style: .strokeGray) {
                passphrase = ""
                confirmation = ""
                model.back()
            }
            actions()
        }
    }
}

struct LockScreen: View {
    let model: LockViewModel

    @State var passphrase = ""

    var body: some View {
        let model = model
        VStack(alignment: .leading, spacing: Int(DashSpacing.l)) {
            SectionHeader(L10n.Lock.title, style: .title2)
            Text(L10n.Lock.prompt).dashFont(.footnote)
            DashSecureField(CrossStrings.passphrase, placeholder: CrossStrings.walletPassphrase, text: $passphrase)
            if let message = model.message {
                Toast(message, kind: .error)
            }
            if model.isDisabled {
                Toast(L10n.Lock.disabled, kind: .error)
            }
            DashButton(
                CrossStrings.unlock, size: .large,
                isEnabled: !model.isWorking && !model.isDisabled && model.retryAfter == nil && !passphrase.isEmpty
            ) {
                let text = passphrase
                passphrase = ""
                Task { await model.unlock(passphrase: text, mixingOnly: false) }
            }
            Spacer()
        }
        .padding(Int(DashSpacing.xxxl))
        .frame(maxWidth: 520)
    }
}
