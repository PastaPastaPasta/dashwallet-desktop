// DashUIKit `Toast` / `SystemMessageView` as an inline banner, and the
// desktop status bar (dash-qt's bottom status row).
import DesignTokens
import SwiftCrossUI

public enum ToastKind: Sendable, Hashable {
    case info, success, warning, error

    var foreground: DashColor {
        switch self {
        case .info: .blue
        case .success: .green
        case .warning: .orange
        case .error: .red
        }
    }

    var background: DashColor {
        switch self {
        case .info: .blueAlpha10
        case .success: .greenAlpha10
        case .warning: .orangeAlpha10
        case .error: .redAlpha10
        }
    }
}

/// An inline message with an optional action. Text, not colour alone,
/// carries the meaning.
public struct Toast: View {
    let message: String
    let kind: ToastKind
    let actionTitle: String?
    let action: (@MainActor @Sendable () -> Void)?

    public init(
        _ message: String, kind: ToastKind = .info, actionTitle: String? = nil,
        action: (@MainActor @Sendable () -> Void)? = nil
    ) {
        self.message = message
        self.kind = kind
        self.actionTitle = actionTitle
        self.action = action
    }

    public var body: some View {
        HStack(spacing: points(DashSpacing.m)) {
            Text(message)
                .dashFont(.footnote)
                .dashForeground(kind.foreground)
            Spacer()
            if let actionTitle, let action {
                DashButton(actionTitle, style: .plainBlue, size: .small, action: action)
            }
        }
        .padding(.horizontal, points(DashSpacing.m))
        .padding(.vertical, points(DashSpacing.s))
        .background(RoundedRectangle(cornerRadius: DashRadius.small).fill(kind.background.color))
    }
}

/// One item of the status bar ("Testnet", "8 peers", "Locked", ...).
public struct StatusBarItem: Sendable, Hashable, Identifiable {
    public let id: String
    public let text: String
    public let help: String?

    public init(id: String, text: String, help: String? = nil) {
        self.id = id
        self.text = text
        self.help = help
    }
}

/// The bottom status row: sync text and progress on the left, items on the right.
public struct StatusBarView: View {
    let syncText: String
    /// 0...1, or `nil` when unknown (no bar is drawn).
    let progress: Double?
    let items: [StatusBarItem]

    public init(syncText: String, progress: Double?, items: [StatusBarItem]) {
        self.syncText = syncText
        self.progress = progress
        self.items = items
    }

    public var body: some View {
        HStack(spacing: points(DashSpacing.m)) {
            Text(syncText)
                .dashFont(.caption1)
                .dashForeground(.secondaryText)
                .lineLimit(1)
            if let progress {
                ProgressView(value: progress)
                    .frame(width: 160)
            }
            Spacer()
            ForEach(items) { item in
                if let help = item.help {
                    Text(item.text)
                        .dashFont(.caption1Medium)
                        .dashForeground(.secondaryText)
                        .lineLimit(1)
                        .help(help)
                } else {
                    Text(item.text)
                        .dashFont(.caption1Medium)
                        .dashForeground(.secondaryText)
                        .lineLimit(1)
                }
            }
        }
        .padding(.horizontal, points(DashSpacing.m))
        .padding(.vertical, points(DashSpacing.xs))
        .background(DashColor.bottomNavBackground.color)
    }
}
