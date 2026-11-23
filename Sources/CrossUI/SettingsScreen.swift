// Settings: network, display options, theme, wallet encryption and the
// recovery phrase (QT-020, QT-102/103, QT-111…113, QT-135…141 subset, IOS-104/106).
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct SettingsScreen: View {
    let model: SettingsViewModel
    let state: CrossAppState

    @State var newPassphrase = ""
    @State var repeatPassphrase = ""
    @State var oldPassphrase = ""
    @State var revealPassphrase = ""
    @State var revealedWords: [String] = []

    var body: some View {
        let model = model
        let amounts = state.env.amounts
        Page(CrossStrings.settings) {
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            if let info = model.infoMessage {
                Toast(info, kind: .success)
            }
            DashCard {
                MenuItem(icon: .connections, title: CrossStrings.network)
                DashPicker(
                    nil, accessibleName: CrossStrings.network,
                    options: model.availableNetworks.map { PickerOption($0, L10n.Settings.networkName($0)) },
                    selection: bind(
                        { model.network ?? model.availableNetworks[0] },
                        { network in Task { await model.switchNetwork(to: network) } }))
                if model.isSwitchingNetwork {
                    Text(Format.transition(model.transition)).dashFont(.footnote)
                }
                if let network = model.network {
                    KeyValueRow(CrossStrings.dataDirectory, state.env.host.dataDirectory(for: network).path)
                }
            }
            DashCard {
                MenuItem(icon: .appearance, title: CrossStrings.display)
                DashPicker(
                    CrossStrings.unit,
                    options: DisplayUnit.allCases.map { PickerOption($0, amounts.unitName($0)) },
                    selection: bind({ model.display.unit }, { model.setUnit($0) }))
                DashPicker(
                    CrossStrings.decimalDigits,
                    options: SettingsViewModel.decimalDigitsRange.map { PickerOption($0, "\($0)") },
                    selection: bind({ model.display.decimalDigits }, { model.setDecimalDigits($0) }))
                DashToggle(CrossStrings.hideBalances, isOn: bind({ model.display.hideBalances }, { model.setDiscreet($0) }))
                DashPicker(
                    CrossStrings.theme,
                    options: [
                        PickerOption(AppTheme.system, L10n.Settings.themeSystem),
                        PickerOption(.light, L10n.Settings.themeLight), PickerOption(.dark, L10n.Settings.themeDark),
                    ],
                    selection: bind({ model.theme }, { model.setTheme($0) }))
            }
            security(model)
        }
        .task { await model.load() }
    }

    @ViewBuilder
    private func security(_ model: SettingsViewModel) -> some View {
        DashCard {
            MenuItem(
                icon: .security, title: CrossStrings.vaultStatus, trailing: Format.lockState(model.vault?.state))
            if model.vault?.encrypted == false {
                SectionHeader(CrossStrings.encryptWallet, style: .footnoteMedium)
                DashSecureField(CrossStrings.newPassphrase, text: $newPassphrase)
                DashSecureField(CrossStrings.repeatPassphrase, text: $repeatPassphrase)
                DashButton(CrossStrings.encryptWallet, style: .tintedBlue, size: .small, isEnabled: !newPassphrase.isEmpty) {
                    let (new, repeated) = (newPassphrase, repeatPassphrase)
                    newPassphrase = ""
                    repeatPassphrase = ""
                    Task { await model.encryptWallet(passphrase: new, confirmation: repeated) }
                }
            } else if model.vault?.encrypted == true {
                SectionHeader(CrossStrings.changePassphrase, style: .footnoteMedium)
                DashSecureField(CrossStrings.oldPassphrase, text: $oldPassphrase)
                DashSecureField(CrossStrings.newPassphrase, text: $newPassphrase)
                DashSecureField(CrossStrings.repeatPassphrase, text: $repeatPassphrase)
                DashButton(CrossStrings.changePassphrase, style: .tintedBlue, size: .small, isEnabled: !oldPassphrase.isEmpty) {
                    let (old, new, repeated) = (oldPassphrase, newPassphrase, repeatPassphrase)
                    oldPassphrase = ""
                    newPassphrase = ""
                    repeatPassphrase = ""
                    Task { await model.changePassphrase(old: old, new: new, confirmation: repeated) }
                }
            }
            SectionHeader(CrossStrings.showRecoveryPhrase, style: .footnoteMedium)
            if revealedWords.isEmpty {
                if model.needsPassphrase {
                    DashSecureField(CrossStrings.passphrase, placeholder: CrossStrings.walletPassphrase, text: $revealPassphrase)
                }
                DashButton(CrossStrings.showRecoveryPhrase, style: .tintedGray, size: .small) {
                    let passphrase = revealPassphrase
                    revealPassphrase = ""
                    Task {
                        let revealed = await model.revealPhrase(passphrase: model.needsPassphrase ? passphrase : nil)
                        if let revealed { revealedWords = Self.words(of: revealed.phrase) }
                    }
                }
            } else {
                PhraseGrid(words: revealedWords)
                DashButton(CrossStrings.hideRecoveryPhrase, style: .tintedGray, size: .small) { revealedWords = [] }
            }
        }
    }

    /// Splits a phrase buffer into display words.
    static func words(of phrase: any SecretBuffer) -> [String] {
        phrase.withUnsafeBytes { bytes in
            String(decoding: bytes, as: UTF8.self).split(separator: " ").map(String.init)
        }
    }
}

/// Recovery-phrase words (C30 `PhraseGrid`, iOS `DWSeedPhraseView`): one
/// card, three columns for 12 words, four for 24; each word in `calloutMedium`
/// blue after its number. The word text stays "N. word" (one label), which
/// the AT-SPI checks read.
struct PhraseGrid: View {
    let words: [String]

    var body: some View {
        let columns = words.count > 12 ? 4 : 3
        let rows = stride(from: 0, to: words.count, by: columns).map { start in
            Array(words.enumerated())[start..<min(start + columns, words.count)].map { ($0.offset, $0.element) }
        }
        VStack(alignment: .leading, spacing: Int(DashSpacing.m)) {
            ForEach(Array(rows.enumerated()), id: \.offset) { row in
                HStack(spacing: Int(DashSpacing.l)) {
                    ForEach(row.element, id: \.0) { word in
                        Text("\(word.0 + 1). \(word.1)")
                            .dashFont(.calloutMedium)
                            .dashForeground(CrossRole.textLink)
                            .frame(width: 140, alignment: .leading)
                    }
                }
            }
        }
        .padding(Int(DashSpacing.xl))
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardBackground(fill: CrossRole.cardRaised)
    }
}
