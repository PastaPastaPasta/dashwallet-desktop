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
                TabView(selection: $model.signVerifyTab) {
                    SignMessageTab(signVerify: signVerify, model: model)
                        .tabItem { Text(MacStrings.SignVerify.signTab) }
                        .tag(SignVerifyTab.sign)
                    VerifyMessageTab(signVerify: signVerify)
                        .tabItem { Text(MacStrings.SignVerify.verifyTab) }
                        .tag(SignVerifyTab.verify)
                }
                .padding(DashSpacing.l)
            } else {
                Text(L10n.Common.noWallet)
                    .foregroundStyle(Color.dash.secondaryText)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .frame(minWidth: 560, minHeight: 440)
        .task(id: model.main?.network) { signVerify = model.makeSignVerify() }
    }
}

private struct ResultLine: View {
    let result: SignVerifyResult?

    var body: some View {
        if let result {
            Text(result.text)
                .dashFont(.footnoteMedium)
                .foregroundStyle(result.isSuccess ? Color.dash.successText : Color.dash.errorText)
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
            Text(MacStrings.SignVerify.signIntro)
                .dashFont(.footnote)
                .foregroundStyle(Color.dash.secondaryText)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                TextField(MacStrings.SignVerify.address, text: $signVerify.address)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(.body, design: .monospaced))
                    .accessibilityIdentifier("signVerify.sign.address")
                Button { choosing = true } label: { Image(systemName: "book.closed") }
                    .help(MacStrings.Send.chooseAddress)
                    .accessibilityLabel(MacStrings.Send.chooseAddress)
                Button {
                    if let text = MacPasteboard.string() { signVerify.address = text }
                } label: { Image(systemName: "doc.on.clipboard") }
                    .accessibilityLabel(MacStrings.Send.pasteAddress)
            }
            TextEditor(text: $signVerify.message)
                .font(.body)
                .frame(minHeight: 100)
                .overlay(RoundedRectangle(cornerRadius: 4).stroke(Color.dash.gray300Alpha40))
                .accessibilityIdentifier("signVerify.sign.message")
            HStack {
                TextField(MacStrings.SignVerify.signature, text: .constant(signVerify.signature))
                    .textFieldStyle(.roundedBorder)
                    .font(.system(.footnote, design: .monospaced))
                    .accessibilityIdentifier("signVerify.sign.signature")
                Button { MacPasteboard.copy(signVerify.signature) } label: { Image(systemName: "doc.on.doc") }
                    .help(MacStrings.SignVerify.copySignature)
                    .accessibilityLabel(MacStrings.SignVerify.copySignature)
                    .disabled(signVerify.signature.isEmpty)
            }
            HStack {
                Button(MacStrings.SignVerify.sign) { Task { await signVerify.sign() } }
                    .keyboardShortcut(.defaultAction)
                    .accessibilityIdentifier("signVerify.sign")
                Button(MacStrings.SignVerify.clearAll) { signVerify.clearSign() }
                Spacer()
                ResultLine(result: signVerify.signResult)
            }
        }
        .padding(DashSpacing.m)
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
                Text(L10n.Lock.title).dashFont(.title3)
                Text(L10n.Lock.prompt).dashFont(.subhead)
                SecureField(MacStrings.Common.passphrase, text: $passphrase)
                    .textFieldStyle(.roundedBorder)
                HStack {
                    Spacer()
                    Button(MacStrings.Common.cancel) { signVerify.cancelPassphrase() }
                        .keyboardShortcut(.cancelAction)
                    Button(MacStrings.Common.ok) {
                        let text = passphrase
                        passphrase = ""
                        Task { await signVerify.sign(passphrase: text) }
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(passphrase.isEmpty)
                }
            }
            .padding(DashSpacing.xl)
            .frame(width: 400)
        }
    }
}

private struct VerifyMessageTab: View {
    @Bindable var signVerify: SignVerifyViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.SignVerify.verifyIntro)
                .dashFont(.footnote)
                .foregroundStyle(Color.dash.secondaryText)
                .fixedSize(horizontal: false, vertical: true)
            TextField(MacStrings.SignVerify.address, text: $signVerify.verifyAddress)
                .textFieldStyle(.roundedBorder)
                .font(.system(.body, design: .monospaced))
                .accessibilityIdentifier("signVerify.verify.address")
            TextEditor(text: $signVerify.verifyMessage)
                .font(.body)
                .frame(minHeight: 100)
                .overlay(RoundedRectangle(cornerRadius: 4).stroke(Color.dash.gray300Alpha40))
                .accessibilityIdentifier("signVerify.verify.message")
            TextField(MacStrings.SignVerify.signature, text: $signVerify.verifySignature)
                .textFieldStyle(.roundedBorder)
                .font(.system(.footnote, design: .monospaced))
                .accessibilityIdentifier("signVerify.verify.signature")
            HStack {
                Button(MacStrings.SignVerify.verify) { signVerify.verify() }
                    .keyboardShortcut(.defaultAction)
                    .accessibilityIdentifier("signVerify.verify")
                Button(MacStrings.SignVerify.clearAll) { signVerify.clearVerify() }
                Spacer()
                ResultLine(result: signVerify.verifyResult)
            }
        }
        .padding(DashSpacing.m)
    }
}
#endif
