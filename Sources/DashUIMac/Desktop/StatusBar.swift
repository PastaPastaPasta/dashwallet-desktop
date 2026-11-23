// dash-qt style window status bar: sync message and progress on the left, state items on the right.
#if os(macOS)
import DesignTokens
import SwiftUI

/// One indicator on the right side of the status bar (unit, HD, lock, proxy, connections, …).
public struct StatusBarItem: Identifiable {
    public let id: String
    public let icon: DashIconSource?
    public let text: String?
    /// Name read by assistive technology; required because many items are icon-only.
    public let accessibilityLabel: String
    /// Tooltip text.
    public let help: String?
    public let tone: DashTone
    /// When set, the item is a button.
    public let action: (() -> Void)?

    public init(
        id: String,
        icon: DashIconSource? = nil,
        text: String? = nil,
        accessibilityLabel: String,
        help: String? = nil,
        tone: DashTone = .neutral,
        action: (() -> Void)? = nil
    ) {
        self.id = id
        self.icon = icon
        self.text = text
        self.accessibilityLabel = accessibilityLabel
        self.help = help
        self.tone = tone
        self.action = action
    }
}

/// A thin bar for the bottom of the main window (UX-SPEC C3): an optional neutral badge ("Demo"),
/// the sync text and a thin accent progress bar on the left, the state items on the right.
///
/// `progress` is the sync fraction in 0...1; nil hides the progress bar (unknown progress is
/// not drawn as a guess).
public struct StatusBar: View {
    public let message: String?
    public let progress: Double?
    public let items: [StatusBarItem]
    public let badge: String?
    public let badgeHelp: String?
    /// Click on the message or progress (dash-qt opens the sync overlay).
    public let onMessage: (() -> Void)?

    public init(
        message: String? = nil, progress: Double? = nil, items: [StatusBarItem] = [], badge: String? = nil,
        badgeHelp: String? = nil, onMessage: (() -> Void)? = nil
    ) {
        self.message = message
        self.progress = progress
        self.items = items
        self.badge = badge
        self.badgeHelp = badgeHelp
        self.onMessage = onMessage
    }

    public var body: some View {
        VStack(spacing: 0) {
            Rectangle()
                .fill(Color.role.separator)
                .frame(height: 0.5)
            HStack(spacing: DashSpacing.sm) {
                if let badge {
                    Badge(badge, tone: .neutral)
                        .help(badgeHelp ?? badge)
                        .accessibilityIdentifier("statusBar.demo")
                }
                leading
                Spacer(minLength: DashSpacing.s)
                ForEach(items) { item in
                    StatusBarItemView(item: item)
                }
            }
            .padding(.horizontal, DashSpacing.m)
            .frame(height: DashLayout.statusBarHeight - 0.5)
        }
        .background(Color.role.card)
    }

    @ViewBuilder
    private var leading: some View {
        let content = HStack(spacing: DashSpacing.sm) {
            if let message {
                Text(message)
                    .font(DesignTokens.DashTextStyle.caption1.font)
                    .foregroundStyle(Color.role.textSecondary)
                    .lineLimit(1)
            }
            if let progress {
                DashProgressBar(value: min(max(progress, 0), 1), height: 4)
                    .frame(width: DashLayout.statusProgressWidth)
                    .accessibilityLabel(Text(message ?? ""))
            }
        }
        if let onMessage {
            Button(action: onMessage) { content }.buttonStyle(.plain)
        } else {
            content
        }
    }
}

private struct StatusBarItemView: View {
    let item: StatusBarItem

    var body: some View {
        Group {
            if let action = item.action {
                Button(action: action) { label }
                    .buttonStyle(.plain)
            } else {
                label
            }
        }
        .help(item.help ?? item.accessibilityLabel)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(item.accessibilityLabel))
        .accessibilityAddTraits(item.action == nil ? [] : .isButton)
    }

    private var label: some View {
        HStack(spacing: DashSpacing.xxs) {
            if let icon = item.icon {
                DashIconImage(icon)
                    .scaledToFit()
                    .frame(width: 14, height: 14)
                    .font(.system(size: 12, weight: .medium))
            }
            if let text = item.text {
                Text(text)
                    .font(DesignTokens.DashTextStyle.caption1Medium.font)
                    .lineLimit(1)
            }
        }
        .foregroundStyle(item.tone == .neutral ? Color.role.textSecondary : item.tone.foreground)
    }
}
#endif
