// The selected transaction: dash-qt's details fields and context-menu
// actions (copy, abandon, resend, unlock dust, explorer links; QT-075,
// QT-090…094, IOS-032), and the day-grouped history (IOS-027, IOS-030).
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

/// Details in dash-qt's order with label editing and the actions.
struct TransactionDetailCard: View {
    let model: TransactionsViewModel
    let state: CrossAppState
    let detail: TransactionDetail
    let labelText: Binding<String>

    @Environment(\.openURL) var openURL

    var body: some View {
        let model = model
        let state = state
        let detail = detail
        let labelText = labelText
        let record = model.rows.first { model.selection.contains($0.id) } ?? detail.records.first
        DashCard {
            HStack {
                SectionHeader(L10n.TransactionsM2.detailsTitle(String(detail.txid.prefix(16)) + "…"), style: .subheadMedium)
                Spacer()
                DashButton(CrossStrings.close, style: .plainBlue, size: .small) { model.clearDetail() }
            }
            ForEach(model.detailFields) { field in
                KeyValueRow(field.title, field.value)
            }
            HStack(spacing: Int(DashSpacing.s)) {
                Menu(CrossStrings.copyMenu) {
                    Button(L10n.TransactionsM2.copyAddress) {
                        state.copy(record.map { model.copyAddress($0) } ?? "", what: CrossStrings.addressWord)
                    }
                    Button(L10n.TransactionsM2.copyLabel) {
                        state.copy(record.map { model.copyLabel($0) } ?? "", what: CrossStrings.labelWord)
                    }
                    Button(L10n.TransactionsM2.copyAmount) {
                        state.copy(record.map { model.copyAmount($0) } ?? "", what: CrossStrings.amountWord)
                    }
                    Button(L10n.TransactionsM2.copyTransactionID) {
                        state.copy(detail.txid, what: CrossStrings.transactionID)
                    }
                    Button(L10n.TransactionsM2.copyRawTransaction) {
                        state.copy(model.copyRawTransaction() ?? "", what: CrossStrings.rawTransactionWord)
                    }
                    Button(L10n.TransactionsM2.copyFullDetails) {
                        state.copy(record.map { model.copyFullDetails($0) } ?? "", what: CrossStrings.detailsWord)
                    }
                }
                DashButton(L10n.TransactionsM2.abandon, style: .plainRed, size: .small, isEnabled: model.canAbandon) {
                    model.requestAbandon()
                }
                DashButton(L10n.TransactionsM2.resend, style: .plainBlue, size: .small, isEnabled: model.canResend) {
                    Task { await model.resend() }
                }
                if model.canUnlockDust {
                    DashButton(L10n.TransactionsM2.unlockDust, style: .plainBlue, size: .small) {
                        Task { await model.unlockDust() }
                    }
                }
            }
            let links = model.thirdPartyLinks(for: detail.txid) + model.explorerLinks(for: detail.txid)
            if !links.isEmpty {
                HStack(spacing: Int(DashSpacing.s)) {
                    ForEach(links) { link in
                        DashButton(link.title, style: .plainBlue, size: .small) { openURL(link.url) }
                    }
                }
            }
            HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                DashTextField(CrossStrings.label, placeholder: CrossStrings.labelPlaceholder, text: labelText)
                DashButton(CrossStrings.saveLabel, style: .tintedBlue, size: .small) {
                    let label = labelText.wrappedValue
                    Task { await model.setLabel(label, txid: detail.txid) }
                }
            }
        }
        .task(id: detail.txid) {
            labelText.wrappedValue = detail.label ?? ""
        }
    }
}

/// Abandon's question (with the SPV warning when the mempool is unknown)
/// and the outcome of an action.
struct TransactionActionBanner: View {
    let model: TransactionsViewModel

    var body: some View {
        let model = model
        switch model.actionState {
        case .idle:
            EmptyView()
        case .confirmingAbandon(_, let mayStillConfirm):
            ConfirmationCard(
                title: L10n.TransactionsM2.abandonTitle,
                message: L10n.TransactionsM2.abandonQuestion
                    + (mayStillConfirm ? "\n\n" + L10n.TransactionsM2.abandonMayConfirm : ""),
                confirmTitle: L10n.TransactionsM2.abandon, destructive: true,
                onConfirm: { Task { await model.confirmAbandon() } }, onCancel: { model.dismissAction() })
        case .working:
            Text(CrossStrings.working).dashFont(.footnote).dashForeground(.secondaryText)
        case .done(let text):
            Toast(text, kind: .success, actionTitle: CrossStrings.dismiss) { model.dismissAction() }
        case .failed(let text):
            Toast(text, kind: .error, actionTitle: CrossStrings.dismiss) { model.dismissAction() }
        }
    }
}

/// The loaded rows by day, newest first, with each day's CoinJoin internal
/// records as one "Mixing Transactions" row. Rows open the details.
struct DayGroupList: View {
    let model: TransactionsViewModel
    let state: CrossAppState

    var body: some View {
        let model = model
        let state = state
        ScrollView {
            VStack(alignment: .leading, spacing: Int(DashSpacing.s)) {
                if model.rows.isEmpty {
                    Text(model.isLoading ? CrossStrings.loading : CrossStrings.noTransactions)
                        .dashFont(.footnote).dashForeground(.secondaryText)
                }
                ForEach(model.dayGroups) { group in
                    SectionHeader(group.title, style: .footnoteMedium)
                    ForEach(group.items) { item in
                        switch item {
                        case .record(let record):
                            HStack(spacing: Int(DashSpacing.s)) {
                                TransactionView(
                                    direction: Format.direction(record.category, amount: record.amount),
                                    title: "\(model.typeText(for: record))  \(model.addressText(for: record))",
                                    subtitle: "\(Format.date(record.date))  \(model.statusText(for: record))",
                                    amount: model.amountText(for: record))
                                DashButton(CrossStrings.show, style: .plainBlue, size: .small) {
                                    Task { await model.select(record.id) }
                                }
                            }
                        case .coinJoinMixing(let row):
                            MenuItem(
                                title: row.title, subtitle: L10n.TransactionsM2.mixingCount(row.records.count),
                                trailing: state.format(row.total))
                        }
                    }
                }
            }
        }
        .frame(height: 360)
    }
}
