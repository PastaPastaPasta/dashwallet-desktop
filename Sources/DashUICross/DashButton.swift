// DashUIKit `DashButton` (C14): a text button, optionally with a leading
// icon, in one of the Dash styles and sizes, with a hover tint.
import DesignTokens
import SwiftCrossUI

/// DashUIKit button styles used by the desktop screens.
public enum DashButtonStyle: Sendable, Hashable, CaseIterable {
    case filledBlue
    case filledRed
    case tintedBlue
    case tintedGray
    case strokeGray
    case plainBlue
    case plainRed
    /// On the blue hero / lock screen: white fill, blue text.
    case filledWhite
    /// On the blue hero / lock screen: translucent white fill, white text.
    case tintedWhite

    func background(enabled: Bool) -> DashColor? {
        switch self {
        case .filledBlue: enabled ? .buttonFilledBlueBackground : .buttonFilledBlueBackgroundDisabled
        case .filledRed: enabled ? .buttonFilledRedBackground : .buttonFilledRedBackgroundDisabled
        case .tintedBlue: enabled ? .buttonTintedBlueBackground : .buttonTintedBlueBackgroundDisabled
        case .tintedGray: enabled ? .buttonTintedGrayBackground : .buttonTintedGrayBackgroundDisabled
        case .filledWhite: enabled ? .buttonFilledWhiteBackground : .buttonFilledWhiteBackgroundDisabled
        case .tintedWhite: enabled ? .buttonTintedWhiteBackground : .buttonTintedWhiteBackgroundDisabled
        case .strokeGray: enabled ? nil : .buttonStrokeGrayBackgroundDisabled
        case .plainBlue, .plainRed: nil
        }
    }

    func content(enabled: Bool) -> DashColor {
        switch self {
        case .filledBlue: enabled ? .buttonFilledBlueContent : .buttonFilledBlueContentDisabled
        case .filledRed: enabled ? .buttonFilledRedContent : .buttonFilledRedContentDisabled
        case .tintedBlue: enabled ? .buttonTintedBlueContent : .buttonTintedBlueContentDisabled
        // DashUIKit's tinted-gray and stroke-gray contents resolve to near-black
        // in dark appearance (UX-SPEC §2.1 notes); the desktop uses the
        // adaptive text colours instead.
        case .tintedGray: enabled ? CrossRole.textPrimary : CrossRole.textTertiary
        case .strokeGray: enabled ? CrossRole.textPrimary : CrossRole.textTertiary
        case .plainBlue: enabled ? .buttonPlainBlueContent : CrossRole.textTertiary
        case .plainRed: enabled ? .buttonPlainRedContent : CrossRole.textTertiary
        case .filledWhite: enabled ? .buttonFilledWhiteContent : .buttonFilledWhiteContentDisabled
        case .tintedWhite: enabled ? .buttonTintedWhiteContent : .buttonTintedWhiteContentDisabled
        }
    }

    /// The hover layer: buttons with a fill darken (light) / lighten (dark)
    /// slightly; text-only buttons get the accent tint (UX-SPEC §3.2).
    var hover: DashColor {
        switch self {
        case .plainBlue, .plainRed, .strokeGray: CrossRole.accentTint
        default: DashColor(light: DashColor.black.light.withAlpha(0.06), dark: DashColor.white.dark.withAlpha(0.08))
        }
    }
}

extension RGBA {
    func withAlpha(_ alpha: Double) -> RGBA {
        RGBA(red: red, green: green, blue: blue, alpha: alpha)
    }
}

public enum DashButtonSize: Sendable, Hashable, CaseIterable {
    case large, medium, small, extraSmall

    var metrics: DashButtonMetrics {
        switch self {
        case .large: .large
        case .medium: .medium
        case .small: .small
        case .extraSmall: .extraSmall
        }
    }
}

/// A Dash-styled button. The title is the button's accessible name.
public struct DashButton: View {
    let title: String
    let style: DashButtonStyle
    let size: DashButtonSize
    let isEnabled: Bool
    let help: String?
    let icon: DashIconToken?
    let fillsWidth: Bool
    let action: @MainActor @Sendable () -> Void

    @State var hovering = false

    public init(
        _ title: String, style: DashButtonStyle = .filledBlue, size: DashButtonSize = .medium,
        isEnabled: Bool = true, help: String? = nil, icon: DashIconToken? = nil, fillsWidth: Bool = false,
        action: @escaping @MainActor @Sendable () -> Void
    ) {
        self.title = title
        self.style = style
        self.size = size
        self.isEnabled = isEnabled
        self.help = help
        self.icon = icon
        self.fillsWidth = fillsWidth
        self.action = action
    }

    public var body: some View {
        let metrics = size.metrics
        let content = style.content(enabled: isEnabled)
        let shaped = label(metrics: metrics, content: content)
            // SwiftCrossUI truncates text to the proposed size; a stack that
            // proposes less than the title needs (the M2 Linux run showed
            // "OK" as "…" and "Wallet" as "Wa…") would cut the title.
            .fixedSize()
            .padding(.horizontal, points(metrics.horizontalPadding))
            .padding(.vertical, points(metrics.verticalPadding))
            .frame(maxWidth: fillsWidth ? .infinity : nil)
            .background {
                let shape = RoundedRectangle(cornerRadius: metrics.cornerRadius)
                if let background = style.background(enabled: isEnabled) {
                    shape.fill(background.color)
                } else if style == .strokeGray {
                    shape.stroke(DashColor.buttonStrokeGrayStroke.color)
                }
                if hovering && isEnabled {
                    shape.fill(style.hover.color)
                }
            }
            .onHover { hovering = $0 }
        if let help {
            shaped.help(help)
        } else {
            shaped
        }
    }

    @ViewBuilder
    private func label(metrics: DashButtonMetrics, content: DashColor) -> some View {
        let font = Font.system(size: metrics.fontSize, weight: .medium)
        if let icon {
            Button(action: action) {
                HStack(spacing: points(metrics.gap)) {
                    DashIcon(icon, size: metrics.fontSize + 2, tint: .color(content))
                    Text(title).font(font).foregroundColor(content.color)
                }
            }
            .buttonStyle(.plain)
            .disabled(!isEnabled)
            .accessibilityLabel(title)
        } else {
            Button(title, action: action)
                .buttonStyle(.plain)
                .font(font)
                .foregroundColor(content.color)
                .disabled(!isEnabled)
        }
    }
}
