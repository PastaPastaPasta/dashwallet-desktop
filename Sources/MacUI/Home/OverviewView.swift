// Overview (QT-034…039, IOS-019…023): balance card with dash-qt's grid,
// sync state and the recent transactions list.
#if os(macOS)
import DashUIMac
import DesignTokens
import SwiftUI
import WalletFeatures
import WalletRuntime

struct OverviewView: View {
    let home: HomeViewModel
    /// Flips discreet mode through the settings view model, which Overview follows.
    let toggleDiscreet: () -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DashSpacing.xl) {
                if home.showChangePeers {
                    HStack {
                        SystemNotice(text: L10n.Home.stalled, tone: .warning)
                        Button(L10n.Home.changePeers) { Task { await home.rotatePeers() } }
                    }
                }
                if let error = home.errorMessage {
                    SystemNotice(text: error, tone: .error)
                }
                HStack(alignment: .top, spacing: DashSpacing.xl) {
                    VStack(alignment: .leading, spacing: DashSpacing.l) {
                        balanceCard
                        balanceGrid
                    }
                    .frame(minWidth: 320, maxWidth: 420)
                    recentCard
                }
            }
            .padding(DashSpacing.xxl)
        }
        .accessibilityIdentifier("overview")
    }

    private var balanceCard: some View {
        BalanceHeader(
            title: MacStrings.Overview.balance,
            amount: home.formattedTotal ?? L10n.Common.unknown,
            // The view model already masks the digits with `#` (QT-039).
            isHidden: home.discreet,
            hiddenPlaceholder: home.formattedTotal ?? L10n.Common.unknown,
            onToggleHidden: toggleDiscreet,
            breakdown: [])
        .overlay(alignment: .topTrailing) {
            if let network = home.networkBadge {
                Badge(L10n.Settings.networkName(network), tone: .info)
                    .padding(DashSpacing.l)
            }
        }
        .accessibilityIdentifier("overview.total")
    }

    /// dash-qt's balances grid with "(out of sync)" (QT-034, QT-037).
    private var balanceGrid: some View {
        VStack(alignment: .leading, spacing: DashSpacing.s) {
            HStack {
                Text(MacStrings.Overview.balances)
                    .dashFont(.headline)
                    .foregroundStyle(Color.dash.primaryText)
                if home.outOfSync {
                    Text(L10n.Home.outOfSync)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.dash.errorText)
                        .help(L10n.Home.outOfSyncTooltip)
                }
            }
            Grid(alignment: .leading, horizontalSpacing: DashSpacing.xl, verticalSpacing: DashSpacing.s) {
                ForEach(home.rows) { row in
                    if row.kind == .total { Divider().gridCellUnsizedAxes(.horizontal) }
                    GridRow {
                        Text(row.title)
                            .dashFont(row.kind == .total ? .subheadMedium : .subhead)
                            .foregroundStyle(Color.dash.secondaryText)
                        Text(row.text)
                            .font(.system(.subheadline, design: .monospaced).weight(row.kind == .total ? .semibold : .regular))
                            .foregroundStyle(Color.dash.primaryText)
                            .gridColumnAlignment(.trailing)
                            .accessibilityIdentifier("overview.balance.\(row.kind)")
                    }
                    .help(row.tooltip ?? "")
                }
            }
            if home.discreet {
                Text(L10n.Home.discreetModeTip)
                    .dashFont(.caption1)
                    .foregroundStyle(Color.dash.secondaryText)
            }
        }
        .padding(DashSpacing.xl)
        .background(RoundedRectangle(cornerRadius: DashRadius.card).fill(Color.dash.secondaryBackground))
    }

    private var recentCard: some View {
        VStack(alignment: .leading, spacing: DashSpacing.s) {
            HStack {
                Text(MacStrings.Overview.recent)
                    .dashFont(.headline)
                    .foregroundStyle(Color.dash.primaryText)
                if home.outOfSync {
                    Text(L10n.Home.outOfSync)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.dash.errorText)
                        .help(L10n.Home.outOfSyncTooltip)
                }
                Spacer()
                Text(home.syncText)
                    .dashFont(.caption1)
                    .foregroundStyle(Color.dash.secondaryText)
            }
            if !home.recentVisible {
                Text(MacStrings.Overview.hiddenRecent)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.secondaryText)
                    .padding(.vertical, DashSpacing.l)
            } else if home.recent.isEmpty {
                Text(MacStrings.Overview.noTransactions)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.secondaryText)
                    .padding(.vertical, DashSpacing.l)
            } else {
                ForEach(home.recent) { transaction in
                    RecentTransactionRow(transaction: transaction) { home.open(transaction) }
                    if transaction.id != home.recent.last?.id { Divider() }
                }
            }
        }
        .padding(DashSpacing.xl)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: DashRadius.card).fill(Color.dash.secondaryBackground))
        .accessibilityIdentifier("overview.recent")
    }
}

/// One recent transaction; click opens it on the Transactions page (QT-038).
struct RecentTransactionRow: View {
    let transaction: RecentTransaction
    let open: () -> Void

    var body: some View {
        Button(action: open) {
            HStack(spacing: DashSpacing.m) {
                ZStack {
                    RoundedRectangle(cornerRadius: DashRadius.transactionIcon)
                        .fill(transaction.isIncoming ? Color.dash.greenAlpha10 : Color.dash.gray300Alpha10)
                    Image(systemName: transaction.isIncoming ? "arrow.down.left" : "arrow.up.right")
                        .foregroundStyle(transaction.isIncoming ? Color.dash.successText : Color.dash.primaryText)
                }
                .frame(width: 32, height: 32)
                .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: DashSpacing.xxxs) {
                    Text(transaction.title.isEmpty ? L10n.Common.unknown : transaction.title)
                        .dashFont(.subheadMedium)
                        .foregroundStyle(Color.dash.primaryText)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    if let date = transaction.date {
                        Text(date.formatted(date: .abbreviated, time: .shortened))
                            .dashFont(.caption1)
                            .foregroundStyle(Color.dash.secondaryText)
                    }
                }
                Spacer(minLength: DashSpacing.s)
                if transaction.instantLocked {
                    Image(systemName: "bolt.fill")
                        .foregroundStyle(Color.dash.blueText)
                        .help("InstantSend")
                        .accessibilityLabel("InstantSend")
                }
                Text(transaction.amountText)
                    .font(.system(.subheadline, design: .monospaced).weight(.medium))
                    .foregroundStyle(transaction.isIncoming ? Color.dash.successText : Color.dash.primaryText)
            }
            .padding(.vertical, DashSpacing.xs)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("overview.recent.row")
    }
}
#endif
