// Transaction details sheet (QT-091, QT-092, QT-094, IOS-031, IOS-032):
// dash-qt's details fields (status without "not in memory pool" guesses on
// SPV), label editing, inputs and outputs, the raw transaction with copy,
// abandon / resend / unlock dust, and block-explorer links.
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

struct TransactionDetailView: View {
    let detail: TransactionDetail
    let transactions: TransactionsViewModel
    let formatAmount: (Amount) -> String
    let onClose: () -> Void
    @State private var label = ""
    @State private var toast: ToastMessage?
    @Environment(\.openURL) private var openURL

    /// What the transaction changed the balance by (all its records).
    private var net: Amount { Amount(duffs: detail.records.reduce(0) { $0 + $1.amount.duffs }) }
    private var first: TxRecord? { detail.records.first }

    var body: some View {
        SheetScaffold(title: MacStrings.Transactions.details, width: DashLayout.sheetWidthMedium, onClose: onClose) {
            ScrollView {
                VStack(alignment: .leading, spacing: DashSpacing.l) {
                    summary
                    MenuCard {
                        ForEach(transactions.detailFields) { field in
                            if field.title == L10n.TransactionsM2.transactionID {
                                CopyRow(
                                    field.title, value: field.value, isTechnical: true, copyLabel: L10n.TransactionsM2.copyTransactionID,
                                    onCopied: copied)
                                .accessibilityIdentifier("transactionDetail.field.\(field.title)")
                            } else {
                                DetailRow(field.title, value: field.value)
                                    .accessibilityIdentifier("transactionDetail.field.\(field.title)")
                            }
                        }
                        if let height = detail.blockHeight {
                            DetailRow(MacStrings.Transactions.block, value: "\(height)")
                        }
                    }
                    MenuCard(title: MacStrings.Transactions.label) {
                        HStack(spacing: DashSpacing.s) {
                            TextField(MacStrings.Transactions.label, text: $label)
                                .textFieldStyle(.dash)
                                .labelsHidden()
                                .accessibilityIdentifier("transactionDetail.label")
                            Button(MacStrings.Common.save) {
                                let text = label
                                Task { await transactions.setLabel(text, txid: detail.txid) }
                            }
                            .buttonStyle(.dash(.tintedBlue, .medium))
                            .disabled(label == (detail.label ?? ""))
                        }
                        .padding(DashSpacing.xs)
                    }
                    actions
                    disclosures
                }
                .padding(.top, DashSpacing.xxs)
            }
            .frame(height: 560)
        } footer: {
            Button(MacStrings.Common.close, action: onClose)
                .buttonStyle(.dash(.filledBlue, .large, fillsWidth: true))
                .keyboardShortcut(.cancelAction)
                .accessibilityIdentifier("transactionDetail.close")
        }
        .dashToast($toast)
        .onAppear { label = detail.label ?? "" }
        .modifier(TransactionActionAlerts(transactions: transactions))
        .accessibilityIdentifier("transactionDetail")
    }

    private func copied() {
        toast = ToastMessage(.copied, L10n.UX.copied)
    }

    /// iOS transaction header: the direction icon, the type title, the amount
    /// at full precision, the status chip and the date.
    private var summary: some View {
        let kind = first.map { TxPresentation.icon(type: $0.type, status: detail.status, amount: net) } ?? .received
        return VStack(spacing: DashSpacing.s) {
            DashIconImage(.token(Self.detailIcon(kind)))
                .scaledToFit()
                .frame(width: 50, height: 50)
                .accessibilityHidden(true)
            Text(first.map { TxPresentation.typeTitle($0.type, category: $0.category, amount: net) } ?? "")
                .dashFont(.title3)
                .foregroundStyle(Color.role.textPrimary)
            AmountText(formatted: formatAmount(net), size: DesignTokens.DashTextStyle.title1.size, weight: .bold)
                .foregroundStyle(Color.role.textPrimary)
            HStack(spacing: DashSpacing.s) {
                HStack(spacing: DashSpacing.xxs) {
                    TransactionStatusIcon(status: detail.status)
                    Text(L10n.Transactions.statusText(detail.status))
                }
                .dashFont(.caption1Medium)
                .foregroundStyle(TxPresentation.chipIsProblem(detail.status) ? Color.role.danger : Color.role.textLink)
                .padding(.horizontal, DashSpacing.s)
                .padding(.vertical, DashSpacing.xxxs)
                .background(RoundedRectangle(cornerRadius: DashRadius.switcher, style: .continuous)
                    .fill(TxPresentation.chipIsProblem(detail.status) ? Color.role.dangerTint : Color.role.accentTint))
                if let date = detail.date {
                    Text(date.formatted(date: .abbreviated, time: .shortened))
                        .dashFont(.footnote)
                        .foregroundStyle(Color.role.textSecondary)
                }
            }
        }
        .frame(maxWidth: .infinity)
    }

    static func detailIcon(_ kind: TxIconKind) -> DashIconToken {
        switch kind {
        case .error: .txDetailError
        case .sent: .txDetailSent
        default: .txDetailReceived
        }
    }

    /// Abandon / resend / unlock dust when they apply, and the explorer links.
    @ViewBuilder
    private var actions: some View {
        let links = transactions.explorerLinks(for: detail.txid) + transactions.thirdPartyLinks(for: detail.txid)
        HStack(spacing: DashSpacing.s) {
            if transactions.canAbandon {
                Button(L10n.TransactionsM2.abandon) { transactions.requestAbandon() }
                    .buttonStyle(.dash(.plainRed, .small))
                    .accessibilityIdentifier("transactionDetail.abandon")
            }
            if transactions.canResend {
                Button(L10n.TransactionsM2.resend) { Task { await transactions.resend() } }
                    .buttonStyle(.dash(.tintedBlue, .small))
                    .accessibilityIdentifier("transactionDetail.resend")
            }
            if transactions.canUnlockDust {
                Button(L10n.TransactionsM2.unlockDust) { Task { await transactions.unlockDust() } }
                    .buttonStyle(.dash(.tintedBlue, .small))
            }
            Spacer(minLength: 0)
            ForEach(links) { link in
                Button { openURL(link.url) } label: {
                    Label(link.title, systemImage: "arrow.up.right.square")
                }
                .buttonStyle(.dash(.plainBlue, .small))
            }
        }
    }

    /// Inputs, outputs and the raw transaction: technical values, collapsed.
    private var disclosures: some View {
        VStack(alignment: .leading, spacing: DashSpacing.s) {
            DisclosureGroup("\(MacStrings.Transactions.inputs) (\(detail.inputs.count))") {
                VStack(spacing: 0) {
                    ForEach(Array(detail.inputs.enumerated()), id: \.offset) { _, input in
                        EndpointRow(
                            address: input.address ?? "\(input.previousOutput.txid.prefix(16))…:\(input.previousOutput.vout)",
                            amount: input.amount.map(formatAmount), isMine: input.isMine, isChange: false)
                    }
                }
                .dashCard(radius: DashRadius.standard, padding: DashSpacing.s, elevation: nil, fill: Color.role.cardRaised)
            }
            DisclosureGroup("\(MacStrings.Transactions.outputs) (\(detail.outputs.count))") {
                VStack(spacing: 0) {
                    ForEach(Array(detail.outputs.enumerated()), id: \.offset) { _, output in
                        EndpointRow(
                            address: output.address ?? output.dataHex.map { "OP_RETURN \($0)" } ?? "",
                            amount: formatAmount(output.amount), isMine: output.isMine, isChange: output.isChange)
                    }
                }
                .dashCard(radius: DashRadius.standard, padding: DashSpacing.s, elevation: nil, fill: Color.role.cardRaised)
            }
            DisclosureGroup(MacStrings.Transactions.rawHex) {
                VStack(alignment: .leading, spacing: DashSpacing.s) {
                    Text(detail.rawHex)
                        .font(.system(size: DesignTokens.DashTextStyle.caption1.size, design: .monospaced))
                        .foregroundStyle(Color.role.textPrimary)
                        .textSelection(.enabled)
                        .lineLimit(6)
                    Button(L10n.TransactionsM2.copyRawTransaction) {
                        if let hex = transactions.copyRawTransaction() {
                            MacPasteboard.copy(hex)
                            copied()
                        }
                    }
                    .buttonStyle(.dash(.tintedGray, .small))
                    .accessibilityIdentifier("transactionDetail.copyRaw")
                }
                .dashCard(radius: DashRadius.standard, padding: DashSpacing.s, elevation: nil, fill: Color.role.cardRaised)
            }
        }
        .dashFont(.subheadMedium)
        .foregroundStyle(Color.role.textPrimary)
    }
}

/// An input or output: the address (proportional, middle-truncated, full
/// value in the tooltip), own/change badge, amount with tabular digits.
private struct EndpointRow: View {
    let address: String
    let amount: String?
    let isMine: Bool
    let isChange: Bool

    var body: some View {
        HStack {
            Text(address)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textPrimary)
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
                .help(address)
            if isMine { Badge(isChange ? MacStrings.Transactions.change : MacStrings.Transactions.mine, tone: .info) }
            Spacer()
            if let amount {
                AmountText(formatted: amount)
                    .foregroundStyle(Color.role.textPrimary)
            }
        }
        .padding(.vertical, DashSpacing.xxs)
    }
}
#endif
