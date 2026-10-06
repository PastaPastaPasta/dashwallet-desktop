// Coin control as Send's one source of truth (review M3, QT-052, QT-068…074):
// Send reads the Coin Selection dialog's selection when it builds the draft,
// Clear All and a sent payment empty it (dash-qt `UnSelectAll`), and a change
// that cannot apply is refused with the reason instead of being dropped.
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Send with coin control")
struct SendCoinControlTests {
    let world = FakeWorld()
    let m2 = FakeM2World()
    let first = OutPoint(txid: txid(1), vout: 0)
    let second = OutPoint(txid: txid(2), vout: 0)

    /// A Send page with its Coin Selection dialog attached and loaded, coin
    /// control features on.
    func make(coinControlOn: Bool = true) async -> (SendViewModel, CoinControlViewModel) {
        let coins = [utxo(1, amount: 300_000_000), utxo(2, amount: 200_000_000)]
        world.coinControl.coins.withLock { $0 = coins }
        m2.fees.coins.withLock { $0 = Dictionary(uniqueKeysWithValues: coins.map { ($0.outpoint, $0.amount) }) }
        m2.desktopPreferences.desktop.options.coinControl = coinControlOn
        let send = SendViewModel(env: world.environment(), network: world.network)
        let coinControl = CoinControlViewModel(env: world.environment(), m2: m2.services)
        send.attach(coinControl)
        await coinControl.load()
        return (send, coinControl)
    }

    func fillValid(_ model: SendViewModel) {
        model.entries[0].address = testnetAddress1
        model.entries[0].amountText = "1.5"
    }

    func reviewToConfirm(_ model: SendViewModel) async {
        await model.review()
        guard case .confirm = model.phase else {
            Issue.record("expected confirm, got \(model.phase)")
            return
        }
        for _ in 0..<SendViewModel.confirmDelaySeconds {
            await eventually { world.sleeper.pendingCount == 1 }
            world.sleeper.fireNext()
        }
        await eventually { model.canConfirm }
    }

    /// The Coin Selection window stays open (nothing hands the selection
    /// over on close): Send still spends exactly the ticked outpoints.
    @Test func M3_windowOpenAndSendSpendsTheSelectedOutpoints() async {
        let (send, coinControl) = await make()
        await coinControl.toggle(second)
        #expect(send.source == .outpoints([second]))
        fillValid(send)
        await reviewToConfirm(send)
        #expect(world.sender.lastDraft?.state.current.source == .outpoints([second]))
        await send.confirm()
        #expect(send.phase == .done(txid: String(repeating: "f", count: 64)))
    }

    @Test func M3_createUnsignedAndUseMaxReadTheSelectionToo() async {
        let (send, coinControl) = await make()
        await coinControl.selectAll()
        fillValid(send)
        let draft = await send.makeUnsignedDraft() as? FakeDraft
        #expect(draft?.state.current.source == .outpoints([first, second]))
        world.sender.maxSpendableValue.withLock { $0 = .success(Amount(duffs: 100_000_000)) }
        await send.useMax(for: send.entries[0].id)
        #expect(world.sender.maxCalls.current.last?.0 == .outpoints([first, second]))
    }

    @Test func M3_aSuccessfulSendClearsTheSelection() async {
        let (send, coinControl) = await make()
        await coinControl.toggle(first)
        fillValid(send)
        await reviewToConfirm(send)
        await send.confirm()
        #expect(coinControl.selected.isEmpty)
        #expect(coinControl.isAutomatic && coinControl.summary == nil)
        #expect(send.source == .any)
        // Clearing is not a user edit: the done page stays.
        #expect(send.phase == .done(txid: String(repeating: "f", count: 64)))
    }

    @Test func QT052_clearAllClearsTheSelection() async {
        let (send, coinControl) = await make()
        await coinControl.toggle(first)
        fillValid(send)
        send.clearAll()
        #expect(coinControl.selected.isEmpty)
        #expect(send.source == .any)
        #expect(send.entries[0].isBlank)
    }

    /// A selection change during the review is an edit (review M-7): the
    /// prepared transaction, which spends the old selection, is abandoned.
    @Test func M3_aSelectionEditDuringTheReviewAbandonsIt() async {
        let (send, coinControl) = await make()
        await coinControl.toggle(first)
        fillValid(send)
        await reviewToConfirm(send)
        await coinControl.toggle(second)
        #expect(send.phase == .editing)
        await eventually { world.sender.lastDraft?.state.current.abandoned.count == 1 }
        await send.review()
        #expect(world.sender.lastDraft?.state.current.source == .outpoints([first, second]))
    }

    /// While the outcome of a broadcast is unknown the form is read-only:
    /// the dialog refuses the change and says why instead of dropping it.
    @Test func M3_selectionChangesWhileSendingAreRefusedWithTheReason() async throws {
        world.sender.configure.withLock {
            $0 = { draft in draft.state.withLock { $0.broadcastError = ServiceError(code: .internal, detail: "timeout") } }
        }
        let (send, coinControl) = await make()
        await coinControl.toggle(first)
        fillValid(send)
        await reviewToConfirm(send)
        await send.confirm()
        #expect(!send.isEditable)
        await coinControl.toggle(second)
        #expect(coinControl.selected == [first])
        #expect(coinControl.errorMessage == L10n.CoinControl.selectionLockedWhileSending)
        await coinControl.selectAll()
        #expect(coinControl.selected == [first])
        // Dismissing clears the form and the selection; picking works again.
        await send.dismiss()
        #expect(coinControl.selected.isEmpty)
        await coinControl.toggle(second)
        #expect(coinControl.selected == [second])
        #expect(coinControl.errorMessage == nil)
    }

    @Test func M3_setSourceSaysWhyItRefuses() async throws {
        let (attached, _) = await make()
        #expect(throws: SendSourceRefusal.coinControlAttached) { try attached.setSource(.outpoints([first])) }
        let coinJoin = SendViewModel(env: world.environment(), network: world.network, page: .coinJoin)
        #expect(throws: SendSourceRefusal.coinJoinPage) { try coinJoin.setSource(.any) }
        #expect(coinJoin.source == .fullyMixed)
        #expect(SendSourceRefusal.coinJoinPage.message == L10n.Send.sourceFixedOnCoinJoinPage)

        world.sender.configure.withLock {
            $0 = { draft in draft.state.withLock { $0.broadcastError = ServiceError(code: .internal, detail: "timeout") } }
        }
        let plain = SendViewModel(env: world.environment(), network: world.network)
        try plain.setSource(.outpoints([first]))
        fillValid(plain)
        await reviewToConfirm(plain)
        await plain.confirm()
        #expect(throws: SendSourceRefusal.notEditable) { try plain.setSource(.any) }
        #expect(plain.source == .outpoints([first]))
    }

    /// dash-qt ignores the selection while "Enable coin control features"
    /// is off: the panel is hidden, so Send pays from any coins.
    @Test func M3_selectionIsIgnoredWhileCoinControlIsOff() async {
        let (send, coinControl) = await make(coinControlOn: false)
        await coinControl.toggle(first)
        #expect(send.source == .any)
        m2.desktopPreferences.desktop.options.coinControl = true
        #expect(send.source == .outpoints([first]))
    }

    /// The main view model gives Send the dialog every window shares, and a
    /// selection of one wallet is dropped when another wallet is selected.
    @Test func M3_mainAttachesTheSharedDialogAndClearsItOnWalletChange() async throws {
        world.walletState.wallets = [walletInfo(walletA), walletInfo(walletB, name: "Savings")]
        world.coinControl.coins.withLock { $0 = [utxo(1, amount: 300_000_000)] }
        m2.fees.coins.withLock { $0 = [first: Amount(duffs: 300_000_000)] }
        m2.desktopPreferences.desktop.options.coinControl = true
        let main = MainViewModel(env: world.environment(), m2: m2.services)
        await main.start()
        let coinControl = try #require(main.coinControl)
        #expect(main.send?.coinControl === coinControl)
        await coinControl.load()
        await coinControl.toggle(first)
        #expect(main.send?.source == .outpoints([first]))
        world.walletState.selectedWalletID = walletB
        world.walletState.notify()
        await eventually { main.selectedWalletID == walletB }
        #expect(coinControl.selected.isEmpty)
        #expect(main.send?.source == .any)
        main.stop()
    }
}
