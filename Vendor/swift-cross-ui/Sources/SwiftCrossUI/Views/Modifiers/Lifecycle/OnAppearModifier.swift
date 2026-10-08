extension View {
    /// Adds an action to be performed before this view appears.
    ///
    /// The exact moment that the action gets called is an internal detail and
    /// may change at any time, but it is guaranteed to be after accessing the
    /// view's ``View/body``. Currently, if these docs have been kept up to date,
    /// the action gets called on the main thread after the view update that
    /// created the view's widget (dashwallet-desktop patch P8), so the view may
    /// already be on screen.
    ///
    /// - Parameter action: The action to perform when this view appears.
    public func onAppear(perform action: @escaping @MainActor () -> Void) -> some View {
        OnAppearModifier(body: TupleView1(self), action: action)
    }
}

struct OnAppearModifier<Content: View>: View {
    var body: TupleView1<Content>
    var action: @MainActor () -> Void

    func asWidget<Backend: BaseAppBackend>(
        _ children: any ViewGraphNodeChildren,
        backend: Backend
    ) -> Backend.Widget {
        // dashwallet-desktop patch P8: after the update, not while the parent lays out its
        // children (the layout rule in Vendor/PATCHES.md, P8).
        backend.runInMainThread(action: action)
        return defaultAsWidget(children, backend: backend)
    }
}
