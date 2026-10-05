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
