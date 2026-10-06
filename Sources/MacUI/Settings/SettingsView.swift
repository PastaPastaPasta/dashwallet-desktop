// Wallet encryption controls and their sheets (QT-111/113): the vault
// section of the Options Security tab, Encrypt Wallet, Change Passphrase and
// Show Recovery Phrase.
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServices
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

/// The vault's encryption state with Encrypt Wallet, Change Passphrase and
/// Show Recovery Phrase (the Options Security tab).
struct VaultSettingsSection: View {
    let settings: SettingsViewModel
    let screenCapture: (any ScreenCaptureGuard)?
    @State private var sheet: SheetItem?

    var body: some View {
        MenuCard {
            MenuRow(icon: .token(.security), title: MacStrings.Settings.vault) {
                Text(vaultText)
                    .dashFont(.subhead)
                    .foregroundStyle(Color.role.textSecondary)
            }
            if settings.vault?.encrypted == false {
                MenuActionRow(icon: .token(.pin), title: MacStrings.Menu.encryptWallet) {
                    sheet = SheetItem(route: .encryptWallet)
                }
            }
            if settings.vault?.encrypted == true {
                MenuActionRow(icon: .token(.pin), title: MacStrings.Menu.changePassphrase) {
                    sheet = SheetItem(route: .changePassphrase)
                }
            }
            MenuActionRow(icon: .token(.recoveryPhrase), title: MacStrings.Menu.showRecoveryPhrase) {
                sheet = SheetItem(route: .showRecoveryPhrase)
            }
            .disabled(settings.vault == nil || settings.vault?.state == .noVault)
            if let info = settings.infoMessage {
                Text(info)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.success)
                    .padding(.horizontal, DashSpacing.sm)
                    .padding(.bottom, DashSpacing.s)
            }
        }
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
        .dashCanvas()
    }
}

private struct EncryptWalletForm: View {
    let settings: SettingsViewModel
    let onClose: () -> Void
    @State private var passphrase = ""
    @State private var confirmation = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Menu.encryptWallet.replacingOccurrences(of: "…", with: "")).dashFont(.title3).foregroundStyle(Color.role.textPrimary)
            PassphraseField(label: MacStrings.Common.newPassphrase, text: $passphrase)
            PassphraseField(label: MacStrings.Common.repeatPassphrase, text: $confirmation)
            SystemNotice(text: L10n.Onboarding.encryptWarning, tone: .warning)
            FormMessages(settings: settings)
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel, action: onClose)
                    .buttonStyle(.dash(.tintedGray, .medium))
                    .keyboardShortcut(.cancelAction)
                Button(MacStrings.Common.ok) {
                    Task {
                        await settings.encryptWallet(passphrase: passphrase, confirmation: confirmation)
                        if settings.errorMessage == nil { onClose() }
                    }
                }
                .buttonStyle(.dash(.filledBlue, .medium))
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
            Text(MacStrings.Menu.changePassphrase.replacingOccurrences(of: "…", with: "")).dashFont(.title3).foregroundStyle(Color.role.textPrimary)
            PassphraseField(label: MacStrings.Common.oldPassphrase, text: $old)
            PassphraseField(label: MacStrings.Common.newPassphrase, text: $new)
            PassphraseField(label: MacStrings.Common.repeatPassphrase, text: $confirmation)
            FormMessages(settings: settings)
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel, action: onClose)
                    .buttonStyle(.dash(.tintedGray, .medium))
                    .keyboardShortcut(.cancelAction)
                Button(MacStrings.Common.ok) {
                    Task {
                        await settings.changePassphrase(old: old, new: new, confirmation: confirmation)
                        if settings.errorMessage == nil { onClose() }
                    }
                }
                .buttonStyle(.dash(.filledBlue, .medium))
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
            Text(error).dashFont(.footnote).foregroundStyle(Color.role.danger)
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
            Text(MacStrings.Settings.phraseTitle).dashFont(.title3).foregroundStyle(Color.role.textPrimary)
            if words.isEmpty {
                if settings.needsPassphrase {
                    Text(L10n.Lock.prompt).dashFont(.subhead)
                    SecureField(MacStrings.Common.passphrase, text: $passphrase)
                        .textFieldStyle(.dash)
                        .onSubmit(reveal)
                }
                FormMessages(settings: settings)
            } else {
                SystemNotice(text: MacStrings.Settings.phraseWarning, tone: .warning)
                PhraseGrid(words: words)
                if !bip39.isEmpty {
                    DetailRow(MacStrings.Settings.bip39, value: bip39)
                }
            }
            HStack {
                Spacer()
                Button(words.isEmpty ? MacStrings.Common.cancel : MacStrings.Common.done, action: close)
                    .buttonStyle(.dash(.tintedGray, .medium))
                    .keyboardShortcut(.cancelAction)
                if words.isEmpty && settings.needsPassphrase {
                    Button(MacStrings.Common.ok, action: reveal)
                        .buttonStyle(.dash(.filledBlue, .medium))
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
