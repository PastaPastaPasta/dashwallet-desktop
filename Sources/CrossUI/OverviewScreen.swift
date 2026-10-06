// Overview (UX-SPEC §4.5; QT-034, QT-036…039, IOS-019…025): the blue
// balance hero with dash-qt's breakdown, the shortcut card straddling its
// edge, the backup reminder, then "History" with the recent transactions
// grouped into one card per day.
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct OverviewScreen: View {
    let model: HomeViewModel
    let state: CrossAppState

    @Environment(\.openURL) var openURL

    var body: some View {
        let model = model
        let state = state
        ScrollView {
            VStack(spacing: 0) {
                HeroBand(bandHeight: CrossLayout.heroBandHeight + 34) {
                    hero(model, presenter: state.amountPresenter)
                } overlap: {
                    shortcutCard(state)
                }
                VStack(alignment: .leading, spacing: Int(DashSpacing.l)) {
                    messages(model, state: state)
                    HistoryHeader(
                        title: CrossStrings.history,
                        syncText: model.sync.flatMap { $0.isDone ? nil : $0.progress }.map(CrossStrings.syncPercent),
                        filterTitle: CrossStrings.filter,
                        onSync: { state.main.syncOverlayRequested = true },
                        onFilter: { Task { await state.perform(.section(.transactions)) } })
                    history(model, state: state)
                }
                .frame(maxWidth: CrossLayout.contentMaxWidth, alignment: .topLeading)
                .padding(.horizontal, CrossLayout.pagePaddingH)
                .padding(.top, CrossLayout.sectionGap)
                .padding(.bottom, CrossLayout.sectionGap)
                .frame(maxWidth: .infinity, alignment: .top)
            }
        }
        .background(CrossRole.canvas.color)
        .task(id: model.recent.map(\.id)) {
            await state.loadOverviewRecords()
        }
    }

    // MARK: Hero (C4, C5)

    @ViewBuilder
    private func hero(_ model: HomeViewModel, presenter: AmountPresenter) -> some View {
        let model = model
        let syncing = model.sync.map { !$0.isDone } ?? true
        let total = model.formattedTotal.map(presenter.number(fromFormatted:))
        let cells = model.rows.filter { $0.kind != .total }.map { row in
            BalanceCell(
                title: row.title.hasSuffix(":") ? String(row.title.dropLast()) : row.title,
                subtitle: Self.explanation(row.kind),
                amount: row.amount == nil ? L10n.Common.unknown : presenter.number(fromFormatted: row.text),
                help: row.tooltip)
        }
        VStack(spacing: Int(DashSpacing.xs)) {
            BalanceHero(
                network: model.networkBadge.map(L10n.Settings.networkName),
                caption: model.outOfSync ? L10n.Home.outOfSync : (syncing ? CrossStrings.syncingBalance : nil),
                captionIsWarning: model.outOfSync, captionHelp: model.outOfSync ? L10n.Home.outOfSyncTooltip : nil,
                amount: total ?? "—", unit: total == nil ? .none : presenter.unitDisplay,
                amountHelp: total.map { "\($0) \(presenter.unitName)" } ?? CrossStrings.balanceUnavailable,
                hint: total == nil ? CrossStrings.balanceUnavailable : nil,
                cells: cells, cellUnit: presenter.unitDisplay)
            // iOS tap-to-hide on the amount; Cross has no tap target with a
            // name, so the hero carries an icon-and-text button.
            DashButton(
                model.discreet ? CrossStrings.showBalance : CrossStrings.hideBalance, style: .tintedWhite,
                size: .extraSmall, icon: model.discreet ? .eyeOpen : .eyeClosed
            ) {
                Task { await model.toggleDiscreet() }
            }
        }
    }

    static func explanation(_ kind: BalanceKind) -> String {
        switch kind {
        case .available: CrossStrings.spendableNow
        case .pending: CrossStrings.awaitingConfirmation
        case .immature: CrossStrings.maturing
        case .total: ""
        }
    }

    // MARK: Shortcuts (C6)

    @ViewBuilder
    private func shortcutCard(_ state: CrossAppState) -> some View {
        let open = openURL
        if let bar = state.shortcuts() {
            let items = bar.slots.map { slot in
                ShortcutItem(
                    id: slot.index, title: slot.action.title, icon: Self.icon(slot.action), isEnabled: slot.isEnabled,
                    help: slot.helpText)
            }
            HStack {
                Spacer()
                ShortcutCard(items: items) { item in
                    guard let slot = bar.slots.first(where: { $0.index == item.id }), let route = bar.route(for: slot)
                    else { return }
                    Task { await state.perform(shortcut: route) { open($0) } }
                }
                Spacer()
            }
            .padding(.horizontal, CrossLayout.pagePaddingH)
        }
    }

    static func icon(_ action: ShortcutAction) -> DashIconToken {
        switch action {
        case .backup: .backup
        case .receive, .testnetFaucet: .receive
        case .send: .send
        case .sendToAddress: .sendToAddress
        case .scanQR: .scanQR
        case .buySell, .coinbase, .uphold, .topper, .atm: .buySell
        case .explore, .spend, .dashDEX: .explore
        case .crowdNode, .nodes: .networkMonitor
        case .switchWallet: .wallet
        }
    }

    // MARK: Messages above History

    @ViewBuilder
    private func messages(_ model: HomeViewModel, state: CrossAppState) -> some View {
        let reminder = state.backupReminder()
        if reminder.isDue {
            DashCard {
                MenuRow(icon: .backup, title: L10n.HomeM2.backupReminderTitle, help: L10n.HomeM2.backupReminderMessage) {
                    DashButton(L10n.HomeM2.later, style: .tintedGray, size: .small) { reminder.markShown() }
                    DashButton(L10n.HomeM2.backupNow, style: .filledBlue, size: .small) {
                        reminder.markShown()
                        Task { await state.perform(.showRecoveryPhrase) }
                    }
                }
            }
        }
        if model.showChangePeers {
            Toast(L10n.Home.stalled, kind: .warning, actionTitle: L10n.Home.changePeers) {
                Task { await model.rotatePeers() }
            }
        }
        if let error = model.errorMessage {
            Toast(error, kind: .error)
        }
    }

    // MARK: History (C7, C8, C9)

    @ViewBuilder
    private func history(_ model: HomeViewModel, state: CrossAppState) -> some View {
        if !model.recentVisible {
            DashCard {
                EmptyState(icon: .eyeClosed, title: L10n.Home.discreetModeTip)
            }
        } else if model.recent.isEmpty {
            DashCard {
                EmptyState(icon: .txAll, title: CrossStrings.noTransactionsToDisplay) {
                    DashButton(CrossStrings.receiveDash, style: .filledBlue, size: .medium) {
                        Task { await state.perform(.section(.receive)) }
                    }
                }
            }
        } else {
            ForEach(Self.days(model.recent)) { day in
                TransactionGroupCard(day: day.title, weekday: day.weekday) {
                    RecentRows(model: model, state: state, rows: day.rows)
                }
            }
            DashButton(CrossStrings.seeAllTransactions, style: .plainBlue, size: .small) {
                Task { await state.perform(.section(.transactions)) }
            }
        }
    }

    struct Day: Identifiable {
        let id: String
        let title: String
        let weekday: String?
        let rows: [RecentTransaction]
    }

    /// The recent rows by calendar day, newest first (the list is already newest first).
    static func days(_ recent: [RecentTransaction]) -> [Day] {
        var days: [Day] = []
        let calendar = Calendar.current
        for row in recent {
            let start = row.date.map { calendar.startOfDay(for: $0) }
            let id = start.map { String($0.timeIntervalSince1970) } ?? "unknown"
            if let index = days.firstIndex(where: { $0.id == id }) {
                let day = days[index]
                days[index] = Day(id: id, title: day.title, weekday: day.weekday, rows: day.rows + [row])
            } else {
                days.append(Day(
                    id: id, title: start.map { Format.dayTitle.string(from: $0) } ?? L10n.TransactionsM2.dateUnknown,
                    weekday: Format.weekday(start), rows: [row]))
            }
        }
        return days
    }
}

/// One day's recent rows as a selectable list: selecting a row opens it on
/// the Transactions page (QT-038). Rows are named for AT-SPI with the type,
/// counterparty, amount and date.
struct RecentRows: View {
    let model: HomeViewModel
    let state: CrossAppState
    let rows: [RecentTransaction]

    var body: some View {
        let model = model
        let state = state
        let presenter = state.amountPresenter
        let selection = bind(
            { Optional<TxRecord.ID>.none },
            { (id: TxRecord.ID?) in
                guard let id, let transaction = model.recent.first(where: { $0.id == id }) else { return }
                model.open(transaction)
                let route = model.route
                model.route = nil
                Task { await state.follow(route) }
            })
        // ADR 0002: every List sits in a ScrollView of fixed height.
        ScrollView {
            List(rows, selection: selection) { row in
                RecentRowView(row: row, record: state.overviewRecords[row.id], presenter: presenter)
            }
            .accessibleRowNames(rows.map { row in
                "\(Self.title(row, state.overviewRecords[row.id])), \(row.title), \(row.amountText), \(Format.date(row.date))"
            })
        }
        .frame(height: Double(CrossLayout.txRowMinHeight * rows.count + 4))
    }

    static func title(_ row: RecentTransaction, _ record: TxRecord?) -> String {
        if let record { return TxPresentation.title(record) }
        return row.isIncoming ? CrossStrings.TxTitle.received : CrossStrings.TxTitle.sent
    }
}

struct RecentRowView: View {
    let row: RecentTransaction
    let record: TxRecord?
    let presenter: AmountPresenter

    var body: some View {
        let amount = presenter.compact(row.amount)
        TransactionView(
            direction: record.map(TxPresentation.direction) ?? (row.isIncoming ? .incoming : .outgoing),
            title: RecentRows.title(row, record),
            subtitle: Format.time(row.date),
            amount: record?.countsTowardBalance == false ? "[\(amount)]" : amount,
            unit: presenter.unitDisplay,
            chip: record.flatMap { TxPresentation.chip($0.status) }
                ?? (record == nil && row.instantLocked && !row.chainLocked ? TransactionChip(CrossStrings.TxChip.instantSend) : nil),
            status: record.flatMap { TxPresentation.trailingStatus($0.status) },
            dimmed: record.map { TxPresentation.dimmed($0.status) } ?? false)
    }
}
