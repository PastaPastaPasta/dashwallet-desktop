// Tool pages: address book (QT-095…098) and sign / verify message (QT-099/100).
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct AddressBookScreen: View {
    let model: AddressBookViewModel

    @Environment(\.chooseFileSaveDestination) var chooseFileSaveDestination
    @State var newAddress = ""
    @State var newLabel = ""
    @State var exportMessage: String?

    var body: some View {
        let model = model
        Page(CrossStrings.addressBook) {
            HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                DashPicker(
                    nil, accessibleName: CrossStrings.purpose,
                    options: [
                        PickerOption(AddressPurpose.send, CrossStrings.sending),
                        PickerOption(AddressPurpose.receive, CrossStrings.receiving),
                    ],
                    selection: bind({ model.purpose }, { model.setPurpose($0) }))
                DashTextField(
                    CrossStrings.search, placeholder: L10n.AddressBook.searchPlaceholder,
                    text: bind({ model.search }, { model.setSearch($0) }))
                DashButton(CrossStrings.exportCSV, style: .tintedBlue, size: .small) { export(model) }
            }
            Text(model.header).dashFont(.footnote).dashForeground(.secondaryText)
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            if let exportMessage {
                Toast(exportMessage)
            }
            DashCard {
                if model.entries.isEmpty {
                    Text(CrossStrings.noEntries).dashFont(.footnote).dashForeground(.secondaryText)
                }
                ForEach(model.entries) { entry in
                    HStack(spacing: Int(DashSpacing.s)) {
                        MenuItem(title: model.labelText(for: entry), subtitle: entry.address)
                        DashButton(CrossStrings.showQR, style: .plainBlue, size: .small) { model.showQR(for: entry) }
                        if model.canDelete {
                            DashButton(CrossStrings.delete, style: .plainRed, size: .small) {
                                Task { await model.delete(address: entry.address) }
                            }
                        }
                    }
                }
            }
            if let entry = model.qrEntry, let qr = model.qr {
                DashCard {
                    HStack {
                        SectionHeader(model.labelText(for: entry), style: .subheadMedium)
                        Spacer()
                        DashButton(CrossStrings.hideQR, style: .plainBlue, size: .small) { model.hideQR() }
                    }
                    QRCodeView(size: qr.size, modules: qr.modules)
                    if let uri = model.qrURI {
                        Text(uri).dashFont(.footnote).textSelectionEnabled()
                    }
                }
            }
            if model.canCreate {
                DashCard {
                    SectionHeader(L10n.AddressBook.newSendingAddress, style: .subheadMedium)
                    DashTextField(CrossStrings.address, placeholder: CrossStrings.payToPlaceholder, text: $newAddress)
                    DashTextField(CrossStrings.label, placeholder: CrossStrings.labelPlaceholder, text: $newLabel)
                    DashButton(CrossStrings.save, isEnabled: !newAddress.isEmpty) {
                        let address = newAddress
                        let label = newLabel
                        Task {
                            if await model.save(address: address, label: label, replace: false) {
                                newAddress = ""
                                newLabel = ""
                            }
                        }
                    }
                }
            }
        }
        .task { await model.load() }
    }

    private func export(_ model: AddressBookViewModel) {
        let choose = chooseFileSaveDestination
        let csv = model.exportCSV()
        Task {
            guard
                let url = await choose(
                    title: CrossStrings.addressBook, defaultButtonLabel: CrossStrings.save,
                    defaultFileName: model.purpose == .send ? "sending-addresses.csv" : "receiving-addresses.csv")
            else {
                exportMessage = CrossStrings.exportCancelled
                return
            }
            do {
                try Data(csv.utf8).write(to: url, options: .atomic)
                exportMessage = "\(CrossStrings.exported) \(url.path)"
            } catch {
                exportMessage = L10n.Transactions.exportFailed
            }
        }
    }
}

struct SignVerifyScreen: View {
    let model: SignVerifyViewModel

    @State var passphrase = ""

    var body: some View {
        let model = model
        Page(L10n.SignVerify.windowTitle) {
            DashCard {
                SectionHeader(CrossStrings.signMessage, style: .subheadMedium)
                DashTextField(
                    CrossStrings.address, placeholder: CrossStrings.signingAddress,
                    text: bind({ model.address }, { model.address = $0 }))
                DashTextField(
                    CrossStrings.message, placeholder: CrossStrings.messageToSign,
                    text: bind({ model.message }, { model.message = $0 }))
                if model.needsPassphrase {
                    DashSecureField(CrossStrings.passphrase, placeholder: CrossStrings.walletPassphrase, text: $passphrase)
                }
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(CrossStrings.sign) {
                        let text = passphrase
                        passphrase = ""
                        Task { await model.sign(passphrase: model.needsPassphrase ? text : nil) }
                    }
                    if model.needsPassphrase {
                        DashButton(CrossStrings.cancel, style: .strokeGray) {
                            passphrase = ""
                            model.cancelPassphrase()
                        }
                    }
                    DashButton(CrossStrings.clearAll, style: .strokeGray) { model.clearSign() }
                }
                if !model.signature.isEmpty {
                    KeyValueRow(CrossStrings.signature, model.signature)
                }
                if let result = model.signResult {
                    ResultText(result: result)
                }
            }
            DashCard {
                SectionHeader(CrossStrings.verifyMessage, style: .subheadMedium)
                DashTextField(
                    CrossStrings.address, placeholder: CrossStrings.verifyingAddress,
                    text: bind({ model.verifyAddress }, { model.verifyAddress = $0 }))
                DashTextField(
                    CrossStrings.message, placeholder: CrossStrings.messageToVerify,
                    text: bind({ model.verifyMessage }, { model.verifyMessage = $0 }))
                DashTextField(
                    CrossStrings.signature, placeholder: CrossStrings.signatureToVerify,
                    text: bind({ model.verifySignature }, { model.verifySignature = $0 }))
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(CrossStrings.verify) { model.verify() }
                    DashButton(CrossStrings.clearAll, style: .strokeGray) { model.clearVerify() }
                }
                if let result = model.verifyResult {
                    ResultText(result: result)
                }
            }
        }
    }
}

/// dash-qt shows sign/verify results in green or red; the text carries the meaning.
struct ResultText: View {
    let result: SignVerifyResult

    var body: some View {
        Text(result.text)
            .dashFont(.footnoteMedium)
            .dashForeground(result.isSuccess ? .green : .red)
    }
}
