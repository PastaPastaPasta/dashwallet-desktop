// Transaction details sheet (QT-092, IOS-031): status, amounts, inputs,
// outputs, label editing and the raw transaction.
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
                        LabeledContent(MacStrings.Transactions.status) {
                            HStack {
                                TransactionStatusIcon(status: detail.status)
                                Text(L10n.Transactions.statusText(detail.status))
                            }
                        }
                        if let date = detail.date {
                            LabeledContent(MacStrings.Transactions.date, value: date.formatted(date: .long, time: .standard))
                        }
                        ForEach(detail.records) { record in
                            LabeledContent(transactions.typeText(for: record)) {
                                Text(transactions.amountText(for: record))
                                    .font(.system(.body, design: .monospaced))
                            }
                        }
                        if let fee = detail.fee {
                            LabeledContent(MacStrings.Transactions.fee) {
                                Text(formatAmount(fee))
                            }
                        }
                        LabeledContent(MacStrings.Transactions.size, value: MacStrings.Transactions.bytes(detail.sizeBytes))
                        LabeledContent(MacStrings.Transactions.txid) {
                            HStack {
                                Text(detail.txid)
                                    .font(.system(.footnote, design: .monospaced))
                                    .textSelection(.enabled)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                                Button {
                                    MacPasteboard.copy(detail.txid)
                                } label: {
                                    Image(systemName: "doc.on.doc")
                                }
                                .buttonStyle(.borderless)
                                .accessibilityLabel(MacStrings.Transactions.copyTxid)
                            }
                        }
                        if let height = detail.blockHeight {
                            LabeledContent(MacStrings.Transactions.block, value: "\(height)")
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
                    Section(MacStrings.Transactions.rawHex) {
                        Text(detail.rawHex)
                            .font(.system(.caption, design: .monospaced))
                            .textSelection(.enabled)
                            .lineLimit(4)
                    }
                }
                .formStyle(.grouped)
            }
        }
        .frame(width: 620, height: 600)
        .onAppear { label = detail.label ?? "" }
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
