// dashwallet-desktop patch P1 (Vendor/PATCHES.md): accessibility modifiers.
import CGtk
import Gtk
@_spi(Backends) import SwiftCrossUI

extension GtkBackend: BackendFeatures.Accessibility {
    /// GTK_ACCESSIBLE_PROPERTY_LABEL is the AT-SPI name and
    /// GTK_ACCESSIBLE_PROPERTY_DESCRIPTION the AT-SPI description.
    public func setAccessibility(of widget: Widget, label: String?, hint: String?) {
        Self.update(widget, GTK_ACCESSIBLE_PROPERTY_LABEL, to: label, key: "scui-accessibility-label")
        Self.update(widget, GTK_ACCESSIBLE_PROPERTY_DESCRIPTION, to: hint, key: "scui-accessibility-hint")
    }

    /// Sets `property` of `widget` to `value`. The value set last is kept as
    /// object data under `key`: an unchanged value is not set again (no
    /// AT-SPI change event per commit), and `nil` resets the property only
    /// when this method set it, so a name GTK computes or another caller set
    /// stays.
    private static func update(
        _ widget: Widget, _ property: GtkAccessibleProperty, to value: String?, key: String
    ) {
        let object = UnsafeMutablePointer<GObject>(OpaquePointer(widget.widgetPointer))
        let stored = g_object_get_data(object, key).map {
            String(cString: $0.assumingMemoryBound(to: CChar.self))
        }
        guard stored != value else { return }
        let accessible = OpaquePointer(widget.widgetPointer)
        if let value {
            var gValue = GValue()
            g_value_init(&gValue, GType(16 << G_TYPE_FUNDAMENTAL_SHIFT))  // G_TYPE_STRING
            g_value_set_string(&gValue, value)
            var properties = [property]
            gtk_accessible_update_property_value(accessible, 1, &properties, &gValue)
            g_value_unset(&gValue)
            g_object_set_data_full(object, key, UnsafeMutableRawPointer(g_strdup(value))) { g_free($0) }
        } else {
            gtk_accessible_reset_property(accessible, property)
            g_object_set_data(object, key, nil)
        }
    }
}
