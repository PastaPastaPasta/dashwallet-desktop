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
        Page(L10n.Navigation.send) {
            phasePanel(model)
            ForEach(model.entries) { entry in
                RecipientEditor(model: model, id: entry.id, canRemove: model.entries.count > 1)
            }
            HStack(spacing: Int(DashSpacing.s)) {
                DashTextField(CrossStrings.pasteCaption, placeholder: CrossStrings.pasteURI, text: $pasteText)
                DashButton(CrossStrings.pasteCaption, style: .tintedBlue, size: .small, isEnabled: !pasteText.isEmpty) {
                    model.paste(pasteText)
                    pasteText = ""
                }
            }
            feeSection(model)
            HStack(spacing: Int(DashSpacing.s)) {
                DashButton(L10n.Send.send, style: .filledBlue, isEnabled: model.phase == .editing) {
                    Task { await model.review() }
                }
                DashButton(CrossStrings.addRecipient, style: .tintedBlue, isEnabled: model.phase == .editing) {
                    model.addRecipient()
                }
                DashButton(CrossStrings.clearAll, style: .strokeGray, isEnabled: model.phase == .editing) {
                    model.clearAll()
                }
            }
        }
    }

    @ViewBuilder
    private func feeSection(_ model: SendViewModel) -> some View {
        DashCard {
            DashPicker(
                CrossStrings.feeTarget,
                options: ConfirmationTarget.all.map { PickerOption($0.blocks, $0.label) },
                selection: bind(
                    {
                        if case .recommended(let blocks) = model.fee { return blocks }
                        return ConfirmationTarget.defaultBlocks
                    },
                    { model.setFee(.recommended(targetBlocks: $0)) }))
            HStack(spacing: Int(DashSpacing.s)) {
                DashTextField(
                    CrossStrings.customFee, placeholder: CrossStrings.customFeePlaceholder, text: $customFeeText,
                    width: 260)
                DashButton(CrossStrings.applyCustomFee, style: .tintedGray, size: .small, isEnabled: Int64(customFeeText) != nil) {
                    if let duffs = Int64(customFeeText) { model.setFee(.perKilobyte(Amount(duffs: duffs))) }
                }
                if case .perKilobyte = model.fee {
                    DashButton(CrossStrings.recommendedFee, style: .plainBlue, size: .small) {
                        model.setFee(.recommended(targetBlocks: ConfirmationTarget.defaultBlocks))
                    }
                }
            }
            if model.customFeeWarning {
                Text(L10n.Send.customFeeTooLow).dashFont(.caption1).dashForeground(.orange)
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
                Text(L10n.Send.duplicateText).dashFont(.footnote)
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(CrossStrings.yes) { Task { await model.acknowledgeDuplicates() } }
                    DashButton(CrossStrings.no, style: .strokeGray) { Task { await model.cancel() } }
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
                    DashButton(CrossStrings.cancel, style: .strokeGray) {
                        passphrase = ""
                        Task { await model.cancel() }
                    }
                }
            }
        case .preparing:
            Toast(CrossStrings.preparing)
        case .confirm:
            DashCard {
                SectionHeader(L10n.Send.confirmTitle)
                ForEach(Array(model.confirmLines.enumerated()), id: \.offset) { line in
                    Text(line.element).dashFont(.footnote).dashForeground(.primaryText).textSelectionEnabled()
                }
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(model.sendButtonTitle, isEnabled: model.canConfirm) {
                        Task {
                            await model.confirm()
                            let route = model.route
                            model.route = nil
                            await state.follow(route)
                        }
                    }
                    DashButton(CrossStrings.cancel, style: .strokeGray) { Task { await model.cancel() } }
                }
            }
        case .broadcasting:
            Toast(CrossStrings.broadcasting)
        case .done(let txid):
            Toast(CrossStrings.sent(txid), kind: .success, actionTitle: CrossStrings.done) {
                Task { await model.dismiss() }
            }
        case .failed(let failure):
            Toast(failure.message, kind: .error, actionTitle: CrossStrings.back) {
                Task { await model.dismiss() }
            }
            if model.canRetryBroadcast {
                DashButton(CrossStrings.retryBroadcast, style: .tintedBlue, size: .small) {
                    Task { await model.retryBroadcast() }
                }
            }
        case .broadcastUnknown(let txid, let failure):
            // The inputs stay reserved: the transaction may already be in
            // the mempool. Dismiss clears the form without releasing them.
            DashCard {
                Toast(CrossStrings.broadcastUnknown(txid), kind: .warning)
                Text(failure.message).dashFont(.footnote).textSelectionEnabled()
                DashButton(CrossStrings.done, style: .strokeGray) { Task { await model.dismiss() } }
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
        DashCard {
            DashTextField(
                CrossStrings.payTo, placeholder: CrossStrings.payToPlaceholder,
                text: field(\.address), error: entry.addressError)
            HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                DashTextField(
                    CrossStrings.amount, placeholder: CrossStrings.amountPlaceholder,
                    text: field(\.amountText), error: entry.amountError, width: 200)
                DashToggle(CrossStrings.subtractFee, isOn: field(\.subtractFee))
                DashButton(CrossStrings.useMax, style: .plainBlue, size: .small) {
                    Task { await model.useMax(for: id) }
                }
            }
            DashTextField(CrossStrings.label, placeholder: CrossStrings.labelPlaceholder, text: field(\.label))
            if let message = entry.message {
                KeyValueRow(CrossStrings.message, message)
            }
            if canRemove {
                DashButton(CrossStrings.removeRecipient, style: .plainRed, size: .small) {
                    model.removeRecipient(id)
                }
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
