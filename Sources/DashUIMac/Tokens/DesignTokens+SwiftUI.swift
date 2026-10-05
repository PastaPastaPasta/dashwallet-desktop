// Bridges toolkit-neutral DesignTokens values (colours, type scale) into SwiftUI and AppKit types.
#if os(macOS)
import AppKit
import DesignTokens
import SwiftUI

public extension NSColor {
    /// A dynamic colour that resolves to the token's light or dark value for the drawing appearance.
    convenience init(dash token: DashColor) {
        self.init(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            return NSColor(dash: token.resolved(for: isDark ? .dark : .light))
        }
    }

    /// A static sRGB colour.
    convenience init(dash rgba: RGBA) {
        self.init(srgbRed: rgba.red, green: rgba.green, blue: rgba.blue, alpha: rgba.alpha)
    }
}

public extension Color {
    /// A colour that follows the view's colour scheme, built from a DesignTokens colour.
    init(dash token: DashColor) {
        self.init(nsColor: NSColor(dash: token))
    }

    /// A static sRGB colour.
    init(dash rgba: RGBA) {
        self.init(.sRGB, red: rgba.red, green: rgba.green, blue: rgba.blue, opacity: rgba.alpha)
    }
}

public extension DashFontWeight {
    /// The matching SwiftUI font weight.
    var swiftUI: Font.Weight {
        switch self {
        case .regular: .regular
        case .medium: .medium
        case .semibold: .semibold
        case .bold: .bold
        }
    }
}

public extension DashTextStyle {
    /// The system font at this style's size and weight, without the design line height.
    var font: Font { .system(size: size, weight: weight.swiftUI) }
}

public extension DashAppearance {
    /// The appearance that matches a SwiftUI colour scheme.
    init(_ colorScheme: ColorScheme) {
        self = colorScheme == .dark ? .dark : .light
    }
}
#endif
