// dashwallet-desktop patch P1 (Vendor/PATCHES.md): accessibility modifiers.
extension View {
    /// Sets the accessible name that assistive technology (and UI tests
    /// through AT-SPI or the macOS accessibility API) reports for the
    /// controls in this view: buttons, toggles, text fields, secure fields,
    /// pickers, sliders and the other focusable views.
    ///
    /// The label is carried in the environment, so it reaches the native
    /// control widget rather than a container around it. It applies to every
    /// control inside the view, so put it on the control (or on a view that
    /// holds one control and its caption).
    public func accessibilityLabel(_ label: String) -> some View {
        EnvironmentModifier(self) { environment in
            environment.with(\.accessibilityLabel, label)
        }
    }

    /// Sets the accessible description of the controls in this view: what
    /// the control does, read after its name. Carried like
    /// ``View/accessibilityLabel(_:)``.
    public func accessibilityHint(_ hint: String) -> some View {
        EnvironmentModifier(self) { environment in
            environment.with(\.accessibilityHint, hint)
        }
    }
}
