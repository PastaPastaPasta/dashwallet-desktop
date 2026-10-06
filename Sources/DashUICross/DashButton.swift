// DashUIKit `DashButton`: a text button in one of the Dash styles and sizes.
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

    func background(enabled: Bool) -> DashColor? {
        switch self {
        case .filledBlue: enabled ? .buttonFilledBlueBackground : .buttonFilledBlueBackgroundDisabled
        case .filledRed: enabled ? .buttonFilledRedBackground : .buttonFilledRedBackgroundDisabled
        case .tintedBlue: enabled ? .buttonTintedBlueBackground : .buttonTintedBlueBackgroundDisabled
        case .tintedGray: enabled ? .buttonTintedGrayBackground : .buttonTintedGrayBackgroundDisabled
        case .strokeGray: enabled ? nil : .buttonStrokeGrayBackgroundDisabled
        case .plainBlue, .plainRed: nil
        }
    }

    func content(enabled: Bool) -> DashColor {
        switch self {
        case .filledBlue: enabled ? .buttonFilledBlueContent : .buttonFilledBlueContentDisabled
        case .filledRed: enabled ? .buttonFilledRedContent : .buttonFilledRedContentDisabled
        case .tintedBlue: enabled ? .buttonTintedBlueContent : .buttonTintedBlueContentDisabled
        case .tintedGray: enabled ? .buttonTintedGrayContent : .buttonTintedGrayContentDisabled
        // DashUIKit's stroke-gray content has no dark variant (#0A0A0D in both),
        // which is unreadable on the dark window background; use the adaptive
        // text colours instead.
        case .strokeGray: enabled ? .primaryText : .tertiaryText
        case .plainBlue: enabled ? .buttonPlainBlueContent : .buttonPlainBlueContentDisabled
        case .plainRed: enabled ? .buttonPlainRedContent : .buttonPlainRedContentDisabled
        }
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
    let action: @MainActor @Sendable () -> Void

    public init(
        _ title: String, style: DashButtonStyle = .filledBlue, size: DashButtonSize = .medium,
        isEnabled: Bool = true, help: String? = nil, action: @escaping @MainActor @Sendable () -> Void
    ) {
        self.title = title
        self.style = style
        self.size = size
        self.isEnabled = isEnabled
        self.help = help
        self.action = action
    }

    public var body: some View {
        let metrics = size.metrics
        let label = Button(title, action: action)
            .buttonStyle(.plain)
            .font(.system(size: metrics.fontSize, weight: .medium))
            .foregroundColor(style.content(enabled: isEnabled).color)
            .disabled(!isEnabled)
            .padding(.horizontal, points(metrics.horizontalPadding))
            .padding(.vertical, points(metrics.verticalPadding))
        let shaped = backgroundShape(label, metrics: metrics)
        if let help {
            shaped.help(help)
        } else {
            shaped
        }
    }

    @ViewBuilder
    private func backgroundShape(_ label: some View, metrics: DashButtonMetrics) -> some View {
        let shape = RoundedRectangle(cornerRadius: metrics.cornerRadius)
        if let background = style.background(enabled: isEnabled) {
            label.background(shape.fill(background.color))
        } else if style == .strokeGray {
            label.background(shape.stroke(DashColor.buttonStrokeGrayStroke.color))
        } else {
            label
        }
    }
}
