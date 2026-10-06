// The selected transaction: dash-qt's details fields and context-menu
// actions (copy, abandon, resend, unlock dust, explorer links; QT-075,
// QT-090…094, IOS-032), and the day-grouped history (IOS-027, IOS-030).
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

/// The selected transaction (UX-SPEC §4.9 detail): icon, title and amount
/// header with the status chip and date, dash-qt's fields in a card, the
/// label, the actions (Copy is the "More"-style menu: Cross has no context
/// menus) and explorer links.
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
        let presenter = state.amountPresenter
        DashCard(padding: Int(DashSpacing.xl)) {
            HStack(alignment: .top) {
                Spacer()
                DashButton(CrossStrings.close, style: .tintedGray, size: .small) { model.clearDetail() }
            }
            if let record {
                VStack(spacing: Int(DashSpacing.xs)) {
                    DashIcon(TxPresentation.direction(record).icon, size: 50, width: 50)
                    Text(TxPresentation.title(record)).dashFont(.title3).dashForeground(CrossRole.textPrimary)
                    AmountText(presenter.full(record.amount, signed: true), unit: presenter.unitDisplay, style: .title1, weight: .bold)
                    HStack(spacing: Int(DashSpacing.s)) {
                        if let chip = TxPresentation.chip(detail.status) {
                            DashBadge(chip.text, tone: chip.tone)
                        }
                        Text(Format.date(detail.date)).dashFont(.footnote).dashForeground(CrossRole.textSecondary)
                    }
                }
                .frame(maxWidth: .infinity)
            }
            SectionHeader(L10n.TransactionsM2.detailsTitle(String(detail.txid.prefix(16)) + "…"), style: .subheadMedium)
            DashCard(padding: Int(DashSpacing.m), spacing: Int(DashSpacing.xs), fill: CrossRole.cardRaised) {
                ForEach(model.detailFields) { field in
                    KeyValueRow(field.title, field.value, monospaced: field.title == CrossStrings.transactionID)
                }
            }
            HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                DashTextField(CrossStrings.label, placeholder: CrossStrings.labelPlaceholder, text: labelText)
                DashButton(CrossStrings.saveLabel, style: .tintedBlue, size: .small) {
                    let label = labelText.wrappedValue
                    Task { await model.setLabel(label, txid: detail.txid) }
                }
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
                // dash-qt shows Abandon and Resend disabled when they cannot apply.
                DashButton(
                    L10n.TransactionsM2.resend, style: .tintedBlue, size: .small, isEnabled: model.canResend,
                    help: model.canResend ? nil : CrossStrings.resendUnavailable
                ) {
                    Task { await model.resend() }
                }
                if model.canUnlockDust {
                    DashButton(L10n.TransactionsM2.unlockDust, style: .tintedBlue, size: .small) {
                        Task { await model.unlockDust() }
                    }
                }
                Spacer()
                DashButton(
                    L10n.TransactionsM2.abandon, style: .plainRed, size: .small, isEnabled: model.canAbandon,
                    help: model.canAbandon ? nil : CrossStrings.abandonUnavailable
                ) {
                    model.requestAbandon()
                }
            }
            let links = model.thirdPartyLinks(for: detail.txid) + model.explorerLinks(for: detail.txid)
            if !links.isEmpty {
                HStack(spacing: Int(DashSpacing.s)) {
                    ForEach(links) { link in
                        DashButton(link.title, style: .plainBlue, size: .small, icon: .externalLink) { openURL(link.url) }
                    }
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
            LoadingState(CrossStrings.working)
        case .done(let text):
            Toast(text, kind: .success, actionTitle: CrossStrings.dismiss) { model.dismissAction() }
        case .failed(let text):
            Toast(text, kind: .error, actionTitle: CrossStrings.dismiss) { model.dismissAction() }
        }
    }
}

/// The empty or loading history (C25, C26).
struct HistoryEmpty: View {
    let loading: Bool

    var body: some View {
        DashCard {
            if loading {
                LoadingState(CrossStrings.loadingTransactions)
            } else {
                EmptyState(icon: .txAll, title: CrossStrings.noTransactionsToDisplay)
            }
        }
    }
}

/// List mode (UX-SPEC §4.9): the loaded rows by day, newest first, one card
/// per day (C8) with iOS rows (C9); each day's CoinJoin internal records are
/// one "Mixing Transactions" row. Selecting a row opens its details.
struct DayGroupList: View {
    let model: TransactionsViewModel
    let state: CrossAppState

    var body: some View {
        let model = model
        let state = state
        if model.rows.isEmpty {
            HistoryEmpty(loading: model.isLoading)
        } else {
            ForEach(model.dayGroups) { group in
                TransactionGroupCard(day: group.title, weekday: Format.weekday(group.day)) {
                    DayRows(model: model, state: state, items: group.items)
                }
            }
        }
    }
}

struct DayRows: View {
    let model: TransactionsViewModel
    let state: CrossAppState
    let items: [HistoryItem]

    var body: some View {
        let model = model
        let items = items
        let presenter = state.amountPresenter
        let selection = bind(
            { model.selection.count == 1 ? model.selection.first.flatMap { id in items.first { $0.recordID == id }?.id } : nil },
            { (id: String?) in
                guard let id, let record = items.first(where: { $0.id == id })?.recordID else { return }
                Task { await model.select(record) }
            })
        // ADR 0002: every List sits in a ScrollView of fixed height.
        ScrollView {
            List(items, selection: selection) { item in
                switch item {
                case .record(let record):
                    let amount = presenter.compact(record.amount)
                    TransactionView(
                        direction: TxPresentation.direction(record), title: TxPresentation.title(record),
                        subtitle: Format.time(record.date),
                        amount: record.countsTowardBalance ? amount : "[\(amount)]", unit: presenter.unitDisplay,
                        chip: TxPresentation.chip(record.status), status: TxPresentation.trailingStatus(record.status),
                        dimmed: TxPresentation.dimmed(record.status))
                case .coinJoinMixing(let row):
                    TransactionView(
                        direction: .mixing, title: row.title,
                        subtitle: L10n.TransactionsM2.mixingCount(row.records.count),
                        amount: presenter.compact(row.total), unit: presenter.unitDisplay)
                }
            }
            .accessibleRowNames(items.map { item in
                switch item {
                case .record(let record):
                    "\(model.typeText(for: record)), \(model.addressText(for: record)), \(model.amountText(for: record)), \(Format.date(record.date))"
                case .coinJoinMixing(let row):
                    "\(row.title), \(L10n.TransactionsM2.mixingCount(row.records.count))"
                }
            })
        }
        .frame(height: Double(CrossLayout.txRowMinHeight * items.count + 4))
    }
}

extension HistoryItem {
    /// The record a row opens: the record itself, or a mixing row's first.
    var recordID: TxRecord.ID? {
        switch self {
        case .record(let record): record.id
        case .coinJoinMixing(let row): row.records.first?.id
        }
    }
}
