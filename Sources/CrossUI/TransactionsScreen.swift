// Transactions: filters, the record list, details with label editing, and
// CSV export through the toolkit's save dialog (QT-086…094, IOS-027…031).
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct TransactionsScreen: View {
    let model: TransactionsViewModel
    let state: CrossAppState

    @Environment(\.chooseFileSaveDestination) var chooseFileSaveDestination
    @State var searchText = ""
    @State var minimumText = ""
    @State var rangeFromText = ""
    @State var rangeUntilText = ""
    @State var labelText = ""
    @State var exportMessage: String?
    @State var exportFailed = false
    /// List mode (iOS day cards) is the default; table mode is the dash-qt view.
    @State var tableMode = false

    var body: some View {
        let model = model
        Page(L10n.Navigation.transactions, width: tableMode ? .full : .content) {
            HStack(spacing: Int(DashSpacing.s)) {
                SegmentedControl(
                    options: [PickerOption(false, CrossStrings.listMode), PickerOption(true, CrossStrings.tableMode)],
                    selection: tableMode
                ) { tableMode = $0 }
                Spacer()
                DashButton(CrossStrings.exportCSV, style: .tintedBlue, size: .small, icon: .csvExport) { export(model) }
            }
            DashCard(spacing: Int(DashSpacing.s)) {
                filters(model)
                chips(model)
            }
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            if let exportMessage {
                Toast(exportMessage, kind: exportFailed ? .error : .success)
            }
            TransactionActionBanner(model: model)
            TransactionDetailHost(model: model, state: state, labelText: $labelText)
            if tableMode {
                recordList(model)
            } else {
                DayGroupList(model: model, state: state)
            }
            HStack(spacing: Int(DashSpacing.s)) {
                if let total = model.totalMatching {
                    Text(CrossStrings.matching(total)).dashFont(.footnote).dashForeground(CrossRole.textSecondary)
                }
                if let selected = model.selectedTotal, model.selection.count > 1 {
                    Text("\(L10n.Transactions.selectedAmount) \(state.format(selected))")
                        .dashFont(.footnote).dashForeground(CrossRole.textSecondary)
                }
                Spacer()
                if model.hasMore {
                    DashButton(CrossStrings.loadMore, style: .tintedGray, size: .small) { Task { await model.loadMore() } }
                }
            }
        }
        .task {
            searchText = model.searchText
            minimumText = model.minimumAmountText
            await model.reload()
            await model.refreshChips()
        }
    }

    /// iOS filter chips (IOS-028) and day grouping (IOS-027, IOS-030).
    @ViewBuilder
    private func chips(_ model: TransactionsViewModel) -> some View {
        HStack(spacing: Int(DashSpacing.xs)) {
            DashButton(
                L10n.TransactionsM2.all,
                style: Set(model.offeredChips).isSubset(of: model.selectedChips) ? .tintedBlue : .tintedGray,
                size: .small
            ) { Task { await model.selectAllChips() } }
            ForEach(model.offeredChips, id: \.self) { chip in
                DashButton(
                    chip.title, style: model.selectedChips.contains(chip) ? .tintedBlue : .tintedGray, size: .small
                ) { Task { await model.toggleChip(chip) } }
            }
            Spacer()
        }
    }

    @ViewBuilder
    private func filters(_ model: TransactionsViewModel) -> some View {
        let searchBinding = bind({ searchText }, { text in
            searchText = text
            model.setSearchText(text)
        })
        let minimumBinding = bind({ minimumText }, { text in
            minimumText = text
            Task { await model.setMinimumAmountText(text) }
        })
        HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
            DashPicker(
                CrossStrings.date,
                options: DateFilterPreset.allCases.map { PickerOption($0, L10n.Transactions.dateFilterName($0)) },
                selection: bind({ model.datePreset }, { preset in Task { await model.setDatePreset(preset) } }))
            DashPicker(
                CrossStrings.type,
                options: model.typeMenu.map { PickerOption($0, L10n.Transactions.typeFilterName($0)) },
                selection: bind({ model.typePreset }, { preset in Task { await model.setTypePreset(preset) } }))
            if model.showsWatchOnly {
                DashPicker(
                    CrossStrings.watchOnly,
                    options: [
                        PickerOption(WatchOnlyFilter.all, "All"), PickerOption(.yes, "Watch-only"),
                        PickerOption(.no, "Not watch-only"),
                    ],
                    selection: bind({ model.watchOnly }, { filter in Task { await model.setWatchOnly(filter) } }))
            }
        }
        HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
            DashTextField(CrossStrings.search, placeholder: L10n.Transactions.searchPlaceholder, text: searchBinding)
            DashTextField(
                CrossStrings.minAmount, placeholder: L10n.Transactions.minAmountPlaceholder, text: minimumBinding,
                error: model.minimumAmountError, width: 140)
        }
        if model.datePreset == .range {
            HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                DashTextField(CrossStrings.from, text: $rangeFromText, width: 160)
                DashTextField(CrossStrings.until, text: $rangeUntilText, width: 160)
                DashButton(CrossStrings.applyRange, style: .tintedGray, size: .small) {
                    let from = Format.day.date(from: rangeFromText)
                    // dash-qt's range end is exclusive: include the whole "to" day.
                    let until = Format.day.date(from: rangeUntilText).map { $0.addingTimeInterval(86_400) }
                    Task { await model.setRange(from: from, until: until) }
                }
            }
        }
    }

    @ViewBuilder
    private func recordList(_ model: TransactionsViewModel) -> some View {
        if model.rows.isEmpty {
            HistoryEmpty(loading: model.isLoading)
        } else {
            let selection = bind(
                { model.selection.count == 1 ? model.selection.first : nil },
                { (id: TxRecord.ID?) in
                    guard let id else { return }
                    Task { await model.select(id) }
                })
            // ADR 0002: the List scrolls inside a fixed-height ScrollView and
            // rows are single-line, so the window cannot outgrow X11's limit.
            // Table mode keeps dash-qt's phrasing ("Received with" + address),
            // the full-precision amount with its unit and the status text.
            ScrollView {
                List(model.rows, selection: selection) { record in
                    TransactionView(
                        direction: TxPresentation.direction(record),
                        title: "\(model.typeText(for: record))  \(model.addressText(for: record))",
                        subtitle: "\(Format.date(record.date))  ·  \(model.statusText(for: record))",
                        amount: model.amountText(for: record))
                }
                .accessibleRowNames(model.rows.map { record in
                    "\(model.typeText(for: record)), \(model.addressText(for: record)), \(model.amountText(for: record)), \(Format.date(record.date))"
                })
            }
            .frame(height: 440)
            .cardBackground(radius: CrossLayout.groupRadius)
        }
    }

    private func export(_ model: TransactionsViewModel) {
        let choose = chooseFileSaveDestination
        Task {
            let csv: String
            do {
                csv = try await model.exportCSV()
            } catch {
                exportFailed = true
                exportMessage = L10n.Transactions.exportFailed
                return
            }
            guard
                let url = await choose(
                    title: L10n.Transactions.exportTitle, defaultButtonLabel: CrossStrings.save,
                    defaultFileName: "transactions.csv")
            else {
                exportFailed = false
                exportMessage = CrossStrings.exportCancelled
                return
            }
            do {
                try ExportFile.write(csv, to: url)
                exportFailed = false
                exportMessage = "\(CrossStrings.exported) \(url.path)"
            } catch {
                exportFailed = true
                exportMessage = L10n.Transactions.exportFailed
            }
        }
    }
}
