import DashKit
import Testing

@Suite struct EventBusTests {
    @Test func fansOutInOrderToEverySubscriber() async {
        let bus = EventBus()
        let a = bus.subscribe()
        let b = bus.subscribe()
        #expect(bus.subscriberCount == 2)
        let events: [EngineEvent] = [
            .sessionOpened(.regtest),
            .spvStateChanged(.regtest, running: true),
            .sessionClosed(.regtest),
        ]
        for e in events { bus.publish(e) }
        bus.finish()

        var gotA: [EngineEvent] = []
        for await e in a { gotA.append(e) }
        var gotB: [EngineEvent] = []
        for await e in b { gotB.append(e) }
        #expect(gotA == events)
        #expect(gotB == events)
        #expect(bus.subscriberCount == 0)
    }

    @Test func subscribeAfterFinishYieldsEmptyStream() async {
        let bus = EventBus()
        bus.finish()
        var count = 0
        for await _ in bus.subscribe() { count += 1 }
        #expect(count == 0)
    }

    @Test func networkFilterKeepsOnlyThatNetworkAndGlobalNotices() async {
        let bus = EventBus()
        let regtestOnly = bus.subscribe(network: .regtest)
        bus.publish(.sessionOpened(.testnet))
        bus.publish(.sessionOpened(.regtest))
        bus.publish(.notice(nil, .spvError, detail: "x"))
        bus.finish()
        var got: [EngineEvent] = []
        for await e in regtestOnly { got.append(e) }
        #expect(got == [.sessionOpened(.regtest), .notice(nil, .spvError, detail: "x")])
    }

    @Test func cancelledConsumerIsRemoved() async {
        let bus = EventBus()
        let task = Task {
            for await _ in bus.subscribe() {}
        }
        // Wait until the subscription is registered.
        while bus.subscriberCount == 0 { await Task.yield() }
        task.cancel()
        await task.value
        #expect(bus.subscriberCount == 0)
    }
}

@Suite struct EventBusOverflowTests {
    static let wallet = WalletID(hex: String(repeating: "ab", count: 32))!

    @Test func lifecycleEventsSurviveSignalOverflow() async {
        let bus = EventBus()
        let sub = bus.subscribe(signalLimit: 4)
        bus.publish(.sessionOpened(.regtest))
        for height in 0..<50 {
            bus.publish(.historyChanged(.regtest, Self.wallet, txids: [String(height)]))
            if height == 25 { bus.publish(.walletCreated(.regtest, Self.wallet)) }
        }
        bus.publish(.lockStateChanged(.regtest))
        bus.finish()

        var got: [EngineEvent] = []
        for await e in sub { got.append(e) }
        let lifecycle = got.filter(\.isLifecycle)
        #expect(lifecycle == [.sessionOpened(.regtest), .walletCreated(.regtest, Self.wallet), .lockStateChanged(.regtest)])
        // The dropped signals are covered by one resynchronize marker.
        #expect(got.contains(.resynchronize))
        #expect(got.filter { !$0.isLifecycle }.count <= 5)
    }

    @Test func duplicateSignalsAreMerged() async {
        let bus = EventBus()
        let sub = bus.subscribe()
        for _ in 0..<10 { bus.publish(.syncChanged(.regtest)) }
        bus.publish(.balancesChanged(.regtest, Self.wallet))
        bus.finish()
        var got: [EngineEvent] = []
        for await e in sub { got.append(e) }
        #expect(got == [.syncChanged(.regtest), .balancesChanged(.regtest, Self.wallet)])
    }
}
