// SwiftCrossUI re-implementations of the DashUIKit components (WS-12), plus
// the bridges from DesignTokens to SwiftCrossUI colours and fonts.
//
// Accessibility: upstream SwiftCrossUI 0.10.0 has no accessibility modifiers
// (ADR 0002, gap A1); the vendored fork adds `accessibilityLabel(_:)` and
// `accessibilityHint(_:)` (patch P1, Vendor/PATCHES.md). Fields, toggles and
// pickers take their caption as the control's name through it, buttons are
// always created with a string title, text fields also carry a descriptive
// placeholder (GTK exposes it as `placeholder-text`, gap A3), and no control
// is icon-only.
import DesignTokens
import SwiftCrossUI

public enum DashUICrossModule {
    public static let name = "DashUICross"
}

extension RGBA {
    /// The token as a SwiftCrossUI colour.
    public var crossColor: Color {
        Color(red: red, green: green, blue: blue, opacity: alpha)
    }
}

extension DashColor {
    /// An adaptive SwiftCrossUI colour that follows the light/dark appearance.
    public var color: Color {
        .adaptive(light: light.crossColor, dark: dark.crossColor)
    }
}

extension DashFontWeight {
    var crossWeight: Font.Weight {
        switch self {
        case .regular: .regular
        case .medium: .medium
        case .semibold: .semibold
        case .bold: .bold
        }
    }
}

extension DashTextStyle {
    /// The style as a system font. Line height and tracking are left to the
    /// toolkit (SwiftCrossUI has no line-height modifier).
    public var font: Font {
        .system(size: size, weight: weight.crossWeight)
    }
}

extension View {
    /// Applies a Dash text style.
    public func dashFont(_ style: DashTextStyle) -> some View {
        font(style.font)
    }

    /// Applies a Dash colour token as the foreground colour.
    public func dashForeground(_ token: DashColor) -> some View {
        foregroundColor(token.color)
    }
}

/// Integer points for the SwiftCrossUI modifiers that take `Int`.
@inline(__always)
func points(_ value: Double) -> Int { Int(value.rounded()) }
