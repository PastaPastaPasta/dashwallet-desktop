// dashwallet-desktop patch P1 (Vendor/PATCHES.md): accessibility modifiers.
import CGtk
import Gtk
@_spi(Backends) import SwiftCrossUI

extension GtkBackend: BackendFeatures.Accessibility {
    /// GTK_ACCESSIBLE_PROPERTY_LABEL is the AT-SPI name and
    /// GTK_ACCESSIBLE_PROPERTY_DESCRIPTION the AT-SPI description.
    ///
    /// GTK computes a name from the labelled-by relation before the label
    /// property, and GtkDropDown's template points that relation at the
    /// selected item, so a drop-down was named after its selection. Setting
    /// a label therefore also removes the widget's labelled-by relation.
    public func setAccessibility(of widget: Widget, label: String?, hint: String?) {
        let labelChanged = Self.update(
            widget, GTK_ACCESSIBLE_PROPERTY_LABEL, to: label, key: "scui-accessibility-label")
        if labelChanged, label != nil {
            gtk_accessible_reset_relation(
                OpaquePointer(widget.widgetPointer), GTK_ACCESSIBLE_RELATION_LABELLED_BY)
        }
        Self.update(widget, GTK_ACCESSIBLE_PROPERTY_DESCRIPTION, to: hint, key: "scui-accessibility-hint")
    }

    /// Sets `property` of `widget` to `value`. The value set last is kept as
    /// object data under `key`: an unchanged value is not set again (no
    /// AT-SPI change event per commit), and `nil` resets the property only
    /// when this method set it, so a name GTK computes or another caller set
    /// stays. Returns whether it changed the property.
    @discardableResult
    private static func update(
        _ widget: Widget, _ property: GtkAccessibleProperty, to value: String?, key: String
    ) -> Bool {
        let object = UnsafeMutablePointer<CGtk.GObject>(OpaquePointer(widget.widgetPointer))
        let stored = g_object_get_data(object, key).map {
            String(cString: $0.assumingMemoryBound(to: CChar.self))
        }
        guard stored != value else { return false }
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
        return true
    }
}
