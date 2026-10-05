// Transactions page (QT-086…093, IOS-027…031).
import Foundation
import Observation
import WalletRuntime

@MainActor
@Observable
public final class TransactionsViewModel {
    public static let pageSize = 100
    /// dash-qt debounces search typing by 200 ms.
    public static let searchDebounce: Duration = .milliseconds(200)

    public private(set) var datePreset: DateFilterPreset
    public private(set) var rangeFrom: Date?
    public private(set) var rangeUntil: Date?
    public private(set) var typePreset: TypeFilterPreset
    public private(set) var searchText = ""
    public private(set) var minimumAmountText = ""
    public private(set) var minimumAmountError: String?
    public private(set) var watchOnly: WatchOnlyFilter = .all

    public private(set) var rows: [TxRecord] = []
    public private(set) var hasMore = false
    public private(set) var totalMatching: Int?
    public private(set) var selection: Set<TxRecord.ID> = []
    public private(set) var detail: TransactionDetail?
    public private(set) var errorMessage: String?
    public private(set) var isLoading = false

    /// The query filter the page shows.
    public var filter: HistoryFilter {
        let bounds = datePreset.bounds(
            now: timing.now(), calendar: timing.calendar, rangeFrom: rangeFrom, rangeUntil: rangeUntil)
        let text = searchText.trimmingCharacters(in: .whitespaces)
        return HistoryFilter(
            types: typePreset.types, from: bounds.from, until: bounds.until, text: text.isEmpty ? nil : text,
            minimumAmount: minimumAmount, watchOnly: showsWatchOnly ? watchOnly : .all)
    }

    /// Watch-only filter and column appear only for wallets that have
    /// watch-only data (QT-088/089).
    public var showsWatchOnly: Bool {
        guard let id = walletState.selectedWalletID else { return false }
        return walletState.wallets?.first(where: { $0.id == id })?.watchOnly ?? false
    }

    /// Sum of the selected rows; `nil` with nothing selected (QT-088).
    public var selectedTotal: Amount? {
        guard !selection.isEmpty else { return nil }
        return Amount(duffs: rows.filter { selection.contains($0.id) }.reduce(0) { $0 + $1.amount.duffs })
    }

    public var typeMenu: [TypeFilterPreset] { TypeFilterPreset.menu(coinJoinEnabled: features.coinJoin) }

    private let walletState: any WalletStateProviding
    private let history: any HistoryProviding
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding
    private let preferences: any UIPreferencesStoring
    private let timing: Timing
    private let features: FeatureFlags
    private var minimumAmount: Amount?
    private var nextCursor: String?
    private var searchTask: Task<Void, Never>?
    private var historyTask: Task<Void, Never>?
    private var loadGeneration = 0

    public init(
        walletState: any WalletStateProviding, history: any HistoryProviding, amounts: any AmountFormatting,
        settings: any SettingsProviding, preferences: any UIPreferencesStoring, timing: Timing,
        features: FeatureFlags = .m1
    ) {
        self.walletState = walletState
        self.history = history
        self.amounts = amounts
        self.settings = settings
        self.preferences = preferences
        self.timing = timing
        self.features = features
        let stored = preferences.preferences
        self.datePreset = stored.transactionDate
        self.rangeFrom = stored.transactionDateFrom
        self.rangeUntil = stored.transactionDateTo
        // A persisted CoinJoin entry falls back to All while CoinJoin is off.
        self.typePreset = stored.transactionType.isCoinJoin && !features.coinJoin ? .all : stored.transactionType
    }

    public convenience init(env: AppEnvironment, features: FeatureFlags = .m1) {
        self.init(
            walletState: env.walletState, history: env.history, amounts: env.amounts, settings: env.settings,
            preferences: env.preferences, timing: env.timing, features: features)
    }

    // MARK: Loading

    /// Loads the first page for the current filter.
    public func reload() async {
        loadGeneration += 1
        let generation = loadGeneration
        guard let wallet = walletState.selectedWalletID else {
            rows = []
            hasMore = false
            return
        }
        isLoading = true
        defer { if generation == loadGeneration { isLoading = false } }
        do {
            let page = try await history.page(wallet: wallet, query: query(cursor: nil))
            guard generation == loadGeneration else { return }
            rows = page.records
            nextCursor = page.nextCursor
            hasMore = page.nextCursor != nil
            totalMatching = page.totalMatching
            selection.formIntersection(Set(rows.map(\.id)))
            errorMessage = nil
        } catch {
            guard generation == loadGeneration else { return }
            errorMessage = ErrorText.common(error.code)
        }
    }

    /// Appends the next page; a stale cursor reloads from the top.
    public func loadMore() async {
        guard hasMore, let cursor = nextCursor, let wallet = walletState.selectedWalletID else { return }
        let generation = loadGeneration
        do {
            let page = try await history.page(wallet: wallet, query: query(cursor: cursor))
            guard generation == loadGeneration else { return }
            rows += page.records
            nextCursor = page.nextCursor
            hasMore = page.nextCursor != nil
        } catch {
            guard generation == loadGeneration else { return }
            if error.code == .historyStaleCursor {
                await reload()
            } else {
                errorMessage = ErrorText.common(error.code)
            }
        }
    }

    /// Reloads after history changes until `stop()`.
    public func start() {
        stop()
        guard let wallet = walletState.selectedWalletID else { return }
        let changes = history.changes(wallet: wallet)
        historyTask = Task { [weak self] in
            for await _ in changes {
                guard let self else { return }
                await self.reload()
                if let txid = self.detail?.txid { await self.loadDetail(txid: txid) }
            }
        }
    }

    public func stop() {
        historyTask?.cancel()
        historyTask = nil
        searchTask?.cancel()
        searchTask = nil
    }

    // MARK: Filters

    public func setDatePreset(_ preset: DateFilterPreset) async {
        datePreset = preset
        persist()
        await reload()
    }

    /// "Range…" bounds; `until` is exclusive (dash-qt).
    public func setRange(from: Date?, until: Date?) async {
        rangeFrom = from
        rangeUntil = until
        datePreset = .range
        persist()
        await reload()
    }

    public func setTypePreset(_ preset: TypeFilterPreset) async {
        guard typeMenu.contains(preset) else { return }
        typePreset = preset
        persist()
        await reload()
    }

    public func setWatchOnly(_ filter: WatchOnlyFilter) async {
        watchOnly = filter
        await reload()
    }

    /// Search by address, txid or label, debounced by 200 ms.
    public func setSearchText(_ text: String) {
        searchText = text
        searchTask?.cancel()
        let sleep = timing.sleep
        searchTask = Task { [weak self] in
            do { try await sleep(Self.searchDebounce) } catch { return }
            guard !Task.isCancelled, let self else { return }
            await self.reload()
        }
    }

    /// Minimum absolute amount in the display unit; "," is accepted as ".".
    public func setMinimumAmountText(_ text: String) async {
        minimumAmountText = text
        switch AmountInput.parse(text, unit: settings.display.unit, formatter: amounts) {
        case .success(let amount):
            minimumAmount = amount.map { Amount(duffs: Int64($0.duffs.magnitude)) }
            minimumAmountError = nil
            await reload()
        case .failure:
            minimumAmountError = L10n.Transactions.invalidMinAmount
        }
    }

    // MARK: Selection and detail

    /// Selects one row and loads its details (QT-092).
    public func select(_ id: TxRecord.ID) async {
        selection = [id]
        await loadDetail(txid: id.txid)
    }

    /// Adds or removes a row from a multi-selection (QT-088).
    public func toggleSelection(_ id: TxRecord.ID) {
        if selection.contains(id) { selection.remove(id) } else { selection.insert(id) }
    }

    /// Selects the record(s) of `txid` after a send or an Overview click.
    public func reveal(txid: String) async {
        if !rows.contains(where: { $0.id.txid == txid }) { await reload() }
        selection = Set(rows.filter { $0.id.txid == txid }.map(\.id))
        await loadDetail(txid: txid)
    }

    public func clearDetail() {
        detail = nil
    }

    public func setLabel(_ label: String?, txid: String) async {
        guard let wallet = walletState.selectedWalletID else { return }
        do {
            try await history.setLabel(wallet: wallet, txid: txid, label: label?.isEmpty == true ? nil : label)
            await reload()
            if detail?.txid == txid { await loadDetail(txid: txid) }
        } catch {
            errorMessage = ErrorText.common(error.code)
        }
    }

    // MARK: Presentation

    /// Amount column text: signed with unit, `[…]` when the record does not
    /// count toward the balance (QT-088).
    public func amountText(for record: TxRecord) -> String {
        let text = amounts.format(
            record.amount, unit: settings.display.unit, style: .withUnit(plusSign: true, separators: .always))
        return record.countsTowardBalance ? text : "[\(text)]"
    }

    public func typeText(for record: TxRecord) -> String { L10n.Transactions.typeName(record.type) }

    public func statusText(for record: TxRecord) -> String { L10n.Transactions.statusText(record.status) }

    /// Label, or the address when there is no label.
    public func addressText(for record: TxRecord) -> String {
        if let label = record.label, !label.isEmpty { return label }
        return record.address ?? ""
    }

    // MARK: Export

    /// CSV of everything the current filter matches, paging through history
    /// (QT-093). Write it as UTF-8.
    public func exportCSV() async throws(ServiceError) -> String {
        guard let wallet = walletState.selectedWalletID else {
            throw ServiceError(code: .walletNotFound, detail: "no wallet selected")
        }
        var records: [TxRecord] = []
        var cursor: String?
        repeat {
            let page = try await history.page(wallet: wallet, query: query(cursor: cursor))
            records += page.records
            cursor = page.nextCursor
        } while cursor != nil
        return TransactionCSV.make(
            records: records, unit: settings.display.unit, amounts: amounts, watchOnlyColumn: showsWatchOnly,
            timeZone: timing.timeZone)
    }

    // MARK: Private

    private func query(cursor: String?) -> HistoryQuery {
        HistoryQuery(filter: filter, sort: .newestFirst, cursor: cursor, limit: Self.pageSize)
    }

    private func loadDetail(txid: String) async {
        guard let wallet = walletState.selectedWalletID else { return }
        do {
            detail = try await history.detail(wallet: wallet, txid: txid)
        } catch {
            detail = nil
            errorMessage = ErrorText.common(error.code)
        }
    }

    private func persist() {
        var stored = preferences.preferences
        stored.transactionDate = datePreset
        stored.transactionDateFrom = rangeFrom
        stored.transactionDateTo = rangeUntil
        stored.transactionType = typePreset
        do {
            try preferences.update(stored)
        } catch {
            errorMessage = L10n.Settings.settingsNotSaved
        }
    }
}
