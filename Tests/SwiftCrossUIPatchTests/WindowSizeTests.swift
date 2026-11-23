import Testing

@_spi(Backends) @testable import SwiftCrossUI

/// Stands in for the live Overview: its minimum content height is 787.
struct OverviewStandIn: View {
    var body: some View {
        Text("Overview").frame(minWidth: 400, minHeight: PatchTests.WindowSizeTests.contentMinimum)
    }
}

extension PatchTests {
    /// Patch P10 (Vendor/PATCHES.md): `WindowReference` over a fake backend that behaves like
    /// GtkBackend. The numbers are the app's: a 1100x760 default window and the live Overview,
    /// whose minimum content height is 787.
    @Suite("SwiftCrossUI patch P10: window sizes")
    @MainActor
    struct WindowSizeTests {
        static let defaultSize = SIMD2(1100, 760)
        static let contentMinimum = 787

        let backend = FakeBackend()
        let window: FakeWindow
        let reference: WindowReference<WindowGroup<TupleView1<OverviewStandIn>>>
        let environment: EnvironmentValues

        init() {
            let scene = WindowGroup("Dash Wallet") { OverviewStandIn() }
            environment = EnvironmentValues(backend: backend)
                .with(\.defaultWindowSize, Self.defaultSize)
            reference = WindowReference(
                scene: scene, backend: backend, environment: environment, onClose: {}, id: "main")
            window = reference.window as! FakeWindow
        }

        func firstUpdate() {
            reference.update(nil, backend: backend, environment: environment)
            #expect(FakeMainLoop.drain())
        }

        @Test("the window grows to the content's minimum, and its report of that costs no update")
        func honouredRequest() {
            firstUpdate()
            #expect(window.content == SIMD2(1100, Self.contentMinimum))
            #expect(window.requests == [SIMD2(1100, Self.contentMinimum)])
            #expect(window.minimum == SIMD2(400, Self.contentMinimum))
            #expect(window.updates == 1, "the report of an honoured request laid the window out again")
        }

        /// The review's finding: an honoured request was remembered until the next resize, so a
        /// user resize below the content's minimum (possible while GtkBackend's window minimum
        /// left out the menu bar) counted as a refusal and the content stayed clipped.
        @Test("a later resize below the content's minimum is not mistaken for a refusal")
        func userShrinkAfterHonouredRequest() {
            firstUpdate()
            let menuBar = 27
            window.userResize(to: SIMD2(1100, Self.contentMinimum - menuBar))
            #expect(FakeMainLoop.drain())
            #expect(window.content == SIMD2(1100, Self.contentMinimum), "content left clipped")
            #expect(window.requests.count == 2)
            // The same again after a width change (each honoured request is cleared again).
            window.userResize(to: SIMD2(1300, Self.contentMinimum))
            #expect(FakeMainLoop.drain())
            window.userResize(to: SIMD2(1300, Self.contentMinimum - menuBar))
            #expect(FakeMainLoop.drain())
            #expect(window.content == SIMD2(1300, Self.contentMinimum), "content left clipped")
        }

        /// What P10 is for: a window manager that keeps the window smaller than requested.
        @Test("a refused request is not repeated")
        func refusedRequestIsNotRepeated() {
            firstUpdate()
            let held = SIMD2(1100, 700)
            window.heldSize = held
            window.userResize(to: held)
            #expect(FakeMainLoop.drain(maxRounds: 50), "resize ping-pong")
            #expect(window.content == held)
            #expect(window.requests.count == 2)
            // Once the window manager lets go, a resize is an ordinary resize again.
            window.heldSize = nil
            window.userResize(to: SIMD2(1200, 700))
            #expect(FakeMainLoop.drain())
            #expect(window.content == SIMD2(1200, Self.contentMinimum))
        }
    }
}
