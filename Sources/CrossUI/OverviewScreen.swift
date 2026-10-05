// Overview: balances, discreet mode, sync state and recent transactions
// (QT-034, QT-036…039, IOS-019…023).
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct OverviewScreen: View {
    let model: HomeViewModel
    let state: CrossAppState

    var body: some View {
        let model = model
        let state = state
        Page(L10n.Navigation.overview) {
            HStack(spacing: Int(DashSpacing.s)) {
                SectionHeader(CrossStrings.balances)
                if model.outOfSync {
                    Text(L10n.Home.outOfSync)
                        .dashFont(.footnoteMedium)
                        .dashForeground(.red)
                        .help(L10n.Home.outOfSyncTooltip)
                }
                if let network = model.networkBadge {
                    DashBadge(L10n.Settings.networkName(network))
                }
                Spacer()
                Toggle(CrossStrings.hideBalances, isOn: bind({ model.discreet }, { _ in Task { await model.toggleDiscreet() } }))
                    .toggleStyle(.switch)
            }
            DashCard {
                ForEach(model.rows) { row in
                    if let tooltip = row.tooltip {
                        MenuItem(title: row.title, trailing: row.text).help(tooltip)
                    } else {
                        MenuItem(title: row.title, trailing: row.text)
                    }
                }
            }
            Text(model.syncText).dashFont(.footnote).dashForeground(.secondaryText)
            if model.showChangePeers {
                Toast(L10n.Home.stalled, kind: .warning, actionTitle: L10n.Home.changePeers) {
                    Task { await model.rotatePeers() }
                }
            }
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            SectionHeader(CrossStrings.recentTransactions)
            if !model.recentVisible {
                Text(L10n.Home.discreetModeTip).dashFont(.footnote).dashForeground(.secondaryText)
            } else if model.recent.isEmpty {
                Text(CrossStrings.noTransactions).dashFont(.footnote).dashForeground(.secondaryText)
            } else {
                // Selecting a row opens it on the Transactions page (QT-038).
                let selection = bind(
                    { Optional<TxRecord.ID>.none },
                    { (id: TxRecord.ID?) in
                        guard let id, let transaction = model.recent.first(where: { $0.id == id }) else { return }
                        model.open(transaction)
                        let route = model.route
                        model.route = nil
                        Task { await state.follow(route) }
                    })
                ScrollView {
                    List(model.recent, selection: selection) { transaction in
                        TransactionView(
                            direction: transaction.isIncoming ? .incoming : .outgoing,
                            title: transaction.title,
                            subtitle: Format.date(transaction.date),
                            amount: transaction.amountText,
                            detail: lockText(transaction))
                    }
                }
                .frame(height: Double(64 * model.recent.count))
            }
        }
    }

    private func lockText(_ transaction: RecentTransaction) -> String? {
        if transaction.chainLocked { return "ChainLocked" }
        if transaction.instantLocked { return "InstantSend" }
        return nil
    }
}
