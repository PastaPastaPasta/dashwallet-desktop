// Menu cards and value rows (UX-SPEC C11, C12, C32): the iOS settings card with 30 pt icon rows,
// label/value detail rows with a copy button, and the technical key/value grid.
#if os(macOS)
import AppKit
import DesignTokens
import SwiftUI

// MARK: Menu card (C11)

/// The rounded menu card: radius 20, 6 pt inner padding, rows two points apart, no separator lines.
public struct MenuCard<Content: View>: View {
    public let title: String?
    public let footer: String?
    @ViewBuilder public let content: () -> Content

    public init(title: String? = nil, footer: String? = nil, @ViewBuilder content: @escaping () -> Content) {
        self.title = title
        self.footer = footer
        self.content = content
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.s) {
            if let title {
                Text(title)
                    .dashFont(.subheadMedium)
                    .foregroundStyle(Color.role.textSecondary)
                    .padding(.horizontal, DashSpacing.xxs)
                    .accessibilityAddTraits(.isHeader)
            }
            VStack(alignment: .leading, spacing: DashSpacing.xxxs) {
                content()
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .dashCard(padding: DashSpacing.menuCardInner, elevation: .menuCard)
            if let footer {
                Text(footer)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.textTertiary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, DashSpacing.xxs)
            }
        }
    }
}

/// A menu row: optional 30 pt icon, title (`subheadMedium`), help text (`footnote` tertiary) and a
/// trailing accessory (toggle, picker, value, button…). Minimum height 56.
public struct MenuRow<Accessory: View>: View {
    public let icon: DashIconSource?
    public let title: String
    public let help: String?
    public let isDestructive: Bool
    @ViewBuilder public let accessory: () -> Accessory
    @Environment(\.isEnabled) private var isEnabled

    public init(
        icon: DashIconSource? = nil, title: String, help: String? = nil, isDestructive: Bool = false,
        @ViewBuilder accessory: @escaping () -> Accessory
    ) {
        self.icon = icon
        self.title = title
        self.help = help
        self.isDestructive = isDestructive
        self.accessory = accessory
    }

    public var body: some View {
        HStack(spacing: DashSpacing.sm) {
            if let icon {
                DashIconImage(icon)
                    .scaledToFit()
                    .frame(width: DashLayout.rowIconSize, height: DashLayout.rowIconSize)
                    .opacity(isEnabled ? 1 : DashOpacity.disabled)
                    .accessibilityHidden(true)
            }
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .dashFont(.subheadMedium)
                    .foregroundStyle(
                        isDestructive ? Color.role.danger : (isEnabled ? Color.role.textPrimary : Color.role.textSecondary))
                    .fixedSize(horizontal: false, vertical: true)
                if let help {
                    Text(help)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.role.textTertiary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(.leading, icon == nil ? DashSpacing.xs : 0)
            Spacer(minLength: DashSpacing.m)
            accessory()
        }
        .padding(DashSpacing.sm)
        .frame(minHeight: DashLayout.rowMinHeight)
    }
}

public extension MenuRow where Accessory == EmptyView {
    init(icon: DashIconSource? = nil, title: String, help: String? = nil, isDestructive: Bool = false) {
        self.init(icon: icon, title: title, help: help, isDestructive: isDestructive) { EmptyView() }
    }
}

/// A menu row that is a button: chevron on the right, hover tint.
public struct MenuActionRow: View {
    public let icon: DashIconSource?
    public let title: String
    public let help: String?
    public let value: String?
    public let isDestructive: Bool
    public let action: () -> Void

    public init(
        icon: DashIconSource? = nil, title: String, help: String? = nil, value: String? = nil,
        isDestructive: Bool = false, action: @escaping () -> Void
    ) {
        self.icon = icon
        self.title = title
        self.help = help
        self.value = value
        self.isDestructive = isDestructive
        self.action = action
    }

    public var body: some View {
        Button(action: action) {
            MenuRow(icon: icon, title: title, help: help, isDestructive: isDestructive) {
                HStack(spacing: DashSpacing.s) {
                    if let value {
                        Text(value)
                            .dashFont(.subhead)
                            .foregroundStyle(Color.role.textSecondary)
                    }
                    Image(systemName: "chevron.right")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(Color.role.textTertiary)
                        .accessibilityHidden(true)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .dashRowHighlight(radius: DashRadius.standard + 2)
    }
}

// MARK: Detail rows (C12)

/// A label and a selectable value: label on the left (160 pt column) or stacked above the value.
public struct DetailRow<Trailing: View>: View {
    public let label: String
    public let value: String
    public let isTechnical: Bool
    public let stacked: Bool
    public let valueHelp: String?
    @ViewBuilder public let trailing: () -> Trailing

    public init(
        _ label: String, value: String, isTechnical: Bool = false, stacked: Bool = false, valueHelp: String? = nil,
        @ViewBuilder trailing: @escaping () -> Trailing
    ) {
        self.label = label
        self.value = value
        self.isTechnical = isTechnical
        self.stacked = stacked
        self.valueHelp = valueHelp
        self.trailing = trailing
    }

    public var body: some View {
        Group {
            if stacked {
                VStack(alignment: .leading, spacing: DashSpacing.xxxs) {
                    labelText
                    HStack(alignment: .firstTextBaseline) {
                        valueText
                        Spacer(minLength: 0)
                        trailing()
                    }
                }
            } else {
                HStack(alignment: .firstTextBaseline, spacing: DashSpacing.m) {
                    labelText
                        .frame(width: DashLayout.detailLabelWidth, alignment: .leading)
                    valueText
                        .frame(maxWidth: .infinity, alignment: .leading)
                    trailing()
                }
            }
        }
        .padding(.vertical, DashSpacing.xs)
        .padding(.horizontal, DashSpacing.sm)
    }

    private var labelText: some View {
        Text(label)
            .dashFont(.footnote)
            .foregroundStyle(Color.role.textSecondary)
    }

    @ViewBuilder
    private var valueText: some View {
        let text = Text(value)
            .foregroundStyle(Color.role.textPrimary)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
            .help(valueHelp ?? "")
        if isTechnical {
            text.font(.system(size: DesignTokens.DashTextStyle.footnote.size, design: .monospaced))
        } else {
            text.dashFont(.subhead)
        }
    }
}

public extension DetailRow where Trailing == EmptyView {
    init(_ label: String, value: String, isTechnical: Bool = false, stacked: Bool = false, valueHelp: String? = nil) {
        self.init(label, value: value, isTechnical: isTechnical, stacked: stacked, valueHelp: valueHelp) {
            EmptyView()
        }
    }
}

/// A detail row with a tinted-gray copy button; copying writes `copyValue` (default: the value) to
/// the pasteboard and calls `onCopied` so the screen can show the "Copied" toast.
public struct CopyRow: View {
    public let label: String
    public let value: String
    public let copyValue: String?
    public let isTechnical: Bool
    public let stacked: Bool
    public let copyLabel: String
    public let onCopied: (() -> Void)?

    public init(
        _ label: String, value: String, copyValue: String? = nil, isTechnical: Bool = false, stacked: Bool = false,
        copyLabel: String, onCopied: (() -> Void)? = nil
    ) {
        self.label = label
        self.value = value
        self.copyValue = copyValue
        self.isTechnical = isTechnical
        self.stacked = stacked
        self.copyLabel = copyLabel
        self.onCopied = onCopied
    }

    public var body: some View {
        DetailRow(label, value: value, isTechnical: isTechnical, stacked: stacked) {
            CopyButton(value: copyValue ?? value, label: copyLabel, onCopied: onCopied)
        }
    }
}

/// A small tinted copy button with an accessibility label.
public struct CopyButton: View {
    public let value: String
    public let label: String
    public let onCopied: (() -> Void)?

    public init(value: String, label: String, onCopied: (() -> Void)? = nil) {
        self.value = value
        self.label = label
        self.onCopied = onCopied
    }

    public var body: some View {
        Button {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(value, forType: .string)
            onCopied?()
        } label: {
            Image(systemName: "doc.on.doc")
                .font(.system(size: 12, weight: .medium))
        }
        .buttonStyle(.dash(.tintedGray, .extraSmall))
        .help(label)
        .accessibilityLabel(Text(label))
    }
}

// MARK: Key/value grid (C32)

/// Two columns of technical facts: label `footnote` secondary, value `footnote` primary, selectable.
/// A missing value is "—" with the reason as tooltip.
public struct KeyValueGrid: View {
    public struct Entry: Identifiable, Hashable, Sendable {
        public var id: String { label }
        public let label: String
        public let value: String?
        public let reason: String?
        public let isTechnical: Bool

        public init(_ label: String, value: String?, reason: String? = nil, isTechnical: Bool = false) {
            self.label = label
            self.value = value
            self.reason = reason
            self.isTechnical = isTechnical
        }
    }

    public let entries: [Entry]

    public init(_ entries: [Entry]) {
        self.entries = entries
    }

    public var body: some View {
        Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: DashSpacing.l, verticalSpacing: DashSpacing.s) {
            ForEach(entries) { entry in
                GridRow {
                    Text(entry.label)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.role.textSecondary)
                    Group {
                        if let value = entry.value {
                            Text(value)
                                .font(.system(
                                    size: DesignTokens.DashTextStyle.footnote.size,
                                    design: entry.isTechnical ? .monospaced : .default))
                                .monospacedDigit()
                                .textSelection(.enabled)
                        } else {
                            Text("—").dashFont(.footnote).help(entry.reason ?? "")
                        }
                    }
                    .foregroundStyle(Color.role.textPrimary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
    }
}
#endif
