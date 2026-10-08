// Tool pages: address book (QT-095…098) and sign / verify message (QT-099/100).
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct AddressBookScreen: View {
    let model: AddressBookViewModel
    let state: CrossAppState

    @Environment(\.chooseFileSaveDestination) var chooseFileSaveDestination
    @State var newAddress = ""
    @State var newLabel = ""
    @State var exportMessage: String?

    var body: some View {
        let model = model
        Page(CrossStrings.addressBook, subtitle: model.header) {
            HStack(spacing: Int(DashSpacing.s)) {
                SegmentedControl(
                    options: [
                        PickerOption(AddressPurpose.send, CrossStrings.sending),
                        PickerOption(AddressPurpose.receive, CrossStrings.receiving),
                    ],
                    selection: model.purpose
                ) { model.setPurpose($0) }
                Spacer()
                DashButton(CrossStrings.exportCSV, style: .plainBlue, size: .small, icon: .csvExport) { export(model) }
            }
            DashTextField(
                CrossStrings.search, placeholder: L10n.AddressBook.searchPlaceholder,
                text: bind({ model.search }, { model.setSearch($0) }))
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            if let exportMessage {
                Toast(exportMessage)
            }
            DashCard(padding: Int(DashSpacing.sm), spacing: Int(DashSpacing.xxxs)) {
                if model.entries.isEmpty {
                    EmptyState(icon: .addressBook, title: CrossStrings.noEntries)
                }
                ForEach(model.entries) { entry in
                    // UX-SPEC §4.10: initial avatar, label, middle-truncated address.
                    HStack(spacing: Int(DashSpacing.m)) {
                        InitialAvatar(text: model.labelText(for: entry))
                        VStack(alignment: .leading, spacing: Int(DashSpacing.xxxs)) {
                            Text(model.labelText(for: entry)).dashFont(.subheadMedium).dashForeground(CrossRole.textPrimary)
                            Text(AmountTextRules.middleTruncated(entry.address))
                                .dashFont(.footnote)
                                .dashForeground(CrossRole.textSecondary)
                                .help(entry.address)
                        }
                        Spacer()
                        DashButton(CrossStrings.copyAddress, style: .tintedGray, size: .small) {
                            state.copy(entry.address, what: CrossStrings.addressWord)
                        }
                        DashButton(CrossStrings.showQR, style: .tintedBlue, size: .small) { model.showQR(for: entry) }
                        if model.canDelete {
                            DashButton(CrossStrings.delete, style: .plainRed, size: .small) {
                                Task { await model.delete(address: entry.address) }
                            }
                        }
                    }
                    .padding(.vertical, Int(DashSpacing.xs))
                }
            }
            if let entry = model.qrEntry, let qr = model.qr {
                DashCard {
                    HStack {
                        SectionHeader(model.labelText(for: entry), style: .headline)
                        Spacer()
                        DashButton(CrossStrings.hideQR, style: .tintedGray, size: .small) { model.hideQR() }
                    }
                    QRCodeView(size: qr.size, modules: qr.modules, side: 200)
                        .padding(Int(DashSpacing.sm))
                        .cardBackground(fill: CrossRole.white, radius: Int(DashRadius.standard))
                    if let uri = model.qrURI {
                        Text(uri).dashFont(.footnote).dashForeground(CrossRole.textSecondary).textSelectionEnabled()
                    }
                }
            }
            if model.canCreate {
                DashCard {
                    SectionHeader(L10n.AddressBook.newSendingAddress, style: .headline)
                    DashTextField(CrossStrings.address, placeholder: CrossStrings.payToPlaceholder, text: $newAddress)
                    DashTextField(CrossStrings.label, placeholder: CrossStrings.labelPlaceholder, text: $newLabel)
                    DashButton(CrossStrings.save, style: .tintedBlue, isEnabled: !newAddress.isEmpty) {
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
                try ExportFile.write(csv, to: url)
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
    @State var verifying = false

    var body: some View {
        let model = model
        Page(L10n.SignVerify.windowTitle, width: .form) {
            SegmentedControl(
                options: [PickerOption(false, CrossStrings.signMessage), PickerOption(true, CrossStrings.verifyMessage)],
                selection: verifying
            ) { verifying = $0 }
            if !verifying {
            DashCard {
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
                        DashButton(CrossStrings.cancel, style: .tintedGray) {
                            passphrase = ""
                            model.cancelPassphrase()
                        }
                    }
                    DashButton(CrossStrings.clearAll, style: .tintedGray) { model.clearSign() }
                }
                if !model.signature.isEmpty {
                    KeyValueRow(CrossStrings.signature, model.signature, monospaced: true)
                }
                if let result = model.signResult {
                    ResultText(result: result)
                }
            }
            } else {
            DashCard {
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
                    DashButton(CrossStrings.clearAll, style: .tintedGray) { model.clearVerify() }
                }
                if let result = model.verifyResult {
                    ResultText(result: result)
                }
            }
            }
        }
    }
}

/// A 30 pt blue circle with a white initial (iOS contact avatar).
struct InitialAvatar: View {
    let text: String

    var body: some View {
        ZStack {
            Circle().fill(CrossRole.accent.color)
            Text(text.first.map { String($0).uppercased() } ?? "")
                .dashFont(.subheadMedium)
                .foregroundColor(CrossRole.white.color)
        }
        .frame(width: 30, height: 30)
    }
}

/// dash-qt shows sign/verify results in green or red; the text carries the meaning.
struct ResultText: View {
    let result: SignVerifyResult

    var body: some View {
        Text(result.text)
            .dashFont(.footnoteMedium)
            .dashForeground(result.isSuccess ? CrossRole.success : CrossRole.danger)
    }
}
