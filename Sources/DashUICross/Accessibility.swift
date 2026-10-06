// Accessible names for controls (ADR 0002 gaps A2–A4).
//
// SwiftCrossUI 0.10.0 has no accessibility modifiers (gap A1; fork patch P1
// is not available yet). Until it is, these modifiers reach the native widget
// through SwiftCrossUI's backend `inspect` hooks and set the name there:
// - Linux (GtkBackend): GTK_ACCESSIBLE_PROPERTY_LABEL on the first control
//   widget inside the view (entry, password entry, switch, check box, drop
//   down, button), and on each row of a list box. AT-SPI reports it as the
//   node's name.
// - macOS (AppKitBackend, development builds): `setAccessibilityLabel` on the
//   first NSControl inside the view. List rows keep AppKit's own names.
// - Windows (WinUIBackend): not wired; the modifiers leave the view as it is.
import SwiftCrossUI

#if os(Linux)
    import CGtk
    import Gtk
    import GtkBackend
#elseif os(macOS)
    import AppKit
    import AppKitBackend
#endif

extension View {
    /// Names the control inside this view (a text field, secure field,
    /// toggle or picker) for assistive technology and UI tests.
    ///
    /// `afterUpdates` also sets it after every update of the view. Toggles
    /// and pickers need it: in the Xvfb run of 2026-10-05, named only on
    /// creation, a switch stayed unnamed and a drop-down kept its selected
    /// option as its name (their native widgets are presumably not in place
    /// yet when the view is created). Text fields are named on creation.
    ///
    /// `enabled: false` leaves the view as it is (a control without a name).
    @ViewBuilder
    public func accessibleName(_ name: String, afterUpdates: Bool = false, enabled: Bool = true) -> some View {
        let points: InspectionPoints = afterUpdates ? [.onCreate, .afterUpdate] : .onCreate
        #if os(Linux)
            inspect(points) { widget in
                if enabled, let control = GtkAccessibleNames.firstControl(in: widget.widgetPointer) {
                    GtkAccessibleNames.setLabel(name, on: control)
                }
            }
        #elseif os(macOS)
            inspect(points) { view in
                if enabled { AppKitAccessibleNames.firstControl(in: view)?.setAccessibilityLabel(name) }
            }
        #else
            self
        #endif
    }

    /// Names the rows of the list inside this view, in order (gap A4). Rows
    /// without a name in `names` keep theirs.
    @ViewBuilder
    public func accessibleRowNames(_ names: [String]) -> some View {
        #if os(Linux)
            inspect([.onCreate, .afterUpdate]) { widget in
                GtkAccessibleNames.labelRows(in: widget.widgetPointer, names: names)
            }
        #else
            self
        #endif
    }
}

#if os(Linux)
    @MainActor
    enum GtkAccessibleNames {
        /// Roles of the widgets that take a name: GtkEntry/GtkPasswordEntry
        /// (text box), GtkSwitch, GtkCheckButton, GtkDropDown, GtkButton.
        static let controlRoles: Set<UInt32> = [
            GTK_ACCESSIBLE_ROLE_TEXT_BOX.rawValue, GTK_ACCESSIBLE_ROLE_SEARCH_BOX.rawValue,
            GTK_ACCESSIBLE_ROLE_SWITCH.rawValue, GTK_ACCESSIBLE_ROLE_CHECKBOX.rawValue,
            GTK_ACCESSIBLE_ROLE_COMBO_BOX.rawValue, GTK_ACCESSIBLE_ROLE_BUTTON.rawValue,
            GTK_ACCESSIBLE_ROLE_TOGGLE_BUTTON.rawValue, GTK_ACCESSIBLE_ROLE_SPIN_BUTTON.rawValue,
        ]

        static func role(of widget: UnsafeMutablePointer<GtkWidget>) -> UInt32 {
            gtk_accessible_get_accessible_role(OpaquePointer(widget)).rawValue
        }

        /// `widget` or its first descendant (depth first) whose role is in `roles`.
        static func firstControl(
            in widget: UnsafeMutablePointer<GtkWidget>, roles: Set<UInt32> = controlRoles
        ) -> UnsafeMutablePointer<GtkWidget>? {
            if roles.contains(role(of: widget)) { return widget }
            var child = gtk_widget_get_first_child(widget)
            while let current = child {
                if let found = firstControl(in: current, roles: roles) { return found }
                child = gtk_widget_get_next_sibling(current)
            }
            return nil
        }

        static func setLabel(_ name: String, on widget: UnsafeMutablePointer<GtkWidget>) {
            var value = GValue()
            g_value_init(&value, GType(16 << G_TYPE_FUNDAMENTAL_SHIFT))  // G_TYPE_STRING
            g_value_set_string(&value, name)
            var property = GTK_ACCESSIBLE_PROPERTY_LABEL
            gtk_accessible_update_property_value(OpaquePointer(widget), 1, &property, &value)
            g_value_unset(&value)
        }

        /// Sets the label of row `i` of the first list box in `widget` to `names[i]`.
        static func labelRows(in widget: UnsafeMutablePointer<GtkWidget>, names: [String]) {
            guard let list = firstControl(in: widget, roles: [GTK_ACCESSIBLE_ROLE_LIST.rawValue]) else { return }
            for (index, name) in names.enumerated() {
                guard let row = gtk_list_box_get_row_at_index(OpaquePointer(list), Int32(index)) else { break }
                setLabel(name, on: UnsafeMutablePointer<GtkWidget>(OpaquePointer(row)))
            }
        }
    }
#elseif os(macOS)
    @MainActor
    enum AppKitAccessibleNames {
        /// `view` or its first descendant (depth first) that is a control.
        /// Text labels (non-editable NSTextFields, such as a picker's
        /// caption) are skipped.
        static func firstControl(in view: NSView) -> NSView? {
            if view is NSControl, (view as? NSTextField)?.isEditable ?? true { return view }
            for subview in view.subviews {
                if let found = firstControl(in: subview) { return found }
            }
            return nil
        }
    }
#endif
