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
    @Environment(\.openURL) private var openURL

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(MacStrings.Transactions.details).dashFont(.title3)
                Spacer()
                Button(MacStrings.Common.close, action: onClose)
                    .keyboardShortcut(.cancelAction)
                    .accessibilityIdentifier("transactionDetail.close")
            }
            .padding(DashSpacing.xl)
            Divider()
            ScrollView {
                Form {
                    Section {
                        HStack {
                            TransactionStatusIcon(status: detail.status)
                            Text(L10n.Transactions.statusText(detail.status)).dashFont(.subheadMedium)
                        }
                        ForEach(transactions.detailFields) { field in
                            LabeledContent(field.title) {
                                Text(field.value)
                                    .textSelection(.enabled)
                                    .multilineTextAlignment(.trailing)
                                    .font(field.title == L10n.TransactionsM2.transactionID
                                        ? .system(.footnote, design: .monospaced) : .body)
                            }
                            .accessibilityIdentifier("transactionDetail.field.\(field.title)")
                        }
                        if let height = detail.blockHeight {
                            LabeledContent(MacStrings.Transactions.block, value: "\(height)")
                        }
                    }
                    Section {
                        HStack {
                            Button(L10n.TransactionsM2.abandon) { transactions.requestAbandon() }
                                .disabled(!transactions.canAbandon)
                                .accessibilityIdentifier("transactionDetail.abandon")
                            Button(L10n.TransactionsM2.resend) { Task { await transactions.resend() } }
                                .disabled(!transactions.canResend)
                                .accessibilityIdentifier("transactionDetail.resend")
                            if transactions.canUnlockDust {
                                Button(L10n.TransactionsM2.unlockDust) { Task { await transactions.unlockDust() } }
                            }
                            Spacer()
                            ForEach(transactions.explorerLinks(for: detail.txid) + transactions.thirdPartyLinks(for: detail.txid)) {
                                link in
                                Button(link.title) { openURL(link.url) }
                            }
                        }
                    }
                    Section(MacStrings.Transactions.label) {
                        HStack {
                            TextField(MacStrings.Transactions.label, text: $label)
                                .labelsHidden()
                                .accessibilityIdentifier("transactionDetail.label")
                            Button(MacStrings.Common.save) {
                                let text = label
                                Task { await transactions.setLabel(text, txid: detail.txid) }
                            }
                            .disabled(label == (detail.label ?? ""))
                        }
                    }
                    Section(MacStrings.Transactions.inputs) {
                        ForEach(Array(detail.inputs.enumerated()), id: \.offset) { _, input in
                            EndpointRow(
                                address: input.address ?? "\(input.previousOutput.txid.prefix(16))…:\(input.previousOutput.vout)",
                                amount: input.amount.map(formatAmount), isMine: input.isMine,
                                isChange: false)
                        }
                    }
                    Section(MacStrings.Transactions.outputs) {
                        ForEach(Array(detail.outputs.enumerated()), id: \.offset) { _, output in
                            EndpointRow(
                                address: output.address ?? output.dataHex.map { "OP_RETURN \($0)" } ?? "",
                                amount: formatAmount(output.amount), isMine: output.isMine,
                                isChange: output.isChange)
                        }
                    }
                    Section {
                        Text(detail.rawHex)
                            .font(.system(.caption, design: .monospaced))
                            .textSelection(.enabled)
                            .lineLimit(4)
                        Button(L10n.TransactionsM2.copyRawTransaction) {
                            if let hex = transactions.copyRawTransaction() { MacPasteboard.copy(hex) }
                        }
                        .accessibilityIdentifier("transactionDetail.copyRaw")
                    } header: {
                        Text(MacStrings.Transactions.rawHex)
                    }
                }
                .formStyle(.grouped)
            }
        }
        .frame(width: 620, height: 640)
        .onAppear { label = detail.label ?? "" }
        .modifier(TransactionActionAlerts(transactions: transactions))
        .accessibilityIdentifier("transactionDetail")
    }
}

private struct EndpointRow: View {
    let address: String
    let amount: String?
    let isMine: Bool
    let isChange: Bool

    var body: some View {
        HStack {
            Text(address)
                .font(.system(.footnote, design: .monospaced))
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
            if isMine { Badge(isChange ? MacStrings.Transactions.change : MacStrings.Transactions.mine, tone: .info) }
            Spacer()
            if let amount {
                Text(amount).font(.system(.footnote, design: .monospaced))
            }
        }
    }
}
#endif
