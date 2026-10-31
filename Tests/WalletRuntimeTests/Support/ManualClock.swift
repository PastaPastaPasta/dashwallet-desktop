// A `RuntimeClock` the test advances by hand.
import Foundation
import WalletRuntime

final class ManualClock: RuntimeClock, @unchecked Sendable {
    private struct Sleeper {
        let deadline: Duration
        let continuation: CheckedContinuation<Void, any Error>
    }

    private let lock = NSLock()
    private var current: Duration = .zero
    private var sleepers: [UUID: Sleeper] = [:]

    func now() -> Duration {
        lock.withLock { current }
    }

    /// How late a `sleep(for:)` begins after it is called, as a task that
    /// runs late under load would: its deadline moves by this much. A
    /// `sleep(until:)` keeps its deadline.
    var lateStart: Duration {
        get { lock.withLock { late } }
        set { lock.withLock { late = newValue } }
    }
    private var late: Duration = .zero

    /// Number of tasks sleeping on this clock.
    var sleeperCount: Int {
        lock.withLock { sleepers.count }
    }

    func sleep(for duration: Duration) async throws {
        try await sleep(.after(duration))
    }

    func sleep(until deadline: Duration) async throws {
        try await sleep(.at(deadline))
    }

    private enum Wake {
        case after(Duration)
        case at(Duration)
    }

    private func sleep(_ wake: Wake) async throws {
        let id = UUID()
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, any Error>) in
                let resumeNow: Bool = lock.withLock {
                    let deadline: Duration
                    switch wake {
                    case .after(let duration): deadline = current + late + duration
                    case .at(let at): deadline = at
                    }
                    if deadline <= current || Task.isCancelled { return true }
                    sleepers[id] = Sleeper(deadline: deadline, continuation: continuation)
                    return false
                }
                if resumeNow {
                    if Task.isCancelled {
                        continuation.resume(throwing: CancellationError())
                    } else {
                        continuation.resume()
                    }
                }
            }
        } onCancel: {
            let sleeper = lock.withLock { sleepers.removeValue(forKey: id) }
            sleeper?.continuation.resume(throwing: CancellationError())
        }
    }

    /// Moves time forward and wakes every sleeper whose deadline passed.
    func advance(by duration: Duration) {
        let due: [Sleeper] = lock.withLock {
            current += duration
            let ready = sleepers.filter { $0.value.deadline <= current }
            for key in ready.keys { sleepers[key] = nil }
            return Array(ready.values)
        }
        for sleeper in due {
            sleeper.continuation.resume()
        }
    }
}

/// Polls `condition` on the main actor until it holds or ~2 s pass.
@MainActor
func eventually(_ condition: @MainActor () -> Bool) async -> Bool {
    for _ in 0..<400 {
        if condition() { return true }
        try? await Task.sleep(for: .milliseconds(5))
    }
    return condition()
}

/// Polls a non-isolated `condition` until it holds or ~2 s pass.
func eventuallyAsync(_ condition: @Sendable () async -> Bool) async -> Bool {
    for _ in 0..<400 {
        if await condition() { return true }
        try? await Task.sleep(for: .milliseconds(5))
    }
    return await condition()
}
