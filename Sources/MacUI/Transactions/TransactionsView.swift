// Transactions (QT-086…094, IOS-027…032): dash-qt filter row, sortable
// table with dash-qt's context menu (copy, abandon, resend, unlock dust,
// third-party links), selected-amount footer, details sheet and CSV export;
// or the iOS history: category chips and day groups with one CoinJoin
// mixing row per day.
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
    var unit: DisplayUnit = .dash
    @State private var selection: Set<TxRecord.ID> = []
    @State private var sortOrder: DataTableSortOrder?
    @State private var showsDetail = false
    @State private var editingLabel: String?
    @State private var exportMessage: String?
    /// The iOS day-grouped list (the default, IOS-027) or dash-qt's table.
    @State var layout: TransactionsLayout = .history
    @Environment(\.openURL) private var openURL

    var body: some View {
        VStack(spacing: 0) {
            header
            TransactionFilterBar(transactions: transactions, layout: layout)
            if let error = transactions.errorMessage {
                SystemNotice(text: error, tone: .error)
                    .padding(.horizontal, DashLayout.pagePaddingH)
                    .padding(.bottom, DashSpacing.m)
            }
            switch layout {
            case .table:
                DataTable(
                    rows: transactions.rows, columns: columns, selection: $selection, sortOrder: $sortOrder,
                    emptyText: MacStrings.Transactions.empty,
                    onActivate: { id in showDetail(id) },
                    contextMenu: contextMenu)
                .clipShape(RoundedRectangle(cornerRadius: DashRadius.group, style: .continuous))
                .dashCard(radius: DashRadius.group, padding: nil)
                .padding(.horizontal, DashLayout.pagePaddingH)
                .padding(.bottom, DashSpacing.m)
                .accessibilityIdentifier("transactions.table")
            case .history:
                HistoryList(
                    transactions: transactions, unit: unit, unitName: unitName, formatAmount: formatAmount,
                    selection: selection, onOpen: { id in showDetail(id) }, contextMenu: contextMenu)
            }
            footer
        }
        .dashCanvas()
        .accessibilityIdentifier("transactions")
        .task { await transactions.refreshChips() }
        .modifier(TransactionActionAlerts(transactions: transactions))
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

    /// Title, the List | Table switch and Export (UX-SPEC §4.9).
    private var header: some View {
        HStack(alignment: .center, spacing: DashSpacing.m) {
            PageTitle(title: L10n.Navigation.transactions)
            Spacer()
            DashSegmentedControl(
                [(TransactionsLayout.history, MacStrings.Transactions.listLayout),
                 (TransactionsLayout.table, MacStrings.Transactions.tableLayoutShort)],
                selection: $layout, segmentIdentifier: { "transactions.layout.\($0.rawValue)" })
            .help("\(MacStrings.Transactions.historyLayout) / \(MacStrings.Transactions.tableLayout)")
            .accessibilityIdentifier("transactions.layout")
            Button(MacStrings.Common.export, systemImage: "square.and.arrow.up") { Task { await export() } }
                .buttonStyle(.dash(.tintedBlue, .small))
                .accessibilityIdentifier("transactions.export")
        }
        .padding(.horizontal, DashLayout.pagePaddingH)
        .padding(.top, DashLayout.pagePaddingTop)
        .padding(.bottom, DashSpacing.m)
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
                    .monospacedDigit()
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
                // Tabular digits, full precision, never coloured by direction (UX-SPEC §5).
                Text(transactions.amountText(for: record))
                    .dashFont(.footnote)
                    .monospacedDigit()
                    .foregroundStyle(Color.role.textPrimary)
                    .accessibilityIdentifier("transactions.amount")
            },
        ]
    }

    private func showDetail(_ id: TxRecord.ID) {
        Task {
            await transactions.select(id)
            showsDetail = transactions.detail != nil
        }
    }

    /// dash-qt's transaction context menu (QT-090, QT-091, QT-094). Abandon,
    /// resend and unlock-dust follow the selected transaction's extras.
    private func contextMenu(_ ids: Set<TxRecord.ID>) -> [DataTableMenuAction] {
        let record = ids.count == 1 ? transactions.rows.first { ids.contains($0.id) } : nil
        let selectedOne = record != nil && transactions.selection == ids
        var actions = [
            DataTableMenuAction(title: L10n.TransactionsM2.copyAddress, isEnabled: record?.address != nil) {
                if let record { MacPasteboard.copy(transactions.copyAddress(record)) }
            },
            DataTableMenuAction(title: L10n.TransactionsM2.copyLabel, isEnabled: record?.label != nil) {
                if let record { MacPasteboard.copy(transactions.copyLabel(record)) }
            },
            DataTableMenuAction(title: L10n.TransactionsM2.copyAmount, isEnabled: record != nil) {
                if let record { MacPasteboard.copy(transactions.copyAmount(record)) }
            },
            DataTableMenuAction(title: L10n.TransactionsM2.copyTransactionID, isEnabled: record != nil) {
                if let record { MacPasteboard.copy(transactions.copyTransactionID(record)) }
            },
            DataTableMenuAction(title: L10n.TransactionsM2.copyRawTransaction, isEnabled: record != nil) {
                guard let record else { return }
                Task {
                    await transactions.select(record.id)
                    if let hex = transactions.copyRawTransaction() { MacPasteboard.copy(hex) }
                }
            },
            DataTableMenuAction(title: L10n.TransactionsM2.copyFullDetails, isEnabled: record != nil) {
                if let record { MacPasteboard.copy(transactions.copyFullDetails(record)) }
            },
            DataTableMenuAction(title: L10n.TransactionsM2.editLabel, isEnabled: record != nil) {
                editingLabel = record?.label ?? ""
            },
            DataTableMenuAction(title: L10n.TransactionsM2.showDetails, isEnabled: record != nil) {
                if let record { showDetail(record.id) }
            },
            DataTableMenuAction(
                title: L10n.TransactionsM2.abandon, isEnabled: selectedOne && transactions.canAbandon
            ) {
                transactions.requestAbandon()
            },
            DataTableMenuAction(
                title: L10n.TransactionsM2.resend, isEnabled: selectedOne && transactions.canResend
            ) {
                Task { await transactions.resend() }
            },
        ]
        if selectedOne && transactions.canUnlockDust {
            actions.append(DataTableMenuAction(title: L10n.TransactionsM2.unlockDust) {
                Task { await transactions.unlockDust() }
            })
        }
        if let record {
            for link in transactions.thirdPartyLinks(for: record.id.txid) {
                actions.append(DataTableMenuAction(id: link.url.absoluteString, title: link.title) { openURL(link.url) })
            }
        }
        return actions
    }

    private var footer: some View {
        HStack(spacing: DashSpacing.m) {
            if let total = transactions.totalMatching {
                Text(MacStrings.Transactions.matching(total))
                    .accessibilityIdentifier("transactions.count")
            }
            if let selected = transactions.selectedTotal, transactions.selection.count > 1 {
                Text("\(L10n.Transactions.selectedAmount) \(formatAmount(selected))")
                    .monospacedDigit()
                    .accessibilityIdentifier("transactions.selectedAmount")
            }
            Spacer()
            if let exportMessage {
                Text(exportMessage)
            }
            if transactions.hasMore {
                Button(MacStrings.Transactions.loadMore) { Task { await transactions.loadMore() } }
                    .buttonStyle(.dash(.plainBlue, .small))
            }
        }
        .dashFont(.footnote)
        .foregroundStyle(Color.role.textSecondary)
        .padding(.horizontal, DashLayout.pagePaddingH)
        .frame(height: 32)
        .background(Color.role.card)
        .overlay(alignment: .top) { Rectangle().fill(Color.role.separator).frame(height: 0.5) }
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

/// Date, type, search, minimum amount and watch-only filters (QT-089);
/// in the history layout, iOS's category chips (IOS-028).
private struct TransactionFilterBar: View {
    let transactions: TransactionsViewModel
    let layout: TransactionsLayout
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
                .labelsHidden()
                .fixedSize()
                .help(MacStrings.Transactions.date)
                .accessibilityIdentifier("transactions.dateFilter")
                Picker(MacStrings.Transactions.type, selection: Binding(
                    get: { transactions.typePreset },
                    set: { preset in Task { await transactions.setTypePreset(preset) } }
                )) {
                    ForEach(transactions.typeMenu, id: \.self) { preset in
                        Text(L10n.Transactions.typeFilterName(preset)).tag(preset)
                    }
                }
                .labelsHidden()
                .fixedSize()
                .help(MacStrings.Transactions.type)
                .accessibilityIdentifier("transactions.typeFilter")
                HStack(spacing: DashSpacing.xs) {
                    Image(systemName: "magnifyingglass")
                        .foregroundStyle(Color.role.textTertiary)
                        .accessibilityHidden(true)
                    TextField(L10n.Transactions.searchPlaceholder, text: $search)
                        .textFieldStyle(.plain)
                        .onChange(of: search) { _, text in transactions.setSearchText(text) }
                        .accessibilityIdentifier("transactions.search")
                }
                .dashFont(.footnote)
                .padding(.horizontal, DashSpacing.m)
                .frame(height: 30)
                .background(RoundedRectangle(cornerRadius: DashRadius.searchField, style: .continuous).fill(Color.role.fieldFill))
                TextField(L10n.Transactions.minAmountPlaceholder, text: $minimum)
                    .textFieldStyle(.plain)
                    .dashFont(.footnote)
                    .monospacedDigit()
                    .padding(.horizontal, DashSpacing.m)
                    .frame(width: 120, height: 30)
                    .background(RoundedRectangle(cornerRadius: DashRadius.searchField, style: .continuous).fill(Color.role.fieldFill))
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
                    .labelsHidden()
                    .fixedSize()
                    .help(MacStrings.Transactions.watchOnly)
                }
            }
            if layout == .history {
                HistoryChips(transactions: transactions)
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
                    .foregroundStyle(Color.role.danger)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(.horizontal, DashLayout.pagePaddingH)
        .padding(.bottom, DashSpacing.m)
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
        case .confirmed: status.chainLocked ? Color.role.success : Color.role.textSecondary
        case .confirming, .unconfirmed: status.instantLocked ? Color.role.textLink : Color.role.textSecondary
        case .conflicted, .notAccepted, .abandoned: Color.role.danger
        case .immature: Color.role.warning
        }
    }
}
#endif
