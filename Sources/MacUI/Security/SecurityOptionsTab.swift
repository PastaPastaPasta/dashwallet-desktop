// Options ▸ Security (IOS-011, IOS-014…016, IOS-108/109): Touch ID quick
// unlock with its spending limit, auto-lock, "require authentication for
// every payment", autohide balance, forgot passphrase and wipe. Rust enforces
// the quick-unlock limit; the controls only change the policy.
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServices
import SwiftUI
import WalletFeatures
import WalletRuntime

struct SecurityOptionsTab: View {
    let security: SecurityViewModel
    let settings: SettingsViewModel
    var screenCapture: (any ScreenCaptureGuard)?
    @State private var passphrase = ""
    @State private var showsForgot = false
    @State private var showsWipe = false

    var body: some View {
        Form {
            VaultSettingsSection(settings: settings, screenCapture: screenCapture)
            Section {
                if security.showsQuickUnlock, let name = security.providerName {
                    Toggle(name, isOn: Binding(
                        get: { security.policy?.enrolled ?? false },
                        set: { on in Task { on ? await security.enableQuickUnlock() : await security.disableQuickUnlock() } }
                    ))
                    .disabled(security.quickUnlockFlow == .working)
                    .accessibilityIdentifier("security.quickUnlock")
                    Picker(L10n.Security.spendingLimit, selection: Binding(
                        get: { security.policy?.spendLimit ?? QuickUnlockPolicy.defaultSpendLimit },
                        set: { limit in Task { await security.setSpendLimit(limit) } }
                    )) {
                        ForEach(security.spendLimitOptions, id: \.self) { limit in
                            Text(security.spendLimitText(limit)).tag(limit)
                        }
                    }
                    .disabled(!(security.policy?.enrolled ?? false))
                    .accessibilityIdentifier("security.spendLimit")
                } else {
                    LabeledContent(L10n.Security.touchID, value: MacStrings.Security.quickUnlockUnavailable)
                        .accessibilityIdentifier("security.quickUnlockUnavailable")
                }
                quickUnlockFlow
            }
            Section {
                Picker(L10n.Security.autoLock, selection: Binding(
                    get: { security.autoLockInterval }, set: { security.setAutoLock($0) }
                )) {
                    ForEach(security.autoLockOptions, id: \.self) { interval in
                        Text(L10n.Security.autoLockName(interval)).tag(interval)
                    }
                }
                .accessibilityIdentifier("security.autoLock")
                Toggle(L10n.Security.requireAuthentication, isOn: Binding(
                    get: { security.requireAuthenticationForEveryPayment },
                    set: { security.setRequireAuthenticationForEveryPayment($0) }))
                Toggle(L10n.Security.autohideBalance, isOn: Binding(
                    get: { security.autohideBalance }, set: { security.setAutohideBalance($0) }))
            }
            Section {
                HStack {
                    Button(L10n.Security.forgotPassphrase) {
                        security.startForgotPassphrase()
                        showsForgot = true
                    }
                    .disabled(!(security.vaultStatus?.encrypted ?? false))
                    Spacer()
                    Button(L10n.Security.wipeTitle, role: .destructive) {
                        security.requestWipe()
                        showsWipe = true
                    }
                    .accessibilityIdentifier("security.wipe")
                }
            }
            if let error = security.errorMessage {
                Text(error).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
            }
        }
        .formStyle(.grouped)
        .task { await security.load() }
        .sheet(isPresented: $showsForgot) {
            ForgotPassphraseSheet(security: security, onClose: {
                security.cancelForgotPassphrase()
                showsForgot = false
            })
        }
        .sheet(isPresented: $showsWipe) {
            WipeSheet(security: security, onClose: {
                security.cancelWipe()
                showsWipe = false
            })
        }
    }

    @ViewBuilder
    private var quickUnlockFlow: some View {
        switch security.quickUnlockFlow {
        case .needsPassphrase:
            HStack {
                SecureField(MacStrings.Common.passphrase, text: $passphrase)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("security.passphrase")
                Button(MacStrings.Common.cancel) {
                    passphrase = ""
                    security.cancelQuickUnlockChange()
                }
                Button(MacStrings.Common.ok) {
                    let text = passphrase
                    passphrase = ""
                    Task { await security.provideQuickUnlockPassphrase(text) }
                }
                .disabled(passphrase.isEmpty)
            }
        case .working:
            ProgressView().controlSize(.small)
        case .failed(let reason):
            Text(reason).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
        case .idle:
            EmptyView()
        }
    }
}

/// IOS-014: the recovery phrase of one wallet, then a new passphrase.
private struct ForgotPassphraseSheet: View {
    let security: SecurityViewModel
    let onClose: () -> Void
    @State private var phrase = ""
    @State private var wallet: WalletID?
    @State private var passphrase = ""
    @State private var confirmation = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(L10n.Security.recoverTitle).dashFont(.title3)
            switch security.forgotPassphrase {
            case .idle, .enteringPhrase:
                Text(L10n.Security.recoverPrompt).dashFont(.subhead)
                Picker(MacStrings.Toolbar.wallet, selection: $wallet) {
                    ForEach(security.wallets) { info in Text(info.name).tag(Optional(info.id)) }
                }
                TextEditor(text: $phrase)
                    .font(.system(.body, design: .monospaced))
                    .frame(height: 80)
                    .border(Color.dash.gray300Alpha40)
                buttons(next: MacStrings.Common.next, enabled: wallet != nil && !phrase.isEmpty) {
                    guard let wallet else { return }
                    let text = phrase
                    phrase = ""
                    Task { await security.submitRecoveryPhrase(text, wallet: wallet) }
                }
            case .choosingPassphrase:
                PassphraseField(label: MacStrings.Common.newPassphrase, text: $passphrase)
                PassphraseField(label: MacStrings.Common.repeatPassphrase, text: $confirmation)
                buttons(next: MacStrings.Common.ok, enabled: !passphrase.isEmpty) {
                    let (new, repeated) = (passphrase, confirmation)
                    passphrase = ""
                    confirmation = ""
                    Task { await security.chooseNewPassphrase(new, confirmation: repeated) }
                }
            case .recovering:
                ProgressView()
            case .done(let result):
                SystemNotice(text: L10n.Security.passphraseReset, tone: .info)
                if let text = security.walletsWithoutSecretsText, !result.walletsWithoutSecrets.isEmpty {
                    SystemNotice(text: text, tone: .warning)
                }
                buttons(next: MacStrings.Common.done, enabled: true, action: onClose)
            case .failed(let reason):
                SystemNotice(text: reason, tone: .error)
                buttons(next: MacStrings.Common.close, enabled: true, action: onClose)
            }
            if let error = security.errorMessage {
                Text(error).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 480)
        .onAppear { wallet = security.wallets.first?.id }
    }

    private func buttons(next: String, enabled: Bool, action: @escaping () -> Void) -> some View {
        HStack {
            Spacer()
            Button(MacStrings.Common.cancel, action: onClose).keyboardShortcut(.cancelAction)
            Button(next, action: action).keyboardShortcut(.defaultAction).disabled(!enabled)
        }
    }
}

/// IOS-109: the typed acceptance sentence (and the passphrase of an
/// encrypted vault) before every wallet and the vault are deleted.
private struct WipeSheet: View {
    let security: SecurityViewModel
    let onClose: () -> Void
    @State private var sentence = ""
    @State private var passphrase = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(L10n.Wallets.deleteAllTitle).dashFont(.title3)
            Text(L10n.Wallets.deleteAllMessage).dashFont(.subhead)
            Text("“\(L10n.Wallets.wipeAcceptPhrase)”")
                .dashFont(.footnoteMedium)
                .textSelection(.enabled)
            TextField(L10n.Wallets.typeSentence, text: $sentence, axis: .vertical)
                .textFieldStyle(.roundedBorder)
                .accessibilityIdentifier("wipe.sentence")
            if security.vaultStatus?.encrypted == true {
                SecureField(MacStrings.Common.passphrase, text: $passphrase).textFieldStyle(.roundedBorder)
            }
            switch security.wipeStep {
            case .wiping: ProgressView()
            case .done: SystemNotice(text: L10n.Security.wiped, tone: .info)
            case .failed(let reason): SystemNotice(text: reason, tone: .error)
            case .idle, .confirming: EmptyView()
            }
            if let error = security.errorMessage {
                Text(error).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
            }
            HStack {
                Spacer()
                Button(security.wipeStep == .done ? MacStrings.Common.done : MacStrings.Common.cancel, action: onClose)
                    .keyboardShortcut(.cancelAction)
                if security.wipeStep == .confirming {
                    Button(L10n.Wallets.deleteAll, role: .destructive) {
                        let (text, secret) = (sentence, passphrase)
                        passphrase = ""
                        Task {
                            await security.wipe(confirmation: text, passphrase: secret.isEmpty ? nil : secret)
                        }
                    }
                    .disabled(sentence.isEmpty)
                }
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 520)
    }
}
#endif
