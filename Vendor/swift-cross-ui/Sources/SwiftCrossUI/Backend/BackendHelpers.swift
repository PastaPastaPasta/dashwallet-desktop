@MainActor
enum BackendHelpers {
    /// Updates the focus properties of a widget.
    static func updateWidgetFocusProperties<Backend: BackendFeatures.FocusHandling>(
        of widget: AnyWidget,
        with backend: Backend,
        environment: EnvironmentValues
    ) {
        backend.registerFocusObservers(
            environment.widgetFocusObservers,
            on: widget.into()
        )

        backend.setFocusEffectDisabled(
            on: widget.into(),
            disabled: environment.focusEffectDisabled
        )
    }

    /// Makes a widget gain or lose focus.
    /// Can also leave the widget's focus unchanged if `nil`.
    static func setFocus<Backend: BackendFeatures.FocusHandling>(
        of widget: AnyWidget,
        to focus: Focus?,
        with backend: Backend
    ) {
        guard let focus else { return }
        backend.setFocus(of: widget.into(), to: focus)
    }

    /// Sets the environment's accessible name and description on a widget.
    static func setAccessibility<Backend: BackendFeatures.Accessibility>(
        of widget: AnyWidget,
        with backend: Backend,
        environment: EnvironmentValues
    ) {
        backend.setAccessibility(
            of: widget.into(),
            label: environment.accessibilityLabel,
            hint: environment.accessibilityHint
        )
    }

    /// Applies the environment's accessible name and description to a
    /// control widget (dashwallet-desktop patch P1) and warns once if a name
    /// or description is set but the backend cannot apply it.
    static func applyAccessibilityProperties<Backend: BaseAppBackend>(
        from environment: EnvironmentValues,
        to widget: AnyWidget,
        with backend: Backend
    ) {
        if let backend2 = backend as? any BackendFeatures.Accessibility {
            setAccessibility(of: widget, with: backend2, environment: environment)
        } else if environment.accessibilityLabel != nil || environment.accessibilityHint != nil {
            logger.warnOnce("\(Backend.self) doesn't support accessibility labels.")
        }
    }

    /// Applies all environment dictated focus property changes to a widget
    /// and warns if the backend doesn't support focus handling.
    static func applyFocusRelatedProperties<Backend: BaseAppBackend>(
        from environment: EnvironmentValues,
        to widget: AnyWidget,
        with backend: Backend
    ) {
        if let backend2 = backend as? any BackendFeatures.FocusHandling {
            BackendHelpers.updateWidgetFocusProperties(
                of: widget,
                with: backend2,
                environment: environment
            )
            BackendHelpers.setFocus(
                of: widget,
                to: environment.focusOverride,
                with: backend2
            )
        } else if
            !environment.widgetFocusObservers.isEmpty ||
            environment.focusEffectDisabled ||
            environment.focusOverride != nil
        {
            logger.warnOnce("\(Backend.self) doesn't support focus control/tracking.")
        }
    }
}
