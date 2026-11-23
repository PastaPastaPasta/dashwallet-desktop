// Receive (QT-081…085, IOS-053…055).
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Receive view model")
struct ReceiveViewModelTests {
    let world = FakeWorld()

    func makeModel() -> ReceiveViewModel {
        ReceiveViewModel(env: world.environment())
    }

    @Test func IOS053_showsTheCurrentAddressAndItsQR() async {
        let model = makeModel()
        await model.load()
        let address = world.receive.state.current.current.address
        #expect(model.address?.address == address)
        #expect(model.uri == "dash:\(address)")
        #expect(model.qr?.size == 21)
        #expect(world.uri.qrRequests.current.last == "dash:\(address)")
        #expect(model.copyAddress() == address)
        #expect(model.copyURI() == "dash:\(address)")
    }

    @Test func noWalletIsReported() async {
        world.walletState.selectedWalletID = nil
        let model = makeModel()
        await model.load()
        #expect(model.errorMessage == L10n.Common.noWallet)
    }

    @Test func QT081_QT082_createRequestShowsItAndClearsTheForm() async {
        let model = makeModel()
        await model.load()
        model.requestAmountText = "0,5"
        model.label = "Invoice 7"
        model.message = "Thanks"
        await model.createRequest()
        let request = model.shownRequest
        #expect(request?.amount == Amount(duffs: 50_000_000))
        #expect(request?.label == "Invoice 7")
        #expect(request?.message == "Thanks")
        #expect(model.uri == request?.uri)
        #expect(model.uri?.contains("amount=0.50000000") == true)
        #expect(model.requests.first == request)
        #expect(model.requestAmountText.isEmpty && model.label.isEmpty && model.message.isEmpty)
        #expect(model.amountText(of: request!) == "0.50000000")
        model.dismissRequest()
        #expect(model.shownRequest == nil)
        #expect(model.uri == "dash:\(world.receive.state.current.current.address)")
    }

    @Test func QT081_zeroOrEmptyAmountRequestsNoAmount() async {
        let model = makeModel()
        await model.load()
        model.requestAmountText = "0"
        await model.createRequest()
        #expect(model.shownRequest?.amount == nil)
        #expect(model.amountText(of: model.shownRequest!) == L10n.Receive.noAmount)
    }

    @Test func QT081_invalidAmountIsRefused() async {
        let model = makeModel()
        await model.load()
        model.requestAmountText = "abc"
        await model.createRequest()
        #expect(model.amountError == L10n.Receive.invalidAmount)
        #expect(world.receive.state.current.requests.isEmpty)
        model.requestAmountText = "-1"
        await model.createRequest()
        #expect(model.amountError == L10n.Receive.invalidAmount)
    }

    @Test func QT084_tooLongURIShowsDashQtError() async {
        let model = makeModel()
        await model.load()
        model.message = String(repeating: "m", count: 300)
        await model.createRequest()
        #expect(model.qr == nil)
        #expect(model.errorMessage == L10n.Receive.uriTooLong)
    }

    @Test func QT083_storedRequestsShowAndDelete() async {
        let model = makeModel()
        await model.load()
        model.label = "a"
        await model.createRequest()
        model.label = "b"
        await model.createRequest()
        #expect(model.requests.map(\.label) == ["b", "a"])
        let first = model.requests[1]
        model.show(first)
        #expect(model.shownRequest == first)
        await model.deleteRequest(first.id)
        #expect(model.requests.map(\.label) == ["b"])
        #expect(model.shownRequest == nil)
        let reloaded = makeModel()
        await reloaded.load()
        #expect(reloaded.requests.map(\.label) == ["b"])
    }

    @Test func IOS054_newAddressOnRequest() async {
        let model = makeModel()
        await model.load()
        model.label = "fresh"
        await model.newAddress()
        #expect(model.address?.address == "XnextAddress0000000000000000000" + "1")
        #expect(world.receive.state.current.nextAddressLabels == ["fresh"])
    }

    @Test func IOS054_paidAddressRotatesAfterHistoryChanges() async {
        let model = makeModel()
        await model.load()
        model.start()
        await eventually { world.history.broadcast.subscriberCount == 1 }
        world.receive.state.withLock { $0.current = FakeReceive.info("XrotatedAddress000000000000000002", index: 1) }
        world.history.broadcast.send([])
        await eventually { model.address?.address == "XrotatedAddress000000000000000002" }
        #expect(model.uri == "dash:XrotatedAddress000000000000000002")
        model.stop()
    }

    @Test func gapLimitError() async {
        world.receive.state.withLock { $0.createError = ServiceError(code: .init(rawValue: "receive.gap_limit")) }
        let model = makeModel()
        await model.load()
        await model.createRequest()
        #expect(model.errorMessage == L10n.Receive.couldNotGenerate)
    }
}
