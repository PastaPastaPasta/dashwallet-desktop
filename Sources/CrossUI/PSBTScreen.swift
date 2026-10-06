// dash-qt's PSBT Operations dialog as a page (QT-076…079): load from a
// file, the clipboard or pasted base64, then the description, status line,
// Sign Tx (with the passphrase when encrypted), Broadcast Tx, Copy to
// Clipboard, Save… and Close.
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct PSBTScreen: View {
    let model: PSBTViewModel
    let state: CrossAppState

    @Environment(\.chooseFile) var chooseFile
    @Environment(\.chooseFileSaveDestination) var chooseFileSaveDestination
    @State var pasted = ""
    @State var passphrase = ""

    var body: some View {
        let model = model
        let state = state
        Page(L10n.PSBT.dialogTitle, width: .form) {
            DashCard {
                SectionHeader(CrossStrings.loadPSBT, style: .headline)
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(L10n.Shell.loadPSBTFromFile, style: .tintedBlue, size: .small) { loadFile(model) }
                    DashButton(L10n.Shell.loadPSBTFromClipboard, style: .tintedBlue, size: .small) {
                        Task { await model.loadFromClipboard() }
                    }
                }
                HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                    DashTextField(CrossStrings.psbtBase64, placeholder: CrossStrings.psbtBase64Placeholder, text: $pasted)
                    DashButton(CrossStrings.load, style: .tintedBlue, size: .small, isEnabled: !pasted.isEmpty) {
                        let text = pasted.trimmingCharacters(in: .whitespacesAndNewlines)
                        Task { await model.load(data: Data(text.utf8)) }
                    }
                }
                if !state.capabilities.clipboard {
                    Text(CrossStrings.noClipboardTool).dashFont(.caption1).dashForeground(CrossRole.textSecondary)
                }
            }
            if model.step == .loading {
                Text(CrossStrings.loading).dashFont(.footnote).dashForeground(CrossRole.textSecondary)
            }
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            if let message = model.message {
                Toast(message, kind: .success)
            }
            if model.analysis != nil {
                DashCard {
                    ForEach(Array(model.descriptionLines.enumerated()), id: \.offset) { line in
                        Text(line.element).dashFont(.footnote).dashForeground(CrossRole.textPrimary).textSelectionEnabled()
                    }
                    if let status = model.statusLine {
                        Text(status).dashFont(.footnoteMedium).dashForeground(CrossRole.textPrimary)
                    }
                }
                if model.step == .needsPassphrase {
                    DashCard {
                        DashSecureField(CrossStrings.passphrase, placeholder: CrossStrings.walletPassphrase, text: $passphrase)
                        HStack(spacing: Int(DashSpacing.s)) {
                            DashButton(L10n.PSBT.signTx, style: .filledBlue, size: .small) {
                                let text = passphrase
                                passphrase = ""
                                Task { await model.sign(passphrase: text) }
                            }
                            DashButton(CrossStrings.cancel, style: .tintedGray, size: .small) {
                                passphrase = ""
                                model.cancelPassphrase()
                            }
                        }
                    }
                }
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(L10n.PSBT.signTx, style: .filledBlue, size: .small, isEnabled: model.canSign) {
                        Task { await model.sign() }
                    }
                    DashButton(L10n.PSBT.broadcastTx, style: .filledBlue, size: .small, isEnabled: model.canBroadcast) {
                        Task { await model.broadcast() }
                    }
                    DashButton(L10n.PSBT.copyToClipboard, style: .tintedGray, size: .small) { copy(model) }
                    DashButton(L10n.PSBT.save, style: .tintedGray, size: .small) { save(model) }
                }
            }
            HStack(spacing: Int(DashSpacing.s)) {
                DashButton(L10n.PSBT.close, style: .tintedGray, size: .small) {
                    model.close()
                    pasted = ""
                    state.closePage()
                }
            }
        }
    }

    private func loadFile(_ model: PSBTViewModel) {
        let choose = chooseFile
        Task {
            guard let url = await choose(title: L10n.Shell.loadPSBTFromFile, defaultButtonLabel: CrossStrings.open) else {
                return
            }
            await model.load(file: url)
        }
    }

    /// Copy to Clipboard; without a clipboard tool the base64 is shown to
    /// copy by hand.
    private func copy(_ model: PSBTViewModel) {
        if state.capabilities.clipboard {
            model.copy()
        } else if let reference = model.reference {
            state.copy((try? state.m2.psbt.base64(reference)) ?? "", what: CrossStrings.psbtWord)
        }
    }

    private func save(_ model: PSBTViewModel) {
        let choose = chooseFileSaveDestination
        let name = model.suggestedFileName
        Task {
            guard
                let url = await choose(
                    title: L10n.PSBT.saveTitle, defaultButtonLabel: CrossStrings.save, defaultFileName: name)
            else { return }
            model.save(to: url)
        }
    }
}
