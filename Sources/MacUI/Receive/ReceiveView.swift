// Receive (QT-081…085, IOS-053…055): current address with QR, the request
// form and the requested-payments table.
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

struct ReceiveView: View {
    @Bindable var receive: ReceiveViewModel
    let unitName: String
    @State private var selection: Set<ReceiveRequest.ID> = []
    @State private var sortOrder: DataTableSortOrder? = DataTableSortOrder(columnID: "date", ascending: false)

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DashSpacing.xl) {
                if let error = receive.errorMessage {
                    SystemNotice(text: error, tone: .error)
                }
                HStack(alignment: .top, spacing: DashSpacing.xl) {
                    addressCard
                    requestForm
                }
                requestsTable
            }
            .padding(DashSpacing.xxl)
        }
        .accessibilityIdentifier("receive")
        .task { await receive.load() }
    }

    private var addressCard: some View {
        VStack(spacing: DashSpacing.l) {
            if let request = receive.shownRequest {
                Text(L10n.Receive.requestTitle(request.label ?? request.address))
                    .dashFont(.headline)
                    .multilineTextAlignment(.center)
            } else {
                Text(MacStrings.Receive.yourAddress)
                    .dashFont(.headline)
            }
            Group {
                if let qr = receive.qr {
                    QRView(size: qr.size, modules: qr.modules, accessibilityLabel: MacStrings.Receive.qrLabel)
                } else {
                    QRView(size: 0, modules: [], accessibilityLabel: MacStrings.Receive.qrLabel)
                }
            }
            .frame(width: 220, height: 220)
            .accessibilityIdentifier("receive.qr")
            Text(receive.copyAddress() ?? L10n.Common.unknown)
                .font(.system(.callout, design: .monospaced))
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
                .accessibilityIdentifier("receive.address")
            if let request = receive.shownRequest {
                VStack(spacing: DashSpacing.xxs) {
                    Text(receive.amountText(of: request))
                    if let message = request.message, !message.isEmpty { Text(message) }
                }
                .dashFont(.footnote)
                .foregroundStyle(Color.dash.secondaryText)
            }
            HStack(spacing: DashSpacing.s) {
                Button(MacStrings.Common.copyAddress, systemImage: "doc.on.doc") {
                    if let address = receive.copyAddress() { MacPasteboard.copy(address) }
                }
                .accessibilityIdentifier("receive.copyAddress")
                Button(MacStrings.Common.copyURI, systemImage: "link") {
                    if let uri = receive.copyURI() { MacPasteboard.copy(uri) }
                }
                .disabled(receive.uri == nil)
                if receive.shownRequest != nil {
                    Button(MacStrings.Receive.backToAddress) { receive.dismissRequest() }
                } else {
                    Button(MacStrings.Receive.newAddress, systemImage: "arrow.clockwise") {
                        Task { await receive.newAddress() }
                    }
                    .accessibilityIdentifier("receive.newAddress")
                }
            }
            .controlSize(.small)
        }
        .padding(DashSpacing.xl)
        .frame(width: 340)
        .background(RoundedRectangle(cornerRadius: DashRadius.card).fill(Color.dash.secondaryBackground))
    }

    private var requestForm: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Receive.requestPayment).dashFont(.headline)
            Text(L10n.Receive.formHeader)
                .dashFont(.footnote)
                .foregroundStyle(Color.dash.secondaryText)
            VStack(alignment: .leading, spacing: DashSpacing.xs) {
                Text(MacStrings.Receive.label).dashFont(.footnote).foregroundStyle(Color.dash.gray500)
                TextField(MacStrings.Receive.label, text: $receive.label, prompt: Text(L10n.Receive.labelPlaceholder))
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("receive.label")
            }
            AmountField(
                label: MacStrings.Receive.amount, text: $receive.requestAmountText, unit: unitName,
                errorText: receive.amountError)
            .accessibilityIdentifier("receive.amount")
            VStack(alignment: .leading, spacing: DashSpacing.xs) {
                Text(MacStrings.Receive.message).dashFont(.footnote).foregroundStyle(Color.dash.gray500)
                TextField(MacStrings.Receive.message, text: $receive.message)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("receive.message")
            }
            HStack {
                Button(MacStrings.Receive.clear) { receive.clearForm() }
                Spacer()
                DashButton(
                    text: MacStrings.Receive.createRequest, size: .medium, style: .filledBlue,
                    action: { Task { await receive.createRequest() } })
                .accessibilityIdentifier("receive.createRequest")
            }
        }
        .padding(DashSpacing.xl)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: DashRadius.card).fill(Color.dash.secondaryBackground))
    }

    private var requestsTable: some View {
        VStack(alignment: .leading, spacing: DashSpacing.s) {
            Text(MacStrings.Receive.requests).dashFont(.headline)
            DataTable(
                rows: receive.requests,
                columns: [
                    DataTableColumn(
                        MacStrings.Receive.date, id: "date", width: .fixed(150),
                        sortBy: { $0.createdAt < $1.createdAt }
                    ) { request in
                        Text(request.createdAt.formatted(date: .abbreviated, time: .shortened)).dashFont(.footnote)
                    },
                    DataTableColumn(MacStrings.Receive.label, id: "label") { $0.label ?? L10n.Receive.noLabel },
                    DataTableColumn(MacStrings.Receive.message, id: "message") { $0.message ?? L10n.Receive.noMessage },
                    DataTableColumn(
                        "\(MacStrings.Receive.amount) (\(unitName))", id: "amount", width: .fixed(170),
                        alignment: .trailing
                    ) { receive.amountText(of: $0) },
                ],
                selection: $selection, sortOrder: $sortOrder, emptyText: MacStrings.Receive.noRequests,
                onActivate: { id in
                    if let request = receive.requests.first(where: { $0.id == id }) { receive.show(request) }
                },
                contextMenu: { ids in
                    [
                        DataTableMenuAction(title: MacStrings.Receive.show, isEnabled: ids.count == 1) {
                            if let id = ids.first, let request = receive.requests.first(where: { $0.id == id }) {
                                receive.show(request)
                            }
                        },
                        DataTableMenuAction(title: MacStrings.Common.copyURI, isEnabled: ids.count == 1) {
                            if let id = ids.first, let request = receive.requests.first(where: { $0.id == id }) {
                                MacPasteboard.copy(request.uri)
                            }
                        },
                        DataTableMenuAction(title: MacStrings.Receive.remove, isDestructive: true) {
                            Task { for id in ids { await receive.deleteRequest(id) } }
                        },
                    ]
                })
            .frame(minHeight: 160, maxHeight: 260)
            .clipShape(RoundedRectangle(cornerRadius: DashRadius.standard))
            .accessibilityIdentifier("receive.requests")
        }
    }
}
#endif
