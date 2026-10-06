// PSBT Operations dialog (QT-076…079): the transaction description, status
// line, Sign Tx (spend grant up to the amount sent out), Broadcast Tx, Copy
// to Clipboard and Save…; File ▸ Load PSBT from file / clipboard open it.
// Dropping a .psbt file on the window loads it too.
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

struct PSBTWindow: View {
    let model: MacAppModel
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        if let psbt = model.features?.psbt {
            PSBTView(psbt: psbt, onClose: {
                psbt.close()
                dismissWindow(id: SceneID.psbt)
            })
        } else {
            Text(model.unavailableReason ?? L10n.Options.unavailable).padding(DashSpacing.xl)
        }
    }
}

struct PSBTView: View {
    let psbt: PSBTViewModel
    let onClose: () -> Void
    @State private var passphrase = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            content
            if case .needsPassphrase = psbt.step {
                HStack {
                    Text(L10n.Lock.prompt).dashFont(.footnote)
                    SecureField(MacStrings.Common.passphrase, text: $passphrase)
                        .textFieldStyle(.roundedBorder)
                        .onSubmit(sign)
                        .accessibilityIdentifier("psbt.passphrase")
                    Button(MacStrings.Common.cancel) {
                        passphrase = ""
                        psbt.cancelPassphrase()
                    }
                    Button(MacStrings.Common.ok, action: sign).disabled(passphrase.isEmpty)
                }
            }
            if let message = psbt.message {
                Text(message)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.successText)
                    .textSelection(.enabled)
                    .accessibilityIdentifier("psbt.message")
            }
            if let error = psbt.errorMessage {
                Text(error)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.errorText)
                    .textSelection(.enabled)
                    .accessibilityIdentifier("psbt.error")
            }
            Divider()
            HStack {
                Button(L10n.PSBT.signTx) { sign() }
                    .disabled(!psbt.canSign)
                    .accessibilityIdentifier("psbt.sign")
                Button(L10n.PSBT.broadcastTx) { Task { await psbt.broadcast() } }
                    .disabled(!psbt.canBroadcast)
                    .accessibilityIdentifier("psbt.broadcast")
                Spacer()
                Button(L10n.PSBT.copyToClipboard) { psbt.copy() }
                    .disabled(psbt.reference == nil)
                    .accessibilityIdentifier("psbt.copy")
                Button(L10n.PSBT.save) { Task { await save() } }
                    .disabled(psbt.reference == nil)
                    .accessibilityIdentifier("psbt.save")
                Button(L10n.PSBT.close, action: onClose)
                    .keyboardShortcut(.cancelAction)
                    .accessibilityIdentifier("psbt.close")
            }
        }
        .padding(DashSpacing.xl)
        .frame(minWidth: 580, minHeight: 380)
        .dropDestination(for: URL.self) { urls, _ in
            guard let url = urls.first else { return false }
            Task { await psbt.load(file: url) }
            return true
        }
        .accessibilityIdentifier("psbt")
    }

    @ViewBuilder
    private var content: some View {
        switch psbt.step {
        case .empty:
            VStack(spacing: DashSpacing.s) {
                Image(systemName: "doc.badge.arrow.up")
                    .font(.system(size: 36))
                    .foregroundStyle(Color.dash.secondaryText)
                    .accessibilityHidden(true)
                Text(MacStrings.PSBT.empty)
                    .foregroundStyle(Color.dash.secondaryText)
                    .multilineTextAlignment(.center)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .loading, .signing, .broadcasting:
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        case .ready, .needsPassphrase, .broadcast:
            ScrollView {
                VStack(alignment: .leading, spacing: DashSpacing.xs) {
                    ForEach(Array(psbt.descriptionLines.enumerated()), id: \.offset) { _, line in
                        Text(line)
                            .font(.system(.footnote, design: .monospaced))
                            .textSelection(.enabled)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(DashSpacing.m)
            }
            .background(RoundedRectangle(cornerRadius: DashRadius.standard).fill(Color.dash.secondaryBackground))
            .accessibilityIdentifier("psbt.description")
            if let status = psbt.statusLine {
                Text(status).dashFont(.footnoteMedium).accessibilityIdentifier("psbt.status")
            }
        }
    }

    private func sign() {
        let text = passphrase
        passphrase = ""
        Task { await psbt.sign(passphrase: text.isEmpty ? nil : text) }
    }

    private func save() async {
        guard let url = await MacSavePanel.chooseDestination(
            suggestedName: psbt.suggestedFileName, title: L10n.PSBT.saveTitle)
        else { return }
        psbt.save(to: url)
    }
}
#endif
