// Create and restore flows (QT-102…105, QT-111, IOS-002…007, IOS-010),
// one screen per `OnboardingStep`.
#if os(macOS)
import DashUIMac
import DesignTokens
import SwiftUI
import WalletFeatures
import WalletRuntime

struct OnboardingView: View {
    @Bindable var model: OnboardingViewModel

    var body: some View {
        ScrollView {
            VStack(spacing: DashSpacing.xl) {
                content
            }
            .frame(maxWidth: 560)
            .padding(DashSpacing.xxxl)
            .frame(maxWidth: .infinity)
        }
        .dashCanvas()
        .accessibilityIdentifier("onboarding")
    }

    @ViewBuilder
    private var content: some View {
        switch model.step {
        case .welcome: WelcomeStep(model: model)
        case .showPhrase: ShowPhraseStep(model: model)
        case .verifyPhrase: VerifyPhraseStep(model: model)
        case .choosePassphrase: ChoosePassphraseStep(model: model)
        case .unlockVault: UnlockVaultStep(model: model)
        case .restorePhrase: RestorePhraseStep(model: model)
        case .restoreOptions: RestoreOptionsStep(model: model)
        case .working, .done:
            VStack(spacing: DashSpacing.m) {
                ProgressView()
                Text(MacStrings.Onboarding.working).dashFont(.subhead)
            }
            .padding(.top, 120)
            .accessibilityIdentifier("onboarding.working")
        case .failed(let failure):
            VStack(spacing: DashSpacing.m) {
                ErrorIllustration()
                    .accessibilityHidden(true)
                Text(MacStrings.Onboarding.failedTitle).dashFont(.title3)
                Text(failure.message)
                    .dashFont(.subhead)
                    .multilineTextAlignment(.center)
                    .accessibilityIdentifier("onboarding.failure")
                Button(MacStrings.Common.back) { model.back() }
                    .buttonStyle(.dash(.tintedGray, .medium))
            }
            .padding(.top, 80)
        }
    }
}

/// Title and body text of a step.
private struct StepHeader: View {
    let title: String
    var message: String?

    var body: some View {
        VStack(spacing: DashSpacing.s) {
            Text(title)
                .dashFont(.title2)
                .foregroundStyle(Color.role.textPrimary)
                .multilineTextAlignment(.center)
            if let message {
                Text(message)
                    .dashFont(.subhead)
                    .foregroundStyle(Color.role.textSecondary)
                    .multilineTextAlignment(.center)
            }
        }
    }
}

private struct BackButton: View {
    let model: OnboardingViewModel

    var body: some View {
        HStack {
            Button {
                model.back()
            } label: {
                Label(MacStrings.Common.back, systemImage: "chevron.left")
            }
            .buttonStyle(.dash(.plainBlue, .small))
            .accessibilityIdentifier("onboarding.back")
            Spacer()
        }
    }
}

// MARK: Welcome

private struct WelcomeStep: View {
    let model: OnboardingViewModel
    @State private var showsAdvanced = false

    /// iOS welcome: the wordmark, two full-width buttons; the network and
    /// phrase length move under "Advanced options" (UX-SPEC §4.3).
    var body: some View {
        VStack(spacing: DashSpacing.xxl) {
            DashIconImage(.token(.dashLogo))
                .scaledToFit()
                .frame(height: 40)
                .padding(.top, DashSpacing.xxxl)
                .accessibilityLabel(L10n.Navigation.appName)
            VStack(spacing: DashSpacing.s) {
                Text(L10n.Onboarding.welcomeTitle)
                    .dashFont(.title1)
                    .foregroundStyle(Color.role.textPrimary)
                    .multilineTextAlignment(.center)
                Text(MacStrings.Onboarding.subtitle)
                    .dashFont(.subhead)
                    .foregroundStyle(Color.role.textSecondary)
                    .multilineTextAlignment(.center)
            }
            VStack(spacing: DashSpacing.m) {
                Button(L10n.Onboarding.createWallet) { Task { await model.startCreate() } }
                    .buttonStyle(.dash(.filledBlue, .large, fillsWidth: true))
                    .keyboardShortcut(.defaultAction)
                    .accessibilityIdentifier("onboarding.create")
                Button(L10n.Onboarding.restoreWallet) { model.startRestore() }
                    .buttonStyle(.dash(.tintedGray, .large, fillsWidth: true))
                    .accessibilityIdentifier("onboarding.restore")
                Button {
                    withAnimation(.easeInOut(duration: DashMotion.overlay)) { showsAdvanced.toggle() }
                } label: {
                    Label(MacStrings.Onboarding.advancedOptions, systemImage: showsAdvanced ? "chevron.down" : "chevron.right")
                }
                .buttonStyle(.dash(.plainBlue, .small))
                .accessibilityIdentifier("onboarding.advanced")
            }
            .frame(width: 360)
            if showsAdvanced {
                OptionsCard {
                    MenuRow(icon: .token(.connections), title: MacStrings.Onboarding.network) {
                        Picker(MacStrings.Onboarding.network, selection: Binding(
                            get: { model.network ?? .mainnet },
                            set: { network in Task { await model.chooseNetwork(network) } }
                        )) {
                            ForEach(model.availableNetworks, id: \.self) { network in
                                Text(L10n.Settings.networkName(network)).tag(network)
                            }
                        }
                        .labelsHidden()
                        .fixedSize()
                        .accessibilityIdentifier("onboarding.network")
                    }
                    MenuRow(icon: .token(.recoveryPhrase), title: MacStrings.Onboarding.wordCount) {
                        DashSegmentedControl(
                            MnemonicWords.createCounts.map { ($0, MacStrings.Onboarding.words($0)) },
                            selection: Binding(get: { model.wordCount }, set: { model.setWordCount($0) }))
                    }
                }
                .frame(width: 460)
            }
        }
    }
}

// MARK: Create

private struct ShowPhraseStep: View {
    let model: OnboardingViewModel

    var body: some View {
        VStack(spacing: DashSpacing.xl) {
            BackButton(model: model)
            StepHeader(title: MacStrings.Onboarding.phraseTitle, message: L10n.Onboarding.showPhraseWarning)
            if model.captureWarning {
                SystemNotice(text: L10n.Onboarding.screenCaptureWarning, tone: .error)
            } else if model.captureProtected {
                SystemNotice(text: MacStrings.Onboarding.captureProtected, tone: .info)
            }
            PhraseGrid(words: model.phraseWords, wordIdentifier: { "onboarding.word.\($0)" })
            Button(MacStrings.Onboarding.writtenDown) { model.confirmWrittenDown() }
                .buttonStyle(.dash(.filledBlue, .large, fillsWidth: true))
                .frame(width: 360)
                .accessibilityIdentifier("onboarding.writtenDown")
        }
    }
}

private struct VerifyPhraseStep: View {
    let model: OnboardingViewModel

    var body: some View {
        VStack(spacing: DashSpacing.xl) {
            BackButton(model: model)
            StepHeader(title: MacStrings.Onboarding.verifyTitle, message: L10n.Onboarding.verifyPrompt)
            HStack(spacing: DashSpacing.m) {
                ForEach(Array(model.verifyChallenge.enumerated()), id: \.offset) { offset, position in
                    let done = offset < model.verifiedCount
                    let current = offset == model.verifiedCount
                    Text(MacStrings.Onboarding.wordNumber(position))
                        .dashFont(.footnoteMedium)
                        .foregroundStyle(done ? Color.role.success : Color.role.textPrimary)
                        .padding(.vertical, DashSpacing.s)
                        .padding(.horizontal, DashSpacing.m)
                        .background(
                            RoundedRectangle(cornerRadius: DashRadius.standard, style: .continuous)
                                .fill(done ? Color.role.successTint : Color.role.card))
                        .overlay(
                            RoundedRectangle(cornerRadius: DashRadius.standard, style: .continuous)
                                .strokeBorder(current ? Color.role.accent : Color.role.separator, lineWidth: current ? 2 : 0.5))
                        .accessibilityIdentifier(current ? "onboarding.challenge.current" : "onboarding.challenge.\(offset)")
                        .accessibilityValue(Text("\(position)"))
                }
            }
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: DashSpacing.m), count: 3),
                      spacing: DashSpacing.m) {
                ForEach(model.verifyChips) { chip in
                    // iOS verify chips: blue to click in order, grey once used.
                    Button(chip.word) {
                        Task { await model.select(chip: chip.id) }
                    }
                    .buttonStyle(.dash(.filledBlue, .medium, fillsWidth: true))
                    .disabled(chip.used)
                    .accessibilityIdentifier("onboarding.chip.\(chip.word)")
                }
            }
            .frame(width: 420)
            if model.verifyMistake {
                Text(L10n.Onboarding.verifyWrongWord)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.danger)
            } else if model.isVerified {
                Text(L10n.Onboarding.verifiedSuccessfully)
                    .dashFont(.footnoteMedium)
                    .foregroundStyle(Color.role.success)
            }
        }
    }
}

private struct ChoosePassphraseStep: View {
    let model: OnboardingViewModel
    @State private var passphrase = ""
    @State private var confirmation = ""

    var body: some View {
        VStack(spacing: DashSpacing.xl) {
            BackButton(model: model)
            StepHeader(title: MacStrings.Onboarding.passphraseTitle, message: MacStrings.Onboarding.passphraseBody)
            VStack(alignment: .leading, spacing: DashSpacing.l) {
                PassphraseField(
                    label: MacStrings.Common.newPassphrase, text: $passphrase,
                    strength: passphrase.isEmpty ? nil : Self.meter(model.passphraseStrength),
                    strengthText: model.passphraseStrength.title)
                .accessibilityIdentifier("onboarding.passphrase")
                PassphraseField(
                    label: MacStrings.Common.repeatPassphrase, text: $confirmation,
                    errorText: model.passphraseError)
                .accessibilityIdentifier("onboarding.passphraseConfirm")
                SystemNotice(text: L10n.Onboarding.encryptWarning, tone: .warning)
            }
            .frame(width: 420)
            .onChange(of: passphrase) { _, text in model.updatePassphraseDraft(text) }
            VStack(spacing: DashSpacing.m) {
                Button(MacStrings.Onboarding.encrypt, action: finishEncrypted)
                    .buttonStyle(.dash(.filledBlue, .large, fillsWidth: true))
                    .disabled(passphrase.isEmpty)
                    .accessibilityIdentifier("onboarding.encrypt")
                Button(MacStrings.Onboarding.skipEncryption, action: finishUnencrypted)
                    .buttonStyle(.dash(.plainBlue, .medium))
                    .help(MacStrings.Onboarding.skipHelp)
                    .accessibilityIdentifier("onboarding.skipEncryption")
            }
            .frame(width: 360)
        }
    }

    private func finishEncrypted() {
        guard model.setPassphrase(passphrase, confirmation: confirmation) else { return }
        passphrase = ""
        confirmation = ""
        Task { await model.finish() }
    }

    private func finishUnencrypted() {
        model.setPassphrase(nil, confirmation: nil)
        Task { await model.finish() }
    }

    /// The view model's four-step scale on the meter's five-step scale.
    static func meter(_ strength: WalletFeatures.PassphraseStrength) -> DashUIMac.PassphraseStrength? {
        switch strength {
        case .none: nil
        case .weak: .weak
        case .fair: .fair
        case .good: .good
        case .strong: .strong
        }
    }
}

/// Adding a wallet to an encrypted vault that is locked: the vault's
/// passphrase unlocks it, then the wallet is added.
private struct UnlockVaultStep: View {
    let model: OnboardingViewModel
    @State private var passphrase = ""

    var body: some View {
        VStack(spacing: DashSpacing.xl) {
            BackButton(model: model)
            StepHeader(title: L10n.Lock.title, message: L10n.Onboarding.vaultLocked)
            PassphraseField(
                label: MacStrings.Common.passphrase, text: $passphrase, errorText: model.unlockError,
                isRevealable: false)
            .frame(width: 420)
            .accessibilityIdentifier("onboarding.unlockPassphrase")
            .onSubmit(unlock)
            Button(MacStrings.Lock.unlock, action: unlock)
                .buttonStyle(.dash(.filledBlue, .large, fillsWidth: true))
                .disabled(passphrase.isEmpty)
                .frame(width: 360)
                .accessibilityIdentifier("onboarding.unlock")
        }
    }

    private func unlock() {
        let text = passphrase
        passphrase = ""
        Task { await model.unlockVault(passphrase: text) }
    }
}

// MARK: Restore

private struct RestorePhraseStep: View {
    let model: OnboardingViewModel
    @State private var text = ""

    var body: some View {
        VStack(spacing: DashSpacing.xl) {
            BackButton(model: model)
            StepHeader(title: MacStrings.Onboarding.restoreTitle, message: MacStrings.Onboarding.restoreBody)
            TextEditor(text: $text)
                .font(DesignTokens.DashTextStyle.callout.font)
                .autocorrectionDisabled()
                .frame(width: 460, height: 120)
                .scrollContentBackground(.hidden)
                .padding(DashSpacing.m)
                .background(RoundedRectangle(cornerRadius: DashRadius.textField, style: .continuous).fill(Color.role.fieldFill))
                .accessibilityIdentifier("onboarding.restoreText")
                .onChange(of: text) { _, value in Task { await model.updateRestoreText(value) } }
            if !model.wordSuggestions.isEmpty {
                HStack(spacing: DashSpacing.s) {
                    Text(MacStrings.Onboarding.suggestions)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.role.textSecondary)
                    ForEach(model.wordSuggestions, id: \.self) { word in
                        Button(word) { complete(with: word) }
                            .buttonStyle(.dash(.tintedBlue, .small))
                    }
                }
            }
            if let problem = model.restoreProblem {
                Text(problem)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.danger)
                    .accessibilityIdentifier("onboarding.restoreProblem")
            }
            if model.coreOnlyWarning {
                SystemNotice(text: L10n.Onboarding.coreOnlyChecksumWarning, tone: .warning)
            }
            Button(MacStrings.Common.continue) { model.continueRestore() }
                .buttonStyle(.dash(.filledBlue, .large, fillsWidth: true))
                .disabled(!model.canContinueRestore)
                .frame(width: 360)
                .accessibilityIdentifier("onboarding.restoreContinue")
        }
    }

    /// Replaces the word being typed with a suggestion.
    private func complete(with word: String) {
        var words = text.split(separator: " ", omittingEmptySubsequences: true).map(String.init)
        if !words.isEmpty { words.removeLast() }
        words.append(word)
        text = words.joined(separator: " ") + " "
    }
}

private struct RestoreOptionsStep: View {
    @Bindable var model: OnboardingViewModel
    @State private var bip39 = ""
    @State private var knowsDate = false
    @State private var date = Date()

    var body: some View {
        VStack(spacing: DashSpacing.xl) {
            BackButton(model: model)
            StepHeader(title: MacStrings.Onboarding.optionsTitle)
            OptionsCard {
                SecureField(MacStrings.Onboarding.bip39Passphrase, text: $bip39)
                    .modifier(DashFieldModifier())
                    .onChange(of: bip39) { _, value in model.setBIP39Passphrase(value) }
                Toggle(MacStrings.Onboarding.coreCompatible, isOn: Binding(
                    get: { model.options.coreCompatible }, set: { model.setCoreCompatible($0) }))
                .help(MacStrings.Onboarding.coreCompatibleHelp)
                .disabled(model.coreOnlyWarning)
                Toggle(MacStrings.Onboarding.knowDate, isOn: $knowsDate)
                    .onChange(of: knowsDate) { _, on in model.setBirthDate(on ? date : nil) }
                if knowsDate {
                    DatePicker(MacStrings.Onboarding.createdOn, selection: $date, in: ...Date(), displayedComponents: .date)
                        .onChange(of: date) { _, value in model.setBirthDate(value) }
                }
                if let error = model.birthDateError {
                    Text(error).foregroundStyle(Color.role.danger)
                }
                Text(model.options.birthHeight.map { $0 == 0 ? MacStrings.Onboarding.scanFromGenesis : MacStrings.Onboarding.birthHeight($0) }
                     ?? MacStrings.Onboarding.scanFromGenesis)
                    .foregroundStyle(Color.role.textSecondary)
            }
            .frame(width: 460)
            Button(MacStrings.Onboarding.restore) { Task { await model.finishRestore() } }
                .buttonStyle(.dash(.filledBlue, .large, fillsWidth: true))
                .frame(width: 360)
                .accessibilityIdentifier("onboarding.restoreFinish")
        }
    }
}

/// Settings rows on a white card.
private struct OptionsCard<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .dashCard(padding: DashSpacing.l)
    }
}

/// A persistent condition on a page (UX-SPEC C22): DashUIKit's system message
/// with the tone's icon and tint.
struct SystemNotice: View {
    let text: String
    let tone: DashTone

    var body: some View {
        SystemMessageView(
            title: text,
            icon: .token(tone == .error || tone == .warning ? .messageWarning : .messageInfo),
            backgroundColor: tone.background)
    }
}
#endif
