import DashWalletCore
import Foundation

/// Fan-out of engine events to any number of `AsyncStream` subscribers.
///
/// `publish` is called synchronously on the Rust thread that emitted the
/// event, so events from one engine thread reach every subscriber in emission
/// order. Each subscriber has its own buffer (newest `bufferLimit` kept), so a
/// slow consumer never blocks the engine.
public final class EventBus: @unchecked Sendable {
    // `lock` guards `subscribers` and `finished`.
    private let lock = NSLock()
    private var subscribers: [UUID: AsyncStream<EngineEvent>.Continuation] = [:]
    private var finished = false

    public init() {}

    /// A new stream receiving every event published after this call. The
    /// stream ends when the bus is finished or the consumer stops iterating.
    public func subscribe(bufferLimit: Int = 256) -> AsyncStream<EngineEvent> {
        let (stream, continuation) = AsyncStream<EngineEvent>.makeStream(
            bufferingPolicy: .bufferingNewest(bufferLimit))
        let id = UUID()
        lock.lock()
        let alreadyFinished = finished
        if !alreadyFinished {
            subscribers[id] = continuation
        }
        lock.unlock()
        if alreadyFinished {
            continuation.finish()
        } else {
            continuation.onTermination = { [weak self] _ in self?.remove(id) }
        }
        return stream
    }

    /// Events for one network only (plus network-less notices).
    public func subscribe(network: DashNetwork, bufferLimit: Int = 256) -> AsyncFilterSequence<AsyncStream<EngineEvent>> {
        subscribe(bufferLimit: bufferLimit).filter { $0.network == nil || $0.network == network }
    }

    public var subscriberCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return subscribers.count
    }

    public func publish(_ event: EngineEvent) {
        lock.lock()
        let targets = Array(subscribers.values)
        lock.unlock()
        for continuation in targets {
            continuation.yield(event)
        }
    }

    /// Ends every stream; later subscribers get an already-finished stream.
    public func finish() {
        lock.lock()
        finished = true
        let targets = Array(subscribers.values)
        subscribers.removeAll()
        lock.unlock()
        // Outside the lock: `finish()` runs `onTermination`, which takes it.
        for continuation in targets {
            continuation.finish()
        }
    }

    private func remove(_ id: UUID) {
        lock.lock()
        subscribers[id] = nil
        lock.unlock()
    }
}

/// Receives callbacks from Rust (`EngineObserver`) and republishes them as
/// DashKit events.
final class EngineObserverAdapter: DashWalletCore.EngineObserver, @unchecked Sendable {
    private let bus: EventBus

    init(bus: EventBus) {
        self.bus = bus
    }

    func onEvent(event: DashWalletCore.EngineEvent) {
        bus.publish(EngineEvent(event))
    }
}
