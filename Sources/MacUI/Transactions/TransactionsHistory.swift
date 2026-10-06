// The Transactions page's iOS history (IOS-027…030: category chips, day
// groups, one CoinJoin mixing row per day) and the dash-qt action dialogs
// (QT-075, QT-091: abandon question, action results).
#if os(macOS)
import DashUIMac
import DesignTokens
import SwiftUI
import WalletFeatures
import WalletRuntime

/// The page's two presentations.
enum TransactionsLayout: String, Hashable, CaseIterable {
    case table, history
}

/// Sent / Received / Rewards / Masternode chips with All and Only.
struct HistoryChips: View {
    let transactions: TransactionsViewModel

    var body: some View {
        HStack(spacing: DashSpacing.s) {
            Button(L10n.TransactionsM2.all) { Task { await transactions.selectAllChips() } }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("transactions.chip.all")
            ForEach(transactions.offeredChips, id: \.self) { chip in
                Toggle(chip.title, isOn: Binding(
                    get: { transactions.selectedChips.contains(chip) },
                    set: { _ in Task { await transactions.toggleChip(chip) } }))
                .toggleStyle(.button)
                .contextMenu {
                    Button(L10n.TransactionsM2.only) { Task { await transactions.selectOnlyChip(chip) } }
                }
                .accessibilityIdentifier("transactions.chip.\(chip.rawValue)")
            }
            Spacer()
        }
    }
}

/// Day groups, newest first; a day's CoinJoin records are one row.
struct HistoryList: View {
    let transactions: TransactionsViewModel
    let formatAmount: (Amount) -> String
    let onOpen: (TxRecord.ID) -> Void
    let contextMenu: (Set<TxRecord.ID>) -> [DataTableMenuAction]

    var body: some View {
        List {
            ForEach(transactions.dayGroups) { group in
                Section(group.title) {
                    ForEach(group.items) { item in
                        switch item {
                        case .record(let record):
                            Button { onOpen(record.id) } label: { recordRow(record) }
                                .buttonStyle(.plain)
                                .contextMenu {
                                    ForEach(contextMenu([record.id])) { action in
                                        Button(action.title, action: action.action).disabled(!action.isEnabled)
                                    }
                                }
                        case .coinJoinMixing(let row):
                            mixingRow(row)
                        }
                    }
                }
            }
            if transactions.rows.isEmpty {
                Text(MacStrings.Transactions.empty).foregroundStyle(Color.dash.secondaryText)
            }
        }
        .listStyle(.inset)
        .accessibilityIdentifier("transactions.history")
    }

    private func recordRow(_ record: TxRecord) -> some View {
        HStack(spacing: DashSpacing.m) {
            TransactionStatusIcon(status: record.status)
            VStack(alignment: .leading, spacing: DashSpacing.xxxs) {
                Text(transactions.typeText(for: record)).dashFont(.subheadMedium)
                Text(transactions.addressText(for: record))
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.secondaryText)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer()
            VStack(alignment: .trailing, spacing: DashSpacing.xxxs) {
                Text(transactions.amountText(for: record))
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundStyle(record.amount.duffs < 0 ? Color.dash.primaryText : Color.dash.successText)
                Text(record.date?.formatted(date: .omitted, time: .shortened) ?? "")
                    .dashFont(.caption1)
                    .foregroundStyle(Color.dash.secondaryText)
            }
        }
        .contentShape(Rectangle())
        .padding(.vertical, DashSpacing.xxxs)
    }

    private func mixingRow(_ row: CoinJoinDayRow) -> some View {
        HStack(spacing: DashSpacing.m) {
            Image(systemName: "shuffle").foregroundStyle(Color.dash.blueText).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: DashSpacing.xxxs) {
                Text(row.title).dashFont(.subheadMedium)
                Text(MacStrings.Transactions.mixingCount(row.records.count))
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.secondaryText)
            }
            Spacer()
            Text(formatAmount(row.total)).font(.system(.footnote, design: .monospaced))
        }
        .padding(.vertical, DashSpacing.xxxs)
        .accessibilityIdentifier("transactions.mixingRow")
    }
}

/// The abandon question (with the SPV may-still-confirm warning) and the
/// result of abandon, resend and unlock dust.
struct TransactionActionAlerts: ViewModifier {
    let transactions: TransactionsViewModel

    func body(content: Content) -> some View {
        content
            .alert(L10n.TransactionsM2.abandonTitle, isPresented: Binding(
                get: { if case .confirmingAbandon = transactions.actionState { true } else { false } },
                set: { if !$0 { transactions.dismissAction() } }
            )) {
                Button(MacStrings.Common.cancel, role: .cancel) { transactions.dismissAction() }
                Button(L10n.TransactionsM2.abandon, role: .destructive) { Task { await transactions.confirmAbandon() } }
            } message: {
                if case .confirmingAbandon(_, let mayStillConfirm) = transactions.actionState {
                    Text(mayStillConfirm
                        ? "\(L10n.TransactionsM2.abandonQuestion)\n\n\(L10n.TransactionsM2.abandonMayConfirm)"
                        : L10n.TransactionsM2.abandonQuestion)
                }
            }
            .alert(MacStrings.Transactions.actionResult, isPresented: Binding(
                get: {
                    switch transactions.actionState {
                    case .done, .failed: true
                    default: false
                    }
                },
                set: { if !$0 { transactions.dismissAction() } }
            )) {
                Button(MacStrings.Common.ok) { transactions.dismissAction() }
            } message: {
                switch transactions.actionState {
                case .done(let text), .failed(let text): Text(text)
                default: Text("")
                }
            }
    }
}
#endif
