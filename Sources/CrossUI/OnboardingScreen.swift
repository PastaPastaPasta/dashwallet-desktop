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
    @State var showsAdvanced = false

    var body: some View {
        let model = model
        // UX-SPEC §4.3: canvas, a centred 560 pt column under the wordmark.
        ScrollView {
            VStack(alignment: .leading, spacing: Int(DashSpacing.l)) {
                HStack {
                    Spacer()
                    DashIcon(.dashLogo, size: 32)
                    Spacer()
                }
                .padding(.bottom, Int(DashSpacing.s))
                step(model)
            }
            .frame(maxWidth: 560, alignment: .topLeading)
            .padding(.horizontal, CrossLayout.pagePaddingH)
            .padding(.top, 60)
            .padding(.bottom, CrossLayout.sectionGap)
            .frame(maxWidth: .infinity, alignment: .top)
        }
        .background(CrossRole.canvas.color)
    }

    @ViewBuilder
    private func step(_ model: OnboardingViewModel) -> some View {
        switch model.step {
        case .welcome:
            VStack(spacing: Int(DashSpacing.s)) {
                Text(L10n.Onboarding.welcomeTitle).dashFont(.title1).dashForeground(CrossRole.textPrimary)
                Text(CrossStrings.welcomeSubtitle).dashFont(.subhead).dashForeground(CrossRole.textSecondary)
            }
            .frame(maxWidth: .infinity)
            DashButton(L10n.Onboarding.createWallet, size: .large, fillsWidth: true) { Task { await model.startCreate() } }
            DashButton(L10n.Onboarding.restoreWallet, style: .tintedGray, size: .large, fillsWidth: true) {
                restoreText = ""
                model.startRestore()
            }
            // The network and phrase length are demoted behind "Advanced options".
            DashButton(
                showsAdvanced ? CrossStrings.hideAdvancedOptions : CrossStrings.advancedOptions, style: .plainBlue,
                size: .small
            ) { showsAdvanced.toggle() }
            if showsAdvanced {
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
            }
            }
        case .showPhrase:
            TopIntro(CrossStrings.recoveryPhrase)
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
                    SectionHeader(CrossStrings.selectWord(model.verifyChallenge[model.verifiedCount] + 1), style: .headline)
                }
                HStack(spacing: Int(DashSpacing.s)) {
                    ForEach(model.verifyChips) { chip in
                        DashButton(chip.word, style: .filledBlue, size: .small, isEnabled: !chip.used) {
                            Task { await model.select(chip: chip.id) }
                        }
                    }
                }
                if model.verifyMistake {
                    Text(L10n.Onboarding.verifyWrongWord).dashFont(.caption1).dashForeground(CrossRole.danger)
                }
                if model.isVerified {
                    Text(L10n.Onboarding.verifiedSuccessfully).dashFont(.caption1).dashForeground(CrossRole.success)
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
                        .dashFont(.caption1).dashForeground(CrossRole.textSecondary)
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
                DashToggle(
                    CrossStrings.coreCompatible,
                    isOn: bind({ model.options.coreCompatible }, { model.setCoreCompatible($0) }))
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
                    Text(error).dashFont(.caption1).dashForeground(CrossRole.danger)
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
            DashCard(padding: Int(DashSpacing.xxl)) {
                VStack(spacing: Int(DashSpacing.m)) {
                    DashIcon(.toastSuccess, size: 60, width: 60)
                    Text(CrossStrings.walletReady).dashFont(.title2).dashForeground(CrossRole.textPrimary)
                }
                .frame(maxWidth: .infinity)
            }
        case .failed(let failure):
            DashCard {
                Toast(failure.message, kind: .error)
                DashButton(CrossStrings.back, style: .tintedGray) { model.back() }
            }
        }
    }

    /// Back (plain blue, iOS navigation style) plus the step's own buttons.
    private func navigation(_ model: OnboardingViewModel, @ViewBuilder _ actions: () -> some View) -> some View {
        HStack(spacing: Int(DashSpacing.s)) {
            DashButton(CrossStrings.back, style: .plainBlue) {
                passphrase = ""
                confirmation = ""
                model.back()
            }
            Spacer()
            actions()
        }
    }
}

/// The lock screen (UX-SPEC §4.4): the whole window on the hero blue with
/// the white wordmark, the network capsule off mainnet, dash-qt's passphrase
/// text in white and a white Unlock button.
struct LockScreen: View {
    let model: LockViewModel
    let network: DashNetwork?

    @State var passphrase = ""

    var body: some View {
        let model = model
        VStack(spacing: Int(DashSpacing.l)) {
            Spacer()
            DashIcon(.dashLogo, size: 36, tint: .color(CrossRole.white))
            if let network, network != .mainnet {
                NetworkCapsule(L10n.Settings.networkName(network))
            }
            Text(L10n.Lock.title).dashFont(.title2).foregroundColor(CrossRole.textOnHero.color)
            Text(L10n.Lock.prompt)
                .dashFont(.subhead)
                .foregroundColor(CrossRole.white.opacity(0.8).color)
            VStack(alignment: .leading, spacing: Int(DashSpacing.s)) {
                SecureField(CrossStrings.walletPassphrase, text: $passphrase)
                    .accessibilityLabel(CrossStrings.passphrase)
                    .frame(width: 360)
                    .onSubmit { unlock(model) }
                if let message = model.message {
                    Text(message).dashFont(.footnoteMedium).foregroundColor(CrossRole.white.color)
                }
                if model.isDisabled {
                    Text(L10n.Lock.disabled).dashFont(.footnoteMedium).foregroundColor(CrossRole.white.color)
                }
            }
            DashButton(
                CrossStrings.unlock, style: .filledWhite, size: .large,
                isEnabled: !model.isWorking && !model.isDisabled && model.retryAfter == nil && !passphrase.isEmpty
            ) {
                unlock(model)
            }
            Spacer()
        }
        .padding(Int(DashSpacing.xxxl))
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(CrossRole.hero.color)
    }

    private func unlock(_ model: LockViewModel) {
        guard !passphrase.isEmpty, !model.isWorking, !model.isDisabled, model.retryAfter == nil else { return }
        let text = passphrase
        passphrase = ""
        Task { await model.unlock(passphrase: text, mixingOnly: false) }
    }
}
