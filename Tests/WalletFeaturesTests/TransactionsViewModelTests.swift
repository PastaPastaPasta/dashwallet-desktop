// Transactions page: dash-qt filters, paging, details, CSV (QT-086…093, IOS-027…031).
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Transactions view model")
struct TransactionsViewModelTests {
    let world = FakeWorld()

    func makeModel(features: FeatureFlags = .m1) -> TransactionsViewModel {
        TransactionsViewModel(env: world.environment(), features: features)
    }

    var utc: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        return calendar
    }

    func date(_ y: Int, _ m: Int, _ d: Int, _ h: Int = 0) -> Date {
        utc.date(from: DateComponents(year: y, month: m, day: d, hour: h))!
    }

    // MARK: Filters (QT-089)

    @Test func QT089_dateFilterBoundsFollowDashQt() {
        let now = date(2026, 10, 8, 15)  // a Thursday
        let all = DateFilterPreset.all.bounds(now: now, calendar: utc)
        #expect(all.from == nil && all.until == nil)
        #expect(DateFilterPreset.today.bounds(now: now, calendar: utc).from == date(2026, 10, 8))
        #expect(DateFilterPreset.thisWeek.bounds(now: now, calendar: utc).from == date(2026, 10, 5))  // Monday
        #expect(DateFilterPreset.thisMonth.bounds(now: now, calendar: utc).from == date(2026, 10, 1))
        let lastMonth = DateFilterPreset.lastMonth.bounds(now: now, calendar: utc)
        #expect(lastMonth.from == date(2026, 9, 1) && lastMonth.until == date(2026, 10, 1))
        #expect(DateFilterPreset.thisYear.bounds(now: now, calendar: utc).from == date(2026, 1, 1))
        let range = DateFilterPreset.range.bounds(now: now, calendar: utc, rangeFrom: date(2026, 1, 2), rangeUntil: date(2026, 1, 5))
        #expect(range.from == date(2026, 1, 2) && range.until == date(2026, 1, 5))
        // Monday itself and Sunday both map to that week's Monday.
        #expect(DateFilterPreset.thisWeek.bounds(now: date(2026, 10, 5, 1), calendar: utc).from == date(2026, 10, 5))
        #expect(DateFilterPreset.thisWeek.bounds(now: date(2026, 10, 11, 23), calendar: utc).from == date(2026, 10, 5))
    }

    @Test func QT089_typeMenuHasSeventeenEntriesAndHidesCoinJoinWhenOff() {
        #expect(TypeFilterPreset.allCases.count == 17)
        #expect(makeModel().typeMenu.count == 12)
        #expect(!makeModel().typeMenu.contains(.coinJoinMixing))
        #expect(makeModel(features: FeatureFlags(coinJoin: true)).typeMenu.count == 17)
        #expect(TypeFilterPreset.sentTo.types == [.sendToAddress, .sendToOther])
        #expect(TypeFilterPreset.all.types.isEmpty)
        #expect(L10n.Transactions.typeFilterName(.mostCommon) == "Most Common")
    }

    @Test func QT089_dateAndTypeFiltersArePersisted() async {
        let model = makeModel()
        await model.setDatePreset(.thisMonth)
        await model.setTypePreset(.receivedWith)
        #expect(world.preferences.preferences.transactionDate == .thisMonth)
        #expect(world.preferences.preferences.transactionType == .receivedWith)
        await model.setRange(from: date(2026, 1, 1), until: date(2026, 2, 1))
        #expect(world.preferences.preferences.transactionDate == .range)
        #expect(world.preferences.preferences.transactionDateTo == date(2026, 2, 1))
        let restored = makeModel()
        #expect(restored.datePreset == .range)
        #expect(restored.typePreset == .receivedWith)
        #expect(restored.filter.from == date(2026, 1, 1) && restored.filter.until == date(2026, 2, 1))
        #expect(restored.filter.types == [.recvWithAddress, .recvFromOther])
    }

    @Test func QT089_persistedCoinJoinTypeFallsBackToAllWhileCoinJoinIsOff() async {
        world.preferences.preferences.transactionType = .coinJoinMixing
        #expect(makeModel().typePreset == .all)
        #expect(makeModel(features: FeatureFlags(coinJoin: true)).typePreset == .coinJoinMixing)
        let model = makeModel()
        await model.setTypePreset(.coinJoinSend)
        #expect(model.typePreset == .all)
    }

    @Test func QT089_searchIsDebounced() async {
        let model = makeModel()
        model.setSearchText("ab")
        model.setSearchText("abc")
        await eventually { world.sleeper.pendingCount == 1 }
        #expect(world.history.state.current.queries.isEmpty)
        world.sleeper.fireNext()
        await eventually { world.history.state.current.queries.count == 1 }
        #expect(world.history.state.current.queries.first?.filter.text == "abc")
        #expect(world.sleeper.requested.current.allSatisfy { $0 == TransactionsViewModel.searchDebounce })
    }

    @Test func QT089_minimumAmountAndWatchOnlyFilters() async {
        let model = makeModel()
        await model.setMinimumAmountText("-0,5")
        #expect(model.filter.minimumAmount == Amount(duffs: 50_000_000))
        await model.setMinimumAmountText("x")
        #expect(model.minimumAmountError == L10n.Transactions.invalidMinAmount)
        await model.setWatchOnly(.yes)
        // The watch-only filter applies only to watch-only wallets (QT-088).
        #expect(model.filter.watchOnly == .all)
        world.walletState.wallets = [walletInfo(walletA, watchOnly: true)]
        #expect(model.showsWatchOnly)
        #expect(model.filter.watchOnly == .yes)
    }

    // MARK: Paging, selection, detail (QT-088, QT-092)

    @Test func IOS027_pagesThroughHistory() async {
        world.history.state.withLock { s in s.records = (1...250).map { record(txid($0), amount: Int64($0)) } }
        let model = makeModel()
        await model.reload()
        #expect(model.rows.count == 100)
        #expect(model.hasMore)
        #expect(model.totalMatching == 250)
        await model.loadMore()
        await model.loadMore()
        #expect(model.rows.count == 250)
        #expect(!model.hasMore)
    }

    @Test func staleCursorWhileLoadingMoreReloads() async {
        world.history.state.withLock { s in s.records = (1...150).map { record(txid($0), amount: 1) } }
        let model = makeModel()
        await model.reload()
        world.history.state.withLock { $0.pageErrors = [ServiceError(code: .historyStaleCursor)] }
        await model.loadMore()
        #expect(model.rows.count == 100)
        #expect(world.history.state.current.queries.last?.cursor == nil)
        #expect(model.errorMessage == nil)
    }

    @Test func QT088_selectionTotalAndAmountText() async {
        world.history.state.withLock {
            $0.records = [record(txid(1), amount: 150_000_000), record(txid(2), amount: -20_000_000, counts: false)]
        }
        let model = makeModel()
        await model.reload()
        #expect(model.selectedTotal == nil)
        model.toggleSelection(model.rows[0].id)
        model.toggleSelection(model.rows[1].id)
        #expect(model.selectedTotal == Amount(duffs: 130_000_000))
        model.toggleSelection(model.rows[1].id)
        #expect(model.selectedTotal == Amount(duffs: 150_000_000))
        #expect(model.amountText(for: model.rows[0]) == "+1.50000000 tDASH")
        #expect(model.amountText(for: model.rows[1]) == "[-0.20000000 tDASH]")
        #expect(model.addressText(for: model.rows[0]) == testnetAddress1)
        #expect(model.typeText(for: model.rows[0]) == "Received with")
    }

    @Test func QT087_statusTexts() {
        let confirming = TxStatus(kind: .confirming, confirmations: 2, instantLocked: true, chainLocked: false, maturesIn: nil)
        #expect(L10n.Transactions.statusText(confirming) == "Confirming (2 of 6 recommended confirmations), verified via InstantSend")
        let immature = TxStatus(kind: .immature, confirmations: 10, instantLocked: false, chainLocked: true, maturesIn: 90)
        #expect(L10n.Transactions.statusText(immature)
            == "Immature (10 confirmations, will be available after 90 more blocks), locked via ChainLocks")
        #expect(L10n.Transactions.typeName(.coinJoinMakeCollaterals) == "CoinJoin Make Collateral Inputs")
        #expect(L10n.Transactions.typeName(.other).isEmpty)
    }

    @Test func QT092_selectLoadsDetailAndRevealSelectsAllRecords() async {
        let detail = TransactionDetail(
            txid: txid(1), records: [], status: TxStatus(kind: .confirmed, confirmations: 7, instantLocked: false, chainLocked: true, maturesIn: nil),
            date: nil, blockHeight: 10, blockHash: nil, fee: nil, sizeBytes: 200, inputs: [], outputs: [], message: nil,
            label: nil, rawHex: "00")
        world.history.state.withLock {
            $0.records = [record(txid(1), amount: 1, index: 0), record(txid(1), amount: 2, index: 1), record(txid(2), amount: 3)]
            $0.details[txid(1)] = detail
        }
        let model = makeModel()
        await model.reload()
        await model.select(model.rows[2].id)
        #expect(model.detail == nil)
        #expect(model.errorMessage == L10n.Common.unexpected)
        await model.reveal(txid: txid(1))
        #expect(model.selection.count == 2)
        #expect(model.detail == detail)
    }

    @Test func setLabelReloads() async {
        let model = makeModel()
        await model.setLabel("", txid: txid(1))
        #expect(world.history.state.current.labels.first?.1 == nil)
        await model.setLabel("Rent", txid: txid(1))
        #expect(world.history.state.current.labels.last?.1 == "Rent")
    }

    @Test func reloadsOnHistoryChanges() async {
        let model = makeModel()
        model.start()
        await eventually { world.history.broadcast.subscriberCount == 1 }
        world.history.state.withLock { $0.records = [record(txid(5), amount: 1)] }
        world.history.broadcast.send([txid(5)])
        await eventually { model.rows.count == 1 }
        model.stop()
    }

    // MARK: CSV (QT-093)

    @Test func QT093_csvColumnsAndQuoting() {
        let records = [
            record(txid(1), type: .recvWithAddress, amount: 150_000_000, date: date(2026, 3, 4, 5), label: "Say \"hi\""),
            record(txid(2), type: .sendToAddress, amount: -20_000_000, counts: false, watchOnly: true),
        ]
        let csv = TransactionCSV.make(
            records: records, unit: .dash, amounts: world.amounts, watchOnlyColumn: false,
            timeZone: TimeZone(identifier: "UTC")!)
        let expected = [
            #""Confirmed","Date","Type","Label","Address","Amount (tDASH)","ID""#,
            #""true","2026-03-04T05:00:00","Received with","Say ""hi""","\#(testnetAddress1)","1.50000000","\#(txid(1))""#,
            #""false","","Sent to","","\#(testnetAddress1)","-0.20000000","\#(txid(2))""#,
        ].map { $0 + "\n" }.joined()
        #expect(csv == expected)
        let watch = TransactionCSV.make(
            records: [records[1]], unit: .duffs, amounts: world.amounts, watchOnlyColumn: true, timeZone: .gmt)
        #expect(watch.hasPrefix("\"Confirmed\",\"Watch-only\",\"Date\",\"Type\",\"Label\",\"Address\",\"Amount (tduffs)\",\"ID\"\n"))
        #expect(watch.contains("\"false\",\"true\",\"\""))
        #expect(watch.contains("\"-20000000\""))
    }

    @Test func QT093_exportPagesThroughTheFilteredHistory() async throws {
        world.history.state.withLock { s in s.records = (1...230).map { record(txid($0), amount: 1) } }
        let model = makeModel()
        let csv = try await model.exportCSV()
        #expect(csv.split(separator: "\n").count == 231)
    }

    @Test func exportRestartsWhenTheHistoryChangesMidway() async throws {
        world.history.state.withLock { s in
            s.records = (1...150).map { record(txid($0), amount: 1) }
        }
        // The second page fails once, as if a sync had changed the history.
        world.history.state.withLock { $0.failAtQuery[2] = ServiceError(code: .historyStaleCursor) }
        let model = makeModel()
        let csv = try await model.exportCSV()
        #expect(csv.split(separator: "\n").count == 151)
        let cursors = world.history.state.current.queries.map(\.cursor)
        #expect(cursors == [nil, "100", nil, "100"])
    }

    @Test func exportGivesUpAfterRepeatedStaleCursors() async {
        world.history.state.withLock { s in
            s.records = (1...10).map { record(txid($0), amount: 1) }
            s.pageErrors = Array(repeating: ServiceError(code: .historyStaleCursor), count: TransactionsViewModel.exportAttempts)
        }
        let model = makeModel()
        await #expect(throws: ServiceError(code: .historyStaleCursor)) { try await model.exportCSV() }
        world.history.state.withLock {
            $0.pageErrors = Array(repeating: ServiceError(code: .historyStaleCursor), count: TransactionsViewModel.exportAttempts - 1)
        }
        let csv = try? await model.exportCSV()
        #expect(csv?.split(separator: "\n").count == 11)
    }
}
