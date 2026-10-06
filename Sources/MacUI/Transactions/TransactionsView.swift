// Transactions (QT-086…094, IOS-027…031): dash-qt filter row, sortable
// table, selected-amount footer, details sheet and CSV export.
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

struct TransactionsView: View {
    let transactions: TransactionsViewModel
    let unitName: String
    /// Amount with unit in the display unit (selected total).
    let formatAmount: (Amount) -> String
    @State private var selection: Set<TxRecord.ID> = []
    @State private var sortOrder: DataTableSortOrder?
    @State private var showsDetail = false
    @State private var editingLabel: String?
    @State private var exportMessage: String?

    var body: some View {
        VStack(spacing: 0) {
            TransactionFilterBar(transactions: transactions)
            Divider()
            if let error = transactions.errorMessage {
                SystemNotice(text: error, tone: .error).padding(DashSpacing.m)
            }
            DataTable(
                rows: transactions.rows, columns: columns, selection: $selection, sortOrder: $sortOrder,
                emptyText: MacStrings.Transactions.empty,
                onActivate: { id in
                    Task {
                        await transactions.select(id)
                        showsDetail = transactions.detail != nil
                    }
                },
                contextMenu: contextMenu)
            .accessibilityIdentifier("transactions.table")
            Divider()
            footer
        }
        .accessibilityIdentifier("transactions")
        .onChange(of: selection) { old, new in syncSelection(old: old, new: new) }
        .onChange(of: transactions.selection) { _, new in if new != selection { selection = new } }
        .sheet(isPresented: $showsDetail) {
            if let detail = transactions.detail {
                TransactionDetailView(
                    detail: detail, transactions: transactions, formatAmount: formatAmount,
                    onClose: { showsDetail = false })
            }
        }
        .alert(
            MacStrings.Transactions.label,
            isPresented: Binding(get: { editingLabel != nil }, set: { if !$0 { editingLabel = nil } })
        ) {
            TextField(MacStrings.Transactions.label, text: Binding(get: { editingLabel ?? "" }, set: { editingLabel = $0 }))
            Button(MacStrings.Common.save) {
                if let txid = transactions.selection.first?.txid {
                    let label = editingLabel
                    Task { await transactions.setLabel(label, txid: txid) }
                }
                editingLabel = nil
            }
            Button(MacStrings.Common.cancel, role: .cancel) { editingLabel = nil }
        }
    }

    private var columns: [DataTableColumn<TxRecord>] {
        [
            DataTableColumn("", id: "status", width: .fixed(28), alignment: .center) { record in
                TransactionStatusIcon(status: record.status)
                    .help(transactions.statusText(for: record))
            },
            DataTableColumn(
                MacStrings.Transactions.date, id: "date", width: .fixed(150),
                sortBy: { ($0.date ?? .distantPast) < ($1.date ?? .distantPast) }
            ) { record in
                Text(record.date?.formatted(date: .numeric, time: .shortened) ?? "")
                    .dashFont(.footnote)
            },
            DataTableColumn(
                MacStrings.Transactions.type, id: "type", width: .fixed(150),
                sortBy: { transactions.typeText(for: $0) < transactions.typeText(for: $1) }
            ) { record in
                Text(transactions.typeText(for: record))
                    .dashFont(.footnote)
                    .accessibilityIdentifier("transactions.type")
            },
            DataTableColumn(MacStrings.Transactions.addressLabel, id: "address", width: .flexible(min: 200)) {
                transactions.addressText(for: $0)
            },
            DataTableColumn(
                "\(MacStrings.Transactions.amount) (\(unitName))", id: "amount", width: .fixed(190),
                alignment: .trailing, sortBy: { $0.amount < $1.amount }
            ) { record in
                Text(transactions.amountText(for: record))
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundStyle(record.amount.duffs < 0 ? Color.dash.primaryText : Color.dash.successText)
                    .accessibilityIdentifier("transactions.amount")
            },
        ]
    }

    private func contextMenu(_ ids: Set<TxRecord.ID>) -> [DataTableMenuAction] {
        let record = ids.count == 1 ? transactions.rows.first { ids.contains($0.id) } : nil
        return [
            DataTableMenuAction(title: MacStrings.Transactions.copyAddress, isEnabled: record?.address != nil) {
                if let address = record?.address { MacPasteboard.copy(address) }
            },
            DataTableMenuAction(title: MacStrings.Transactions.copyTxid, isEnabled: record != nil) {
                if let txid = record?.id.txid { MacPasteboard.copy(txid) }
            },
            DataTableMenuAction(title: MacStrings.Transactions.editLabel, isEnabled: record != nil) {
                editingLabel = record?.label ?? ""
            },
            DataTableMenuAction(title: MacStrings.Transactions.showDetails, isEnabled: record != nil) {
                guard let record else { return }
                Task {
                    await transactions.select(record.id)
                    showsDetail = transactions.detail != nil
                }
            },
        ]
    }

    private var footer: some View {
        HStack(spacing: DashSpacing.m) {
            if let total = transactions.totalMatching {
                Text(MacStrings.Transactions.matching(total))
                    .accessibilityIdentifier("transactions.count")
            }
            if let selected = transactions.selectedTotal, transactions.selection.count > 1 {
                Text("\(L10n.Transactions.selectedAmount) \(formatAmount(selected))")
                    .accessibilityIdentifier("transactions.selectedAmount")
            }
            Spacer()
            if let exportMessage {
                Text(exportMessage).foregroundStyle(Color.dash.secondaryText)
            }
            if transactions.hasMore {
                Button(MacStrings.Transactions.loadMore) { Task { await transactions.loadMore() } }
            }
            Button(MacStrings.Common.export, systemImage: "square.and.arrow.up") { Task { await export() } }
                .accessibilityIdentifier("transactions.export")
        }
        .dashFont(.footnote)
        .padding(.horizontal, DashSpacing.l)
        .padding(.vertical, DashSpacing.s)
        .background(Color.dash.secondaryBackground)
    }

    /// Keeps the view model's selection (selected total, detail) in step with the table.
    private func syncSelection(old: Set<TxRecord.ID>, new: Set<TxRecord.ID>) {
        guard new != transactions.selection else { return }
        if new.count == 1, let id = new.first {
            Task { await transactions.select(id) }
        } else {
            for id in new.symmetricDifference(transactions.selection) { transactions.toggleSelection(id) }
        }
    }

    private func export() async {
        exportMessage = nil
        do {
            let csv = try await transactions.exportCSV()
            switch await MacSavePanel.save(
                text: csv, suggestedName: MacStrings.Transactions.exportName, title: L10n.Transactions.exportTitle)
            {
            case .saved: exportMessage = MacStrings.Common.exportSaved
            case .cancelled: break
            case .failed: exportMessage = L10n.Transactions.exportFailed
            }
        } catch {
            exportMessage = L10n.Transactions.exportFailed
        }
    }
}

/// Date, type, search, minimum amount and watch-only filters (QT-089).
private struct TransactionFilterBar: View {
    let transactions: TransactionsViewModel
    @State private var search = ""
    @State private var minimum = ""
    @State private var from = Calendar.current.date(byAdding: .month, value: -1, to: Date()) ?? Date()
    @State private var until = Date()

    var body: some View {
        VStack(spacing: DashSpacing.s) {
            HStack(spacing: DashSpacing.m) {
                Picker(MacStrings.Transactions.date, selection: Binding(
                    get: { transactions.datePreset },
                    set: { preset in
                        Task {
                            if preset == .range {
                                await transactions.setRange(from: from, until: until)
                            } else {
                                await transactions.setDatePreset(preset)
                            }
                        }
                    }
                )) {
                    ForEach(DateFilterPreset.allCases, id: \.self) { preset in
                        Text(L10n.Transactions.dateFilterName(preset)).tag(preset)
                    }
                }
                .frame(width: 170)
                .accessibilityIdentifier("transactions.dateFilter")
                Picker(MacStrings.Transactions.type, selection: Binding(
                    get: { transactions.typePreset },
                    set: { preset in Task { await transactions.setTypePreset(preset) } }
                )) {
                    ForEach(transactions.typeMenu, id: \.self) { preset in
                        Text(L10n.Transactions.typeFilterName(preset)).tag(preset)
                    }
                }
                .frame(width: 230)
                .accessibilityIdentifier("transactions.typeFilter")
                TextField(L10n.Transactions.searchPlaceholder, text: $search)
                    .textFieldStyle(.roundedBorder)
                    .onChange(of: search) { _, text in transactions.setSearchText(text) }
                    .accessibilityIdentifier("transactions.search")
                TextField(L10n.Transactions.minAmountPlaceholder, text: $minimum)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 110)
                    .onSubmit { Task { await transactions.setMinimumAmountText(minimum) } }
                    .accessibilityIdentifier("transactions.minAmount")
                if transactions.showsWatchOnly {
                    Picker(MacStrings.Transactions.watchOnly, selection: Binding(
                        get: { transactions.watchOnly },
                        set: { filter in Task { await transactions.setWatchOnly(filter) } }
                    )) {
                        Text(MacStrings.Transactions.watchOnlyAll).tag(WatchOnlyFilter.all)
                        Text(MacStrings.Transactions.watchOnlyYes).tag(WatchOnlyFilter.yes)
                        Text(MacStrings.Transactions.watchOnlyNo).tag(WatchOnlyFilter.no)
                    }
                    .frame(width: 140)
                }
            }
            if transactions.datePreset == .range {
                HStack(spacing: DashSpacing.m) {
                    DatePicker(MacStrings.Transactions.from, selection: $from, displayedComponents: .date)
                    DatePicker(MacStrings.Transactions.to, selection: $until, displayedComponents: .date)
                    Spacer()
                }
                .onChange(of: from) { _, value in Task { await transactions.setRange(from: value, until: until) } }
                .onChange(of: until) { _, value in Task { await transactions.setRange(from: from, until: value) } }
            }
            if let error = transactions.minimumAmountError {
                Text(error)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.errorText)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(.horizontal, DashSpacing.l)
        .padding(.vertical, DashSpacing.m)
        .onAppear {
            search = transactions.searchText
            minimum = transactions.minimumAmountText
            if let start = transactions.rangeFrom { from = start }
            if let end = transactions.rangeUntil { until = end }
        }
    }
}

/// dash-qt status column icon (QT-087).
struct TransactionStatusIcon: View {
    let status: TxStatus

    var body: some View {
        Image(systemName: symbol)
            .foregroundStyle(color)
            .accessibilityLabel(L10n.Transactions.statusText(status))
    }

    private var symbol: String {
        switch status.kind {
        case .confirmed: status.chainLocked ? "lock.fill" : "checkmark.circle.fill"
        case .confirming, .unconfirmed: status.instantLocked ? "bolt.fill" : "clock"
        case .conflicted, .notAccepted: "exclamationmark.triangle.fill"
        case .abandoned: "xmark.circle"
        case .immature: "hourglass"
        }
    }

    private var color: Color {
        switch status.kind {
        case .confirmed: Color.dash.successText
        case .confirming, .unconfirmed: status.instantLocked ? Color.dash.blueText : Color.dash.secondaryText
        case .conflicted, .notAccepted, .abandoned: Color.dash.errorText
        case .immature: Color.dash.orange
        }
    }
}
#endif
