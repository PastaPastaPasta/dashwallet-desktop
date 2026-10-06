// Overview / Home (QT-034, QT-036…039, IOS-019…023).
import Foundation
import Observation
import WalletRuntime

public enum BalanceKind: Sendable, Hashable, CaseIterable {
    case available, pending, immature, total
}

/// One row of dash-qt's balances grid.
public struct BalanceRow: Sendable, Hashable, Identifiable {
    public var id: BalanceKind { kind }
    public let kind: BalanceKind
    public let title: String
    public let tooltip: String?
    /// `nil` = unknown (not loaded yet); never shown as zero.
    public let amount: Amount?
    public let text: String
}

/// A recent-transactions row (QT-038).
public struct RecentTransaction: Sendable, Hashable, Identifiable {
    public let id: TxRecord.ID
    public let date: Date?
    public let amount: Amount
    /// Signed, floored to the decimal-digits setting; `[…]` when the
    /// transaction does not count toward the balance.
    public let amountText: String
    public let title: String
    public let instantLocked: Bool
    public let chainLocked: Bool
    public let watchOnly: Bool
    public let isIncoming: Bool
}

@MainActor
@Observable
public final class HomeViewModel {
    /// dash-qt shows 5 rows with CoinJoin off, 6 with it on.
    public static func recentLimit(coinJoin: Bool) -> Int { coinJoin ? 6 : 5 }

    /// Types the Overview hides (QT-038).
    public nonisolated static let hiddenRecentTypes: Set<TxType> = [
        .coinJoinMixing, .coinJoinCollateralPayment, .coinJoinMakeCollaterals, .coinJoinCreateDenominations,
        .dustReceive, .recvWithCoinJoin,
    ]

    public private(set) var balances: WalletBalances?
    public private(set) var rows: [BalanceRow] = []
    public private(set) var formattedTotal: String?
    public private(set) var recent: [RecentTransaction] = []
    public private(set) var sync: SyncStatus?
    public private(set) var syncText = L10n.Home.notConnected
    public let networkBadge: DashNetwork?
    public private(set) var discreet: Bool
    public private(set) var errorMessage: String?
    /// Set when a recent transaction is opened (QT-038).
    public var route: AppRoute?

    /// "(out of sync)" next to the headers until sync is done (QT-037).
    public var outOfSync: Bool { !(sync?.isDone ?? false) }
    /// "Change peers" after a 45 s stall (IOS-023).
    public var showChangePeers: Bool { sync?.isStalled ?? false }
    /// Discreet mode hides the list entirely (QT-039).
    public var recentVisible: Bool { !discreet }

    private let walletState: any WalletStateProviding
    private let syncStatus: any SyncStatusProviding
    private let history: any HistoryProviding
    private let settings: any SettingsProviding
    private let amounts: any AmountFormatting
    private let features: FeatureFlags
    private var tasks: [Task<Void, Never>] = []
    private var historyTask: Task<Void, Never>?
    private var observedWallet: WalletID?

    public init(
        walletState: any WalletStateProviding, sync: any SyncStatusProviding, history: any HistoryProviding,
        settings: any SettingsProviding, amounts: any AmountFormatting, network: DashNetwork,
        features: FeatureFlags = .m1
    ) {
        self.walletState = walletState
        self.syncStatus = sync
        self.history = history
        self.settings = settings
        self.amounts = amounts
        self.features = features
        self.networkBadge = network == .mainnet ? nil : network
        self.discreet = settings.display.hideBalances
        self.sync = sync.status
        updateSyncText()
        rebuildRows()
    }

    public convenience init(env: AppEnvironment, network: DashNetwork, features: FeatureFlags = .m1) {
        self.init(
            walletState: env.walletState, sync: env.sync, history: env.history, settings: env.settings,
            amounts: env.amounts, network: network, features: features)
    }

    /// Re-reads balances, sync and the recent list.
    public func refresh() async {
        balances = walletState.balances
        discreet = settings.display.hideBalances
        sync = syncStatus.status
        updateSyncText()
        rebuildRows()
        await reloadRecent()
    }

    /// Follows wallet, sync, settings and history changes until `stop()`.
    public func start() {
        stop()
        let walletChanges = walletState.changes()
        let syncChanges = syncStatus.changes()
        let settingsChanges = settings.changes()
        tasks.append(Task { [weak self] in
            for await _ in walletChanges {
                guard let self else { return }
                self.balances = self.walletState.balances
                self.rebuildRows()
                self.observeHistoryOfSelectedWallet()
                await self.reloadRecent()
            }
        })
        tasks.append(Task { [weak self] in
            for await status in syncChanges {
                guard let self else { return }
                self.sync = status
                self.updateSyncText()
            }
        })
        tasks.append(Task { [weak self] in
            for await display in settingsChanges {
                guard let self else { return }
                self.discreet = display.hideBalances
                self.rebuildRows()
                await self.reloadRecent()
            }
        })
        observeHistoryOfSelectedWallet()
    }

    public func stop() {
        tasks.forEach { $0.cancel() }
        tasks = []
        historyTask?.cancel()
        historyTask = nil
        observedWallet = nil
    }

    /// Toggles discreet mode and persists it (QT-039, IOS-020).
    public func toggleDiscreet() async {
        var display = settings.display
        display.hideBalances.toggle()
        do {
            try settings.update(display)
            discreet = display.hideBalances
            errorMessage = nil
        } catch {
            // Nothing changed; keep the message instead of reloading over it.
            errorMessage = L10n.Settings.settingsNotSaved
            return
        }
        rebuildRows()
        await reloadRecent()
    }

    public func rotatePeers() async {
        do {
            try await syncStatus.rotatePeers()
        } catch {
            errorMessage = ErrorText.common(error.code)
        }
    }

    /// Opens the transaction on the Transactions page.
    public func open(_ transaction: RecentTransaction) {
        route = .transaction(txid: transaction.id.txid)
    }

    // MARK: Private

    private func observeHistoryOfSelectedWallet() {
        let selected = walletState.selectedWalletID
        guard selected != observedWallet else { return }
        historyTask?.cancel()
        observedWallet = selected
        guard let selected else { return }
        let changes = history.changes(wallet: selected)
        historyTask = Task { [weak self] in
            for await _ in changes {
                guard let self else { return }
                await self.reloadRecent()
            }
        }
    }

    private func reloadRecent() async {
        guard !discreet, let wallet = walletState.selectedWalletID else {
            recent = []
            return
        }
        let filter = HistoryFilter(
            types: Set(TxType.allCases).subtracting(Self.hiddenRecentTypes),
            statuses: Set(TxStatusKind.allCases).subtracting([.conflicted]))
        let query = HistoryQuery(
            filter: filter, sort: .newestFirst, limit: Self.recentLimit(coinJoin: features.coinJoin))
        do {
            let page = try await history.page(wallet: wallet, query: query)
            // Discreet mode or the selected wallet may have changed while
            // the page loaded; those rows must not be shown.
            guard !discreet, walletState.selectedWalletID == wallet else { return }
            recent = page.records.map(makeRecent)
            errorMessage = nil
        } catch {
            errorMessage = ErrorText.common(error.code)
        }
    }

    private func makeRecent(_ record: TxRecord) -> RecentTransaction {
        let display = settings.display
        var text = amounts.format(
            record.amount, unit: display.unit,
            style: .floored(plusSign: true, separators: .always, digits: display.decimalDigits))
        if !record.countsTowardBalance { text = "[\(text)]" }
        return RecentTransaction(
            id: record.id, date: record.date, amount: record.amount, amountText: text,
            title: record.label.flatMap { $0.isEmpty ? nil : $0 } ?? record.address ?? "",
            instantLocked: record.status.instantLocked, chainLocked: record.status.chainLocked,
            watchOnly: record.involvesWatchOnly, isIncoming: record.amount.duffs >= 0)
    }

    private func rebuildRows() {
        let display = settings.display
        func text(_ amount: Amount?) -> String {
            guard let amount else { return L10n.Common.unknown }
            if discreet {
                // dash-qt `floorWithPrivacy`: every digit of a zero amount shown as `#`.
                return amounts.format(
                    .zero, unit: display.unit,
                    style: .floored(plusSign: false, separators: .always, digits: display.decimalDigits)
                ).replacingOccurrences(of: "0", with: "#")
            }
            return amounts.format(
                amount, unit: display.unit,
                style: .floored(plusSign: false, separators: .always, digits: display.decimalDigits))
        }
        let b = balances
        var result = [
            BalanceRow(
                kind: .available, title: L10n.Home.available, tooltip: L10n.Home.availableTooltip,
                amount: b?.confirmed, text: text(b?.confirmed)),
            BalanceRow(
                kind: .pending, title: L10n.Home.pending, tooltip: L10n.Home.pendingTooltip,
                amount: b?.unconfirmed, text: text(b?.unconfirmed)),
        ]
        if let immature = b?.immature, immature.duffs != 0 {
            result.append(
                BalanceRow(
                    kind: .immature, title: L10n.Home.immature, tooltip: L10n.Home.immatureTooltip,
                    amount: immature, text: text(immature)))
        }
        result.append(BalanceRow(kind: .total, title: L10n.Home.total, tooltip: nil, amount: b?.total, text: text(b?.total)))
        rows = result
        formattedTotal = b == nil ? nil : text(b?.total)
    }

    private func updateSyncText() {
        guard let status = sync, status.running else {
            syncText = L10n.Home.notConnected
            return
        }
        if status.isDone {
            syncText = L10n.Home.synced
        } else if status.connectedPeers == 0 {
            syncText = L10n.Home.connectingToPeers
        } else if let phase = status.activePhase, let progress = status.progress {
            let percent = Int((min(max(progress, 0), 1) * 100).rounded(.down))
            syncText = L10n.Home.syncingPhase(Self.phaseName(phase), percent: percent)
        } else {
            syncText = L10n.Home.synchronizing
        }
    }

    static func phaseName(_ phase: SyncPhase) -> String {
        switch phase {
        case .headers: L10n.Home.headers
        case .filterHeaders: L10n.Home.filterHeaders
        case .filters: L10n.Home.filters
        case .masternodes: L10n.Home.masternodes
        }
    }
}
