// Settings scene (QT-020, QT-036, QT-039, QT-135…141 subset, IOS-104/106):
// General (network), Display (unit, digits, discreet, theme, language),
// Security (encryption, passphrase, recovery phrase).
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServices
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

struct SettingsView: View {
    @Bindable var model: MacAppModel

    var body: some View {
        if let main = model.main {
            TabView {
                GeneralSettings(model: model, settings: main.settings)
                    .tabItem { Label(MacStrings.Settings.general, systemImage: "gearshape") }
                DisplaySettingsTab(model: model, settings: main.settings)
                    .tabItem { Label(MacStrings.Settings.display, systemImage: "textformat") }
                SecuritySettings(settings: main.settings, screenCapture: model.env?.screenCapture)
                    .tabItem { Label(MacStrings.Settings.security, systemImage: "lock.shield") }
            }
            .frame(width: 520)
            .task { await main.settings.load() }
        } else {
            Text(model.unavailableReason ?? "").padding()
        }
    }
}

private struct GeneralSettings: View {
    let model: MacAppModel
    let settings: SettingsViewModel

    var body: some View {
        Form {
            Picker(MacStrings.Settings.network, selection: Binding(
                get: { settings.network ?? .mainnet },
                set: { network in Task { await settings.switchNetwork(to: network) } }
            )) {
                ForEach(settings.availableNetworks, id: \.self) { network in
                    Text(L10n.Settings.networkName(network)).tag(network)
                }
            }
            .disabled(settings.isSwitchingNetwork)
            .accessibilityIdentifier("settings.network")
            Text(MacStrings.Settings.networkHelp)
                .font(.footnote)
                .foregroundStyle(.secondary)
            if settings.isSwitchingNetwork {
                ProgressView().controlSize(.small)
            }
            Toggle(MacStrings.Settings.menuBar, isOn: Binding(
                get: { model.showsMenuBarExtra }, set: { model.showsMenuBarExtra = $0 }))
            .accessibilityIdentifier("settings.menuBar")
            if let error = model.preferencesError {
                Text(error).foregroundStyle(Color.dash.errorText)
            }
            if let error = settings.errorMessage {
                Text(error).foregroundStyle(Color.dash.errorText)
            }
        }
        .formStyle(.grouped)
        .padding(DashSpacing.m)
    }
}

private struct DisplaySettingsTab: View {
    let model: MacAppModel
    let settings: SettingsViewModel

    var body: some View {
        Form {
            Picker(MacStrings.Settings.unit, selection: Binding(
                get: { settings.display.unit }, set: { settings.setUnit($0) }
            )) {
                ForEach(DisplayUnit.allCases, id: \.self) { unit in
                    Text(model.env?.amounts.unitName(unit) ?? "").tag(unit)
                }
            }
            .accessibilityIdentifier("settings.unit")
            Stepper(
                value: Binding(get: { settings.display.decimalDigits }, set: { settings.setDecimalDigits($0) }),
                in: SettingsViewModel.decimalDigitsRange
            ) {
                LabeledContent(MacStrings.Settings.decimalDigits, value: "\(settings.display.decimalDigits)")
            }
            Toggle(MacStrings.Settings.discreet, isOn: Binding(
                get: { settings.display.hideBalances }, set: { settings.setDiscreet($0) }))
            .help(MacStrings.Settings.discreetHelp)
            Picker(MacStrings.Settings.theme, selection: Binding(get: { settings.theme }, set: { settings.setTheme($0) })) {
                Text(L10n.Settings.themeSystem).tag(AppTheme.system)
                Text(L10n.Settings.themeLight).tag(AppTheme.light)
                Text(L10n.Settings.themeDark).tag(AppTheme.dark)
            }
            .pickerStyle(.segmented)
            .accessibilityIdentifier("settings.theme")
            Picker(MacStrings.Settings.language, selection: Binding(
                get: { settings.languageCode }, set: { settings.setLanguage($0) }
            )) {
                ForEach(settings.availableLanguages, id: \.self) { code in
                    Text(code == nil ? L10n.Settings.languageSystem : MacStrings.Settings.english).tag(code)
                }
            }
            if let error = settings.errorMessage {
                Text(error).foregroundStyle(Color.dash.errorText)
            }
        }
        .formStyle(.grouped)
        .padding(DashSpacing.m)
    }
}

private struct SecuritySettings: View {
    let settings: SettingsViewModel
    let screenCapture: (any ScreenCaptureGuard)?
    @State private var sheet: SheetItem?

    var body: some View {
        Form {
            LabeledContent(MacStrings.Settings.vault) {
                Text(vaultText)
            }
            HStack {
                if settings.vault?.encrypted == false {
                    Button(MacStrings.Menu.encryptWallet) { sheet = SheetItem(route: .encryptWallet) }
                }
                if settings.vault?.encrypted == true {
                    Button(MacStrings.Menu.changePassphrase) { sheet = SheetItem(route: .changePassphrase) }
                }
                Button(MacStrings.Menu.showRecoveryPhrase) { sheet = SheetItem(route: .showRecoveryPhrase) }
                    .disabled(settings.vault == nil || settings.vault?.state == .noVault)
            }
            if let info = settings.infoMessage {
                Text(info).foregroundStyle(Color.dash.successText)
            }
        }
        .formStyle(.grouped)
        .padding(DashSpacing.m)
        .sheet(item: $sheet) { item in
            SecuritySheet(route: item.route, settings: settings, screenCapture: screenCapture, onClose: { sheet = nil })
        }
    }

    private var vaultText: String {
        guard let vault = settings.vault else { return MacStrings.App.unknown }
        if vault.state == .noVault { return MacStrings.Settings.noVault }
        return vault.encrypted ? MacStrings.Settings.encrypted : MacStrings.Settings.unencrypted
    }
}

/// Encrypt Wallet, Change Passphrase and Show Recovery Phrase (QT-111/113).
struct SecuritySheet: View {
    let route: SheetRoute
    let settings: SettingsViewModel
    var screenCapture: (any ScreenCaptureGuard)?
    let onClose: () -> Void

    var body: some View {
        Group {
            switch route {
            case .encryptWallet: EncryptWalletForm(settings: settings, onClose: onClose)
            case .changePassphrase: ChangePassphraseForm(settings: settings, onClose: onClose)
            case .showRecoveryPhrase: RevealPhraseView(settings: settings, screenCapture: screenCapture, onClose: onClose)
            case .signMessage, .verifyMessage, .sendingAddresses, .receivingAddresses, .settings:
                // Separate windows on macOS; never presented as a sheet.
                EmptyView().onAppear(perform: onClose)
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 460)
    }
}

private struct EncryptWalletForm: View {
    let settings: SettingsViewModel
    let onClose: () -> Void
    @State private var passphrase = ""
    @State private var confirmation = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Menu.encryptWallet.replacingOccurrences(of: "…", with: "")).dashFont(.title3)
            PassphraseField(label: MacStrings.Common.newPassphrase, text: $passphrase)
            PassphraseField(label: MacStrings.Common.repeatPassphrase, text: $confirmation)
            SystemNotice(text: L10n.Onboarding.encryptWarning, tone: .warning)
            FormMessages(settings: settings)
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel, action: onClose).keyboardShortcut(.cancelAction)
                Button(MacStrings.Common.ok) {
                    Task {
                        await settings.encryptWallet(passphrase: passphrase, confirmation: confirmation)
                        if settings.errorMessage == nil { onClose() }
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(passphrase.isEmpty)
            }
        }
    }
}

private struct ChangePassphraseForm: View {
    let settings: SettingsViewModel
    let onClose: () -> Void
    @State private var old = ""
    @State private var new = ""
    @State private var confirmation = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Menu.changePassphrase.replacingOccurrences(of: "…", with: "")).dashFont(.title3)
            PassphraseField(label: MacStrings.Common.oldPassphrase, text: $old)
            PassphraseField(label: MacStrings.Common.newPassphrase, text: $new)
            PassphraseField(label: MacStrings.Common.repeatPassphrase, text: $confirmation)
            FormMessages(settings: settings)
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel, action: onClose).keyboardShortcut(.cancelAction)
                Button(MacStrings.Common.ok) {
                    Task {
                        await settings.changePassphrase(old: old, new: new, confirmation: confirmation)
                        if settings.errorMessage == nil { onClose() }
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(old.isEmpty || new.isEmpty)
            }
        }
    }
}

private struct FormMessages: View {
    let settings: SettingsViewModel

    var body: some View {
        if let error = settings.errorMessage {
            Text(error).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
        }
    }
}

/// Shows the phrase after authorization; the words exist only while shown.
private struct RevealPhraseView: View {
    let settings: SettingsViewModel
    let screenCapture: (any ScreenCaptureGuard)?
    let onClose: () -> Void
    @State private var passphrase = ""
    @State private var words: [String] = []
    @State private var bip39 = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Settings.phraseTitle).dashFont(.title3)
            if words.isEmpty {
                if settings.needsPassphrase {
                    Text(L10n.Lock.prompt).dashFont(.subhead)
                    SecureField(MacStrings.Common.passphrase, text: $passphrase)
                        .textFieldStyle(.roundedBorder)
                        .onSubmit(reveal)
                }
                FormMessages(settings: settings)
            } else {
                SystemNotice(text: MacStrings.Settings.phraseWarning, tone: .warning)
                LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 3), spacing: DashSpacing.s) {
                    ForEach(Array(words.enumerated()), id: \.offset) { index, word in
                        Text("\(index + 1). \(word)")
                            .font(.system(.body, design: .monospaced))
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                if !bip39.isEmpty {
                    LabeledContent(MacStrings.Settings.bip39, value: bip39)
                }
            }
            HStack {
                Spacer()
                Button(words.isEmpty ? MacStrings.Common.cancel : MacStrings.Common.done, action: close)
                    .keyboardShortcut(.cancelAction)
                if words.isEmpty && settings.needsPassphrase {
                    Button(MacStrings.Common.ok, action: reveal)
                        .keyboardShortcut(.defaultAction)
                        .disabled(passphrase.isEmpty)
                }
            }
        }
        .task { await load(passphrase: nil) }
        .onDisappear(perform: hide)
    }

    private func reveal() {
        let text = passphrase
        passphrase = ""
        Task { await load(passphrase: text) }
    }

    private func load(passphrase: String?) async {
        guard let revealed = await settings.revealPhrase(passphrase: passphrase) else { return }
        screenCapture?.setSecretContentVisible(true)
        words = revealed.phrase.withUnsafeBytes { String(decoding: $0, as: UTF8.self) }
            .split(separator: " ").map(String.init)
        bip39 = revealed.bip39Passphrase.withUnsafeBytes { String(decoding: $0, as: UTF8.self) }
    }

    private func hide() {
        words = []
        bip39 = ""
        screenCapture?.setSecretContentVisible(false)
    }

    private func close() {
        hide()
        onClose()
    }
}
#endif
