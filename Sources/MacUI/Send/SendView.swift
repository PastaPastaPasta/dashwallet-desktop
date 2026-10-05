// Send (QT-051…063, IOS-041…052): recipient entries, fee, review →
// authorize → confirm with countdown → broadcast. Only the confirm sheet's
// Send button broadcasts (iOS rule 4).
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

struct SendView: View {
    let model: MacAppModel
    @Bindable var send: SendViewModel
    @State private var choosingFor: RecipientEntry.ID?

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: DashSpacing.l) {
                    ForEach($send.entries) { $entry in
                        RecipientCard(
                            entry: $entry, number: (send.entries.firstIndex(where: { $0.id == entry.id }) ?? 0) + 1,
                            unitName: model.unitName, canRemove: send.entries.count > 1,
                            onChoose: { choosingFor = entry.id },
                            onPaste: { if let text = MacPasteboard.string() { send.paste(text, into: entry.id) } },
                            onMax: { Task { await send.useMax(for: entry.id) } },
                            onRemove: { send.removeRecipient(entry.id) })
                    }
                    FeeSection(send: send, unitName: model.unitName, amounts: model.env?.amounts)
                }
                .padding(DashSpacing.xxl)
            }
            Divider()
            actionBar
        }
        .accessibilityIdentifier("send")
        .overlay { progressOverlay }
        .sheet(isPresented: confirmBinding) { SendConfirmSheet(send: send) }
        .sheet(isPresented: authorizeBinding) { SendAuthorizeSheet(send: send) }
        .sheet(item: Binding(get: { choosingFor.map(ChooserItem.init) }, set: { choosingFor = $0?.id })) { item in
            if let book = model.makeAddressBook(purpose: .send, selectionMode: true) {
                AddressBookView(book: book, onChoose: { chosen in
                    if let index = send.entries.firstIndex(where: { $0.id == item.id }) {
                        send.entries[index].address = chosen.address
                        send.entries[index].label = chosen.label
                    }
                    choosingFor = nil
                })
                .frame(width: 620, height: 420)
            }
        }
        .alert(L10n.Send.duplicateTitle, isPresented: duplicatesBinding) {
            Button(MacStrings.Common.yes) { Task { await send.acknowledgeDuplicates() } }
            Button(MacStrings.Common.cancel, role: .cancel) { Task { await send.cancel() } }
        } message: {
            Text(L10n.Send.duplicateText)
        }
        .alert(L10n.Send.creationFailed, isPresented: failedBinding) {
            Button(MacStrings.Common.ok) { Task { await send.dismiss() } }
        } message: {
            if case .failed(let failure) = send.phase { Text(failure.message) }
        }
    }

    private var actionBar: some View {
        HStack(spacing: DashSpacing.m) {
            Button(MacStrings.Send.addRecipient, systemImage: "plus") { send.addRecipient() }
                .accessibilityIdentifier("send.addRecipient")
            Button(MacStrings.Send.clearAll) { send.clearAll() }
                .accessibilityIdentifier("send.clearAll")
            // Coin control needs a coin-selection view model (QT-068…075), not in M1.
            Button(MacStrings.Send.coinControl) {}
                .disabled(true)
                .help(MacStrings.Send.coinControlUnavailable)
            Spacer()
            if let estimate = send.estimate {
                Text("\(MacStrings.Send.estimate): \(model.formatAmount(estimate.fee))")
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.secondaryText)
            }
            DashButton(
                text: send.sendButtonTitle, leadingIcon: .system("paperplane.fill"),
                isEnabled: send.phase == .editing, size: .medium, style: .filledBlue,
                action: { Task { await send.review() } })
            .keyboardShortcut(.return, modifiers: .command)
            .accessibilityIdentifier("send.review")
        }
        .padding(.horizontal, DashSpacing.xxl)
        .padding(.vertical, DashSpacing.m)
        .background(Color.dash.secondaryBackground)
    }

    @ViewBuilder
    private var progressOverlay: some View {
        switch send.phase {
        case .preparing, .broadcasting:
            ZStack {
                Color.dash.backgroundOverlay
                VStack(spacing: DashSpacing.m) {
                    ProgressView()
                    Text(send.phase == .preparing ? MacStrings.Send.preparing : MacStrings.Send.broadcasting)
                        .dashFont(.subheadMedium)
                }
                .padding(DashSpacing.xxl)
                .background(RoundedRectangle(cornerRadius: DashRadius.card).fill(Color.dash.secondaryBackground))
            }
        default:
            EmptyView()
        }
    }

    private var confirmBinding: Binding<Bool> {
        Binding(
            get: { if case .confirm = send.phase { true } else { false } },
            set: { presented in
                if !presented, case .confirm = send.phase { Task { await send.cancel() } }
            })
    }

    private var authorizeBinding: Binding<Bool> {
        Binding(
            get: { send.phase == .authorizing },
            set: { presented in
                if !presented, send.phase == .authorizing { Task { await send.cancel() } }
            })
    }

    private var duplicatesBinding: Binding<Bool> {
        Binding(get: { send.phase == .confirmDuplicates }, set: { _ in })
    }

    private var failedBinding: Binding<Bool> {
        Binding(get: { if case .failed = send.phase { true } else { false } }, set: { _ in })
    }
}

private struct ChooserItem: Identifiable {
    let id: RecipientEntry.ID
}

/// One payment entry (dash-qt `SendCoinsEntry`).
private struct RecipientCard: View {
    @Binding var entry: RecipientEntry
    let number: Int
    let unitName: String
    let canRemove: Bool
    let onChoose: () -> Void
    let onPaste: () -> Void
    let onMax: () -> Void
    let onRemove: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            HStack {
                Text(MacStrings.Send.recipient(number))
                    .dashFont(.headline)
                    .foregroundStyle(Color.dash.primaryText)
                Spacer()
                if canRemove {
                    Button(action: onRemove) { Image(systemName: "xmark.circle.fill") }
                        .buttonStyle(.plain)
                        .foregroundStyle(Color.dash.secondaryText)
                        .help(MacStrings.Send.removeRecipient)
                        .accessibilityLabel(MacStrings.Send.removeRecipient)
                }
            }
            VStack(alignment: .leading, spacing: DashSpacing.xs) {
                Text(MacStrings.Send.payTo).dashFont(.footnote).foregroundStyle(Color.dash.gray500)
                HStack(spacing: DashSpacing.s) {
                    TextField(MacStrings.Send.payTo, text: $entry.address, prompt: Text(MacStrings.Send.payToPlaceholder))
                        .textFieldStyle(.roundedBorder)
                        .font(.system(.body, design: .monospaced))
                        .accessibilityIdentifier("send.address.\(number - 1)")
                    Button(action: onChoose) { Image(systemName: "book.closed") }
                        .help(MacStrings.Send.chooseAddress)
                        .accessibilityLabel(MacStrings.Send.chooseAddress)
                    Button(action: onPaste) { Image(systemName: "doc.on.clipboard") }
                        .help(MacStrings.Send.pasteAddress)
                        .accessibilityLabel(MacStrings.Send.pasteAddress)
                }
                if let error = entry.addressError {
                    Text(error)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.dash.errorText)
                        .accessibilityIdentifier("send.addressError.\(number - 1)")
                }
            }
            VStack(alignment: .leading, spacing: DashSpacing.xs) {
                Text(MacStrings.Send.label).dashFont(.footnote).foregroundStyle(Color.dash.gray500)
                TextField(MacStrings.Send.label, text: $entry.label, prompt: Text(MacStrings.Send.labelPlaceholder))
                    .textFieldStyle(.roundedBorder)
            }
            HStack(alignment: .top, spacing: DashSpacing.l) {
                AmountField(
                    label: MacStrings.Send.amount, text: $entry.amountText, unit: unitName,
                    errorText: entry.amountError, onMax: onMax)
                .accessibilityIdentifier("send.amount.\(number - 1)")
                Toggle(MacStrings.Send.subtractFee, isOn: $entry.subtractFee)
                    .toggleStyle(.checkbox)
                    .padding(.top, 34)
            }
            if let message = entry.message {
                LabeledContent(MacStrings.Send.message) { Text(message).textSelection(.enabled) }
                    .dashFont(.footnote)
            }
        }
        .padding(DashSpacing.xl)
        .background(RoundedRectangle(cornerRadius: DashRadius.card).fill(Color.dash.secondaryBackground))
    }
}

/// Recommended confirmation target or a custom per-kB rate (QT-057/058).
private struct FeeSection: View {
    let send: SendViewModel
    let unitName: String
    let amounts: (any AmountFormatting)?
    @State private var customText = ""

    private var isCustom: Bool {
        if case .perKilobyte = send.fee { true } else { false }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Send.transactionFee)
                .dashFont(.headline)
                .foregroundStyle(Color.dash.primaryText)
            Picker(MacStrings.Send.transactionFee, selection: Binding(
                get: { isCustom },
                set: { custom in
                    if custom {
                        send.setFee(.perKilobyte(SendViewModel.minimumFeePerKilobyte))
                    } else {
                        send.setFee(.recommended(targetBlocks: ConfirmationTarget.defaultBlocks))
                    }
                }
            )) {
                Text(MacStrings.Send.recommended).tag(false)
                Text(MacStrings.Send.custom).tag(true)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(width: 260)
            switch send.fee {
            case .recommended(let blocks):
                Picker(MacStrings.Send.confirmationTime, selection: Binding(
                    get: { blocks }, set: { send.setFee(.recommended(targetBlocks: $0)) }
                )) {
                    ForEach(ConfirmationTarget.all, id: \.blocks) { target in
                        Text("\(target.label) (\(target.blocks) blocks)").tag(target.blocks)
                    }
                }
                .frame(width: 360)
            case .perKilobyte(let rate):
                HStack {
                    TextField(MacStrings.Send.perKilobyte, text: $customText)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 160)
                        .onSubmit(applyCustom)
                    Text("\(unitName) / kB").foregroundStyle(Color.dash.secondaryText)
                }
                .onAppear {
                    customText = amounts?.format(rate, unit: send.unit, style: .plain(plusSign: false, separators: .never)) ?? ""
                }
                if send.customFeeWarning {
                    Text(L10n.Send.customFeeTooLow)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.dash.orange)
                }
            }
        }
        .padding(DashSpacing.xl)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: DashRadius.card).fill(Color.dash.secondaryBackground))
    }

    private func applyCustom() {
        guard let amounts, case .success(let value?) = AmountInput.parse(customText, unit: send.unit, formatter: amounts)
        else { return }
        send.setFee(.perKilobyte(value))
    }
}

/// "Confirm send coins" (QT-059) with the 3 s countdown before Send is enabled.
struct SendConfirmSheet: View {
    let send: SendViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(L10n.Send.confirmTitle)
                .dashFont(.title3)
            VStack(alignment: .leading, spacing: DashSpacing.xs) {
                ForEach(Array(send.confirmLines.enumerated()), id: \.offset) { index, line in
                    Text(line)
                        .dashFont(index < 2 ? .subheadMedium : .subhead)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .accessibilityIdentifier("send.confirm.text")
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel) { Task { await send.cancel() } }
                    .keyboardShortcut(.cancelAction)
                    .accessibilityIdentifier("send.confirm.cancel")
                Button(send.sendButtonTitle) { Task { await send.confirm() } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!send.canConfirm)
                    .accessibilityIdentifier("send.confirm.send")
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 520)
        .accessibilityIdentifier("send.confirm.sheet")
    }
}

/// Passphrase for the spend grant (QT-061).
struct SendAuthorizeSheet: View {
    let send: SendViewModel
    @State private var passphrase = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Send.authorizeTitle).dashFont(.title3)
            Text(MacStrings.Send.authorizePrompt).dashFont(.subhead)
            SecureField(MacStrings.Common.passphrase, text: $passphrase)
                .textFieldStyle(.roundedBorder)
                .onSubmit(authorize)
                .accessibilityIdentifier("send.authorize.passphrase")
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel) { Task { await send.cancel() } }
                    .keyboardShortcut(.cancelAction)
                Button(MacStrings.Common.ok, action: authorize)
                    .keyboardShortcut(.defaultAction)
                    .disabled(passphrase.isEmpty)
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 420)
    }

    private func authorize() {
        let text = passphrase
        passphrase = ""
        Task { await send.authorize(passphrase: text) }
    }
}
#endif
