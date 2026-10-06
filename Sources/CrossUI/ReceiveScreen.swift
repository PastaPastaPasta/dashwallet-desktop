// Receive: current address with QR, payment request form and the requests
// history (QT-081…085, IOS-053…055).
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct ReceiveScreen: View {
    let model: ReceiveViewModel

    var body: some View {
        let model = model
        Page(L10n.Navigation.receive) {
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            DashCard {
                if let request = model.shownRequest {
                    SectionHeader(L10n.Receive.requestTitle(request.label ?? request.address))
                }
                HStack(alignment: .top, spacing: Int(DashSpacing.l)) {
                    if let qr = model.qr {
                        QRCodeView(size: qr.size, modules: qr.modules)
                    }
                    VStack(alignment: .leading, spacing: Int(DashSpacing.s)) {
                        KeyValueRow(CrossStrings.address, model.copyAddress() ?? L10n.Common.unknown)
                        if let uri = model.uri {
                            KeyValueRow(CrossStrings.uri, uri)
                        }
                        if let request = model.shownRequest {
                            KeyValueRow(CrossStrings.amount, model.amountText(of: request))
                            KeyValueRow(CrossStrings.label, request.label ?? L10n.Receive.noLabel)
                            KeyValueRow(CrossStrings.message, request.message ?? L10n.Receive.noMessage)
                            DashButton(CrossStrings.close, style: .strokeGray, size: .small) { model.dismissRequest() }
                        } else {
                            DashButton(CrossStrings.newAddress, style: .tintedBlue, size: .small) {
                                Task { await model.newAddress() }
                            }
                        }
                    }
                }
            }
            DashCard {
                SectionHeader(L10n.Receive.formHeader, style: .subheadMedium)
                DashTextField(
                    CrossStrings.amount, placeholder: CrossStrings.requestAmountPlaceholder,
                    text: bind({ model.requestAmountText }, { model.requestAmountText = $0 }), error: model.amountError,
                    width: 200)
                DashTextField(
                    CrossStrings.label, placeholder: L10n.Receive.labelPlaceholder,
                    text: bind({ model.label }, { model.label = $0 }))
                DashTextField(
                    CrossStrings.message, placeholder: CrossStrings.requestMessagePlaceholder,
                    text: bind({ model.message }, { model.message = $0 }))
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(CrossStrings.requestPayment) { Task { await model.createRequest() } }
                    DashButton(CrossStrings.clear, style: .strokeGray) { model.clearForm() }
                }
            }
            if !model.requests.isEmpty {
                SectionHeader(CrossStrings.requests)
                DashCard {
                    ForEach(model.requests) { request in
                        HStack(spacing: Int(DashSpacing.s)) {
                            MenuItem(
                                title: request.label ?? L10n.Receive.noLabel,
                                subtitle: "\(Format.date(request.createdAt))  \(request.message ?? L10n.Receive.noMessage)",
                                trailing: model.amountText(of: request))
                            DashButton(CrossStrings.show, style: .plainBlue, size: .small) { model.show(request) }
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
}
