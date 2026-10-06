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

/// Sent / Received / Rewards / Masternode chips with All and Only, as iOS
/// filter capsules: selected chips on the accent tint.
struct HistoryChips: View {
    let transactions: TransactionsViewModel

    var body: some View {
        HStack(spacing: DashSpacing.s) {
            chip(L10n.TransactionsM2.all, isOn: Set(transactions.offeredChips).isSubset(of: transactions.selectedChips)) {
                Task { await transactions.selectAllChips() }
            }
            .accessibilityIdentifier("transactions.chip.all")
            ForEach(transactions.offeredChips, id: \.self) { item in
                chip(item.title, isOn: transactions.selectedChips.contains(item)) {
                    Task { await transactions.toggleChip(item) }
                }
                .contextMenu {
                    Button(L10n.TransactionsM2.only) { Task { await transactions.selectOnlyChip(item) } }
                }
                .accessibilityIdentifier("transactions.chip.\(item.rawValue)")
            }
            Spacer()
        }
    }

    private func chip(_ title: String, isOn: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .dashFont(.footnoteMedium)
                .foregroundStyle(isOn ? Color.role.textLink : Color.role.textSecondary)
                .padding(.horizontal, DashSpacing.m)
                .padding(.vertical, DashSpacing.xxs + 1)
                .background(Capsule().fill(isOn ? Color.role.accentTint : Color.role.neutralTint))
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

/// Day cards of iOS rows, newest first; a day's CoinJoin records are one
/// "Mixing Transactions" row (UX-SPEC §4.9 list mode).
struct HistoryList: View {
    let transactions: TransactionsViewModel
    let unit: DisplayUnit
    let unitName: String
    let formatAmount: (Amount) -> String
    let selection: Set<TxRecord.ID>
    let onOpen: (TxRecord.ID) -> Void
    let contextMenu: (Set<TxRecord.ID>) -> [DataTableMenuAction]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DashSpacing.m) {
                ForEach(transactions.dayGroups) { group in
                    TransactionGroupCard(day: group.title, weekday: group.day.map { HistoryDay.weekday($0) }) {
                        ForEach(group.items) { item in
                            switch item {
                            case .record(let record):
                                DashTransactionRow(
                                    record: record, unit: unit, unitName: unitName,
                                    isSelected: selection.contains(record.id), action: { onOpen(record.id) })
                                .contextMenu {
                                    ForEach(contextMenu([record.id])) { action in
                                        Button(action.title, action: action.action).disabled(!action.isEnabled)
                                    }
                                }
                                .accessibilityIdentifier("transactions.row")
                            case .coinJoinMixing(let row):
                                mixingRow(row)
                            }
                        }
                    }
                }
                if transactions.rows.isEmpty {
                    EmptyState(icon: .token(.txAll), title: L10n.UX.noTransactions)
                        .dashCard(padding: nil)
                }
            }
            .frame(maxWidth: DashLayout.contentMaxWidth)
            .padding(.horizontal, DashLayout.pagePaddingH)
            .padding(.bottom, DashSpacing.l)
            .frame(maxWidth: .infinity)
        }
        .accessibilityIdentifier("transactions.history")
    }

    private func mixingRow(_ row: CoinJoinDayRow) -> some View {
        DashTransactionRow(
            icon: .token(.txMixing), title: row.title, subtitle: nil,
            topText: L10n.TransactionsM2.mixingCount(row.records.count),
            amount: CompactAmount.format(row.total, unit: unit, signed: false),
            unit: AmountUnitDisplay(unit: unit, name: unitName), amountHelp: formatAmount(row.total), isInternal: true)
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
