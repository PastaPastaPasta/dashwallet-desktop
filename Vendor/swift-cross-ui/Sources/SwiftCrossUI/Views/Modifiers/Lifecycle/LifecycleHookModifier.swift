// dashwallet-desktop patch P8: lifecycle actions that run after the view update.
//
// `.onAppear` and `.onChange` run their actions after the update (see Vendor/PATCHES.md, P8).
// An action queued that way must not outlive its view: if the view is removed before the
// main loop gets to the action, its `.onDisappear` cleanup may already have run, and a
// late action could start something that nothing will stop. The modifier's children own a
// lifetime flag that ends synchronously when the node goes away (the same moment
// `OnDisappearModifierChildren` schedules the cleanup), and the queued action checks it.

/// A modifier whose node runs actions after the update, for as long as the node lives.
@MainActor
protocol LifecycleHookModifier: TypeSafeView where Children == LifecycleHookChildren {
    associatedtype Wrapped: View
    var body: TupleView1<Wrapped> { get }
}

@MainActor
extension LifecycleHookModifier {
    func children<Backend: BaseAppBackend>(
        backend: Backend,
        snapshots: [ViewGraphSnapshotter.NodeSnapshot]?,
        environment: EnvironmentValues
    ) -> LifecycleHookChildren {
        LifecycleHookChildren(
            wrapping: defaultChildren(backend: backend, snapshots: snapshots, environment: environment)
        )
    }

    func layoutableChildren<Backend: BaseAppBackend>(
        backend: Backend,
        children: LifecycleHookChildren
    ) -> [LayoutSystem.LayoutableChild] {
        defaultLayoutableChildren(backend: backend, children: children.wrapped)
    }

    func asWidget<Backend: BaseAppBackend>(
        _ children: LifecycleHookChildren,
        backend: Backend
    ) -> Backend.Widget {
        defaultAsWidget(children.wrapped, backend: backend)
    }

    func computeLayout<Backend: BaseAppBackend>(
        _ widget: Backend.Widget,
        children: LifecycleHookChildren,
        proposedSize: ProposedViewSize,
        environment: EnvironmentValues,
        backend: Backend
    ) -> ViewLayoutResult {
        defaultComputeLayout(
            widget,
            children: children.wrapped,
            proposedSize: proposedSize,
            environment: environment,
            backend: backend
        )
    }

    func commit<Backend: BaseAppBackend>(
        _ widget: Backend.Widget,
        children: LifecycleHookChildren,
        layout: ViewLayoutResult,
        environment: EnvironmentValues,
        backend: Backend
    ) {
        defaultCommit(
            widget,
            children: children.wrapped,
            layout: layout,
            environment: environment,
            backend: backend
        )
    }
}

/// The children of a ``LifecycleHookModifier`` node, and that node's lifetime.
final class LifecycleHookChildren: ViewGraphNodeChildren {
    /// Set when the node goes away. A class of its own, so that a queued action can hold it
    /// without keeping the children (and so the node's lifetime) alive.
    private final class Lifetime: @unchecked Sendable {
        var hasEnded = false
    }

    let wrapped: any ViewGraphNodeChildren
    private let lifetime = Lifetime()

    var widgets: [AnyWidget] { wrapped.widgets }
    var erasedNodes: [ErasedViewGraphNode] { wrapped.erasedNodes }

    init(wrapping wrapped: any ViewGraphNodeChildren) {
        self.wrapped = wrapped
    }

    deinit {
        lifetime.hasEnded = true
    }

    /// Runs `action` on the main thread after the current update, unless the node is gone by
    /// then.
    func runAfterUpdate<Backend: BaseAppBackend>(
        _ action: @escaping @MainActor () -> Void,
        backend: Backend
    ) {
        backend.runInMainThread { [lifetime] in
            guard !lifetime.hasEnded else { return }
            action()
        }
    }
}
