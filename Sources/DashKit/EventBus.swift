import DashWalletCore
import Foundation

/// Fan-out of engine events to any number of subscribers.
///
/// `publish` runs synchronously on the Rust thread that emitted the event and
/// never blocks: each subscriber has its own mailbox. A mailbox keeps two
/// kinds of event apart (review M4):
/// - lifecycle events (`EngineEvent.isLifecycle`: sessions, wallets added or
///   removed, lock state, SPV state, notices) are always queued, in order;
/// - signals (sync, balances, history, …) say "re-query". A signal equal to
///   one already queued is not queued again. When more than `signalLimit`
///   signals are waiting, they are all replaced by one `.resynchronize`
///   marker, which tells the consumer to re-query everything.
/// So a slow consumer may see fewer signals, but never misses a lifecycle
/// event or a change: every dropped signal is covered by a later re-query.
public final class EventBus: @unchecked Sendable {
    // `lock` guards `mailboxes` and `finished`.
    private let lock = NSLock()
    private var mailboxes: [UUID: Mailbox] = [:]
    private var finished = false

    public init() {}

    /// A new subscription receiving every event published after this call.
    /// It ends when the bus is finished, the consuming task is cancelled, or
    /// the subscription and its iterator are released.
    public func subscribe(signalLimit: Int = 256) -> EventSubscription {
        let mailbox = Mailbox(signalLimit: max(1, signalLimit))
        let id = UUID()
        let alreadyFinished = lock.withLock {
            if !finished { mailboxes[id] = mailbox }
            return finished
        }
        if alreadyFinished { mailbox.finish() }
        return EventSubscription(registration: Registration(bus: self, id: id, mailbox: mailbox))
    }

    /// Events for one network only, plus network-less ones (global notices,
    /// `.resynchronize`).
    public func subscribe(network: DashNetwork, signalLimit: Int = 256) -> AsyncFilterSequence<EventSubscription> {
        subscribe(signalLimit: signalLimit).filter { $0.network == nil || $0.network == network }
    }

    public var subscriberCount: Int {
        lock.withLock { mailboxes.count }
    }

    public func publish(_ event: EngineEvent) {
        let targets = lock.withLock { Array(mailboxes.values) }
        for mailbox in targets {
            mailbox.deliver(event)
        }
    }

    /// Ends every subscription once its queued events are consumed; later
    /// subscriptions are already finished.
    public func finish() {
        let targets = lock.withLock {
            finished = true
            defer { mailboxes.removeAll() }
            return Array(mailboxes.values)
        }
        for mailbox in targets {
            mailbox.finish()
        }
    }

    fileprivate func remove(_ id: UUID) {
        lock.withLock { mailboxes[id] = nil }
    }
}

/// One subscriber's view of the bus. Iterate it once.
public struct EventSubscription: AsyncSequence, Sendable {
    public typealias Element = EngineEvent
    fileprivate let registration: Registration

    public struct AsyncIterator: AsyncIteratorProtocol, Sendable {
        fileprivate let registration: Registration

        public mutating func next() async -> EngineEvent? {
            await registration.mailbox.next()
        }
    }

    public func makeAsyncIterator() -> AsyncIterator {
        AsyncIterator(registration: registration)
    }
}

/// Keeps a mailbox registered while a subscription or its iterator exists.
private final class Registration: @unchecked Sendable {
    weak var bus: EventBus?
    let id: UUID
    let mailbox: Mailbox

    init(bus: EventBus, id: UUID, mailbox: Mailbox) {
        self.bus = bus
        self.id = id
        self.mailbox = mailbox
    }

    deinit {
        mailbox.finish()
        bus?.remove(id)
    }
}

/// A subscriber's queue and its single waiting consumer.
private final class Mailbox: @unchecked Sendable {
    // `lock` guards every stored property below it.
    private let lock = NSLock()
    private var queue: [EngineEvent] = []
    private var signalCount = 0
    private var waiter: CheckedContinuation<EngineEvent?, Never>?
    private var finished = false
    private let signalLimit: Int

    init(signalLimit: Int) {
        self.signalLimit = signalLimit
    }

    func deliver(_ event: EngineEvent) {
        lock.lock()
        if finished {
            lock.unlock()
            return
        }
        if let waiter {
            self.waiter = nil
            lock.unlock()
            waiter.resume(returning: event)
            return
        }
        enqueue(event)
        lock.unlock()
    }

    /// Caller holds `lock`.
    private func enqueue(_ event: EngineEvent) {
        if event.isLifecycle {
            queue.append(event)
            return
        }
        // A queued signal (or a pending full resync) already covers this one.
        if queue.contains(event) || queue.contains(.resynchronize) { return }
        if signalCount >= signalLimit {
            queue.removeAll { !$0.isLifecycle }
            queue.append(.resynchronize)
            signalCount = 1
            return
        }
        queue.append(event)
        signalCount += 1
    }

    /// The next event; `nil` once finished and drained, or when the calling
    /// task is cancelled.
    func next() async -> EngineEvent? {
        await withTaskCancellationHandler {
            await withCheckedContinuation { (continuation: CheckedContinuation<EngineEvent?, Never>) in
                lock.lock()
                if !queue.isEmpty {
                    let event = queue.removeFirst()
                    if !event.isLifecycle { signalCount -= 1 }
                    lock.unlock()
                    continuation.resume(returning: event)
                } else if finished || Task.isCancelled {
                    lock.unlock()
                    continuation.resume(returning: nil)
                } else {
                    waiter = continuation
                    lock.unlock()
                }
            }
        } onCancel: {
            cancel()
        }
    }

    /// Delivers what is queued, then ends.
    func finish() {
        let waiter = lock.withLock {
            finished = true
            defer { self.waiter = nil }
            return self.waiter
        }
        waiter?.resume(returning: nil)
    }

    /// Ends at once and drops what is queued.
    private func cancel() {
        let waiter = lock.withLock {
            finished = true
            queue.removeAll()
            signalCount = 0
            defer { self.waiter = nil }
            return self.waiter
        }
        waiter?.resume(returning: nil)
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
