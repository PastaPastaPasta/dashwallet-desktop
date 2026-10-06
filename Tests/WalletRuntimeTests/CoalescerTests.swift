import Foundation
import Testing
import WalletRuntime

@MainActor
@Suite struct CoalescerTests {
    /// Work that blocks until the test lets it go, counting passes.
    @MainActor
    final class Gate {
        var waiting: [CheckedContinuation<Void, Never>] = []
        var started = 0

        func pass() async {
            started += 1
            await withCheckedContinuation { waiting.append($0) }
        }

        func releaseAll() {
            let all = waiting
            waiting = []
            all.forEach { $0.resume() }
        }
    }

    @Test func requestsDuringAPassCollapseIntoOneMore() async {
        let gate = Gate()
        let coalescer = Coalescer { await gate.pass() }
        coalescer.request()
        #expect(await eventually { gate.started == 1 })
        coalescer.request()
        coalescer.request()
        coalescer.request()
        gate.releaseAll()
        #expect(await eventually { gate.started == 2 })
        gate.releaseAll()
        await coalescer.idle()
        #expect(coalescer.passes == 2)
    }

    /// A session switch cancels the running pass; the new session's first
    /// request must still run, not be absorbed by the cancelled pass.
    @Test func requestAfterCancelStartsAFreshPass() async {
        let gate = Gate()
        let coalescer = Coalescer { await gate.pass() }
        coalescer.request()
        #expect(await eventually { gate.started == 1 })
        coalescer.cancel()
        coalescer.request()
        #expect(await eventually { gate.started == 2 })
        gate.releaseAll()
        await coalescer.idle()
        #expect(gate.started == 2)
        #expect(coalescer.passes == 2)
    }
}
