// Sign / Verify Message window (QT-099/100).
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

struct SignVerifyWindow: View {
    @Bindable var model: MacAppModel
    @State private var signVerify: SignVerifyViewModel?

    var body: some View {
        Group {
            if let signVerify {
                // The iOS segmented control instead of a system tab view (UX-SPEC §4.11).
                VStack(alignment: .leading, spacing: DashSpacing.l) {
                    DashSegmentedControl(
                        [(SignVerifyTab.sign, MacStrings.SignVerify.signTab),
                         (SignVerifyTab.verify, MacStrings.SignVerify.verifyTab)],
                        selection: $model.signVerifyTab,
                        segmentIdentifier: { "signVerify.tab.\($0 == .sign ? "sign" : "verify")" })
                    .frame(maxWidth: .infinity)
                    switch model.signVerifyTab {
                    case .sign: SignMessageTab(signVerify: signVerify, model: model)
                    case .verify: VerifyMessageTab(signVerify: signVerify)
                    }
                    Spacer(minLength: 0)
                }
                .padding(DashSpacing.xl)
            } else {
                Text(L10n.Common.noWallet)
                    .foregroundStyle(Color.role.textSecondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .frame(minWidth: 560, minHeight: 440)
        .dashCanvas()
        .task(id: model.main?.network) { signVerify = model.makeSignVerify() }
    }
}

/// The message box: a radius-16 field-filled editor.
private struct MessageEditor: View {
    @Binding var text: String

    var body: some View {
        TextEditor(text: $text)
            .font(DesignTokens.DashTextStyle.callout.font)
            .foregroundStyle(Color.role.textPrimary)
            .scrollContentBackground(.hidden)
            .frame(minHeight: 100)
            .padding(DashSpacing.s)
            .background(RoundedRectangle(cornerRadius: DashRadius.textField, style: .continuous).fill(Color.role.fieldFill))
    }
}

private struct ResultLine: View {
    let result: SignVerifyResult?

    var body: some View {
        if let result {
            Text(result.text)
                .dashFont(.footnoteMedium)
                .foregroundStyle(result.isSuccess ? Color.role.success : Color.role.danger)
                .accessibilityIdentifier("signVerify.result")
        }
    }
}

private struct SignMessageTab: View {
    @Bindable var signVerify: SignVerifyViewModel
    let model: MacAppModel
    @State private var choosing = false
    @State private var passphrase = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            SystemNotice(text: MacStrings.SignVerify.signIntro, tone: .warning)
            FieldCaption(MacStrings.SignVerify.address)
            HStack(spacing: DashSpacing.s) {
                TextField(MacStrings.SignVerify.address, text: $signVerify.address)
                    .textFieldStyle(.dash)
                    .accessibilityIdentifier("signVerify.sign.address")
                Button { choosing = true } label: { Image(systemName: "book.closed") }
                    .buttonStyle(.dash(.tintedBlue, .small))
                    .help(MacStrings.Send.chooseAddress)
                    .accessibilityLabel(MacStrings.Send.chooseAddress)
                Button {
                    if let text = MacPasteboard.string() { signVerify.address = text }
                } label: { Image(systemName: "doc.on.clipboard") }
                    .buttonStyle(.dash(.tintedBlue, .small))
                    .help(MacStrings.Send.pasteAddress)
                    .accessibilityLabel(MacStrings.Send.pasteAddress)
            }
            MessageEditor(text: $signVerify.message)
                .accessibilityIdentifier("signVerify.sign.message")
            FieldCaption(MacStrings.SignVerify.signature)
            HStack(spacing: DashSpacing.s) {
                // The signature is a technical value.
                TextField(MacStrings.SignVerify.signature, text: .constant(signVerify.signature))
                    .textFieldStyle(.dash(isTechnical: true))
                    .accessibilityIdentifier("signVerify.sign.signature")
                Button { MacPasteboard.copy(signVerify.signature) } label: { Image(systemName: "doc.on.doc") }
                    .buttonStyle(.dash(.tintedGray, .small))
                    .help(MacStrings.SignVerify.copySignature)
                    .accessibilityLabel(MacStrings.SignVerify.copySignature)
                    .disabled(signVerify.signature.isEmpty)
            }
            HStack(spacing: DashSpacing.s) {
                Button(MacStrings.SignVerify.sign) { Task { await signVerify.sign() } }
                    .buttonStyle(.dash(.filledBlue, .medium))
                    .keyboardShortcut(.defaultAction)
                    .accessibilityIdentifier("signVerify.sign")
                Button(MacStrings.SignVerify.clearAll) { signVerify.clearSign() }
                    .buttonStyle(.dash(.tintedGray, .medium))
                Spacer()
                ResultLine(result: signVerify.signResult)
            }
        }
        .sheet(isPresented: $choosing) {
            if let book = model.makeAddressBook(purpose: .receive, selectionMode: true) {
                AddressBookView(book: book, onChoose: { entry in
                    signVerify.address = entry.address
                    choosing = false
                })
                .frame(width: 600, height: 400)
            }
        }
        .sheet(isPresented: Binding(
            get: { signVerify.needsPassphrase },
            set: { if !$0, signVerify.needsPassphrase { signVerify.cancelPassphrase() } }
        )) {
            VStack(alignment: .leading, spacing: DashSpacing.m) {
                Text(L10n.Lock.title).dashFont(.title3).foregroundStyle(Color.role.textPrimary)
                Text(L10n.Lock.prompt).dashFont(.subhead).foregroundStyle(Color.role.textSecondary)
                SecureField(MacStrings.Common.passphrase, text: $passphrase)
                    .textFieldStyle(.dash)
                HStack {
                    Spacer()
                    Button(MacStrings.Common.cancel) { signVerify.cancelPassphrase() }
                        .buttonStyle(.dash(.tintedGray, .medium))
                        .keyboardShortcut(.cancelAction)
                    Button(MacStrings.Common.ok) {
                        let text = passphrase
                        passphrase = ""
                        Task { await signVerify.sign(passphrase: text) }
                    }
                    .buttonStyle(.dash(.filledBlue, .medium))
                    .keyboardShortcut(.defaultAction)
                    .disabled(passphrase.isEmpty)
                }
            }
            .padding(DashSpacing.xl)
            .frame(width: 400)
            .dashCanvas()
        }
    }
}

private struct VerifyMessageTab: View {
    @Bindable var signVerify: SignVerifyViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            SystemNotice(text: MacStrings.SignVerify.verifyIntro, tone: .info)
            FieldCaption(MacStrings.SignVerify.address)
            TextField(MacStrings.SignVerify.address, text: $signVerify.verifyAddress)
                .textFieldStyle(.dash)
                .accessibilityIdentifier("signVerify.verify.address")
            MessageEditor(text: $signVerify.verifyMessage)
                .accessibilityIdentifier("signVerify.verify.message")
            FieldCaption(MacStrings.SignVerify.signature)
            TextField(MacStrings.SignVerify.signature, text: $signVerify.verifySignature)
                .textFieldStyle(.dash(isTechnical: true))
                .accessibilityIdentifier("signVerify.verify.signature")
            HStack(spacing: DashSpacing.s) {
                Button(MacStrings.SignVerify.verify) { signVerify.verify() }
                    .buttonStyle(.dash(.filledBlue, .medium))
                    .keyboardShortcut(.defaultAction)
                    .accessibilityIdentifier("signVerify.verify")
                Button(MacStrings.SignVerify.clearAll) { signVerify.clearVerify() }
                    .buttonStyle(.dash(.tintedGray, .medium))
                Spacer()
                ResultLine(result: signVerify.verifyResult)
            }
        }
    }
}
#endif
