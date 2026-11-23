import Testing

@testable import SwiftCrossUI

extension PatchTests {
    /// Patch P8 (Vendor/PATCHES.md): the updates one batch of model changes
    /// triggers run once, shallowest first, and an ancestor's update drops the
    /// pending updates of the descendants it renewed.
    @Suite("SwiftCrossUI patch P8: the update queue")
    @MainActor
    struct ModelObserverUpdateQueueTests {
        /// Stands in for the backend's `runInMainThread`: keeps the flushes it is given.
        @MainActor
        final class MainThread {
            var scheduled: [@MainActor @Sendable () -> Void] = []

            func schedule(_ action: @escaping @MainActor @Sendable () -> Void) { scheduled.append(action) }

            func runScheduled() {
                let actions = scheduled
                scheduled = []
                actions.forEach { $0() }
            }

            /// Runs everything still scheduled, so a failed test leaves the shared queue empty.
            func drain() {
                while !scheduled.isEmpty { runScheduled() }
            }
        }

        /// A view graph node: its observation is current until `renew()`.
        @MainActor
        final class Node {
            let depth: Int
            var observation = 0
            var updates = 0

            init(depth: Int) { self.depth = depth }

            func renew() { observation += 1 }

            func enqueue(on mainThread: MainThread, update: @escaping @MainActor () -> Void = {}) {
                let observed = observation
                ModelObserverUpdateQueue.enqueue(
                    depth: depth,
                    isCurrent: { [self] in observation == observed },
                    update: { [self] in
                        updates += 1
                        renew()
                        update()
                    },
                    scheduleFlush: { mainThread.schedule($0) }
                )
            }
        }

        @Test("one flush per batch, run shallowest first")
        func batchRunsOnceShallowestFirst() {
            let mainThread = MainThread()
            defer { mainThread.drain() }
            var order: [Int] = []
            // The registrar notifies observers in no particular order.
            for depth in [3, 1, 2] {
                Node(depth: depth).enqueue(on: mainThread) { order.append(depth) }
            }
            #expect(mainThread.scheduled.count == 1)
            #expect(order.isEmpty)
            mainThread.runScheduled()
            #expect(order == [1, 2, 3])
        }

        @Test("an ancestor's update drops the pending updates of the descendants it renewed")
        func ancestorUpdateDropsRenewedDescendants() {
            let mainThread = MainThread()
            defer { mainThread.drain() }
            let ancestor = Node(depth: 1)
            let renewed = Node(depth: 4)
            let notReached = Node(depth: 5)
            renewed.enqueue(on: mainThread)
            notReached.enqueue(on: mainThread)
            // Laying out the ancestor's subtree renews `renewed` but not `notReached`.
            ancestor.enqueue(on: mainThread) { renewed.renew() }
            mainThread.runScheduled()
            #expect(ancestor.updates == 1)
            #expect(renewed.updates == 0)
            #expect(notReached.updates == 1)
        }

        @Test("a change made during a flush starts a new batch")
        func changeDuringFlushStartsNewBatch() {
            let mainThread = MainThread()
            defer { mainThread.drain() }
            let child = Node(depth: 2)
            Node(depth: 1).enqueue(on: mainThread) { child.enqueue(on: mainThread) }
            mainThread.runScheduled()
            #expect(child.updates == 0)
            #expect(mainThread.scheduled.count == 1)
            mainThread.runScheduled()
            #expect(child.updates == 1)
        }

        @Test("a stale duplicate from an earlier change is dropped")
        func duplicateUpdateRunsOnce() {
            let mainThread = MainThread()
            defer { mainThread.drain() }
            let node = Node(depth: 1)
            node.enqueue(on: mainThread)
            node.enqueue(on: mainThread)
            mainThread.runScheduled()
            #expect(node.updates == 1)
        }
    }
}
