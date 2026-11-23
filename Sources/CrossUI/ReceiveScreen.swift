// Receive (UX-SPEC §4.8; QT-081…085, IOS-053…055): the QR card with the
// full address (the verification surface: never truncated), the payment
// request form and the requests history.
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct ReceiveScreen: View {
    let model: ReceiveViewModel
    let state: CrossAppState

    var body: some View {
        let model = model
        let state = state
        Page(L10n.Navigation.receive, subtitle: CrossStrings.receiveSubtitle) {
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            qrCard(model, state: state)
            DashCard {
                SectionHeader(CrossStrings.requestPayment, style: .headline)
                Text(L10n.Receive.formHeader).dashFont(.footnote).dashForeground(CrossRole.textSecondary)
                DashTextField(
                    CrossStrings.label, placeholder: L10n.Receive.labelPlaceholder,
                    text: bind({ model.label }, { model.label = $0 }))
                DashTextField(
                    CrossStrings.amount, placeholder: CrossStrings.requestAmountPlaceholder,
                    text: bind({ model.requestAmountText }, { model.requestAmountText = $0 }), error: model.amountError,
                    width: 220)
                DashTextField(
                    CrossStrings.message, placeholder: CrossStrings.requestMessagePlaceholder,
                    text: bind({ model.message }, { model.message = $0 }))
                HStack(spacing: Int(DashSpacing.s)) {
                    Spacer()
                    DashButton(CrossStrings.clear, style: .tintedGray) { model.clearForm() }
                    DashButton(CrossStrings.requestPayment) { Task { await model.createRequest() } }
                }
            }
            if !model.requests.isEmpty {
                SectionHeader(CrossStrings.requests, style: .headline)
                DashCard(spacing: Int(DashSpacing.xxs)) {
                    ForEach(model.requests) { request in
                        MenuRow(
                            title: request.label ?? L10n.Receive.noLabel,
                            help: "\(Format.date(request.createdAt))  ·  \(request.message ?? L10n.Receive.noMessage)"
                        ) {
                            Text(model.amountText(of: request))
                                .dashFont(.footnoteMedium)
                                .dashForeground(CrossRole.textPrimary)
                            DashButton(CrossStrings.show, style: .tintedBlue, size: .small) { model.show(request) }
                            DashButton(CrossStrings.delete, style: .plainRed, size: .small) {
                                Task { await model.deleteRequest(request.id) }
                            }
                        }
                    }
                }
            }
        }
        .task {
            await model.load()
            model.start()
        }
    }

    /// The QR card (C17): code in a white well, the address as a copy row,
    /// then the URI and the actions; a shown request adds its fields.
    @ViewBuilder
    private func qrCard(_ model: ReceiveViewModel, state: CrossAppState) -> some View {
        let address = model.copyAddress()
        DashCard(padding: Int(DashSpacing.xl)) {
            if let request = model.shownRequest {
                SectionHeader(L10n.Receive.requestTitle(request.label ?? request.address), style: .headline)
            }
            HStack(alignment: .top, spacing: Int(DashSpacing.xl)) {
                if let qr = model.qr {
                    QRCodeView(size: qr.size, modules: qr.modules, side: 200)
                        .padding(Int(DashSpacing.sm))
                        .cardBackground(fill: CrossRole.white, radius: Int(DashRadius.standard))
                }
                VStack(alignment: .leading, spacing: Int(DashSpacing.m)) {
                    CopyRow(CrossStrings.receivingAddress, value: address ?? L10n.Common.unknown) {
                        state.copy(address ?? "", what: CrossStrings.addressWord)
                    }
                    if let uri = model.uri {
                        KeyValueRow(CrossStrings.uri, uri)
                    }
                    if let request = model.shownRequest {
                        KeyValueRow(CrossStrings.amount, model.amountText(of: request))
                        KeyValueRow(CrossStrings.label, request.label ?? L10n.Receive.noLabel)
                        KeyValueRow(CrossStrings.message, request.message ?? L10n.Receive.noMessage)
                    }
                    HStack(spacing: Int(DashSpacing.s)) {
                        if let uri = model.uri {
                            DashButton(CrossStrings.copyURI, style: .tintedBlue, size: .small) {
                                state.copy(uri, what: CrossStrings.uri)
                            }
                        }
                        if model.shownRequest != nil {
                            DashButton(CrossStrings.close, style: .tintedGray, size: .small) { model.dismissRequest() }
                        } else {
                            DashButton(CrossStrings.newAddress, style: .tintedBlue, size: .small, icon: .reload) {
                                Task { await model.newAddress() }
                            }
                        }
                    }
                }
            }
        }
    }
}
