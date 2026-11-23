// SwiftUI side of the UX-SPEC colour roles (§2.1), card surfaces and elevation (§2.5), the
// DashButton look as a ButtonStyle (C14) and the hover tint of rows (§3.2).
#if os(macOS)
import DesignTokens
import SwiftUI

public extension Color {
    /// `Color.role.<name>`: the desktop colour roles (`DashColor.Role`), following the view's
    /// light/dark appearance.
    static var role: DashRoleColors.Type { DashRoleColors.self }
}

/// The colour roles of `DashColor.Role` as SwiftUI colours.
public enum DashRoleColors {
    public static var canvas: Color { Color(dash: DashColor.Role.canvas) }
    public static var card: Color { Color(dash: DashColor.Role.card) }
    public static var cardRaised: Color { Color(dash: DashColor.Role.cardRaised) }
    public static var hero: Color { Color(dash: DashColor.Role.hero) }
    public static var heroCard: Color { Color(dash: DashColor.Role.heroCard).opacity(DashOpacity.heroCard) }
    public static var heroDivider: Color { Color(dash: DashColor.Role.heroCard).opacity(DashOpacity.heroDivider) }
    public static var accent: Color { Color(dash: DashColor.Role.accent) }
    public static var accentTint: Color { Color(dash: DashColor.Role.accentTint) }
    public static var accentTintStrong: Color { Color(dash: DashColor.Role.accentTintStrong) }
    public static var textPrimary: Color { Color(dash: DashColor.Role.textPrimary) }
    public static var textSecondary: Color { Color(dash: DashColor.Role.textSecondary) }
    public static var textTertiary: Color { Color(dash: DashColor.Role.textTertiary) }
    public static var textOnHero: Color { Color(dash: DashColor.Role.textOnHero) }
    public static var textOnHeroSecondary: Color {
        Color(dash: DashColor.Role.textOnHero).opacity(DashOpacity.heroSecondaryText)
    }
    public static var textLink: Color { Color(dash: DashColor.Role.textLink) }
    public static var separator: Color { Color(dash: DashColor.Role.separator) }
    public static var fieldFill: Color { Color(dash: DashColor.Role.fieldFill) }
    public static var fieldStroke: Color { Color(dash: DashColor.Role.fieldStroke) }
    public static var success: Color { Color(dash: DashColor.Role.success) }
    public static var successTint: Color { Color(dash: DashColor.Role.successTint) }
    public static var danger: Color { Color(dash: DashColor.Role.danger) }
    public static var dangerTint: Color { Color(dash: DashColor.Role.dangerTint) }
    public static var warning: Color { Color(dash: DashColor.Role.warning) }
    public static var warningTint: Color { Color(dash: DashColor.Role.warningTint) }
    public static var caution: Color { Color(dash: DashColor.Role.caution) }
    public static var cautionTint: Color { Color(dash: DashColor.Role.cautionTint) }
    public static var neutralTint: Color { Color(dash: DashColor.Role.neutralTint) }
    public static var overlay: Color { Color(dash: DashColor.Role.overlay) }
    public static var progressTrack: Color { Color(dash: DashColor.Role.progressTrack) }
    public static var cardBorder: Color { Color(dash: DashColor.Role.cardBorder) }
    public static var shadow: Color { Color(dash: DashColor.Role.shadow) }
}

// MARK: Cards and elevation

/// A card surface: `card` fill, a continuous rounded rectangle, the elevation's shadow in light
/// appearance and, for floating elevations, a hairline border in dark appearance.
public struct DashCardModifier: ViewModifier {
    let radius: Double
    let padding: Double?
    let elevation: DashElevation?
    let fill: Color
    @Environment(\.colorScheme) private var colorScheme

    public func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: radius, style: .continuous)
        // The shadow belongs to the card shape only, never to the text on it.
        content
            .padding(padding ?? 0)
            .background(
                shape.fill(fill)
                    .shadow(
                        color: elevation.map { Color(dash: $0.color) } ?? .clear,
                        radius: elevation?.radius ?? 0, x: 0, y: elevation?.y ?? 0))
            .overlay {
                if colorScheme == .dark, elevation?.borderInDark == true {
                    shape.strokeBorder(Color.role.separator, lineWidth: 0.5)
                }
            }
    }
}

public extension View {
    /// Draws the view on a Dash card (UX-SPEC §2.4, §2.5).
    func dashCard(
        radius: Double = DashRadius.card, padding: Double? = DashLayout.cardPadding,
        elevation: DashElevation? = .card, fill: Color = Color.role.card
    ) -> some View {
        modifier(DashCardModifier(radius: radius, padding: padding, elevation: elevation, fill: fill))
    }

    /// The page background of every window (`canvas`), never DashUIKit's black `primaryBackground`.
    func dashCanvas() -> some View {
        background(Color.role.canvas)
    }
}

// MARK: Buttons

/// DashUIKit `DashButton`'s look for a plain SwiftUI `Button`, so screens keep their buttons,
/// keyboard shortcuts and identifiers and only change the style: `.buttonStyle(.dash(.filledBlue))`.
/// Hover darkens by 4 %; pressed by 8 %. Disabled content of the stroke and plain styles uses the
/// desktop roles (DashUIKit's are near-black in dark appearance).
public struct DashActionButtonStyle: ButtonStyle {
    public let style: DashButtonStyle
    public let size: DashButtonSize
    public let fillsWidth: Bool

    public init(style: DashButtonStyle, size: DashButtonSize, fillsWidth: Bool) {
        self.style = style
        self.size = size
        self.fillsWidth = fillsWidth
    }

    public func makeBody(configuration: Configuration) -> some View {
        DashActionButtonBody(configuration: configuration, style: style, size: size, fillsWidth: fillsWidth)
    }
}

private struct DashActionButtonBody: View {
    let configuration: ButtonStyleConfiguration
    let style: DashButtonStyle
    let size: DashButtonSize
    let fillsWidth: Bool
    @Environment(\.isEnabled) private var isEnabled
    @State private var isHovering = false

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: size.radius, style: .continuous)
        configuration.label
            .font(size.fontSize.weight(.semibold))
            .lineLimit(1)
            .fixedSize(horizontal: !fillsWidth, vertical: false)
            .padding(.horizontal, size.hPadding)
            .padding(.vertical, size.vPadding)
            .frame(maxWidth: fillsWidth ? .infinity : nil)
            .foregroundStyle(foreground)
            .background(shape.fill(style.backgroundColor(isEnabled: isEnabled)))
            .overlay {
                if style == .strokeGray {
                    shape.inset(by: 0.5).stroke(Color.dash.buttonStrokeGrayStroke, lineWidth: 1)
                }
            }
            .overlay {
                shape.fill(Color.black.opacity(overlayOpacity)).allowsHitTesting(false)
            }
            .contentShape(shape)
            .onHover { isHovering = $0 }
            .animation(.easeOut(duration: DashMotion.hover), value: isHovering)
    }

    private var overlayOpacity: Double {
        guard isEnabled, !isPlain else { return 0 }
        if configuration.isPressed { return 0.08 }
        return isHovering ? 0.04 : 0
    }

    private var isPlain: Bool {
        switch style {
        case .plainBlue, .plainBlack, .plainRed, .plainWhite: true
        default: false
        }
    }

    private var foreground: Color {
        if !isEnabled {
            switch style {
            case .plainBlue, .plainRed, .strokeGray, .tintedBlue: return Color(dash: DashColor.Role.plainContentDisabled)
            default: return style.foregroundColor(isEnabled: false)
            }
        }
        if style == .strokeGray { return Color(dash: DashColor.Role.strokeGrayContent) }
        if isPlain, configuration.isPressed { return style.foregroundColor(isEnabled: true).opacity(0.7) }
        if isPlain, isHovering { return style.foregroundColor(isEnabled: true).opacity(0.85) }
        return style.foregroundColor(isEnabled: true)
    }
}

public extension ButtonStyle where Self == DashActionButtonStyle {
    /// DashUIKit's button look: `.buttonStyle(.dash(.filledBlue, .large))`.
    static func dash(_ style: DashButtonStyle, _ size: DashButtonSize = .medium, fillsWidth: Bool = false) -> Self {
        DashActionButtonStyle(style: style, size: size, fillsWidth: fillsWidth)
    }
}

// MARK: Hover rows

/// Tints a row with `accentTint` under the pointer and `accentTintStrong` when selected (§3.2).
public struct DashRowHighlight: ViewModifier {
    let isSelected: Bool
    let radius: Double
    @State private var isHovering = false

    public func body(content: Content) -> some View {
        content
            .background(
                RoundedRectangle(cornerRadius: radius, style: .continuous)
                    .fill(isSelected ? Color.role.accentTintStrong : (isHovering ? Color.role.accentTint : .clear)))
            .onHover { isHovering = $0 }
            .animation(.easeOut(duration: DashMotion.hover), value: isHovering)
    }
}

public extension View {
    func dashRowHighlight(isSelected: Bool = false, radius: Double = DashRadius.standard) -> some View {
        modifier(DashRowHighlight(isSelected: isSelected, radius: radius))
    }
}
#endif

#if os(macOS)
// MARK: Text fields

/// The iOS field look (`AddressFieldView`, C16): `fieldFill` on a radius-16 rounded rectangle, a
/// `fieldStroke` border while focused, `callout` text. `isError` tints the fill red.
public struct DashTextFieldStyle: TextFieldStyle {
    let isError: Bool
    let isTechnical: Bool

    public init(isError: Bool = false, isTechnical: Bool = false) {
        self.isError = isError
        self.isTechnical = isTechnical
    }

    public func _body(configuration: TextField<Self._Label>) -> some View {
        configuration.modifier(DashFieldModifier(isError: isError, isTechnical: isTechnical))
    }
}

/// The Dash field look for any text control (also `TextEditor` and `SecureField`).
public struct DashFieldModifier: ViewModifier {
    let isError: Bool
    let isTechnical: Bool
    @FocusState private var isFocused: Bool

    public init(isError: Bool = false, isTechnical: Bool = false) {
        self.isError = isError
        self.isTechnical = isTechnical
    }

    public func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: DashRadius.textField, style: .continuous)
        content
            .textFieldStyle(.plain)
            .font(isTechnical
                ? .system(size: DesignTokens.DashTextStyle.footnote.size, design: .monospaced)
                : DesignTokens.DashTextStyle.callout.font)
            .foregroundStyle(Color.role.textPrimary)
            .focused($isFocused)
            .padding(.horizontal, DashSpacing.m)
            .padding(.vertical, DashSpacing.sm)
            .frame(minHeight: 40)
            .background(shape.fill(isError ? Color.role.dangerTint : Color.role.fieldFill))
            .overlay(shape.strokeBorder(isFocused ? Color.role.accent : (isError ? Color.role.danger : .clear), lineWidth: 1))
    }
}

public extension TextFieldStyle where Self == DashTextFieldStyle {
    /// The Dash field look: `.textFieldStyle(.dash)`.
    static var dash: DashTextFieldStyle { DashTextFieldStyle() }
    static func dash(isError: Bool = false, isTechnical: Bool = false) -> DashTextFieldStyle {
        DashTextFieldStyle(isError: isError, isTechnical: isTechnical)
    }
}

/// A `footnote` secondary caption over a form control.
public struct FieldCaption: View {
    public let text: String

    public init(_ text: String) {
        self.text = text
    }

    public var body: some View {
        Text(text)
            .dashFont(.footnote)
            .foregroundStyle(Color.role.textSecondary)
    }
}
#endif
