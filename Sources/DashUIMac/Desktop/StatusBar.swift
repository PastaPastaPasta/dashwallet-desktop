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

/// A thin bar for the bottom of the main window.
///
/// `progress` is the sync fraction in 0...1; nil hides the progress bar (unknown progress is
/// not drawn as a guess).
public struct StatusBar: View {
    public let message: String?
    public let progress: Double?
    public let items: [StatusBarItem]

    public init(message: String? = nil, progress: Double? = nil, items: [StatusBarItem] = []) {
        self.message = message
        self.progress = progress
        self.items = items
    }

    public var body: some View {
        VStack(spacing: 0) {
            Rectangle()
                .fill(Color.dash.gray300Alpha20)
                .frame(height: 1)
            HStack(spacing: DashSpacing.sm) {
                if let message {
                    Text(message)
                        .font(DashTextStyle.caption1.font)
                        .foregroundStyle(Color.dash.secondaryText)
                        .lineLimit(1)
                }
                if let progress {
                    ProgressView(value: min(max(progress, 0), 1))
                        .progressViewStyle(.linear)
                        .tint(Color.dash.blue)
                        .frame(width: 160)
                        .accessibilityLabel(Text(message ?? ""))
                }
                Spacer(minLength: DashSpacing.s)
                ForEach(items) { item in
                    StatusBarItemView(item: item)
                }
            }
            .padding(.horizontal, DashSpacing.m)
            .frame(height: 26)
        }
        .background(Color.dash.secondaryBackground)
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
            }
            if let text = item.text {
                Text(text)
                    .font(DashTextStyle.caption1Medium.font)
                    .lineLimit(1)
            }
        }
        .foregroundStyle(item.tone == .neutral ? Color.dash.secondaryText : item.tone.foreground)
    }
}
#endif
