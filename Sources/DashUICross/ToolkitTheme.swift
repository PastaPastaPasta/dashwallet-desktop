// Toolkit-level styling that SwiftCrossUI 0.10 has no modifier for
// (UX-SPEC §2.2 fonts, §2.8 item 6 accent):
// - Linux (GtkBackend): registers the bundled Inter files with fontconfig and
//   makes Inter GTK's default face; installs one CSS provider, built from the
//   colour tokens, that makes Dash blue the accent of the native widgets
//   (switches, check boxes, progress bars, list selection, focus rings, text
//   selection) whatever the desktop theme's accent is; follows the window's
//   appearance with `gtk-application-prefer-dark-theme` so native entries
//   and drop-downs match the dark canvas.
// - macOS (AppKitBackend development builds) and Windows: nothing; the
//   system face and controls stay.
import DesignTokens
import Foundation
import SwiftCrossUI

#if os(Linux)
    import CGtk
    import Gtk
    import GtkBackend
#endif

public enum ToolkitTheme {
    /// The face the CSS and GTK settings name.
    public static let fontFamily = "Inter"
    /// GTK's default size for text the app does not size itself.
    static let defaultFontSize = 11

    /// The bundled font files (Inter and its OFL licence).
    static var fontDirectory: URL? { Bundle.module.url(forResource: "Fonts", withExtension: nil) }

    /// Makes the bundled Inter files available to the toolkit. Call before the
    /// first window exists (fontconfig adds them to the current configuration,
    /// which Pango's font map uses). Returns the number of font files found.
    @discardableResult
    public static func registerFonts() -> Int {
        guard let directory = fontDirectory else { return 0 }
        let fonts = (try? FileManager.default.contentsOfDirectory(atPath: directory.path))?.filter {
            $0.hasSuffix(".ttf")
        } ?? []
        #if os(Linux)
            guard !fonts.isEmpty, Fontconfig.addDirectory(directory.path) else { return 0 }
            return fonts.count
        #else
            return 0
        #endif
    }

    /// The CSS the GTK provider loads for `appearance`; colours come from the tokens.
    public static func css(for appearance: DashAppearance) -> String {
        func c(_ token: DashColor) -> String { cssColor(token.resolved(for: appearance)) }
        let selectedText = c(CrossRole.white)
        return """
            @define-color accent_color \(c(CrossRole.accent));
            @define-color accent_bg_color \(c(CrossRole.accent));
            @define-color accent_fg_color \(selectedText);
            .navigation-sidebar { background-color: transparent; }
            .navigation-sidebar > row { border-radius: \(CrossLayout.sidebarRowRadius)px; }
            .navigation-sidebar > row:hover { background-color: \(c(CrossRole.accentTint)); }
            .navigation-sidebar > row:selected,
            .navigation-sidebar > row:selected:hover { background-color: \(c(CrossRole.accentTintStrong)); }
            .dwd-sidebar > row:selected,
            .dwd-sidebar > row:selected:hover { background-color: \(c(CrossRole.accent)); }
            switch:checked { background-color: \(c(CrossRole.switchOn)); }
            progressbar > trough > progress { background-color: \(c(CrossRole.accent)); border-radius: 2px; }
            progressbar > trough { min-height: 4px; border-radius: 2px; }
            checkbutton check:checked, checkbutton radio:checked {
                background-color: \(c(CrossRole.accent)); color: \(selectedText);
            }
            *:focus-visible { outline-color: \(c(CrossRole.accent.opacity(0.5))); }
            selection { background-color: \(c(CrossRole.accentTintStrong)); }
            """
    }

    static func cssColor(_ color: RGBA) -> String {
        func byte(_ value: Double) -> Int { Int((min(max(value, 0), 1) * 255).rounded()) }
        let alpha = (color.alpha * 1000).rounded() / 1000
        return "rgba(\(byte(color.red)),\(byte(color.green)),\(byte(color.blue)),\(alpha))"
    }
}

extension View {
    /// Applies the toolkit theme for this view's appearance (Linux; a no-op
    /// elsewhere). Put it on the window's root view.
    @ViewBuilder
    public func toolkitTheme(_ colorScheme: ColorScheme) -> some View {
        #if os(Linux)
            let appearance: DashAppearance = colorScheme == .dark ? .dark : .light
            inspect([.onCreate, .afterUpdate]) { _ in
                DashGtkTheme.apply(appearance)
            }
        #else
            self
        #endif
    }

    /// Applies the toolkit theme for the appearance in effect around this
    /// view (the window's preferred colour scheme, or the system's).
    public func toolkitThemeFromEnvironment() -> some View {
        ToolkitThemed(content: self)
    }

    /// Adds a CSS class to the first list box inside this view (Linux), so
    /// the toolkit theme can style it (the sidebar's accent selection).
    @ViewBuilder
    public func listCSSClass(_ name: String) -> some View {
        #if os(Linux)
            inspect([.onCreate]) { widget in
                guard
                    let list = GtkAccessibleNames.firstWidget(
                        in: widget.widgetPointer, roles: [GTK_ACCESSIBLE_ROLE_LIST.rawValue])
                else { return }
                gtk_widget_add_css_class(list, name)
            }
        #else
            self
        #endif
    }
}

struct ToolkitThemed<Content: View>: View {
    @Environment(\.colorScheme) var colorScheme
    let content: Content

    var body: some View {
        content.toolkitTheme(colorScheme)
    }
}

#if os(Linux)
    /// fontconfig, loaded at run time: GTK links it, but its header is not in
    /// the CGtk module map.
    enum Fontconfig {
        private typealias AddDir = @convention(c) (OpaquePointer?, UnsafePointer<CChar>) -> Int32

        static func addDirectory(_ path: String) -> Bool {
            guard
                let handle = dlopen("libfontconfig.so.1", RTLD_NOW),
                let symbol = dlsym(handle, "FcConfigAppFontAddDir")
            else { return false }
            let addDir = unsafeBitCast(symbol, to: AddDir.self)
            return addDir(nil, path) != 0
        }
    }

    @MainActor
    enum DashGtkTheme {
        private static var provider: Gtk.CSSProvider?
        private static var applied: DashAppearance?

        static func apply(_ appearance: DashAppearance) {
            guard applied != appearance else { return }
            applied = appearance
            if provider == nil {
                // Above the per-widget providers SwiftCrossUI adds at APPLICATION priority.
                provider = Gtk.CSSProvider(priority: UInt32(GTK_STYLE_PROVIDER_PRIORITY_APPLICATION) + 1)
                setFontName("\(ToolkitTheme.fontFamily) \(ToolkitTheme.defaultFontSize)")
            }
            provider?.loadCss(from: ToolkitTheme.css(for: appearance))
            setPreferDark(appearance == .dark)
        }

        private static func settingsObject() -> UnsafeMutablePointer<CGtk.GObject>? {
            guard let settings = gtk_settings_get_default() else { return nil }
            return UnsafeMutableRawPointer(settings).assumingMemoryBound(to: CGtk.GObject.self)
        }

        private static func setFontName(_ name: String) {
            guard let object = settingsObject() else { return }
            var value = GValue()
            g_value_init(&value, GType(16 << G_TYPE_FUNDAMENTAL_SHIFT))  // G_TYPE_STRING
            g_value_set_string(&value, name)
            g_object_set_property(object, "gtk-font-name", &value)
            g_value_unset(&value)
        }

        private static func setPreferDark(_ dark: Bool) {
            guard let object = settingsObject() else { return }
            var value = GValue()
            g_value_init(&value, GType(5 << G_TYPE_FUNDAMENTAL_SHIFT))  // G_TYPE_BOOLEAN
            g_value_set_boolean(&value, dark ? 1 : 0)
            g_object_set_property(object, "gtk-application-prefer-dark-theme", &value)
            g_value_unset(&value)
        }
    }
#endif
