import Foundation

/// Holds a current value and fans every change out to `AsyncStream`
/// subscribers. Each stream yields the current value (if any) on
/// subscription, then each change; a slow subscriber sees only the newest.
final class StateBroadcaster<Value: Sendable>: @unchecked Sendable {
    // `lock` guards `current` and `continuations`.
    private let lock = NSLock()
    private var current: Value?
    private var continuations: [UUID: AsyncStream<Value>.Continuation] = [:]

    init(_ initial: Value? = nil) {
        current = initial
    }

    var value: Value? {
        lock.withLock { current }
    }

    func send(_ value: Value) {
        let targets = lock.withLock {
            current = value
            return Array(continuations.values)
        }
        for continuation in targets {
            continuation.yield(value)
        }
    }

    /// Clears the current value without notifying (a later subscriber gets
    /// nothing until the next `send`).
    func clear() {
        lock.withLock { current = nil }
    }

    func stream() -> AsyncStream<Value> {
        let (stream, continuation) = AsyncStream<Value>.makeStream(bufferingPolicy: .bufferingNewest(1))
        let id = UUID()
        let initial = lock.withLock {
            continuations[id] = continuation
            return current
        }
        if let initial { continuation.yield(initial) }
        continuation.onTermination = { [weak self] _ in
            self?.lock.withLock { _ = self?.continuations.removeValue(forKey: id) }
        }
        return stream
    }
}

/// Fans "something changed" out to `AsyncStream<Void>` subscribers. Changes
/// that arrive while a subscriber is busy collapse into one.
final class ChangeNotifier: @unchecked Sendable {
    private let lock = NSLock()
    private var continuations: [UUID: AsyncStream<Void>.Continuation] = [:]

    init() {}

    func notify() {
        let targets = lock.withLock { Array(continuations.values) }
        for continuation in targets {
            continuation.yield()
        }
    }

    func stream() -> AsyncStream<Void> {
        let (stream, continuation) = AsyncStream<Void>.makeStream(bufferingPolicy: .bufferingNewest(1))
        let id = UUID()
        lock.withLock { continuations[id] = continuation }
        continuation.onTermination = { [weak self] _ in
            self?.lock.withLock { _ = self?.continuations.removeValue(forKey: id) }
        }
        return stream
    }
}

/// A single-consumer stream of txid lists where values the consumer has not
/// taken yet are merged, never lost: an empty list means "reload all" and
/// absorbs everything; otherwise the lists are united in arrival order.
final class TxidSignal: @unchecked Sendable {
    private let lock = NSLock()
    private let continuation: AsyncStream<[String]>.Continuation
    let stream: AsyncStream<[String]>

    init() {
        (stream, continuation) = AsyncStream<[String]>.makeStream(bufferingPolicy: .bufferingNewest(1))
    }

    func send(_ txids: [String]) {
        lock.withLock {
            // `bufferingNewest(1)` hands back the value it replaced; re-yield
            // the union so the waiting value covers both.
            if case .dropped(let older) = continuation.yield(txids) {
                _ = continuation.yield(Self.merge(older, txids))
            }
        }
    }

    func finish() {
        continuation.finish()
    }

    static func merge(_ a: [String], _ b: [String]) -> [String] {
        if a.isEmpty || b.isEmpty { return [] }
        var seen = Set(a)
        var merged = a
        for txid in b where seen.insert(txid).inserted {
            merged.append(txid)
        }
        return merged
    }
}
