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
        .background(Color.dash.primaryBackground)
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
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 40))
                    .foregroundStyle(Color.dash.orange)
                Text(MacStrings.Onboarding.failedTitle).dashFont(.title3)
                Text(failure.message)
                    .dashFont(.subhead)
                    .multilineTextAlignment(.center)
                    .accessibilityIdentifier("onboarding.failure")
                DashButton(text: MacStrings.Common.back, size: .medium, style: .strokeGray, action: { model.back() })
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
                .foregroundStyle(Color.dash.primaryText)
                .multilineTextAlignment(.center)
            if let message {
                Text(message)
                    .dashFont(.subhead)
                    .foregroundStyle(Color.dash.secondaryText)
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
            .buttonStyle(.link)
            .accessibilityIdentifier("onboarding.back")
            Spacer()
        }
    }
}

// MARK: Welcome

private struct WelcomeStep: View {
    let model: OnboardingViewModel

    var body: some View {
        VStack(spacing: DashSpacing.xxl) {
            Image(systemName: "d.circle.fill")
                .font(.system(size: 72))
                .foregroundStyle(Color.dash.blue)
                .padding(.top, DashSpacing.xxxl)
                .accessibilityHidden(true)
            StepHeader(title: L10n.Onboarding.welcomeTitle, message: MacStrings.Onboarding.subtitle)
            // A plain card, not a grouped Form: a Form is a scroll view and
            // does not size inside the onboarding ScrollView.
            OptionsCard {
                Picker(MacStrings.Onboarding.network, selection: Binding(
                    get: { model.network ?? .mainnet },
                    set: { network in Task { await model.chooseNetwork(network) } }
                )) {
                    ForEach(model.availableNetworks, id: \.self) { network in
                        Text(L10n.Settings.networkName(network)).tag(network)
                    }
                }
                .accessibilityIdentifier("onboarding.network")
                Picker(MacStrings.Onboarding.wordCount, selection: Binding(
                    get: { model.wordCount }, set: { model.setWordCount($0) }
                )) {
                    ForEach(MnemonicWords.createCounts, id: \.self) { count in
                        Text(MacStrings.Onboarding.words(count)).tag(count)
                    }
                }
                .pickerStyle(.segmented)
            }
            .frame(width: 420)
            VStack(spacing: DashSpacing.m) {
                DashButton(
                    text: L10n.Onboarding.createWallet, fillsWidth: true, size: .large, style: .filledBlue,
                    action: { Task { await model.startCreate() } })
                .accessibilityIdentifier("onboarding.create")
                DashButton(
                    text: L10n.Onboarding.restoreWallet, fillsWidth: true, size: .large, style: .strokeGray,
                    action: { model.startRestore() })
                .accessibilityIdentifier("onboarding.restore")
            }
            .frame(width: 320)
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
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: DashSpacing.m), count: 3),
                      spacing: DashSpacing.m) {
                ForEach(Array(model.phraseWords.enumerated()), id: \.offset) { index, word in
                    HStack(spacing: DashSpacing.s) {
                        Text("\(index + 1)")
                            .dashFont(.footnote)
                            .foregroundStyle(Color.dash.secondaryText)
                            .frame(width: 22, alignment: .trailing)
                        Text(word)
                            .dashFont(.calloutMedium)
                            .foregroundStyle(Color.dash.primaryText)
                            .accessibilityIdentifier("onboarding.word.\(index)")
                        Spacer(minLength: 0)
                    }
                    .padding(.vertical, DashSpacing.s)
                    .padding(.horizontal, DashSpacing.m)
                    .background(RoundedRectangle(cornerRadius: DashRadius.standard).fill(Color.dash.secondaryBackground))
                }
            }
            DashButton(
                text: MacStrings.Onboarding.writtenDown, fillsWidth: true, size: .large, style: .filledBlue,
                action: { model.confirmWrittenDown() })
            .frame(width: 320)
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
                        .foregroundStyle(done ? Color.dash.successText : Color.dash.primaryText)
                        .padding(.vertical, DashSpacing.s)
                        .padding(.horizontal, DashSpacing.m)
                        .background(
                            RoundedRectangle(cornerRadius: DashRadius.small)
                                .stroke(current ? Color.dash.blue : Color.dash.gray300Alpha40, lineWidth: current ? 2 : 1))
                        .accessibilityIdentifier(current ? "onboarding.challenge.current" : "onboarding.challenge.\(offset)")
                        .accessibilityValue(Text("\(position)"))
                }
            }
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: DashSpacing.m), count: 3),
                      spacing: DashSpacing.m) {
                ForEach(model.verifyChips) { chip in
                    Button(chip.word) {
                        Task { await model.select(chip: chip.id) }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.large)
                    .disabled(chip.used)
                    .accessibilityIdentifier("onboarding.chip.\(chip.word)")
                }
            }
            .frame(width: 420)
            if model.verifyMistake {
                Text(L10n.Onboarding.verifyWrongWord)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.errorText)
            } else if model.isVerified {
                Text(L10n.Onboarding.verifiedSuccessfully)
                    .dashFont(.footnoteMedium)
                    .foregroundStyle(Color.dash.successText)
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
                DashButton(
                    text: MacStrings.Onboarding.encrypt, isEnabled: !passphrase.isEmpty, fillsWidth: true,
                    size: .large, style: .filledBlue, action: finishEncrypted)
                .accessibilityIdentifier("onboarding.encrypt")
                DashButton(
                    text: MacStrings.Onboarding.skipEncryption, fillsWidth: true, size: .medium, style: .plainBlue,
                    action: finishUnencrypted)
                .help(MacStrings.Onboarding.skipHelp)
                .accessibilityIdentifier("onboarding.skipEncryption")
            }
            .frame(width: 320)
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
            DashButton(
                text: MacStrings.Lock.unlock, isEnabled: !passphrase.isEmpty, fillsWidth: true, size: .large,
                style: .filledBlue, action: unlock)
            .frame(width: 320)
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
                .font(.system(.body, design: .monospaced))
                .autocorrectionDisabled()
                .frame(width: 460, height: 120)
                .scrollContentBackground(.hidden)
                .padding(DashSpacing.s)
                .background(RoundedRectangle(cornerRadius: DashRadius.textField).fill(Color.dash.gray300Alpha10))
                .accessibilityIdentifier("onboarding.restoreText")
                .onChange(of: text) { _, value in Task { await model.updateRestoreText(value) } }
            if !model.wordSuggestions.isEmpty {
                HStack(spacing: DashSpacing.s) {
                    Text(MacStrings.Onboarding.suggestions)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.dash.secondaryText)
                    ForEach(model.wordSuggestions, id: \.self) { word in
                        Button(word) { complete(with: word) }
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                    }
                }
            }
            if let problem = model.restoreProblem {
                Text(problem)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.errorText)
                    .accessibilityIdentifier("onboarding.restoreProblem")
            }
            if model.coreOnlyWarning {
                SystemNotice(text: L10n.Onboarding.coreOnlyChecksumWarning, tone: .warning)
            }
            DashButton(
                text: MacStrings.Common.continue, isEnabled: model.canContinueRestore, fillsWidth: true,
                size: .large, style: .filledBlue, action: { model.continueRestore() })
            .frame(width: 320)
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
                    .textFieldStyle(.roundedBorder)
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
                    Text(error).foregroundStyle(Color.dash.errorText)
                }
                Text(model.options.birthHeight.map { $0 == 0 ? MacStrings.Onboarding.scanFromGenesis : MacStrings.Onboarding.birthHeight($0) }
                     ?? MacStrings.Onboarding.scanFromGenesis)
                    .foregroundStyle(Color.dash.secondaryText)
            }
            .frame(width: 460)
            DashButton(
                text: MacStrings.Onboarding.restore, fillsWidth: true, size: .large, style: .filledBlue,
                action: { Task { await model.finishRestore() } })
            .frame(width: 320)
            .accessibilityIdentifier("onboarding.restoreFinish")
        }
    }
}

/// Settings rows on a rounded card.
private struct OptionsCard<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            content
        }
        .padding(DashSpacing.l)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: DashRadius.standard).fill(Color.dash.secondaryBackground))
    }
}

/// A tinted notice box (DashUIKit `SystemMessageView` look).
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
