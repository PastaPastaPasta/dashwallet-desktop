// Overview / Home (QT-034, QT-036…039, IOS-019…023).
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Home view model")
struct HomeViewModelTests {
    let world = FakeWorld()

    func makeModel(network: DashNetwork = .testnet, features: FeatureFlags = .m1) -> HomeViewModel {
        HomeViewModel(env: world.environment(), network: network, features: features)
    }

    @Test func IOS019_unknownBalancesAreNotZero() {
        let model = makeModel()
        #expect(model.balances == nil)
        #expect(model.formattedTotal == nil)
        #expect(model.rows.map(\.kind) == [.available, .pending, .total])
        #expect(model.rows.allSatisfy { $0.amount == nil && $0.text == L10n.Common.unknown })
    }

    @Test func QT034_QT036_balancesFlooredToDecimalDigits() async {
        world.walletState.balances = balances(confirmed: 123_456_789, unconfirmed: 1_999)
        let model = makeModel()
        await model.refresh()
        #expect(model.rows.map(\.text) == ["1.23 tDASH", "0.00 tDASH", "1.23 tDASH"])
        #expect(model.rows.map(\.title) == [L10n.Home.available, L10n.Home.pending, L10n.Home.total])
        #expect(model.formattedTotal == "1.23 tDASH")
        world.settings.display.decimalDigits = 8
        await model.refresh()
        #expect(model.formattedTotal == "1.23458788 tDASH")
    }

    @Test func QT034_immatureRowOnlyWhenNonZero() async {
        world.walletState.balances = balances(confirmed: 100_000_000, immature: 50_000_000)
        let model = makeModel()
        await model.refresh()
        #expect(model.rows.map(\.kind) == [.available, .pending, .immature, .total])
    }

    @Test func QT039_discreetModeMasksDigitsHidesRecentAndPersists() async {
        world.walletState.balances = balances(confirmed: 123_456_789)
        world.history.state.withLock { $0.records = [record(txid(1), amount: 5_000)] }
        let model = makeModel()
        await model.refresh()
        #expect(model.recent.count == 1)
        await model.toggleDiscreet()
        #expect(model.discreet)
        #expect(world.settings.updates.last?.hideBalances == true)
        #expect(model.formattedTotal == "#.## tDASH")
        #expect(model.rows.first?.text == "#.## tDASH")
        #expect(model.recent.isEmpty)
        #expect(!model.recentVisible)
        await model.toggleDiscreet()
        #expect(model.formattedTotal == "1.23 tDASH")
    }

    @Test func IOS020_discreetToggleFailureKeepsState() async {
        world.settings.updateError = ServiceError(code: .settingsWriteFailed)
        let model = makeModel()
        await model.toggleDiscreet()
        #expect(!model.discreet)
        #expect(model.errorMessage == L10n.Settings.settingsNotSaved)
    }

    @Test func QT038_recentListHidesInternalTypesAndConflicted() async {
        world.history.state.withLock {
            $0.records = [
                record(txid(1), type: .recvWithAddress, amount: 100_000_000, label: "Salary"),
                record(txid(2), type: .sendToAddress, amount: -50_000_000, counts: false),
            ]
        }
        let model = makeModel()
        await model.refresh()
        let query = world.history.state.current.queries.last
        #expect(query?.limit == 5)
        #expect(query?.sort == .newestFirst)
        #expect(query?.filter.types.isDisjoint(with: HomeViewModel.hiddenRecentTypes) == true)
        #expect(query?.filter.types.contains(.recvWithAddress) == true)
        #expect(query?.filter.statuses.contains(.conflicted) == false)
        #expect(model.recent.map(\.title) == ["Salary", testnetAddress1])
        #expect(model.recent.map(\.amountText) == ["+1.00 tDASH", "[-0.50 tDASH]"])
        #expect(model.recent.map(\.isIncoming) == [true, false])
        model.open(model.recent[0])
        #expect(model.route == .transaction(txid: txid(1)))
    }

    @Test func QT038_sixRowsWithCoinJoin() async {
        let model = makeModel(features: FeatureFlags(coinJoin: true))
        await model.refresh()
        #expect(world.history.state.current.queries.last?.limit == 6)
    }

    @Test func QT037_IOS023_syncTextAndStall() async {
        world.sync.status = FakeSync.syncing(phase: .filters, progress: 0.426)
        let model = makeModel()
        #expect(model.outOfSync)
        #expect(model.syncText == "Syncing Filters (42%)…")
        #expect(!model.showChangePeers)
        model.start()
        world.sync.publish(FakeSync.syncing(phase: .filters, progress: 0.43, stalled: true))
        await eventually { model.showChangePeers }
        await model.rotatePeers()
        #expect(world.sync.rotateCount == 1)
        world.sync.publish(FakeSync.synced())
        await eventually { !model.outOfSync }
        #expect(model.syncText == L10n.Home.synced)
        world.sync.publish(FakeSync.syncing(phase: .headers, progress: 0.1, peers: 0))
        await eventually { model.syncText == L10n.Home.connectingToPeers }
        model.stop()
    }

    @Test func notConnectedBeforeTheFirstSnapshot() {
        world.sync.status = nil
        let model = makeModel()
        #expect(model.syncText == L10n.Home.notConnected)
        #expect(model.outOfSync)
    }

    @Test func IOS022_networkBadgeOffMainnet() {
        #expect(makeModel(network: .testnet).networkBadge == .testnet)
        #expect(makeModel(network: .mainnet).networkBadge == nil)
    }

    @Test func followsBalanceAndHistoryChanges() async {
        let model = makeModel()
        model.start()
        world.walletState.balances = balances(confirmed: 200_000_000)
        world.walletState.notify()
        await eventually { model.formattedTotal == "2.00 tDASH" }
        await eventually { world.history.broadcast.subscriberCount == 1 }
        world.history.state.withLock { $0.records = [record(txid(9), amount: 1)] }
        world.history.broadcast.send([txid(9)])
        await eventually { model.recent.count == 1 }
        try? world.settings.update(DisplaySettings(unit: .duffs, decimalDigits: 2, hideBalances: false))
        await eventually { model.formattedTotal == "200\u{2009}000\u{2009}000 tduffs" }
        model.stop()
    }
}
