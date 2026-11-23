// dashwallet-desktop patch P1 (Vendor/PATCHES.md): accessibility modifiers.
extension BackendFeatures {
    /// Backend methods for accessible names and descriptions of controls.
    ///
    /// These are used by ``View/accessibilityLabel(_:)`` and
    /// ``View/accessibilityHint(_:)``. Optional: a backend without it leaves
    /// controls with their default names (SwiftCrossUI logs a warning once
    /// when a label or hint is set).
    @MainActor
    public protocol Accessibility: Core {
        /// Sets the accessible name (`label`) and description (`hint`) of a
        /// control widget. `nil` restores the widget's default.
        ///
        /// Called by ``ViewGraphNode/commit()`` for every focusable view (the
        /// controls) and by the built-in picker for its picker widget, on
        /// every commit.
        func setAccessibility(of widget: Widget, label: String?, hint: String?)
    }
}
