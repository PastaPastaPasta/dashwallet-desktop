// Send: recipients, fee, review → (authorize) → confirm → broadcast
// (QT-051…063, IOS-041…052). Only the confirm step's Send button broadcasts.
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct SendScreen: View {
    let model: SendViewModel
    let state: CrossAppState

    @State var pasteText = ""
    @State var customFeeText = ""
    @State var passphrase = ""

    var body: some View {
        let model = model
        Page(L10n.Navigation.send, subtitle: CrossStrings.sendSubtitle, width: .form) {
            phasePanel(model)
            ForEach(model.entries) { entry in
                RecipientEditor(model: model, id: entry.id, canRemove: model.entries.count > 1)
            }
            DashButton(CrossStrings.addRecipient, style: .tintedBlue, isEnabled: model.phase == .editing) {
                model.addRecipient()
            }
            // The form is read-only while broadcasting and while the outcome
            // is unknown: "Broadcast again" sends what was reviewed (L6).
            // `disabled` covers the fields; each button gets `isEnabled`
            // because its own `disabled(false)` would re-enable it.
            DashCard {
                HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                    DashTextField(CrossStrings.pasteCaption, placeholder: CrossStrings.pasteURI, text: $pasteText)
                    DashButton(
                        CrossStrings.pasteCaption, style: .tintedBlue, size: .small,
                        isEnabled: model.isEditable && !pasteText.isEmpty
                    ) {
                        model.paste(pasteText)
                        pasteText = ""
                    }
                }
            }
            .disabled(!model.isEditable)
            let options = state.m2.desktopPreferences.desktop.options
            if options.coinControl {
                CoinControlPanel(state: state)
            }
            feeSection(model)
            if options.psbtControls {
                // QT-076: an unsigned PSBT of this form, copied to the
                // clipboard and shown in the PSBT Operations page.
                DashButton(
                    L10n.PSBT.createUnsigned, style: .tintedGray, size: .small,
                    isEnabled: model.phase == .editing
                ) {
                    Task {
                        guard let draft = await model.makeUnsignedDraft() else { return }
                        await state.psbt.createUnsigned(draft: draft)
                        state.open(.psbt)
                    }
                }
            }
            // The footer bar (UX-SPEC §4.7): balance, Clear All, Send.
            DashCard {
                HStack(spacing: Int(DashSpacing.s)) {
                    if let available = state.main.home?.rows.first(where: { $0.kind == .available }) {
                        let presenter = state.amountPresenter
                        Text(CrossStrings.balanceLabel).dashFont(.footnote).dashForeground(CrossRole.textSecondary)
                        AmountText(
                            available.amount == nil ? L10n.Common.unknown : presenter.number(fromFormatted: available.text),
                            unit: available.amount == nil ? .none : presenter.unitDisplay, style: .footnoteMedium)
                    }
                    Spacer()
                    DashButton(CrossStrings.clearAll, style: .tintedGray, isEnabled: model.phase == .editing) {
                        model.clearAll()
                    }
                    DashButton(L10n.Send.send, style: .filledBlue, size: .large, isEnabled: model.phase == .editing) {
                        Task { await model.review() }
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func feeSection(_ model: SendViewModel) -> some View {
        DashCard {
            SectionHeader(CrossStrings.transactionFeeTitle, style: .headline)
            DashPicker(
                CrossStrings.feeTarget,
                options: ConfirmationTarget.all.map { PickerOption($0.blocks, $0.label) },
                selection: bind(
                    {
                        if case .recommended(let blocks) = model.fee { return blocks }
                        return ConfirmationTarget.defaultBlocks
                    },
                    { model.setFee(.recommended(targetBlocks: $0)) }))
                .disabled(!model.isEditable)
            HStack(spacing: Int(DashSpacing.s)) {
                DashTextField(
                    CrossStrings.customFee, placeholder: CrossStrings.customFeePlaceholder, text: $customFeeText,
                    width: 260)
                .disabled(!model.isEditable)
                DashButton(
                    CrossStrings.applyCustomFee, style: .tintedGray, size: .small,
                    isEnabled: model.isEditable && Int64(customFeeText) != nil
                ) {
                    if let duffs = Int64(customFeeText) { model.setFee(.perKilobyte(Amount(duffs: duffs))) }
                }
                if case .perKilobyte = model.fee {
                    DashButton(CrossStrings.recommendedFee, style: .plainBlue, size: .small, isEnabled: model.isEditable) {
                        model.setFee(.recommended(targetBlocks: ConfirmationTarget.defaultBlocks))
                    }
                }
            }
            if model.customFeeWarning {
                Text(L10n.Send.customFeeTooLow).dashFont(.caption1).dashForeground(CrossRole.warning)
            }
        }
    }

    @ViewBuilder
    private func phasePanel(_ model: SendViewModel) -> some View {
        let state = state
        switch model.phase {
        case .editing:
            EmptyView()
        case .confirmDuplicates:
            DashCard {
                SectionHeader(L10n.Send.duplicateTitle)
                Text(L10n.Send.duplicateMergeText).dashFont(.footnote)
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(L10n.Send.combine) { Task { await model.acknowledgeDuplicates() } }
                    DashButton(CrossStrings.cancel, style: .tintedGray) { Task { await model.cancel() } }
                }
            }
        case .authorizing:
            DashCard {
                Text(L10n.Common.passphraseRequired).dashFont(.footnote)
                DashSecureField(CrossStrings.passphrase, placeholder: CrossStrings.walletPassphrase, text: $passphrase)
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(CrossStrings.authorize, isEnabled: !passphrase.isEmpty) {
                        let text = passphrase
                        passphrase = ""
                        Task { await model.authorize(passphrase: text) }
                    }
                    DashButton(CrossStrings.cancel, style: .tintedGray) {
                        passphrase = ""
                        Task { await model.cancel() }
                    }
                }
            }
        case .preparing:
            Toast(CrossStrings.preparing)
        case .confirm(let summary):
            // The confirm sheet (UX-SPEC §4.7): total at full precision, then
            // dash-qt's lines in a card, Cancel and the counting-down Send.
            DashCard(padding: Int(DashSpacing.xl)) {
                VStack(spacing: Int(DashSpacing.xs)) {
                    SectionHeader(L10n.Send.confirmTitle, style: .subheadMedium)
                    AmountText(
                        state.amountPresenter.full(summary.totalDebit), unit: state.amountPresenter.unitDisplay,
                        style: .title1, weight: .bold)
                }
                .frame(maxWidth: .infinity)
                DashCard(padding: Int(DashSpacing.m), spacing: Int(DashSpacing.xs), fill: CrossRole.cardRaised) {
                    ForEach(Array(model.confirmLines.enumerated()), id: \.offset) { line in
                        Text(line.element).dashFont(.footnote).dashForeground(CrossRole.textPrimary).textSelectionEnabled()
                    }
                }
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(CrossStrings.cancel, style: .tintedGray, size: .large, fillsWidth: true) {
                        Task { await model.cancel() }
                    }
                    DashButton(model.sendButtonTitle, size: .large, isEnabled: model.canConfirm, fillsWidth: true) {
                        Task {
                            await model.confirm()
                            let route = model.route
                            model.route = nil
                            await state.follow(route)
                        }
                    }
                }
            }
        case .broadcasting:
            Toast(CrossStrings.broadcasting)
        case .done(let txid):
            ResultCard(
                icon: .toastSuccess, title: CrossStrings.transactionSent, detail: txid, monospacedDetail: true,
                buttonTitle: CrossStrings.done
            ) {
                Task { await model.dismiss() }
            }
        case .failed(let failure):
            ResultCard(icon: .toastError, title: failure.message, detail: nil, buttonTitle: CrossStrings.back) {
                Task { await model.dismiss() }
            }
        case .broadcastUnknown(let txid, let failure):
            // The inputs stay reserved: the transaction may already be in
            // the mempool. Dismiss clears the form without releasing them.
            DashCard {
                Toast(CrossStrings.broadcastUnknown(txid), kind: .warning)
                Text(failure.message).dashFont(.footnote).textSelectionEnabled()
                HStack(spacing: Int(DashSpacing.s)) {
                    if model.canBroadcastAgain {
                        DashButton(CrossStrings.broadcastAgain, style: .tintedBlue) {
                            Task { await model.broadcastAgain() }
                        }
                    }
                    DashButton(CrossStrings.done, style: .tintedGray) { Task { await model.dismiss() } }
                }
            }
        }
    }
}

/// One recipient: address, amount, subtract-fee, label (QT-052…055).
struct RecipientEditor: View {
    let model: SendViewModel
    let id: RecipientEntry.ID
    let canRemove: Bool

    var body: some View {
        let model = model
        let id = id
        let entry = model.entries.first { $0.id == id } ?? RecipientEntry(id: id)
        let editable = model.isEditable
        DashCard {
            HStack {
                SectionHeader(CrossStrings.recipient, style: .headline)
                Spacer()
                if canRemove {
                    DashButton(CrossStrings.removeRecipient, style: .plainRed, size: .small, isEnabled: editable) {
                        model.removeRecipient(id)
                    }
                }
            }
            DashTextField(
                CrossStrings.payTo, placeholder: CrossStrings.payToPlaceholder,
                text: field(\.address), error: entry.addressError)
            .disabled(!editable)
            HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                DashTextField(
                    CrossStrings.amount, placeholder: CrossStrings.amountPlaceholder,
                    text: field(\.amountText), error: entry.amountError, width: 200)
                .disabled(!editable)
                DashToggle(CrossStrings.subtractFee, isOn: field(\.subtractFee))
                    .disabled(!editable)
                DashButton(CrossStrings.useMax, style: .plainBlue, size: .small, isEnabled: editable) {
                    Task { await model.useMax(for: id) }
                }
            }
            DashTextField(CrossStrings.label, placeholder: CrossStrings.labelPlaceholder, text: field(\.label))
                .disabled(!editable)
            if let message = entry.message {
                KeyValueRow(CrossStrings.message, message)
            }
        }
    }

    /// A binding to one field of this entry. The view model validates the
    /// entries again on review.
    private func field<T: Sendable>(_ keyPath: WritableKeyPath<RecipientEntry, T>) -> Binding<T> {
        let model = model
        let id = id
        return bind(
            { (model.entries.first { $0.id == id } ?? RecipientEntry(id: id))[keyPath: keyPath] },
            { value in
                guard let index = model.entries.firstIndex(where: { $0.id == id }) else { return }
                model.entries[index][keyPath: keyPath] = value
            })
    }
}
