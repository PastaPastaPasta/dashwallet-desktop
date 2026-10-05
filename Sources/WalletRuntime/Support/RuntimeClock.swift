import Foundation

/// Monotonic time for the runtime's timers (sync damping, auth watchdog).
/// Injected so tests can drive time by hand.
public protocol RuntimeClock: Sendable {
    /// Time elapsed since an arbitrary fixed origin.
    func now() -> Duration
    /// Suspends for `duration`; throws `CancellationError` when cancelled.
    func sleep(for duration: Duration) async throws
}

/// `RuntimeClock` over `ContinuousClock`.
public struct SystemClock: RuntimeClock {
    private let origin = ContinuousClock.now

    public init() {}

    public func now() -> Duration {
        ContinuousClock.now - origin
    }

    public func sleep(for duration: Duration) async throws {
        try await Task.sleep(for: duration)
    }
}
