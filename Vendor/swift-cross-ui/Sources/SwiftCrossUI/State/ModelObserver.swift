import Foundation
import ObservationPolyfillCore

/// This protocol can be adopted by classes responsible for handling part of the stateful hierarchy. It makes
/// it easy to automatically update the view when observable models change.
///
/// The most important rule: Every computation of a `body` property MUST be performed inside a call to
/// ``observe(with:_:)``. For example:
///
///     let body = self.observe(in: backend) { view.body }
///     // Use `body`
///
/// Then, ``viewModelDidChange(backend:)`` will automatically be called the next time a view
/// model conforming to `Observable` and used inside the `body` computation changes.
///
/// - Important: `self` MUST only be used to observe a single view because
/// ``viewModelDidChange(backend:)`` will only be called for the most recent call to
/// ``observe(with:_:)`` in order to prevent duplicate view updates.
@MainActor
protocol ModelObserver: AnyObject, Sendable {
    /// Used by the ``ModelObserver`` protocol to prevent duplicate view updates.
    var currentViewModelObservationID: UUID? { get set }

    /// This method is called at most once after a call to `observe()` if an object conforming to
    /// `Observable` used in the `computation` closure of the last call to ``observe(with:_:)``
    /// has changed.
    ///
    /// When this method has been called, it will not be called again until the next call to
    /// ``observe(with:_:)``.
    ///
    /// - Parameter backend: The backend passed to the last call to ``observe(with:_:)``.
    func viewModelDidChange<Backend: BaseAppBackend>(backend: Backend)

    /// How deep the observer sits in the scene and view graph (dashwallet-desktop patch P8).
    /// Pending updates run shallowest first; see ``ModelObserverUpdateQueue``.
    var observationDepth: Int { get }
}

extension ModelObserver {
    var observationDepth: Int { 0 }

    /// Performs a computation and tracks accesses to properties of objects conforming to
    /// `Observable` inside the computation. The next time one of those properties changes,
    /// ``viewModelDidChange(backend:)`` will be called.
    ///
    /// If this method is called multiple times, only the last call will be tracked. The reason is that view
    /// updates may be caused by other triggers. If all of those would be tracked, view updates would
    /// multiply.
    ///
    /// - Parameters:
    ///   - backend: The backend used to schedule calls to
    ///     ``viewModelDidChange(backend:)`` on the main thread. A strong reference will be
    ///     held until that call has been made.
    ///   - computation: The computation to be tracked. Usually, accesses to a view's `body`
    ///     property will be encapsulated in this closure.
    /// - Returns: The result of the computation.
    func observe<Backend: BaseAppBackend, Result>(
        with backend: Backend,
        _ computation: () -> Result
    ) -> Result {
        let observationTrackingID = UUID()
        self.currentViewModelObservationID = observationTrackingID
        return ObservationPolyfillCore.withObservationTracking {
            computation()
        } onChange: { [backend, weak self] in
            backend.runInMainThread {
                guard
                    let self, self.currentViewModelObservationID == observationTrackingID
                else { return }
                // dashwallet-desktop patch P8: queue the update instead of running it here.
                ModelObserverUpdateQueue.enqueue(
                    depth: self.observationDepth,
                    isCurrent: { [weak self] in
                        self?.currentViewModelObservationID == observationTrackingID
                    },
                    update: { [weak self] in self?.viewModelDidChange(backend: backend) },
                    scheduleFlush: { backend.runInMainThread(action: $0) }
                )
            }
        }
    }
}

/// Coalesces the updates that one batch of model changes triggers (dashwallet-desktop patch P8).
///
/// Observation tracking nests: a view graph node lays out its children inside its own
/// ``ModelObserver/observe(with:_:)`` call, so every ancestor of a view that reads a property
/// observes that property too. Run one by one, a single change then updates a node, then its
/// parent (whose observation the child's update did not renew), then the grandparent, and so on,
/// each re-laying out a larger subtree. In dashwallet-desktop one assignment queued about 950
/// such updates, which kept the main thread busy for far longer than anyone waited.
///
/// Instead, the changes that arrive before the main thread gets to them are collected and run
/// together, shallowest node first. An ancestor's update recomputes its subtree, which renews
/// the observations of the descendants it lays out, so their own pending updates are dropped.
@MainActor
enum ModelObserverUpdateQueue {
    private struct Pending {
        var depth: Int
        var isCurrent: @MainActor () -> Bool
        var update: @MainActor () -> Void
    }

    private static var pending: [Pending] = []
    private static var isFlushScheduled = false

    /// Queues `update`, which runs at the next flush if `isCurrent` still holds then.
    /// `scheduleFlush` runs its argument later on the main thread (the backend's
    /// `runInMainThread`); it is called once per batch.
    static func enqueue(
        depth: Int,
        isCurrent: @escaping @MainActor () -> Bool,
        update: @escaping @MainActor () -> Void,
        scheduleFlush: (@escaping @MainActor @Sendable () -> Void) -> Void
    ) {
        pending.append(Pending(depth: depth, isCurrent: isCurrent, update: update))
        guard !isFlushScheduled else { return }
        isFlushScheduled = true
        // Runs after the main-thread hops that the same changes have already scheduled.
        scheduleFlush { flush() }
    }

    private static func flush() {
        isFlushScheduled = false
        // `sort` is stable, so observers at the same depth keep their order.
        let batch = pending.sorted { $0.depth < $1.depth }
        pending = []
        for item in batch where item.isCurrent() {
            item.update()
        }
    }
}
