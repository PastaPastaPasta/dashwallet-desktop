// Small capsule label for a state: InstantSend, ChainLocked, unconfirmed, network name.
#if os(macOS)
import DesignTokens
import SwiftUI

/// Colour role of a badge or status-bar item.
public enum DashTone: Sendable, Hashable, CaseIterable {
    case neutral, info, success, warning, error

    /// Text and icon colour.
    var foreground: Color {
        switch self {
        case .neutral: .dash.secondaryText
        case .info: .dash.blueText
        case .success: .dash.successText
        case .warning: .dash.orange
        case .error: .dash.errorText
        }
    }

    /// Capsule fill.
    var background: Color {
        switch self {
        case .neutral: .dash.gray300Alpha10
        case .info: .dash.blueAlpha10
        case .success: .dash.greenAlpha10
        case .warning: .dash.orangeAlpha10
        case .error: .dash.redAlpha10
        }
    }
}

/// A short status label on a tinted capsule, with an optional leading icon.
public struct Badge: View {
    public let text: String
    public let tone: DashTone
    public let icon: DashIconSource?

    public init(_ text: String, tone: DashTone = .neutral, icon: DashIconSource? = nil) {
        self.text = text
        self.tone = tone
        self.icon = icon
    }

    public var body: some View {
        HStack(spacing: DashSpacing.xxs) {
            if let icon {
                DashIconImage(icon)
                    .scaledToFit()
                    .frame(width: 12, height: 12)
                    .accessibilityHidden(true)
            }
            Text(text)
                .font(DashTextStyle.caption1Medium.font)
                .lineLimit(1)
        }
        .foregroundStyle(tone.foreground)
        .padding(.horizontal, DashSpacing.xs)
        .padding(.vertical, DashSpacing.xxxs)
        .background(
            RoundedRectangle(cornerRadius: DashRadius.switcher, style: .continuous)
                .fill(tone.background)
        )
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(text))
    }
}

public extension Badge {
    /// The transaction is locked by an InstantSend lock.
    static func instantSend(_ text: String? = nil) -> Badge {
        Badge(text ?? NSLocalizedString("InstantSend", bundle: .module, comment: "Badge"), tone: .info)
    }

    /// The transaction's block is ChainLocked.
    static func chainLocked(_ text: String? = nil) -> Badge {
        Badge(text ?? NSLocalizedString("ChainLocked", bundle: .module, comment: "Badge"), tone: .success)
    }

    /// The transaction has no lock and too few confirmations.
    static func unconfirmed(_ text: String? = nil) -> Badge {
        Badge(text ?? NSLocalizedString("Unconfirmed", bundle: .module, comment: "Badge"), tone: .warning)
    }
}
#endif
