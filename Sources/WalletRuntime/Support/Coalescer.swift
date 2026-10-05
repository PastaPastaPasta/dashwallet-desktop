import Foundation

/// Runs a reload at most once at a time (iOS `reloadPassInFlight` /
/// `reloadPassRequestedAgain`). A request while a pass runs schedules exactly
/// one more pass after it, however many requests arrive meanwhile.
@MainActor
public final class Coalescer {
    private let work: @MainActor () async -> Void
    private var running: Task<Void, Never>?
    private var requestedAgain = false
    /// Passes run so far (tests and diagnostics).
    public private(set) var passes = 0

    public init(_ work: @escaping @MainActor () async -> Void) {
        self.work = work
    }

    /// Asks for a pass. Starts one now, or marks one to follow the running pass.
    public func request() {
        if running != nil {
            requestedAgain = true
            return
        }
        running = Task { await self.loop() }
    }

    /// Returns once no pass is running or requested.
    public func idle() async {
        while let task = running {
            await task.value
        }
    }

    /// Stops after the running pass; drops a pending request.
    public func cancel() {
        requestedAgain = false
        running?.cancel()
    }

    private func loop() async {
        repeat {
            requestedAgain = false
            passes += 1
            await work()
        } while requestedAgain && !Task.isCancelled
        running = nil
    }
}
