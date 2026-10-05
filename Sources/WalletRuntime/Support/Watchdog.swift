import Foundation

/// Runs `operation` and returns its result, or `nil` if `timeout` elapses
/// first. The operation is not cancelled on timeout (an engine call cannot
/// be); `onLateResult` receives its result if it finishes after the timeout,
/// so the caller can undo it (e.g. revoke a grant nobody will use).
func withWatchdog<T: Sendable>(
    timeout: Duration,
    clock: any RuntimeClock,
    operation: @escaping @Sendable () async -> T,
    onLateResult: @escaping @Sendable (T) async -> Void
) async -> T? {
    await withCheckedContinuation { (continuation: CheckedContinuation<T?, Never>) in
        let race = FirstWins(continuation)
        let timer = Task {
            do {
                try await clock.sleep(for: timeout)
                race.finish(nil)
            } catch {
                // Cancelled because the operation finished first.
            }
        }
        Task {
            let result = await operation()
            timer.cancel()
            if !race.finish(result) {
                await onLateResult(result)
            }
        }
    }
}

/// Resumes a continuation with the first value offered; later offers are refused.
private final class FirstWins<T: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<T?, Never>?

    init(_ continuation: CheckedContinuation<T?, Never>) {
        self.continuation = continuation
    }

    /// `true` if this call resumed the continuation.
    @discardableResult
    func finish(_ value: T?) -> Bool {
        let taken = lock.withLock {
            defer { continuation = nil }
            return continuation
        }
        taken?.resume(returning: value)
        return taken != nil
    }
}
