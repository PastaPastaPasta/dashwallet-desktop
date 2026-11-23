import Foundation

/// Runs a reload at most once at a time (iOS `reloadPassInFlight` /
/// `reloadPassRequestedAgain`). A request while a pass runs schedules exactly
/// one more pass after it, however many requests arrive meanwhile.
///
/// `cancel()` detaches the running pass: it finishes its current `work()` but
/// runs no further pass, and the next `request()` starts a fresh pass at once
/// instead of being absorbed by the cancelled one (a session switch must not
/// lose the first refresh of the new session).
@MainActor
public final class Coalescer {
    private let work: @MainActor () async -> Void
    private var running: Task<Void, Never>?
    private var requestedAgain = false
    /// Bumped by `cancel()`; a pass of an older generation stops after its
    /// current `work()` and leaves the state alone.
    private var generation = 0
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
        let generation = generation
        running = Task { await self.loop(generation) }
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
        running = nil
        generation &+= 1
    }

    private func loop(_ generation: Int) async {
        repeat {
            requestedAgain = false
            passes += 1
            await work()
        } while generation == self.generation && requestedAgain && !Task.isCancelled
        if generation == self.generation {
            running = nil
        }
    }
}
