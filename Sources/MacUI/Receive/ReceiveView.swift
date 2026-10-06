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
    @State private var toast: ToastMessage?

    var body: some View {
        DashPage(title: L10n.Navigation.receive, maxWidth: DashLayout.contentMaxWidth + 160) {
            VStack(alignment: .leading, spacing: DashLayout.sectionGap) {
                if let error = receive.errorMessage {
                    SystemNotice(text: error, tone: .error)
                }
                // Two columns when there is room, stacked below (UX-SPEC §4.8).
                ViewThatFits(in: .horizontal) {
                    HStack(alignment: .top, spacing: DashLayout.sectionGap) {
                        addressCard
                        requestForm
                    }
                    VStack(alignment: .leading, spacing: DashLayout.sectionGap) {
                        addressCard
                        requestForm
                    }
                }
                requestsTable
            }
        }
        .dashToast($toast)
        .accessibilityIdentifier("receive")
        .task { await receive.load() }
    }

    private func copied(_ text: String?) {
        guard let text else { return }
        MacPasteboard.copy(text)
        toast = ToastMessage(.copied, L10n.UX.copied)
    }

    /// The QR card: the code on white, the address in full (the surface the
    /// payer checks), and the copy / new-address actions.
    private var addressCard: some View {
        VStack(spacing: DashSpacing.l) {
            if let request = receive.shownRequest {
                Text(L10n.Receive.requestTitle(request.label ?? request.address))
                    .dashFont(.headline)
                    .foregroundStyle(Color.role.textPrimary)
                    .multilineTextAlignment(.center)
            } else {
                Text(MacStrings.Receive.yourAddress)
                    .dashFont(.headline)
                    .foregroundStyle(Color.role.textPrimary)
            }
            Group {
                if let qr = receive.qr {
                    QRView(size: qr.size, modules: qr.modules, accessibilityLabel: MacStrings.Receive.qrLabel)
                } else {
                    QRView(size: 0, modules: [], accessibilityLabel: MacStrings.Receive.qrLabel)
                }
            }
            .frame(width: 200, height: 200)
            .padding(DashSpacing.sm)
            .background(RoundedRectangle(cornerRadius: DashRadius.standard, style: .continuous).fill(Color.white))
            .overlay(RoundedRectangle(cornerRadius: DashRadius.standard, style: .continuous)
                .strokeBorder(Color.role.separator.opacity(0.6), lineWidth: 0.5))
            .accessibilityIdentifier("receive.qr")
            HStack(alignment: .center, spacing: DashSpacing.s) {
                Text(receive.copyAddress() ?? L10n.Common.unknown)
                    .dashFont(.subhead)
                    .foregroundStyle(Color.role.textPrimary)
                    .multilineTextAlignment(.center)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("receive.address")
                CopyButton(value: receive.copyAddress() ?? "", label: MacStrings.Common.copyAddress) {
                    toast = ToastMessage(.copied, L10n.UX.copied)
                }
                .disabled(receive.copyAddress() == nil)
                .accessibilityIdentifier("receive.copyAddress")
            }
            .padding(.horizontal, DashSpacing.m)
            .padding(.vertical, DashSpacing.s)
            .background(RoundedRectangle(cornerRadius: DashRadius.standard, style: .continuous).fill(Color.role.fieldFill))
            if let request = receive.shownRequest {
                VStack(spacing: DashSpacing.xxs) {
                    AmountText(formatted: receive.amountText(of: request))
                    if let message = request.message, !message.isEmpty { Text(message).dashFont(.footnote) }
                }
                .foregroundStyle(Color.role.textSecondary)
            }
            HStack(spacing: DashSpacing.s) {
                Button(MacStrings.Common.copyURI, systemImage: "link") { copied(receive.copyURI()) }
                    .buttonStyle(.dash(.tintedBlue, .small))
                    .disabled(receive.uri == nil)
                if receive.shownRequest != nil {
                    Button(MacStrings.Receive.backToAddress) { receive.dismissRequest() }
                        .buttonStyle(.dash(.tintedGray, .small))
                } else {
                    Button(MacStrings.Receive.newAddress, systemImage: "arrow.clockwise") {
                        Task { await receive.newAddress() }
                    }
                    .buttonStyle(.dash(.tintedGray, .small))
                    .accessibilityIdentifier("receive.newAddress")
                }
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 360)
        .dashCard(padding: nil)
    }

    private var requestForm: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Receive.requestPayment)
                .dashFont(.headline)
                .foregroundStyle(Color.role.textPrimary)
            Text(L10n.Receive.formHeader)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            VStack(alignment: .leading, spacing: DashSpacing.xs) {
                FieldCaption(MacStrings.Receive.label)
                TextField(MacStrings.Receive.label, text: $receive.label, prompt: Text(L10n.Receive.labelPlaceholder))
                    .textFieldStyle(.dash)
                    .accessibilityIdentifier("receive.label")
            }
            AmountField(
                label: MacStrings.Receive.amount, text: $receive.requestAmountText, unit: unitName,
                errorText: receive.amountError)
            .accessibilityIdentifier("receive.amount")
            VStack(alignment: .leading, spacing: DashSpacing.xs) {
                FieldCaption(MacStrings.Receive.message)
                TextField(MacStrings.Receive.message, text: $receive.message)
                    .textFieldStyle(.dash)
                    .accessibilityIdentifier("receive.message")
            }
            HStack(spacing: DashSpacing.s) {
                Button(MacStrings.Receive.clear) { receive.clearForm() }
                    .buttonStyle(.dash(.tintedGray, .medium))
                Spacer(minLength: 0)
                Button(MacStrings.Receive.createRequest) { Task { await receive.createRequest() } }
                    .buttonStyle(.dash(.filledBlue, .medium))
                    .accessibilityIdentifier("receive.createRequest")
            }
        }
        .padding(DashSpacing.xl)
        .frame(minWidth: 340, maxWidth: .infinity, alignment: .leading)
        .dashCard(padding: nil)
    }

    private var requestsTable: some View {
        VStack(alignment: .leading, spacing: DashSpacing.s) {
            Text(MacStrings.Receive.requests)
                .dashFont(.headline)
                .foregroundStyle(Color.role.textPrimary)
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
                                copied(request.uri)
                            }
                        },
                        DataTableMenuAction(title: MacStrings.Receive.remove, isDestructive: true) {
                            Task { for id in ids { await receive.deleteRequest(id) } }
                        },
                    ]
                })
            .frame(minHeight: 160, maxHeight: 260)
            .clipShape(RoundedRectangle(cornerRadius: DashRadius.group, style: .continuous))
            .dashCard(radius: DashRadius.group, padding: nil)
            .accessibilityIdentifier("receive.requests")
        }
    }
}
#endif
