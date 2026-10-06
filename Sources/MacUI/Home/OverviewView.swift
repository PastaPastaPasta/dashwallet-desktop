// Overview (QT-034…039, IOS-019…025) after dashwallet-iOS Home (UX-SPEC
// §4.5): the blue balance hero with dash-qt's breakdown, the shortcut card
// straddling it, banners, and History as day-grouped cards of iOS rows.
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
    var unit: DisplayUnit = .dash
    var unitName: String = "DASH"
    var shortcuts: ShortcutBarViewModel?
    var reminder: BackupReminderViewModel?
    var perform: (ShortcutRoute) -> Void = { _ in }
    var backUp: () -> Void = {}
    var setUnit: ((DisplayUnit) -> Void)?
    var unitTitle: (DisplayUnit) -> String = { "\($0)" }
    var showSyncDetails: (() -> Void)?
    var seeAll: (() -> Void)?
    @State private var toast: ToastMessage?

    var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                hero
                VStack(alignment: .leading, spacing: DashLayout.sectionGap) {
                    banners
                    history
                }
                .frame(maxWidth: DashLayout.contentMaxWidth)
                .padding(.horizontal, DashLayout.pagePaddingH)
                .padding(.top, DashLayout.pagePaddingTop)
                .padding(.bottom, DashLayout.sectionGap)
                .frame(maxWidth: .infinity)
            }
        }
        .dashCanvas()
        .dashToast($toast)
        .accessibilityIdentifier("overview")
    }

    // MARK: Hero

    private var hero: some View {
        BalanceHero(
            network: home.networkBadge.map(L10n.Settings.networkName),
            caption: caption,
            amount: home.formattedTotal.map { AmountParts.split($0).number },
            unit: totalUnit,
            isHidden: home.discreet,
            unavailableText: L10n.UX.balanceUnavailable,
            breakdown: breakdown,
            onToggleHidden: toggleDiscreet,
            toggleLabel: home.discreet ? MacStrings.Toolbar.showBalances : MacStrings.Toolbar.hideBalances,
            overlapHeight: shortcuts == nil ? 0 : 46
        ) {
            if let shortcuts {
                ShortcutBarView(bar: shortcuts, perform: perform)
            }
        }
        .help(home.discreet ? L10n.UX.clickToShowBalance : L10n.UX.clickToHideBalance)
        .contextMenu {
            Button(home.discreet ? MacStrings.Toolbar.showBalances : MacStrings.Toolbar.hideBalances, action: toggleDiscreet)
            if let setUnit {
                Menu(MacStrings.Overview.displayUnit) {
                    ForEach(DisplayUnit.allCases, id: \.self) { unit in
                        Button(unitTitle(unit)) { setUnit(unit) }
                    }
                }
            }
            if !home.discreet, let total = home.rows.first(where: { $0.kind == .total })?.text, home.formattedTotal != nil {
                Button(MacStrings.Overview.copyBalance) {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(total, forType: .string)
                    toast = ToastMessage(.copied, L10n.UX.copied)
                }
            }
        }
    }

    private var totalUnit: AmountUnitDisplay {
        home.formattedTotal.map { AmountUnitDisplay(parts: AmountParts.split($0)) } ?? .none
    }

    /// While catching up: "Syncing Balance" (pulsing) with dash-qt's
    /// out-of-sync tooltip; without a connection: "(out of sync)" (QT-037).
    private var caption: BalanceHeroCaption? {
        guard home.outOfSync else { return nil }
        if let sync = home.sync, sync.running, sync.connectedPeers > 0, !sync.isStalled {
            return BalanceHeroCaption(text: L10n.UX.syncingBalance, help: L10n.Home.outOfSyncTooltip, pulses: true)
        }
        return BalanceHeroCaption(text: L10n.Home.outOfSync, help: L10n.Home.outOfSyncTooltip, isWarning: true)
    }

    /// dash-qt's Available / Pending / Immature (only when non-zero) (QT-034).
    private var breakdown: [BalanceBreakdownCell] {
        home.rows.filter { $0.kind != .total }.map { row in
            let parts = row.amount == nil ? nil : AmountParts.split(row.text)
            return BalanceBreakdownCell(
                id: "\(row.kind)", title: Self.title(row.kind), subtitle: Self.hint(row.kind),
                amount: parts?.number, unit: parts.map(AmountUnitDisplay.init(parts:)) ?? .none,
                help: row.amount == nil ? L10n.UX.notAvailableUntilSynced : row.tooltip)
        }
    }

    static func title(_ kind: BalanceKind) -> String {
        switch kind {
        case .available: L10n.UX.available
        case .pending: L10n.UX.pending
        case .immature: L10n.UX.immature
        case .total: L10n.Home.total
        }
    }

    static func hint(_ kind: BalanceKind) -> String? {
        switch kind {
        case .available: L10n.UX.availableHint
        case .pending: L10n.UX.pendingHint
        case .immature: L10n.UX.immatureHint
        case .total: nil
        }
    }

    // MARK: Banners

    @ViewBuilder
    private var banners: some View {
        if let reminder {
            BackupReminderBanner(reminder: reminder, backUp: backUp)
        }
        if home.showChangePeers {
            SystemMessageView(
                title: L10n.Home.stalled, icon: .token(.messageWarning), backgroundColor: Color.role.warningTint,
                buttonName: L10n.Home.changePeers, onAction: { Task { await home.rotatePeers() } })
                .accessibilityIdentifier("overview.stalled")
        }
        if let error = home.errorMessage {
            SystemMessageView(title: error, icon: .token(.messageWarning), backgroundColor: Color.role.dangerTint)
                .accessibilityIdentifier("overview.error")
        }
    }

    // MARK: History

    @ViewBuilder
    private var history: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            HistoryHeader(
                title: L10n.UX.history, syncText: syncText, onSync: showSyncDetails,
                filterTitle: seeAll == nil ? nil : L10n.UX.filter, onFilter: seeAll)
            if !home.recentVisible {
                EmptyState(icon: .system("eye.slash"), title: MacStrings.Overview.hiddenRecent)
                    .dashCard(padding: nil)
            } else if home.recent.isEmpty {
                EmptyState(icon: .token(.txAll), title: L10n.UX.noTransactions) {
                    Button(L10n.UX.receiveDash) { perform(.section(.receive)) }
                        .buttonStyle(.dash(.filledBlue, .medium))
                }
                .dashCard(padding: nil)
            } else {
                ForEach(HistoryDay.group(home.recent, date: \.date)) { group in
                    TransactionGroupCard(
                        day: group.day.map { HistoryDay.title($0) } ?? L10n.TransactionsM2.dateUnknown,
                        weekday: group.day.map { HistoryDay.weekday($0) }
                    ) {
                        ForEach(group.items) { transaction in
                            DashTransactionRow(recent: transaction, unit: unit, unitName: unitName) {
                                home.open(transaction)
                            }
                            .accessibilityIdentifier("overview.recent.row")
                        }
                    }
                }
                if let seeAll {
                    Button(L10n.UX.seeAllTransactions, action: seeAll)
                        .buttonStyle(.dash(.plainBlue, .small))
                        .frame(maxWidth: .infinity)
                }
            }
        }
        .accessibilityIdentifier("overview.recent")
    }

    /// "Syncing 47.0%" while catching up; nothing once synced.
    private var syncText: String? {
        guard let sync = home.sync, sync.running, !sync.isDone, let progress = sync.progress else { return nil }
        return L10n.UX.syncingPercent(progress)
    }
}
#endif
