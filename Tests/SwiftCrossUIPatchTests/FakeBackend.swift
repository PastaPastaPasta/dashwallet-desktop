import Foundation

@_spi(Backends) @testable import SwiftCrossUI

// A backend over plain objects, built on SwiftCrossUI's `BackendFeatures.BaseStubs`, so the
// tests can drive a real `ViewGraph` and a real `WindowReference` without GTK. Adapted from
// the D4 review harness (node-program reviews/DW-D4-r1-opus.md).

final class FakeWidget: @unchecked Sendable {
    let kind: String
    var text = ""
    var children: [FakeWidget] = []

    init(_ kind: String) { self.kind = kind }

    /// Text widgets reachable from this one through container children, in order.
    var texts: [FakeWidget] {
        (kind == "text" ? [self] : []) + children.flatMap(\.texts)
    }
}

final class FakeWindow: @unchecked Sendable {
    /// Content size (GtkBackend's `size(ofWindow:)` is the root widget's allocation).
    var content = SIMD2<Int>(0, 0)
    var minimum = SIMD2<Int>(0, 0)
    /// Every `setSize(ofWindow:to:)`.
    var requests: [SIMD2<Int>] = []
    /// Completed `WindowReference` updates (each ends with `updateWindow`).
    var updates = 0
    /// A window manager that keeps the window at this content size whatever is requested.
    var heldSize: SIMD2<Int>?
    var resizeHandler: ((SIMD2<Int>) -> Void)?

    /// A user or window manager resize: GTK allocates the root widget a size it was not told
    /// about, so the root widget fires its resize callback.
    func userResize(to size: SIMD2<Int>) {
        content = size
        resizeHandler?(size)
    }
}

/// The main thread's queue, drained by hand (stands in for `g_idle_add`). Thread-safe:
/// SwiftCrossUI's `@State` updater posts from a background queue.
enum FakeMainLoop {
    private nonisolated(unsafe) static var scheduled: [@MainActor () -> Void] = []
    private static let lock = NSLock()

    static func post(_ action: @escaping @MainActor () -> Void) {
        lock.withLock { scheduled.append(action) }
    }

    /// Takes the actions scheduled so far, leaving the queue empty.
    static func take() -> [@MainActor () -> Void] {
        lock.withLock {
            defer { scheduled = [] }
            return scheduled
        }
    }

    /// Runs scheduled actions, and the ones they schedule, until nothing is scheduled for
    /// 0.2 s (the `@State` updater posts from a background queue). Returns false if the
    /// main loop does not settle within `maxRounds` rounds.
    @MainActor @discardableResult
    static func drain(maxRounds: Int = 1_000) -> Bool {
        var rounds = 0
        var idleRounds = 0
        while idleRounds < 4 {
            let actions = take()
            if actions.isEmpty {
                idleRounds += 1
                Thread.sleep(forTimeInterval: 0.05)
                continue
            }
            idleRounds = 0
            rounds += 1
            if rounds > maxRounds { return false }
            for action in actions { action() }
        }
        return true
    }
}

@MainActor
final class FakeBackend: BackendFeatures.BaseStubs {
    typealias Widget = FakeWidget
    typealias Window = FakeWindow

    /// `updateTextView` calls: how much text the updates laid out.
    var textLayouts = 0
    /// Called from `updateTextView`, which `Text` calls while it lays out.
    var onTextLayout: ((String) -> Void)?
    /// Called from `setSize(of:to:)`, which `Text` calls while it commits.
    var onTextCommit: ((String) -> Void)?

    nonisolated func runInMainThread(action: @escaping @MainActor () -> Void) {
        FakeMainLoop.post(action)
    }

    func show(widget: Widget) {}
    func naturalSize(of widget: Widget) -> SIMD2<Int> { .zero }
    func setSize(of widget: Widget, to size: SIMD2<Int>) {
        if widget.kind == "text" { onTextCommit?(widget.text) }
    }

    func createContainer() -> Widget { FakeWidget("container") }
    func removeAllChildren(of container: Widget) { container.children = [] }
    func insert(_ child: Widget, into container: Widget, at index: Int) {
        container.children.insert(child, at: min(index, container.children.count))
    }
    func swap(childAt firstIndex: Int, withChildAt secondIndex: Int, in container: Widget) {
        container.children.swapAt(firstIndex, secondIndex)
    }
    func setPosition(ofChildAt index: Int, in container: Widget, to position: SIMD2<Int>) {}
    func remove(childAt index: Int, from container: Widget) { container.children.remove(at: index) }

    var supportsMultipleWindows: Bool { true }
    var canOverrideWindowColorScheme: Bool { false }
    var restoresWindowFrames: Bool { false }
    func createWindow(withDefaultSize defaultSize: SIMD2<Int>?, id: String) -> Window {
        let window = FakeWindow()
        if let defaultSize { window.content = defaultSize }
        return window
    }
    func updateWindow(_ window: Window, environment: EnvironmentValues) { window.updates += 1 }
    func setTitle(ofWindow window: Window, to title: String) {}
    func setChild(ofWindow window: Window, to child: Widget) {}
    func size(ofWindow window: Window) -> SIMD2<Int> { window.content }
    func isWindowProgrammaticallyResizable(_ window: Window) -> Bool { true }
    /// Like GtkBackend with patch P10: the content gets the requested size unless a window
    /// manager holds the window, and the root widget reports the first allocation after the
    /// request either way.
    func setSize(ofWindow window: Window, to newSize: SIMD2<Int>) {
        window.requests.append(newSize)
        window.content = window.heldSize ?? newSize
        window.resizeHandler?(window.content)
    }
    func setSizeLimits(
        ofWindow window: Window, minimum minimumSize: SIMD2<Int>, maximum maximumSize: SIMD2<Int>?
    ) {
        window.minimum = minimumSize
    }
    func setResizeHandler(ofWindow window: Window, to action: @escaping (_ newSize: SIMD2<Int>) -> Void) {
        // GtkBackend runs the handler on a later main loop iteration too.
        window.resizeHandler = { size in FakeMainLoop.post { action(size) } }
    }
    func show(window: Window) {}
    func activate(window: Window) {}
    func computeWindowEnvironment(window: Window, rootEnvironment: EnvironmentValues) -> EnvironmentValues {
        rootEnvironment
    }
    func setWindowEnvironmentChangeHandler(
        of window: Window, to action: @escaping @Sendable @MainActor () -> Void
    ) {}

    func createTextView() -> Widget { FakeWidget("text") }
    func updateTextView(_ textView: Widget, content: String, environment: EnvironmentValues) {
        textLayouts += 1
        textView.text = content
        onTextLayout?(content)
    }
    func size(
        of text: String, whenDisplayedIn widget: Widget, proposedWidth: Int?, proposedHeight: Int?,
        environment: EnvironmentValues
    ) -> SIMD2<Int> {
        SIMD2(text.count * 7, 12)
    }
}
