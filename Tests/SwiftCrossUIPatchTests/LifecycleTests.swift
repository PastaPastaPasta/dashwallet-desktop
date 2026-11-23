import Foundation
import Testing

@_spi(Backends) @testable import SwiftCrossUI

// Lifecycle actions under patch P8 (Vendor/PATCHES.md): `.onAppear` and `.onChange` run after
// the update, and never once their view is gone. Adapted from the D4 round-2 review probes
// (node-program reviews/DW-D4-r2-gpt.md).

/// What the lifecycle actions did, in order, and the resource they manage.
@MainActor
final class LifecycleProbe {
    var events: [String] = []
    var active = false
    var task: Task<Void, Never>?
}

/// Hooks that start a resource on appear / change and stop it on disappear.
struct VanishingHook: View {
    let m: Model
    let p: LifecycleProbe
    var body: some View {
        VStack {
            if m.show {
                Text("shown")
                    .onAppear { p.events.append("appear"); p.active = true }
                    .onChange(of: m.title, initial: true) { p.events.append("change"); p.active = true }
                    .onDisappear { p.events.append("disappear"); p.active = false }
            }
        }
        .frame(width: 300, height: 100)
    }
}

/// `.onAppear` starts a task that `.onDisappear` cancels.
struct VanishingResource: View {
    let m: Model
    let p: LifecycleProbe
    var body: some View {
        VStack {
            if m.show {
                Text("resource")
                    .onAppear {
                        p.events.append("appear")
                        p.task = Task { @MainActor in
                            p.events.append("resource-start")
                            do { try await Task.sleep(for: .seconds(30)) } catch { p.events.append("resource-stop") }
                        }
                    }
                    .onDisappear {
                        p.events.append("disappear")
                        p.task?.cancel()
                    }
            }
        }
        .frame(width: 300, height: 100)
    }
}

extension PatchTests {
    @Suite("SwiftCrossUI patch P8: lifecycle actions")
    @MainActor
    struct LifecycleTests {
        /// Lets main-actor tasks run (`.onDisappear` delivers its action through one).
        func yieldToTasks(until done: () -> Bool = { false }) async {
            for _ in 0..<1_000 where !done() { await Task.yield() }
        }

        @Test("queued .onAppear and .onChange actions are dropped when the view goes first")
        func queuedHooksDroppedAfterRemoval() async {
            let m = Model()
            let p = LifecycleProbe()
            // The first update queues the appear and initial change actions.
            let h = Harness(VanishingHook(m: m, p: p), drain: false)
            // The view goes before the main loop runs them; its cleanup runs.
            m.show = false
            h.windowUpdate()
            await yieldToTasks { p.events.contains("disappear") }
            FakeMainLoop.drain()
            #expect(p.events == ["disappear"])
            #expect(!p.active, "a removed view's queued hooks restarted its resource")
        }

        @Test("a queued non-initial .onChange action is dropped when the view goes first")
        func queuedChangeDroppedAfterRemoval() async {
            let m = Model()
            let p = LifecycleProbe()
            let h = Harness(VanishingHook(m: m, p: p))
            #expect(p.events == ["appear", "change"])
            p.events = []
            m.title = "B"
            h.windowUpdate()  // commits the change and queues its action
            m.show = false
            h.windowUpdate()  // removes the view
            await yieldToTasks { p.events.contains("disappear") }
            FakeMainLoop.drain()
            #expect(p.events == ["disappear"])
            #expect(!p.active, "a queued .onChange action ran after the view's cleanup")
        }

        @Test("a queued .onAppear cannot start a task after the view's cleanup")
        func lateAppearStartsNoTask() async {
            let m = Model()
            let p = LifecycleProbe()
            let h = Harness(VanishingResource(m: m, p: p), drain: false)
            m.show = false
            h.windowUpdate()
            await yieldToTasks { p.events.contains("disappear") }
            FakeMainLoop.drain()
            await yieldToTasks { p.events.contains("resource-start") }
            #expect(p.events == ["disappear"])
            #expect(p.task == nil, "a task started after cleanup, which nothing will cancel")
            p.task?.cancel()
        }

        @Test(".task started by an update is cancelled when the view goes before the main loop runs")
        func taskCancelledAfterImmediateRemoval() async {
            let p = LifecycleProbe()
            var h: Harness<some View>? = Harness(
                Text("task").task { @MainActor in
                    p.events.append("task-start")
                    do { try await Task.sleep(for: .seconds(30)) } catch { p.events.append("task-cancelled") }
                },
                drain: false
            )
            #expect(h != nil)
            h = nil
            await yieldToTasks { p.events.contains("task-cancelled") }
            FakeMainLoop.drain()
            await yieldToTasks { p.events.contains("task-cancelled") }
            #expect(p.events.contains("task-cancelled"))
        }

        /// The documented order (PATCHES.md, P8): `.task` starts during the update, `.onAppear`
        /// runs after it, so a task body can run before the view's appear action.
        @Test("a .task body can run before .onAppear")
        func taskMayRunBeforeAppear() async {
            let p = LifecycleProbe()
            let h = Harness(
                Text("order")
                    .onAppear { p.events.append("appear"); p.active = true }
                    .task { @MainActor in p.events.append(p.active ? "task-after-appear" : "task-before-appear") },
                drain: false
            )
            await yieldToTasks { !p.events.isEmpty }
            FakeMainLoop.drain()
            #expect(p.events == ["task-before-appear", "appear"])
            #expect(h.texts == ["order"])
        }
    }
}
