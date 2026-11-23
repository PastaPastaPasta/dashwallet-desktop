// Cards, rows and headers (UX-SPEC §3.1): the menu card (C11, DashUIKit
// `MenuViewModifier`), `MenuItem` / `MenuRow`, `TopIntro` (C13), the detail
// and copy rows (C12, C32), badges and the network capsule (C20), empty and
// loading states (C25, C26) and the wizard header (C29).
//
// SwiftCrossUI 0.10 has no shadows: level-1 and level-2 elevation are a
// hairline border in light appearance (UX-SPEC §2.5).
import DesignTokens
import SwiftCrossUI

/// A rounded card on the canvas (C11 menu card, page cards).
public struct DashCard<Content: View>: View {
    let content: Content
    let padding: Int
    let spacing: Int
    let radius: Int
    let fill: DashColor

    public init(
        padding: Int = CrossLayout.cardPadding, spacing: Int = points(DashSpacing.m), radius: Int = points(DashRadius.card),
        fill: DashColor = CrossRole.card, @ViewBuilder content: () -> Content
    ) {
        self.content = content()
        self.padding = padding
        self.spacing = spacing
        self.radius = radius
        self.fill = fill
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: spacing) {
            content
        }
        .padding(padding)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardBackground(fill: fill, radius: radius)
    }
}

extension View {
    /// A filled rounded rectangle with the level-1 hairline behind this view.
    public func cardBackground(
        fill: DashColor = CrossRole.card, radius: Int = points(DashRadius.card), border: DashColor = CrossRole.cardBorder
    ) -> some View {
        background {
            RoundedRectangle(cornerRadius: Double(radius)).fill(fill.color)
            RoundedRectangle(cornerRadius: Double(radius)).stroke(border.color)
        }
    }
}

/// DashUIKit `MenuItem`: optional 30 pt icon, title, optional help line and
/// trailing text (C11 row with a value accessory).
public struct MenuItem: View {
    let icon: DashIconToken?
    let title: String
    let subtitle: String?
    let trailing: String?
    let destructive: Bool

    public init(
        icon: DashIconToken? = nil, title: String, subtitle: String? = nil, trailing: String? = nil,
        destructive: Bool = false
    ) {
        self.icon = icon
        self.title = title
        self.subtitle = subtitle
        self.trailing = trailing
        self.destructive = destructive
    }

    public var body: some View {
        MenuRow(icon: icon, title: title, help: subtitle, destructive: destructive) {
            if let trailing {
                Text(trailing)
                    .dashFont(.subhead)
                    .dashForeground(CrossRole.textSecondary)
                    .lineLimit(1)
            }
        }
    }
}

/// A menu-card row (C11): 30 pt icon, title `subheadMedium`, help `footnote`
/// tertiary and a trailing accessory (value, toggle, button, chevron text).
public struct MenuRow<Accessory: View>: View {
    let icon: DashIconToken?
    let title: String
    let help: String?
    let destructive: Bool
    let accessory: Accessory

    public init(
        icon: DashIconToken? = nil, title: String, help: String? = nil, destructive: Bool = false,
        @ViewBuilder accessory: () -> Accessory
    ) {
        self.icon = icon
        self.title = title
        self.help = help
        self.destructive = destructive
        self.accessory = accessory()
    }

    public var body: some View {
        HStack(spacing: points(DashSpacing.m)) {
            if let icon {
                DashIcon(icon, size: 30, width: 30)
            }
            VStack(alignment: .leading, spacing: points(DashSpacing.xxxs)) {
                Text(title)
                    .dashFont(.subheadMedium)
                    .dashForeground(destructive ? CrossRole.danger : CrossRole.textPrimary)
                if let help {
                    Text(help)
                        .dashFont(.footnote)
                        .dashForeground(CrossRole.textTertiary)
                }
            }
            Spacer()
            accessory
        }
        .padding(.vertical, points(DashSpacing.xs))
        .frame(minHeight: Double(CrossLayout.rowMinHeight) - 2 * DashSpacing.xs)
    }
}

/// DashUIKit `TopIntroView` (C13): the page title and up to two lines of
/// description.
public struct TopIntro: View {
    let title: String
    let description: String?

    public init(_ title: String, description: String? = nil) {
        self.title = title
        self.description = description
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: points(DashSpacing.xs)) {
            Text(title)
                .dashFont(.title2)
                .dashForeground(CrossRole.textPrimary)
            if let description {
                Text(description)
                    .dashFont(.subhead)
                    .dashForeground(CrossRole.textSecondary)
            }
        }
        .padding(.trailing, 60)
    }
}

/// A page or card heading.
public struct SectionHeader: View {
    let title: String
    let style: DashTextStyle
    let color: DashColor

    public init(_ title: String, style: DashTextStyle = .headline, color: DashColor = CrossRole.textPrimary) {
        self.title = title
        self.style = style
        self.color = color
    }

    public var body: some View {
        Text(title)
            .dashFont(style)
            .dashForeground(color)
    }
}

/// Label and value on one line, value selectable (C32 `KeyValueGrid` row):
/// label `footnote` secondary, value `footnote` primary. `monospaced` is for
/// the UX-SPEC §5.6 value kinds only (hashes, keys, scripts, payloads).
public struct KeyValueRow: View {
    let key: String
    let value: String
    let help: String?
    let monospaced: Bool

    public init(_ key: String, _ value: String, help: String? = nil, monospaced: Bool = false) {
        self.key = key
        self.value = value
        self.help = help
        self.monospaced = monospaced
    }

    public var body: some View {
        HStack(alignment: .top, spacing: points(DashSpacing.m)) {
            Text(key)
                .dashFont(.footnote)
                .dashForeground(CrossRole.textSecondary)
                .frame(width: 160, alignment: .leading)
            valueText
            Spacer()
        }
    }

    @ViewBuilder
    private var valueText: some View {
        let text = Text(value)
            .font(monospaced ? DashTextStyle.footnote.font.monospaced() : DashTextStyle.footnote.font)
            .dashForeground(CrossRole.textPrimary)
            .textSelectionEnabled()
        if let help {
            text.help(help)
        } else {
            text
        }
    }
}

/// A copyable value (C12 `CopyRow`, stacked layout): label over a selectable
/// value and a "Copy" button (icon + text: no icon-only controls on Cross).
public struct CopyRow: View {
    let label: String
    let value: String
    let copyTitle: String
    let monospaced: Bool
    let onCopy: @MainActor @Sendable () -> Void

    public init(
        _ label: String, value: String, copyTitle: String = "Copy", monospaced: Bool = false,
        onCopy: @escaping @MainActor @Sendable () -> Void
    ) {
        self.label = label
        self.value = value
        self.copyTitle = copyTitle
        self.monospaced = monospaced
        self.onCopy = onCopy
    }

    public var body: some View {
        HStack(alignment: .center, spacing: points(DashSpacing.m)) {
            VStack(alignment: .leading, spacing: points(DashSpacing.xxs)) {
                Text(label)
                    .dashFont(.footnote)
                    .dashForeground(CrossRole.textSecondary)
                Text(value)
                    .font(monospaced ? DashTextStyle.footnote.font.monospaced() : DashTextStyle.subhead.font)
                    .dashForeground(CrossRole.textPrimary)
                    .textSelectionEnabled()
            }
            Spacer()
            DashButton(copyTitle, style: .tintedGray, size: .small, icon: .copy, action: onCopy)
        }
    }
}

/// Status chip / badge tones (C20).
public enum BadgeTone: Sendable, Hashable {
    case info, success, warning, danger, neutral

    var foreground: DashColor {
        switch self {
        case .info: CrossRole.textLink
        case .success: CrossRole.success
        case .warning: CrossRole.warning
        case .danger: CrossRole.danger
        case .neutral: CrossRole.textSecondary
        }
    }

    var background: DashColor {
        switch self {
        case .info: CrossRole.accentTint
        case .success: CrossRole.successTint
        case .warning: CrossRole.warningTint
        case .danger: CrossRole.dangerTint
        case .neutral: CrossRole.neutralTint
        }
    }
}

/// A status chip (C20): `caption1Medium` on a tinted rounded rectangle (r7).
public struct DashBadge: View {
    let text: String
    let tone: BadgeTone
    let help: String?

    public init(_ text: String, tone: BadgeTone = .info, help: String? = nil) {
        self.text = text
        self.tone = tone
        self.help = help
    }

    public var body: some View {
        let chip = Text(text)
            .dashFont(.caption1Medium)
            .dashForeground(tone.foreground)
            .lineLimit(1)
            .fixedSize()
            .padding(.horizontal, points(DashSpacing.xs))
            .padding(.vertical, points(DashSpacing.xxxs))
            .background(RoundedRectangle(cornerRadius: Double(CrossLayout.chipRadius)).fill(tone.background.color))
        if let help {
            chip.help(help)
        } else {
            chip
        }
    }
}

/// The network marker on the hero and the lock screen (C20): uppercase
/// `caption2` bold, white on orange, capsule.
public struct NetworkCapsule: View {
    let name: String

    public init(_ name: String) {
        self.name = name
    }

    public var body: some View {
        Text(name.uppercased())
            .font(.system(size: DashTextStyle.caption2.size, weight: .bold))
            .foregroundColor(CrossRole.white.color)
            .fixedSize()
            .padding(.horizontal, points(DashSpacing.sm))
            .padding(.vertical, points(DashSpacing.xxxs) + 1)
            .background(Capsule().fill(CrossRole.warning.opacity(0.9).color))
    }
}

/// An empty list or page (C25): icon, title, message and an optional action.
public struct EmptyState<Action: View>: View {
    let icon: DashIconToken?
    let title: String
    let message: String?
    let action: Action

    public init(
        icon: DashIconToken? = nil, title: String, message: String? = nil, @ViewBuilder action: () -> Action
    ) {
        self.icon = icon
        self.title = title
        self.message = message
        self.action = action()
    }

    public var body: some View {
        VStack(spacing: points(DashSpacing.s)) {
            if let icon {
                DashIcon(icon, size: 48, tint: .color(CrossRole.textTertiary))
            }
            Text(title)
                .dashFont(.headline)
                .dashForeground(CrossRole.textPrimary)
            if let message {
                Text(message)
                    .dashFont(.subhead)
                    .dashForeground(CrossRole.textSecondary)
            }
            action
        }
        .padding(points(DashSpacing.xxl))
        .frame(maxWidth: .infinity)
    }
}

extension EmptyState where Action == EmptyView {
    public init(icon: DashIconToken? = nil, title: String, message: String? = nil) {
        self.icon = icon
        self.title = title
        self.message = message
        self.action = EmptyView()
    }
}

/// A spinner and a short `footnote` line (C26).
public struct LoadingState: View {
    let text: String

    public init(_ text: String) {
        self.text = text
    }

    public var body: some View {
        HStack(spacing: points(DashSpacing.s)) {
            ProgressView()
            Text(text)
                .dashFont(.footnote)
                .dashForeground(CrossRole.textSecondary)
        }
        .padding(points(DashSpacing.m))
    }
}

/// "Step %1 of %2 · %3" with a progress bar (C29).
public struct WizardHeader: View {
    let step: Int
    let count: Int
    let title: String

    public init(step: Int, of count: Int, title: String) {
        self.step = step
        self.count = count
        self.title = title
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: points(DashSpacing.xs)) {
            Text("Step \(step) of \(count)")
                .dashFont(.footnote)
                .dashForeground(CrossRole.textSecondary)
            Text(title)
                .dashFont(.title3)
                .dashForeground(CrossRole.textPrimary)
            ProgressView(value: count > 0 ? Double(step) / Double(count) : 0)
        }
    }
}

/// The outcome of an operation (C24): a large status icon, the title, an
/// optional detail (a txid is technical text: `monospacedDetail`) and one
/// button.
public struct ResultCard: View {
    let icon: DashIconToken
    let title: String
    let detail: String?
    let monospacedDetail: Bool
    let buttonTitle: String
    let action: @MainActor @Sendable () -> Void

    public init(
        icon: DashIconToken, title: String, detail: String?, monospacedDetail: Bool = false, buttonTitle: String,
        action: @escaping @MainActor @Sendable () -> Void
    ) {
        self.icon = icon
        self.title = title
        self.detail = detail
        self.monospacedDetail = monospacedDetail
        self.buttonTitle = buttonTitle
        self.action = action
    }

    public var body: some View {
        DashCard(padding: points(DashSpacing.xxl)) {
            VStack(spacing: points(DashSpacing.m)) {
                DashIcon(icon, size: 60, width: 60)
                Text(title)
                    .dashFont(.title3)
                    .dashForeground(CrossRole.textPrimary)
                if let detail {
                    Text(detail)
                        .font(monospacedDetail ? DashTextStyle.footnote.font.monospaced() : DashTextStyle.footnote.font)
                        .dashForeground(CrossRole.textSecondary)
                        .textSelectionEnabled()
                }
                DashButton(buttonTitle, style: .filledBlue, size: .large, action: action)
            }
            .frame(maxWidth: .infinity)
        }
    }
}

/// A hairline between rows or sections.
public struct Hairline: View {
    public init() {}

    public var body: some View {
        Rectangle()
            .fill(CrossRole.separator.color)
            .frame(height: 1)
            .frame(maxWidth: .infinity)
    }
}
