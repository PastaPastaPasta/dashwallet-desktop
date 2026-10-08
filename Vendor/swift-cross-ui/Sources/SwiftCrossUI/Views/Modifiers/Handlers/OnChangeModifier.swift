extension View {
    /// A view modifier that runs an action whenever a piece of state changes.
    ///
    /// The action runs after the view update that saw the change has finished
    /// (dashwallet-desktop patch P8).
    ///
    /// - Parameters:
    ///   - value: The value to observe for changes. Must be `Equatable`.
    ///   - initial: Whether to call `action` when the view first appears.
    ///   - action: The action to perform.
    public func onChange<Value: Equatable>(
        of value: Value,
        initial: Bool = false,
        perform action: @escaping () -> Void
    ) -> some View {
        OnChangeModifier(
            body: TupleView1(self),
            value: value,
            action: action,
            initial: initial
        )
    }
}

struct OnChangeModifier<Value: Equatable, Content: View>: View {
    // TODO: This probably doesn't have to trigger view updates. We're only
    //   really using @State here to persist the data.
    @State var previousValue: Value?

    var body: TupleView1<Content>

    var value: Value
    var action: () -> Void
    var initial: Bool
    /// Whether `action` waits until after the update (dashwallet-desktop patch P8). Only
    /// `TaskModifier` turns it off: starting a task writes no observed state.
    var runsAfterUpdate = true

    // dashwallet-desktop patch P8: compared in `commit` (upstream: `computeLayout`) and run after
    // the update, so the ancestors see what `action` writes (the layout rule in
    // Vendor/PATCHES.md, P8).
    func commit<Backend: BaseAppBackend>(
        _ widget: Backend.Widget,
        children: any ViewGraphNodeChildren,
        layout: ViewLayoutResult,
        environment: EnvironmentValues,
        backend: Backend
    ) {
        if let previousValue, value != previousValue {
            run(action, backend: backend)
        } else if initial, previousValue == nil {
            run(action, backend: backend)
        }

        if previousValue != value {
            previousValue = value
        }

        defaultCommit(
            widget,
            children: children,
            layout: layout,
            environment: environment,
            backend: backend
        )
    }

    private func run<Backend: BaseAppBackend>(_ action: @escaping () -> Void, backend: Backend) {
        if runsAfterUpdate {
            backend.runInMainThread(action: action)
        } else {
            action()
        }
    }
}
