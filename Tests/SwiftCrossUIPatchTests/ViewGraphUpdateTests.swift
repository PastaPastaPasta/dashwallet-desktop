import Foundation
import Observation
import Testing

@_spi(Backends) @testable import SwiftCrossUI

@Observable
final class Model: @unchecked Sendable {
    var title = "A"
    var show = true
    var other = 0
    var derived = "A"
}

struct Leaf: View {
    let m: Model
    var body: some View { Text("leaf:\(m.title)") }
}

struct Mid: View {
    let m: Model
    var body: some View {
        VStack {
            Text("mid")
            Leaf(m: m)
            Leaf(m: m)
        }
    }
}

/// Reads `title` itself and through every descendant.
struct Root: View {
    let m: Model
    var body: some View {
        VStack {
            Text("root:\(m.title)")
            Mid(m: m)
            Mid(m: m)
        }
    }
}

/// Never reads `title` itself; only the leaves do.
struct QuietRoot: View {
    let m: Model
    var body: some View {
        VStack {
            Text("quiet")
            Mid(m: m)
        }
    }
}

/// Like `QuietRoot`, wider, and reads `other` itself (and nothing below it does).
struct WideQuietRoot: View {
    let m: Model
    var body: some View {
        _ = m.other
        return VStack {
            Text("quiet")
            Mid(m: m)
            Mid(m: m)
            Mid(m: m)
        }
    }
}

/// Drops the leaves when `show` goes false.
struct Dropping: View {
    let m: Model
    var body: some View {
        VStack {
            if m.show {
                Mid(m: m)
            } else {
                Text("gone")
            }
        }
    }
}

/// A descendant's `.onChange` writes state the ancestor's own body read earlier in the pass.
struct OnChangeWriter: View {
    let m: Model
    var body: some View {
        Text("w:\(m.title)").onChange(of: m.title) { m.derived = m.title }
    }
}

struct OnChangeRoot: View {
    let m: Model
    var body: some View {
        VStack {
            Text("derived:\(m.derived)")
            OnChangeWriter(m: m)
        }
    }
}

/// A descendant that appears in the ancestor's update and whose `.onAppear` writes state the
/// ancestor's own body read earlier in the pass. The fixed frame keeps the root's size, so no
/// resize lays the root out again (which would hide a missed write).
struct OnAppearRoot: View {
    let m: Model
    var body: some View {
        VStack {
            Text("derived:\(m.derived)")
            if m.show {
                Text("shown").onAppear { m.derived = "B" }
            }
        }
        .frame(width: 300, height: 100)
    }
}

struct OnChangeCounter: View {
    let m: Model
    let count: @MainActor () -> Void
    var body: some View {
        Text("c:\(m.title)").onChange(of: m.title, initial: true) { count() }
    }
}

/// A view graph in a stand-in for `WindowReference`: a resize of the root re-lays out and
/// commits the graph.
@MainActor
final class Harness<V: View> {
    let backend = FakeBackend()
    var graph: ViewGraph<V>!
    var env: EnvironmentValues!

    /// `drain: false` leaves whatever the first update scheduled on the fake main loop.
    init(_ view: V, drain: Bool = true) {
        var env = EnvironmentValues(backend: backend).with(\.window, FakeWindow())
        env.onResize = { [weak self] _ in self?.windowUpdate() }
        self.env = env
        graph = ViewGraph(for: view, backend: backend, environment: env)
        windowUpdate()
        if drain { FakeMainLoop.drain() }
        backend.textLayouts = 0
    }

    func windowUpdate() {
        // Like WindowReference.update: a probing pass, then the final pass and the commit.
        _ = graph.computeLayout(proposedSize: .zero, environment: env.with(\.allowLayoutCaching, true))
        _ = graph.computeLayout(proposedSize: ProposedViewSize(800, 600), environment: env)
        graph.commit()
    }

    var rootWidget: FakeWidget { graph.rootNode.concreteNode(for: FakeBackend.self).widget }
    var texts: [String] { rootWidget.texts.map(\.text) }
    var leafTexts: [String] { texts.filter { $0.hasPrefix("leaf") } }
}

/// A view graph node's depth and children, to check the depths patch P8 hands out.
@MainActor
protocol DepthReporting {
    var depthAndChildren: (depth: Int, children: [AnyObject]) { get }
}

extension ViewGraphNode: DepthReporting {
    var depthAndChildren: (depth: Int, children: [AnyObject]) {
        (observationDepth, children.erasedNodes.map(\.node))
    }
}

extension PatchTests {
    /// Patch P8 (Vendor/PATCHES.md) in a real view graph over a fake backend.
    @Suite("SwiftCrossUI patch P8: view graph updates")
    @MainActor
    struct ViewGraphUpdateTests {
        @Test("a model change queues the updates; they run in one flush after the hops")
        func updatesWaitForTheFlush() {
            let m = Model()
            let h = Harness(Root(m: m))
            m.title = "B"
            // The observation hops, one per observing node.
            let hops = FakeMainLoop.take()
            #expect(hops.count > 1)
            hops.forEach { $0() }
            #expect(h.backend.textLayouts == 0, "an update ran in its observation hop")
            let flush = FakeMainLoop.take()
            #expect(flush.count == 1, "expected exactly one flush")
            flush.forEach { $0() }
            FakeMainLoop.drain()
            #expect(h.texts.first == "root:B")
        }

        @Test("each node is one level deeper than its parent; a window's root view is 1")
        func depthFollowsTheGraph() {
            let h = Harness(Root(m: Model()))
            var maxDepth = 0
            func check(_ node: Any, expected: Int) {
                guard let node = node as? any DepthReporting else {
                    Issue.record("not a view graph node: \(type(of: node))")
                    return
                }
                let (depth, children) = node.depthAndChildren
                #expect(depth == expected)
                maxDepth = max(maxDepth, depth)
                for child in children { check(child, expected: expected + 1) }
            }
            check(h.graph.rootNode.node, expected: 1)
            #expect(maxDepth >= 5)
        }

        @Test("a batch costs one update of the shallowest affected subtree")
        func batchCostsOneSubtreeUpdate() {
            let m = Model()
            let h = Harness(WideQuietRoot(m: m))
            // Only the root reads `other`: one update of the root's subtree.
            m.other += 1
            FakeMainLoop.drain()
            let subtreeUpdate = h.backend.textLayouts
            #expect(subtreeUpdate > 0)
            // `title` is read by every leaf and so observed by every node above them. Several
            // batches, so that a run of lucky registrar orders cannot hide a missing depth.
            for title in ["B", "C", "D", "E", "F"] {
                h.backend.textLayouts = 0
                m.title = title
                FakeMainLoop.drain()
                #expect(h.backend.textLayouts == subtreeUpdate, "title \(title)")
            }
            #expect(h.leafTexts == Array(repeating: "leaf:F", count: 6))
        }

        @Test func nestedReadersAllUpdated() {
            let m = Model()
            let h = Harness(Root(m: m))
            #expect(h.texts == ["root:A", "mid", "leaf:A", "leaf:A", "mid", "leaf:A", "leaf:A"])
            m.title = "LongerTitle"
            FakeMainLoop.drain()
            #expect(
                h.texts == [
                    "root:LongerTitle", "mid", "leaf:LongerTitle", "leaf:LongerTitle", "mid",
                    "leaf:LongerTitle", "leaf:LongerTitle",
                ])
        }

        @Test func quietAncestorStillUpdatesLeaves() {
            let m = Model()
            let h = Harness(QuietRoot(m: m))
            m.title = "B"
            FakeMainLoop.drain()
            #expect(h.texts == ["quiet", "mid", "leaf:B", "leaf:B"])
        }

        @Test func ancestorDropsDescendantInSameBatch() {
            let m = Model()
            let h = Harness(Dropping(m: m))
            m.title = "B"
            m.show = false
            FakeMainLoop.drain()
            #expect(h.texts == ["gone"])
            m.show = true
            FakeMainLoop.drain()
            #expect(h.texts == ["mid", "leaf:B", "leaf:B"])
        }

        @Test func changeBeforeTheFlush() {
            let m = Model()
            let h = Harness(Root(m: m))
            m.title = "B"
            // A change that lands after the hops but before the flush.
            FakeMainLoop.post { m.title = "C" }
            FakeMainLoop.drain()
            #expect(h.texts.first == "root:C")
            #expect(h.leafTexts == Array(repeating: "leaf:C", count: 4))
        }

        @Test("a change made while the update commits (a GTK signal handler) is not lost")
        func changeDuringCommit() {
            let m = Model()
            let h = Harness(Root(m: m))
            var fired = false
            h.backend.onTextCommit = { content in
                if content == "leaf:B" && !fired {
                    fired = true
                    m.title = "C"
                }
            }
            m.title = "B"
            FakeMainLoop.drain()
            #expect(fired)
            #expect(h.texts.first == "root:C")
            #expect(h.leafTexts == Array(repeating: "leaf:C", count: 4))
        }

        @Test("an ancestor sees what a descendant's .onChange writes")
        func onChangeWriteSeenByAncestor() {
            let m = Model()
            let h = Harness(OnChangeRoot(m: m))
            #expect(h.texts == ["derived:A", "w:A"])
            m.title = "B"
            FakeMainLoop.drain()
            #expect(m.derived == "B")
            #expect(h.texts == ["derived:B", "w:B"])
        }

        @Test("an ancestor sees what a descendant's .onAppear writes")
        func onAppearWriteSeenByAncestor() {
            let m = Model()
            m.show = false
            let h = Harness(OnAppearRoot(m: m))
            #expect(h.texts == ["derived:A"])
            m.show = true
            FakeMainLoop.drain()
            #expect(m.derived == "B")
            #expect(h.texts == ["derived:B", "shown"])
        }

        @Test(".onChange runs once per change, and once at first with initial: true")
        func onChangeRunsOncePerChange() {
            let m = Model()
            var count = 0
            let h = Harness(OnChangeCounter(m: m) { count += 1 })
            #expect(count == 1)
            m.title = "B"
            FakeMainLoop.drain()
            #expect(count == 2)
            m.other += 1  // read by no view
            FakeMainLoop.drain()
            #expect(count == 2)
            #expect(h.texts == ["c:B"])
        }

        /// The rule in PATCHES.md (P8): state must not be written while a view lays out. A
        /// write made by backend code that the layout calls (here a widget update's signal
        /// handler) is missed by the ancestors that read the old value earlier in the same
        /// pass: their observations start only after their layout returns. The known issue is
        /// not intermittent, so this also fails if the root stops being stale, which would
        /// mean the updates no longer run shallowest first.
        @Test("a write during layout leaves the ancestor stale (the documented limit)")
        func changeDuringLayoutIsTheDocumentedLimit() {
            let m = Model()
            let h = Harness(Root(m: m))
            var fired = false
            h.backend.onTextLayout = { content in
                if content == "leaf:B" && !fired {
                    fired = true
                    m.title = "C"
                }
            }
            m.title = "B"
            FakeMainLoop.drain()
            #expect(fired)
            #expect(h.leafTexts == Array(repeating: "leaf:C", count: 4))
            withKnownIssue("an ancestor that read the old value during the same layout stays stale") {
                #expect(h.texts.first == "root:C")
            }
        }

        /// Unlike other `.onChange` actions, `.task` starts its task during the update (upstream's
        /// behaviour): deferred, the start could come after the view's `.onDisappear`.
        @Test(".task starts during the update, before the main loop runs")
        func taskStartsDuringTheUpdate() async {
            var started = false
            let h = Harness(Text("t").task { @MainActor in started = true }, drain: false)
            for _ in 0..<1_000 where !started { await Task.yield() }
            #expect(started)
            FakeMainLoop.drain()
            #expect(h.texts == ["t"])
        }

        @Test func twoWindowsShareAModel() {
            let m = Model()
            let a = Harness(Root(m: m))
            let b = Harness(QuietRoot(m: m))
            m.title = "Z"
            FakeMainLoop.drain()
            #expect(a.texts == ["root:Z", "mid", "leaf:Z", "leaf:Z", "mid", "leaf:Z", "leaf:Z"])
            #expect(b.texts == ["quiet", "mid", "leaf:Z", "leaf:Z"])
        }
    }
}
