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
            DashPage(title: L10n.Navigation.send, subtitle: MacStrings.Send.subtitle, maxWidth: DashLayout.formMaxWidth) {
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
                    if let features = model.features, features.options.wallet.coinControl,
                        let coinControl = send.coinControl
                    {
                        SendCoinControlPanel(
                            coinControl: coinControl, send: send,
                            openInputs: { model.windowOpener?(id: SceneID.coinControl) })
                    }
                }
                // Read-only while broadcasting and while the outcome is
                // unknown: "Broadcast again" sends what was reviewed (L6).
                .disabled(!send.isEditable)
            }
            actionBar
        }
        .accessibilityIdentifier("send")
        .overlay { progressOverlay }
        .task(id: CoinControlInputs(send: send)) {
            // The coin-control labels follow the recipients and the fee (QT-072).
            guard let features = model.features, features.options.wallet.coinControl,
                let coinControl = send.coinControl
            else { return }
            await coinControl.updatePayment(amounts: send.coinControlAmounts, fee: send.fee)
        }
        .sheet(isPresented: confirmBinding) { SendConfirmSheet(send: send, formatAmount: model.formatAmount) }
        .sheet(isPresented: authorizeBinding) { SendAuthorizeSheet(send: send) }
        .sheet(item: Binding(get: { choosingFor.map(ChooserItem.init) }, set: { choosingFor = $0?.id })) { item in
            if let book = model.makeAddressBook(purpose: .send, selectionMode: true) {
                AddressBookView(book: book, onChoose: { chosen in
                    if send.isEditable, let index = send.entries.firstIndex(where: { $0.id == item.id }) {
                        send.entries[index].address = chosen.address
                        send.entries[index].label = chosen.label
                    }
                    choosingFor = nil
                })
                .frame(width: 620, height: 420)
            }
        }
        .alert(L10n.Send.duplicateTitle, isPresented: duplicatesBinding) {
            Button(L10n.Send.combine) { Task { await send.acknowledgeDuplicates() } }
            Button(MacStrings.Common.cancel, role: .cancel) { Task { await send.cancel() } }
        } message: {
            Text(L10n.Send.duplicateMergeText)
        }
        // The broadcast may have reached a peer: the inputs stay reserved and
        // the user is sent to the transaction instead of sending again (M-7).
        .alert(MacStrings.Send.outcomeUnknownTitle, isPresented: unknownBinding) {
            Button(MacStrings.Send.showTransaction) {
                let route = send.route
                send.route = nil
                Task {
                    if let route { await model.main?.navigate(route) }
                    await send.dismiss()
                }
            }
            .accessibilityIdentifier("send.unknown.show")
            if send.canBroadcastAgain {
                Button(MacStrings.Send.broadcastAgain) { Task { await send.broadcastAgain() } }
                    .accessibilityIdentifier("send.unknown.broadcastAgain")
            }
        } message: {
            if case .broadcastUnknown(_, let failure) = send.phase {
                Text("\(MacStrings.Send.outcomeUnknownText)\n\n\(failure.message)")
            }
        }
        .alert(L10n.Send.creationFailed, isPresented: failedBinding) {
            Button(MacStrings.Common.ok) { Task { await send.dismiss() } }
        } message: {
            if case .failed(let failure) = send.phase { Text(failure.message) }
        }
    }

    /// The sticky footer bar (UX-SPEC §4.7): secondary actions on the left,
    /// the estimate and the one primary button on the right.
    private var actionBar: some View {
        HStack(spacing: DashSpacing.s) {
            Button(MacStrings.Send.addRecipient, systemImage: "plus") { send.addRecipient() }
                .buttonStyle(.dash(.tintedBlue, .medium))
                .disabled(!send.isEditable)
                .accessibilityIdentifier("send.addRecipient")
            Button(MacStrings.Send.clearAll) { send.clearAll() }
                .buttonStyle(.dash(.tintedGray, .medium))
                .disabled(!send.isEditable)
                .accessibilityIdentifier("send.clearAll")
            if let features = model.features, features.options.wallet.psbtControls {
                // QT-076: an unsigned PSBT of this form, copied to the
                // clipboard and shown in the PSBT Operations window.
                Button(L10n.PSBT.createUnsigned) {
                    Task {
                        guard let draft = await send.makeUnsignedDraft() else { return }
                        await features.psbt.createUnsigned(draft: draft)
                        model.windowOpener?(id: SceneID.psbt)
                    }
                }
                .buttonStyle(.dash(.tintedBlue, .medium))
                .disabled(send.phase != .editing)
                .accessibilityIdentifier("send.createUnsigned")
            }
            Spacer()
            if let estimate = send.estimate {
                VStack(alignment: .trailing, spacing: 0) {
                    Text(MacStrings.Send.estimate)
                        .dashFont(.caption1)
                        .foregroundStyle(Color.role.textSecondary)
                    AmountText(formatted: model.formatAmount(estimate.fee))
                        .foregroundStyle(Color.role.textPrimary)
                }
            }
            Button {
                Task { await send.review() }
            } label: {
                Label(send.sendButtonTitle, systemImage: "paperplane.fill")
            }
            .buttonStyle(.dash(.filledBlue, .large))
            .disabled(send.phase != .editing)
            .keyboardShortcut(.return, modifiers: .command)
            .accessibilityIdentifier("send.review")
        }
        .padding(.horizontal, DashLayout.pagePaddingH)
        .padding(.vertical, DashSpacing.m)
        .background(Color.role.card)
        .overlay(alignment: .top) { Rectangle().fill(Color.role.separator).frame(height: 0.5) }
    }

    @ViewBuilder
    private var progressOverlay: some View {
        switch send.phase {
        case .preparing, .broadcasting:
            ZStack {
                Color.role.overlay
                VStack(spacing: DashSpacing.m) {
                    ProgressView()
                    Text(send.phase == .preparing ? MacStrings.Send.preparing : MacStrings.Send.broadcasting)
                        .dashFont(.subheadMedium)
                        .foregroundStyle(Color.role.textPrimary)
                }
                .dashCard(padding: DashSpacing.xxl, elevation: .floating)
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

    private var unknownBinding: Binding<Bool> {
        Binding(get: { if case .broadcastUnknown = send.phase { true } else { false } }, set: { _ in })
    }

    private var failedBinding: Binding<Bool> {
        Binding(get: { if case .failed = send.phase { true } else { false } }, set: { _ in })
    }
}

/// What the coin-control summary depends on.
private struct CoinControlInputs: Hashable {
    let amounts: [String]
    let fee: FeeChoice

    @MainActor
    init(send: SendViewModel) {
        amounts = send.entries.map(\.amountText)
        fee = send.fee
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
                    .foregroundStyle(Color.role.textPrimary)
                Spacer()
                if canRemove {
                    Button(action: onRemove) { Image(systemName: "xmark") }
                        .buttonStyle(.dash(.tintedGray, .extraSmall))
                        .help(MacStrings.Send.removeRecipient)
                        .accessibilityLabel(MacStrings.Send.removeRecipient)
                }
            }
            VStack(alignment: .leading, spacing: DashSpacing.xs) {
                FieldCaption(MacStrings.Send.payTo)
                HStack(spacing: DashSpacing.s) {
                    // Addresses use the proportional face (UX-SPEC §5.5).
                    TextField(MacStrings.Send.payTo, text: $entry.address, prompt: Text(MacStrings.Send.payToPlaceholder))
                        .textFieldStyle(.dash(isError: entry.addressError != nil))
                        .accessibilityIdentifier("send.address.\(number - 1)")
                    Button(action: onChoose) { Image(systemName: "book.closed") }
                        .buttonStyle(.dash(.tintedBlue, .small))
                        .help(MacStrings.Send.chooseAddress)
                        .accessibilityLabel(MacStrings.Send.chooseAddress)
                    Button(action: onPaste) { Image(systemName: "doc.on.clipboard") }
                        .buttonStyle(.dash(.tintedBlue, .small))
                        .help(MacStrings.Send.pasteAddress)
                        .accessibilityLabel(MacStrings.Send.pasteAddress)
                }
                if let error = entry.addressError {
                    Text(error)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.role.danger)
                        .accessibilityIdentifier("send.addressError.\(number - 1)")
                }
            }
            VStack(alignment: .leading, spacing: DashSpacing.xs) {
                FieldCaption(MacStrings.Send.label)
                TextField(MacStrings.Send.label, text: $entry.label, prompt: Text(MacStrings.Send.labelPlaceholder))
                    .textFieldStyle(.dash)
            }
            HStack(alignment: .top, spacing: DashSpacing.l) {
                AmountField(
                    label: MacStrings.Send.amount, text: $entry.amountText, unit: unitName,
                    errorText: entry.amountError, onMax: onMax)
                .accessibilityIdentifier("send.amount.\(number - 1)")
                Toggle(MacStrings.Send.subtractFee, isOn: $entry.subtractFee)
                    .toggleStyle(.checkbox)
                    .dashFont(.footnote)
                    .padding(.top, 34)
            }
            if let message = entry.message {
                DetailRow(MacStrings.Send.message, value: message, stacked: true)
            }
        }
        .dashCard(padding: DashSpacing.xl)
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
            HStack {
                Text(MacStrings.Send.transactionFee)
                    .dashFont(.headline)
                    .foregroundStyle(Color.role.textPrimary)
                Spacer()
                DashSegmentedControl(
                    [(false, MacStrings.Send.recommended), (true, MacStrings.Send.custom)],
                    selection: Binding(
                        get: { isCustom },
                        set: { custom in
                            if custom {
                                send.setFee(.perKilobyte(SendViewModel.minimumFeePerKilobyte))
                            } else {
                                send.setFee(.recommended(targetBlocks: ConfirmationTarget.defaultBlocks))
                            }
                        }))
                .accessibilityLabel(MacStrings.Send.transactionFee)
            }
            switch send.fee {
            case .recommended(let blocks):
                HStack {
                    FieldCaption(MacStrings.Send.confirmationTime)
                    Spacer()
                    Picker(MacStrings.Send.confirmationTime, selection: Binding(
                        get: { blocks }, set: { send.setFee(.recommended(targetBlocks: $0)) }
                    )) {
                        ForEach(ConfirmationTarget.all, id: \.blocks) { target in
                            Text("\(target.label) (\(target.blocks) blocks)").tag(target.blocks)
                        }
                    }
                    .labelsHidden()
                    .fixedSize()
                }
            case .perKilobyte(let rate):
                HStack {
                    TextField(MacStrings.Send.perKilobyte, text: $customText)
                        .textFieldStyle(.dash)
                        .monospacedDigit()
                        .frame(width: 180)
                        .onSubmit(applyCustom)
                    Text("\(unitName) / kB")
                        .dashFont(.subhead)
                        .foregroundStyle(Color.role.textSecondary)
                }
                .onAppear {
                    customText = amounts?.format(rate, unit: send.unit, style: .plain(plusSign: false, separators: .never)) ?? ""
                }
                if send.customFeeWarning {
                    Text(L10n.Send.customFeeTooLow)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.role.warning)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .dashCard(padding: DashSpacing.xl)
    }

    private func applyCustom() {
        guard let amounts, case .success(let value?) = AmountInput.parse(customText, unit: send.unit, formatter: amounts)
        else { return }
        send.setFee(.perKilobyte(value))
    }
}

/// "Confirm send coins" (QT-059) after iOS's confirm sheet: the total at full
/// precision, dash-qt's question, one row per recipient (at most ten, then
/// dash-qt's "(n of m entries displayed)"), funds used, fee, size and total;
/// Cancel is the default and Send is enabled after the 3 s countdown.
struct SendConfirmSheet: View {
    let send: SendViewModel
    var formatAmount: (Amount) -> String = { _ in "" }

    private var summary: PreparedTxSummary? {
        if case .confirm(let summary) = send.phase { summary } else { nil }
    }

    var body: some View {
        SheetScaffold(title: L10n.Send.confirmTitle, onClose: { Task { await send.cancel() } }) {
            VStack(spacing: DashSpacing.l) {
                if let summary {
                    VStack(spacing: DashSpacing.xs) {
                        AmountText(
                            formatted: formatAmount(summary.totalDebit), size: DesignTokens.DashTextStyle.title1.size,
                            weight: .bold)
                            .foregroundStyle(Color.role.textPrimary)
                        Text(L10n.Send.confirmQuestion)
                            .dashFont(.subhead)
                            .foregroundStyle(Color.role.textSecondary)
                    }
                    .frame(maxWidth: .infinity)
                    MenuCard {
                        ForEach(Array(recipients(summary).enumerated()), id: \.offset) { _, output in
                            confirmRow(MacStrings.Send.confirmPayTo) {
                                VStack(alignment: .trailing, spacing: 1) {
                                    AmountText(formatted: formatAmount(output.amount))
                                    Text(payee(output))
                                        .dashFont(.footnote)
                                        .foregroundStyle(Color.role.textSecondary)
                                        .lineLimit(1)
                                        .truncationMode(.middle)
                                        .help(output.address ?? "")
                                }
                            }
                        }
                        if summary.outputs.filter({ !$0.isChange }).count > SendViewModel.maxConfirmLines {
                            Text(L10n.Send.entriesDisplayed(
                                SendViewModel.maxConfirmLines, of: summary.outputs.filter { !$0.isChange }.count))
                                .dashFont(.footnote)
                                .foregroundStyle(Color.role.textTertiary)
                                .padding(.horizontal, DashSpacing.sm)
                        }
                        confirmRow(MacStrings.Send.confirmUsing) {
                            Text(send.page == .coinJoin ? L10n.Send.usingCoinJoinFunds : L10n.Send.usingAnyFunds)
                                .dashFont(.footnote)
                                .foregroundStyle(Color.role.textPrimary)
                        }
                        confirmRow(MacStrings.Send.confirmFee) {
                            AmountText(formatted: formatAmount(summary.fee))
                        }
                        confirmRow(MacStrings.Send.confirmSize) {
                            Text(String(format: "%.3f kB", Double(summary.sizeBytes) / 1000)
                                + " · " + formatAmount(summary.feeRatePerKilobyte) + "/kB")
                                .dashFont(.footnote)
                                .monospacedDigit()
                                .foregroundStyle(Color.role.textPrimary)
                        }
                        confirmRow(MacStrings.Send.confirmTotal) {
                            AmountText(formatted: formatAmount(summary.totalDebit), weight: .semibold)
                        }
                    }
                    if send.page == .coinJoin {
                        Text("\(L10n.Send.coinJoinFeeNote) \(L10n.Send.inputCount(summary.inputCount))")
                            .dashFont(.footnote)
                            .foregroundStyle(Color.role.textSecondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("send.confirm.text")
            .foregroundStyle(Color.role.textPrimary)
        } footer: {
            Button(MacStrings.Common.cancel) { Task { await send.cancel() } }
                .buttonStyle(.dash(.tintedGray, .large, fillsWidth: true))
                // Return cancels (dash-qt's default button); Escape closes the sheet.
                .keyboardShortcut(.defaultAction)
                .accessibilityIdentifier("send.confirm.cancel")
            Button(send.sendButtonTitle) { Task { await send.confirm() } }
                .buttonStyle(.dash(.filledBlue, .large, fillsWidth: true))
                .disabled(!send.canConfirm)
                .accessibilityIdentifier("send.confirm.send")
        }
        .accessibilityIdentifier("send.confirm.sheet")
    }

    private func recipients(_ summary: PreparedTxSummary) -> [PreparedOutput] {
        Array(summary.outputs.filter { !$0.isChange }.prefix(SendViewModel.maxConfirmLines))
    }

    private func payee(_ output: PreparedOutput) -> String {
        let address = AddressText.shortened(output.address ?? "")
        guard let label = output.label, !label.isEmpty else { return address }
        return "\(label) · \(address)"
    }

    private func confirmRow<Value: View>(_ title: String, @ViewBuilder value: () -> Value) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textSecondary)
            Spacer(minLength: DashSpacing.m)
            value()
        }
        .padding(.horizontal, DashSpacing.sm)
        .padding(.vertical, DashSpacing.s)
    }
}

/// Passphrase for the spend grant (QT-061).
struct SendAuthorizeSheet: View {
    let send: SendViewModel
    @State private var passphrase = ""

    var body: some View {
        SheetScaffold(title: MacStrings.Send.authorizeTitle, onClose: { Task { await send.cancel() } }) {
            VStack(alignment: .leading, spacing: DashSpacing.m) {
                Text(MacStrings.Send.authorizePrompt)
                    .dashFont(.subhead)
                    .foregroundStyle(Color.role.textSecondary)
                    .fixedSize(horizontal: false, vertical: true)
                SecureField(MacStrings.Common.passphrase, text: $passphrase)
                    .modifier(DashFieldModifier())
                    .onSubmit(authorize)
                    .accessibilityIdentifier("send.authorize.passphrase")
            }
        } footer: {
            Button(MacStrings.Common.cancel) { Task { await send.cancel() } }
                .buttonStyle(.dash(.tintedGray, .large, fillsWidth: true))
                .keyboardShortcut(.cancelAction)
            Button(MacStrings.Common.ok, action: authorize)
                .buttonStyle(.dash(.filledBlue, .large, fillsWidth: true))
                .keyboardShortcut(.defaultAction)
                .disabled(passphrase.isEmpty)
        }
    }

    private func authorize() {
        let text = passphrase
        passphrase = ""
        Task { await send.authorize(passphrase: text) }
    }
}
#endif
