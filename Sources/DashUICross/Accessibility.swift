// Accessible names of list rows (ADR 0002 gap A4).
//
// Controls (text fields, secure fields, toggles, pickers, buttons) are named
// with `accessibilityLabel(_:)` from the vendored SwiftCrossUI fork (patch P1,
// Vendor/PATCHES.md). List rows are not controls and fork patch P4 (row
// names) is not done, so `accessibleRowNames` still reaches the native list
// through SwiftCrossUI's backend `inspect` hook:
// - Linux (GtkBackend): GTK_ACCESSIBLE_PROPERTY_LABEL on each row of the
//   first list box inside the view. AT-SPI reports it as the row's name.
// - macOS (AppKitBackend, development builds) and Windows: rows keep the
//   toolkit's own names.
import SwiftCrossUI

#if os(Linux)
    import CGtk
    import Gtk
    import GtkBackend
#endif

extension View {
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
        static func role(of widget: UnsafeMutablePointer<GtkWidget>) -> UInt32 {
            gtk_accessible_get_accessible_role(OpaquePointer(widget)).rawValue
        }

        /// `widget` or its first descendant (depth first) whose role is in `roles`.
        static func firstWidget(
            in widget: UnsafeMutablePointer<GtkWidget>, roles: Set<UInt32>
        ) -> UnsafeMutablePointer<GtkWidget>? {
            if roles.contains(role(of: widget)) { return widget }
            var child = gtk_widget_get_first_child(widget)
            while let current = child {
                if let found = firstWidget(in: current, roles: roles) { return found }
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
            guard let list = firstWidget(in: widget, roles: [GTK_ACCESSIBLE_ROLE_LIST.rawValue]) else { return }
            for (index, name) in names.enumerated() {
                guard let row = gtk_list_box_get_row_at_index(OpaquePointer(list), Int32(index)) else { break }
                setLabel(name, on: UnsafeMutablePointer<GtkWidget>(OpaquePointer(row)))
            }
        }
    }
#endif
