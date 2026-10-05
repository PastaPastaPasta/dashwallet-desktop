import Foundation

/// Owns long-running tasks (event pumps, timers) and cancels them when it is
/// released or `cancelAll()` is called.
final class TaskBag: @unchecked Sendable {
    private let lock = NSLock()
    private var tasks: [String: Task<Void, Never>] = [:]

    init() {}

    /// Stores `task` under `key`, cancelling the task it replaces.
    func set(_ key: String, _ task: Task<Void, Never>?) {
        let old = lock.withLock {
            defer { tasks[key] = task }
            return tasks[key]
        }
        old?.cancel()
    }

    func cancelAll() {
        let all = lock.withLock {
            defer { tasks.removeAll() }
            return Array(tasks.values)
        }
        for task in all {
            task.cancel()
        }
    }

    deinit {
        for task in tasks.values {
            task.cancel()
        }
    }
}
