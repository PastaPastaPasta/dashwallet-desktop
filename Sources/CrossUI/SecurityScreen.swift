// Security settings (IOS-011, IOS-014…016, IOS-108, IOS-109): quick unlock
// where the OS has it (Linux has no biometric store, which the page says),
// auto-lock, "require authentication for every payment", autohide balance,
// the recovery phrase per wallet, forgot passphrase and wipe.
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct SecurityScreen: View {
    let model: SecurityViewModel
    let state: CrossAppState

    @State var passphrase = ""
    @State var revealWallet: WalletID?
    @State var revealPassphrase = ""
    @State var revealedWords: [String] = []
    @State var recoveryPhrase = ""
    @State var recoveryWallet: WalletID?
    @State var newPassphrase = ""
    @State var repeatPassphrase = ""
    @State var wipeSentence = ""
    @State var wipePassphrase = ""

    var body: some View {
        let model = model
        Page(CrossStrings.securityPage) {
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            quickUnlock(model)
            DashCard {
                DashPicker(
                    L10n.Security.autoLock,
                    options: model.autoLockOptions.map { PickerOption($0, L10n.Security.autoLockName($0)) },
                    selection: bind({ model.autoLockInterval }, { model.setAutoLock($0) }))
                DashToggle(
                    L10n.Security.requireAuthentication,
                    isOn: bind({ model.requireAuthenticationForEveryPayment }, {
                        model.setRequireAuthenticationForEveryPayment($0)
                    }))
                DashToggle(
                    L10n.Security.autohideBalance,
                    isOn: bind({ model.autohideBalance }, { model.setAutohideBalance($0) }))
            }
            recoveryPhraseCard(model)
            forgotPassphraseCard(model)
            wipeCard(model)
        }
        .task { await model.load() }
    }

    // MARK: Quick unlock (IOS-011, IOS-016)

    @ViewBuilder
    private func quickUnlock(_ model: SecurityViewModel) -> some View {
        DashCard {
            SectionHeader(CrossStrings.quickUnlock, style: .subheadMedium)
            if model.provider == .unavailable {
                Text(CrossStrings.quickUnlockUnavailable).dashFont(.footnote).dashForeground(.secondaryText)
            } else if !model.showsQuickUnlock {
                Text(CrossStrings.quickUnlockNeedsEncryption).dashFont(.footnote).dashForeground(.secondaryText)
            } else if let policy = model.policy {
                DashToggle(
                    model.providerName ?? CrossStrings.quickUnlock,
                    isOn: bind({ policy.enrolled }, { on in
                        Task { if on { await model.enableQuickUnlock() } else { await model.disableQuickUnlock() } }
                    }))
                DashPicker(
                    L10n.Security.spendingLimit,
                    options: model.spendLimitOptions.map { PickerOption($0, model.spendLimitText($0)) },
                    selection: bind({ policy.spendLimit }, { limit in Task { await model.setSpendLimit(limit) } }))
                if case .needsPassphrase = model.quickUnlockFlow {
                    DashSecureField(CrossStrings.passphrase, placeholder: CrossStrings.walletPassphrase, text: $passphrase)
                    HStack(spacing: Int(DashSpacing.s)) {
                        DashButton(CrossStrings.continueTitle, style: .filledBlue, size: .small) {
                            let text = passphrase
                            passphrase = ""
                            Task { await model.provideQuickUnlockPassphrase(text) }
                        }
                        DashButton(CrossStrings.cancel, style: .strokeGray, size: .small) {
                            passphrase = ""
                            model.cancelQuickUnlockChange()
                        }
                    }
                }
                if case .failed(let text) = model.quickUnlockFlow {
                    Toast(text, kind: .error)
                }
            }
        }
    }

    // MARK: Recovery phrase (IOS-108)

    @ViewBuilder
    private func recoveryPhraseCard(_ model: SecurityViewModel) -> some View {
        let wallets = model.wallets
        DashCard {
            SectionHeader(CrossStrings.showRecoveryPhrase, style: .subheadMedium)
            if let first = wallets.first {
                DashPicker(
                    CrossStrings.wallet, options: wallets.map { PickerOption($0.id, $0.name) },
                    selection: bind({ revealWallet ?? first.id }, { revealWallet = $0 }))
                if revealedWords.isEmpty {
                    if model.revealNeedsPassphrase || state.main.lockState == .locked || state.main.lockState == .unlocked {
                        DashSecureField(CrossStrings.passphrase, placeholder: CrossStrings.walletPassphrase, text: $revealPassphrase)
                    }
                    DashButton(CrossStrings.showRecoveryPhrase, style: .strokeGray, size: .small) {
                        let text = revealPassphrase
                        let wallet = revealWallet ?? first.id
                        revealPassphrase = ""
                        Task {
                            let revealed = await model.revealPhrase(wallet: wallet, passphrase: text.isEmpty ? nil : text)
                            if let revealed { revealedWords = SettingsScreen.words(of: revealed.phrase) }
                        }
                    }
                } else {
                    PhraseGrid(words: revealedWords)
                    DashButton(CrossStrings.hideRecoveryPhrase, style: .strokeGray, size: .small) { revealedWords = [] }
                }
            } else {
                Text(L10n.Shell.noWalletsAvailable).dashFont(.footnote).dashForeground(.secondaryText)
            }
        }
    }

    // MARK: Forgot passphrase (IOS-014)

    @ViewBuilder
    private func forgotPassphraseCard(_ model: SecurityViewModel) -> some View {
        let wallets = model.wallets
        DashCard {
            SectionHeader(L10n.Security.recoverTitle, style: .subheadMedium)
            switch model.forgotPassphrase {
            case .idle:
                DashButton(L10n.Security.forgotPassphrase, style: .plainBlue, size: .small) { model.startForgotPassphrase() }
            case .enteringPhrase:
                Text(L10n.Security.recoverPrompt).dashFont(.footnote)
                if let first = wallets.first {
                    DashPicker(
                        CrossStrings.wallet, options: wallets.map { PickerOption($0.id, $0.name) },
                        selection: bind({ recoveryWallet ?? first.id }, { recoveryWallet = $0 }))
                }
                DashSecureField(CrossStrings.recoveryPhrase, placeholder: CrossStrings.recoveryPhrasePlaceholder, text: $recoveryPhrase)
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(CrossStrings.continueTitle, style: .filledBlue, size: .small, isEnabled: !recoveryPhrase.isEmpty) {
                        let phrase = recoveryPhrase
                        recoveryPhrase = ""
                        guard let wallet = recoveryWallet ?? wallets.first?.id else { return }
                        Task { await model.submitRecoveryPhrase(phrase, wallet: wallet) }
                    }
                    DashButton(CrossStrings.cancel, style: .strokeGray, size: .small) {
                        recoveryPhrase = ""
                        model.cancelForgotPassphrase()
                    }
                }
            case .choosingPassphrase:
                DashSecureField(CrossStrings.newPassphrase, text: $newPassphrase)
                DashSecureField(CrossStrings.repeatPassphrase, text: $repeatPassphrase)
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(CrossStrings.save, style: .filledBlue, size: .small, isEnabled: !newPassphrase.isEmpty) {
                        let (new, repeated) = (newPassphrase, repeatPassphrase)
                        newPassphrase = ""
                        repeatPassphrase = ""
                        Task { await model.chooseNewPassphrase(new, confirmation: repeated) }
                    }
                    DashButton(CrossStrings.cancel, style: .strokeGray, size: .small) { model.cancelForgotPassphrase() }
                }
            case .recovering:
                Text(CrossStrings.working).dashFont(.footnote).dashForeground(.secondaryText)
            case .done:
                Toast(L10n.Security.passphraseReset, kind: .success)
                if let text = model.walletsWithoutSecretsText {
                    Text(text).dashFont(.footnote)
                }
                DashButton(CrossStrings.done, style: .tintedBlue, size: .small) { model.cancelForgotPassphrase() }
            case .failed(let text):
                Toast(text, kind: .error, actionTitle: CrossStrings.dismiss) { model.cancelForgotPassphrase() }
            }
        }
    }

    // MARK: Wipe (IOS-109)

    @ViewBuilder
    private func wipeCard(_ model: SecurityViewModel) -> some View {
        DashCard {
            SectionHeader(L10n.Security.wipeTitle, style: .subheadMedium)
            switch model.wipeStep {
            case .idle:
                DashButton(L10n.Security.wipeTitle, style: .plainRed, size: .small) { model.requestWipe() }
            case .confirming:
                Text(L10n.Wallets.deleteAllMessage).dashFont(.footnote)
                Text(L10n.Wallets.wipeAcceptPhrase).dashFont(.footnoteMedium).textSelectionEnabled()
                DashTextField(CrossStrings.typeSentence, text: $wipeSentence)
                if model.vaultStatus?.encrypted ?? false {
                    DashSecureField(CrossStrings.passphrase, placeholder: CrossStrings.walletPassphrase, text: $wipePassphrase)
                }
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(L10n.Security.wipeTitle, style: .filledRed, size: .small, isEnabled: !wipeSentence.isEmpty) {
                        let (sentence, secret) = (wipeSentence, wipePassphrase)
                        wipePassphrase = ""
                        Task { await model.wipe(confirmation: sentence, passphrase: secret.isEmpty ? nil : secret) }
                    }
                    DashButton(CrossStrings.cancel, style: .strokeGray, size: .small) {
                        wipeSentence = ""
                        wipePassphrase = ""
                        model.cancelWipe()
                    }
                }
            case .wiping:
                Text(CrossStrings.working).dashFont(.footnote).dashForeground(.secondaryText)
            case .done:
                Toast(L10n.Security.wiped, kind: .success)
            case .failed(let text):
                Toast(text, kind: .error)
            }
        }
    }
}
