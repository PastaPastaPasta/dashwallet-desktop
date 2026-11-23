// Transactions page M2: actions, details, copy, links, CSV through the
// engine, iOS day groups and chips (QT-075, QT-090…094, IOS-027…034).
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Transactions view model (M2)")
struct TransactionsM2Tests {
    let world = FakeWorld()
    let m2 = FakeM2World()

    func makeModel(network: DashNetwork? = .testnet) -> TransactionsViewModel {
        TransactionsViewModel(env: world.environment(), m2: m2.services, network: network)
    }

    func extras(
        _ id: String, canAbandon: Bool = true, canResend: Bool = true, inMempool: Bool? = nil,
        dust: [OutPoint] = []
    ) -> TransactionDetailExtras {
        TransactionDetailExtras(
            txid: id, isCoinbase: false, totalCredit: .zero, totalDebit: Amount(duffs: 100_000_226),
            net: Amount(duffs: -100_000_226), maturesIn: nil, inMempool: inMempool, abandoned: false,
            canAbandon: canAbandon, canResend: canResend, dustLockedOutputs: dust, lastAnnouncedAt: nil)
    }

    func detail(_ id: String, kind: TxStatusKind = .unconfirmed, confirmations: UInt32 = 0) -> TransactionDetail {
        let status = TxStatus(kind: kind, confirmations: confirmations, instantLocked: false, chainLocked: false, maturesIn: nil)
        return TransactionDetail(
            txid: id, records: [record(id, type: .sendToAddress, amount: -100_000_226, status: status)], status: status,
            date: Date(timeIntervalSince1970: 1_760_000_000), blockHeight: nil, blockHash: nil,
            fee: Amount(duffs: 226), sizeBytes: 226,
            inputs: [TxInput(previousOutput: OutPoint(txid: txid(99), vout: 0), address: testnetAddress1, amount: nil, isMine: true)],
            outputs: [TxOutput(vout: 0, address: testnetAddress2, amount: Amount(duffs: 100_000_000), isMine: false, isChange: false, dataHex: nil)],
            message: nil, label: "Rent", rawHex: "0200abcd")
    }

    func setUp(_ id: String, extras value: TransactionDetailExtras) {
        world.history.state.withLock {
            $0.records = [record(id, type: .sendToAddress, amount: -100_000_226)]
            $0.details[id] = detail(id)
        }
        m2.actions.extras.withLock { $0[id] = value }
    }

    // MARK: Abandon / resend (QT-091, IOS-034)

    @Test func QT091_abandonWarnsThatItMayStillConfirmOnSPV() async {
        let id = txid(1)
        setUp(id, extras: extras(id))
        let model = makeModel()
        await model.reload()
        await model.select(TxRecord.ID(txid: id, recordIndex: 0))
        #expect(model.canAbandon)
        model.requestAbandon()
        #expect(model.actionState == .confirmingAbandon(txid: id, mayStillConfirm: true))
        await model.confirmAbandon()
        #expect(m2.actions.abandoned.current == [id])
        #expect(model.actionState == .done(L10n.TransactionsM2.abandoned))
    }

    @Test func QT091_abandonIsDisabledWhenTheEngineSaysSo() async {
        let id = txid(2)
        setUp(id, extras: extras(id, canAbandon: false, inMempool: true))
        let model = makeModel()
        await model.select(TxRecord.ID(txid: id, recordIndex: 0))
        #expect(!model.canAbandon)
        model.requestAbandon()
        #expect(model.actionState == .idle)
    }

    @Test func QT091_refusalSelectsItsCopy() async {
        let id = txid(3)
        setUp(id, extras: extras(id))
        m2.actions.refusals.withLock { $0[id] = .instantLocked }
        let model = makeModel()
        await model.select(TxRecord.ID(txid: id, recordIndex: 0))
        await model.resend()
        #expect(model.actionState == .failed("This transaction is locked by InstantSend."))
    }

    @Test func QT091_resendNeedsASingleSelection() async {
        let id = txid(4)
        setUp(id, extras: extras(id))
        let model = makeModel()
        await model.select(TxRecord.ID(txid: id, recordIndex: 0))
        await model.resend()
        #expect(m2.actions.resent.current == [id])
        model.toggleSelection(TxRecord.ID(txid: txid(5), recordIndex: 0))
        #expect(!model.canResend)
    }

    @Test func QT091_extrasNotImplementedLeavesActionsDisabled() async {
        let id = txid(6)
        world.history.state.withLock { $0.details[id] = detail(id) }
        let model = makeModel()
        await model.select(TxRecord.ID(txid: id, recordIndex: 0))
        #expect(model.extras == nil && !model.canAbandon && !model.canResend)
        #expect(model.errorMessage == nil)
    }

    @Test func QT075_unlockDustReleasesTheLockedOutputs() async {
        let id = txid(7)
        let dust = OutPoint(txid: id, vout: 1)
        setUp(id, extras: extras(id, dust: [dust]))
        world.coinControl.coins.withLock { $0 = [] }
        let model = makeModel()
        await model.select(TxRecord.ID(txid: id, recordIndex: 0))
        #expect(model.canUnlockDust)
        await model.unlockDust()
        #expect(world.coinControl.unlockCalls.current == [[dust]])
        #expect(model.actionState == .done(L10n.TransactionsM2.dustUnlocked))
    }

    // MARK: Copy, details, links (QT-090, QT-092, QT-094, IOS-032)

    @Test func QT090_copyEntriesFollowDashQt() async {
        let id = txid(8)
        setUp(id, extras: extras(id))
        let model = makeModel()
        await model.select(TxRecord.ID(txid: id, recordIndex: 0))
        let row = record(
            id, type: .sendToAddress, amount: -100_000_226, date: Date(timeIntervalSince1970: 1_760_000_000),
            label: "Rent", address: testnetAddress2)
        #expect(model.copyAmount(row) == "-1.00000226")
        #expect(model.copyRawTransaction() == "0200abcd")
        #expect(model.copyFullDetails(row) == "10/9/25 08:53 Confirmed (10 confirmations), locked via ChainLocks. Sent to (Rent) \(testnetAddress2) -1.00000226")
        let unlabeled = record(id, type: .recvWithAddress, amount: 5, address: testnetAddress1)
        #expect(model.copyFullDetails(unlabeled).contains("(no label) \(testnetAddress1)"))
    }

    @Test func QT092_detailStatusNeverClaimsNotInMempoolOnSPV() async {
        let id = txid(9)
        setUp(id, extras: extras(id))
        let model = makeModel()
        await model.select(TxRecord.ID(txid: id, recordIndex: 0))
        #expect(model.detailStatusText() == "0/unconfirmed")
        m2.actions.extras.withLock { $0[id] = extras(id, inMempool: false) }
        await model.select(TxRecord.ID(txid: id, recordIndex: 0))
        #expect(model.detailStatusText() == "0/unconfirmed, not in memory pool")
        let fields = model.detailFields.map(\.title)
        #expect(fields.first == "Status")
        #expect(fields.contains("To") && fields.contains("Transaction fee") && fields.contains("Net amount"))
        #expect(fields.contains("Comment") && fields.contains("Transaction ID") && fields.contains("Transaction total size"))
    }

    @Test func QT094_thirdPartyLinksReplaceTxid() {
        m2.desktopPreferences.desktop.options.thirdPartyTxURLs = "https://a.example/tx/%s|bad|https://b.example/%s"
        let model = makeModel()
        let links = model.thirdPartyLinks(for: txid(1))
        #expect(links.map(\.title) == ["Show in a.example", "Show in b.example"])
        #expect(links[0].url.absoluteString == "https://a.example/tx/\(txid(1))")
    }

    @Test func IOS032_explorerLinksPerNetwork() {
        #expect(makeModel(network: .mainnet).explorerLinks(for: txid(1)).map(\.title) == ["Insight", "Blockchair"])
        #expect(makeModel(network: .testnet).explorerLinks(for: txid(1)).count == 1)
        #expect(makeModel(network: .regtest).explorerLinks(for: txid(1)).isEmpty)
    }

    // MARK: CSV (QT-093)

    @Test func QT093_exportUsesTheEngineBytesWithTypeNames() async throws {
        m2.actions.csv.withLock { $0 = Data("\"Confirmed\"\n".utf8) }
        let model = makeModel()
        let csv = try await model.exportCSV()
        #expect(csv == "\"Confirmed\"\n")
        #expect(m2.actions.csvCalls.current.first?.1.typeNames.count == 19)
        #expect(m2.actions.csvCalls.current.first?.1.typeNames[1] == "Mined")
    }

    @Test func QT093_exportFallsBackToTheSwiftWriterWhileNotImplemented() async throws {
        world.history.state.withLock { $0.records = [record(txid(1), amount: 5)] }
        let csv = try await makeModel().exportCSV()
        #expect(csv.hasPrefix("\"Confirmed\",\"Date\""))
    }

    // MARK: iOS history (IOS-027, IOS-028, IOS-030)

    @Test func IOS027_rowsAreGroupedByDayNewestFirstWithUnknownLast() async {
        let day1 = Date(timeIntervalSince1970: 1_760_000_000)
        let day0 = day1.addingTimeInterval(-86_400)
        world.history.state.withLock {
            $0.records = [
                record(txid(1), amount: 5, date: day1),
                record(txid(2), amount: 6, date: day1.addingTimeInterval(-60)),
                record(txid(3), amount: 7, date: day0),
                record(txid(4), amount: 8, date: nil),
            ]
        }
        let model = makeModel()
        await model.reload()
        let groups = model.dayGroups
        #expect(groups.count == 3)
        #expect(groups[0].items.count == 2)
        #expect(groups[2].day == nil && groups[2].title == "Date unknown")
    }

    @Test func IOS030_coinJoinMixingRowsCollapsePerDay() async {
        let day = Date(timeIntervalSince1970: 1_760_000_000)
        world.history.state.withLock {
            $0.records = [
                record(txid(1), type: .coinJoinMixing, amount: -10, date: day),
                record(txid(2), type: .recvWithAddress, amount: 500, date: day),
                record(txid(3), type: .coinJoinCreateDenominations, amount: -20, date: day),
            ]
        }
        let model = makeModel()
        await model.reload()
        let items = model.dayGroups[0].items
        #expect(items.count == 2)
        guard case .coinJoinMixing(let row) = items[0] else {
            Issue.record("first item is not the mixing row")
            return
        }
        #expect(row.records.count == 2 && row.total == Amount(duffs: -30))
        #expect(row.title == "Mixing Transactions")
    }

    @Test func IOS028_chipsFilterByCategoryAndOfferRewardsOnlyWithHistory() async {
        world.history.state.withLock {
            $0.records = [record(txid(1), amount: 5), record(txid(2), amount: -5)]
        }
        let model = makeModel()
        await model.refreshChips()
        #expect(model.offeredChips == [.sent, .received])
        #expect(model.filter.categories.isEmpty)
        await model.selectOnlyChip(.received)
        #expect(model.filter.categories == [.received])
        #expect(model.rows.map(\.id.txid) == [txid(1)])
        await model.selectAllChips()
        #expect(model.filter.categories.isEmpty && model.rows.count == 2)
    }
}
