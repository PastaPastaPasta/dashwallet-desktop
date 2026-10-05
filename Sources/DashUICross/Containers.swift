// Cards, rows and headers: DashUIKit `MenuViewModifier` (the rounded card),
// `MenuItem`, plus the desktop key/value row and section header.
import DesignTokens
import SwiftCrossUI

/// The rounded menu card of DashUIKit (`MenuViewModifier`).
public struct DashCard<Content: View>: View {
    let content: Content

    public init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: points(DashSpacing.s)) {
            content
        }
        .padding(points(DashSpacing.l))
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: DashRadius.card).fill(DashColor.secondaryBackground.color))
    }
}

/// DashUIKit `MenuItem`: a title, an optional subtitle and trailing text.
public struct MenuItem: View {
    let title: String
    let subtitle: String?
    let trailing: String?

    public init(title: String, subtitle: String? = nil, trailing: String? = nil) {
        self.title = title
        self.subtitle = subtitle
        self.trailing = trailing
    }

    public var body: some View {
        HStack(spacing: points(DashSpacing.m)) {
            VStack(alignment: .leading, spacing: points(DashSpacing.xxxs)) {
                Text(title)
                    .dashFont(.subheadMedium)
                    .dashForeground(.primaryText)
                    .lineLimit(1)
                if let subtitle {
                    Text(subtitle)
                        .dashFont(.footnote)
                        .dashForeground(.secondaryText)
                        .lineLimit(1)
                }
            }
            Spacer()
            if let trailing {
                Text(trailing)
                    .dashFont(.subhead)
                    .dashForeground(.primaryText)
                    .lineLimit(1)
            }
        }
    }
}

/// A page or card heading.
public struct SectionHeader: View {
    let title: String
    let style: DashTextStyle

    public init(_ title: String, style: DashTextStyle = .headline) {
        self.title = title
        self.style = style
    }

    public var body: some View {
        Text(title)
            .dashFont(style)
            .dashForeground(.primaryText)
    }
}

/// "Label: value" on one line, value selectable (detail panes, confirm text).
public struct KeyValueRow: View {
    let key: String
    let value: String

    public init(_ key: String, _ value: String) {
        self.key = key
        self.value = value
    }

    public var body: some View {
        HStack(alignment: .top, spacing: points(DashSpacing.s)) {
            Text(key)
                .dashFont(.footnoteMedium)
                .dashForeground(.secondaryText)
                .frame(width: 150, alignment: .leading)
            Text(value)
                .dashFont(.footnote)
                .dashForeground(.primaryText)
                .textSelectionEnabled()
            Spacer()
        }
    }
}

/// A small coloured pill (network badge, status tags).
public struct DashBadge: View {
    let text: String
    let foreground: DashColor
    let background: DashColor

    public init(_ text: String, foreground: DashColor = .orange, background: DashColor = .orangeAlpha10) {
        self.text = text
        self.foreground = foreground
        self.background = background
    }

    public var body: some View {
        Text(text)
            .dashFont(.caption1Medium)
            .dashForeground(foreground)
            .padding(.horizontal, points(DashSpacing.s))
            .padding(.vertical, points(DashSpacing.xxxs))
            .background(RoundedRectangle(cornerRadius: DashRadius.small).fill(background.color))
    }
}
