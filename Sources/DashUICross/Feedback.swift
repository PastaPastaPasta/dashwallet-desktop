// Feedback (UX-SPEC §3.1): the inline message (C22 `SystemMessage`, kept
// under its M1 name `Toast`), the transient toast pill (C21) and the window's
// status bar (C3, dash-qt's bottom status row).
import DesignTokens
import SwiftCrossUI

public enum ToastKind: Sendable, Hashable {
    case info, success, warning, error

    var icon: DashIconToken {
        switch self {
        case .info: .toastInfo
        case .success: .toastSuccess
        case .warning: .toastWarning
        case .error: .toastError
        }
    }

    var background: DashColor {
        switch self {
        case .info: CrossRole.accentTint
        case .success: CrossRole.successTint
        case .warning: CrossRole.warningTint
        case .error: CrossRole.dangerTint
        }
    }
}

/// An inline message (DashUIKit `SystemMessageView`, C22): status icon,
/// message and an optional action, on the kind's tint. The icon and the
/// text, not colour alone, carry the meaning.
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
        HStack(alignment: .center, spacing: points(DashSpacing.m)) {
            DashIcon(kind.icon, size: 18, width: 18)
            Text(message)
                .dashFont(.footnote)
                .dashForeground(CrossRole.textPrimary)
                .textSelectionEnabled()
            Spacer()
            if let actionTitle, let action {
                DashButton(actionTitle, style: .plainBlue, size: .small, action: action)
            }
        }
        .padding(.horizontal, points(DashSpacing.m))
        .padding(.vertical, points(DashSpacing.sm))
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: DashRadius.standard).fill(kind.background.color))
    }
}

/// The transient toast (DashUIKit `Toast`, C21): a dark pill with an icon,
/// the message and an optional action, shown bottom-centre above the status bar.
public struct ToastPill: View {
    let message: String
    let icon: DashIconToken
    let actionTitle: String?
    let action: (@MainActor @Sendable () -> Void)?

    public init(
        _ message: String, icon: DashIconToken = .toastCopied, actionTitle: String? = nil,
        action: (@MainActor @Sendable () -> Void)? = nil
    ) {
        self.message = message
        self.icon = icon
        self.actionTitle = actionTitle
        self.action = action
    }

    public var body: some View {
        HStack(spacing: points(DashSpacing.s)) {
            DashIcon(icon, size: 18, width: 18, tint: .color(CrossRole.white))
            Text(message)
                .dashFont(.footnoteMedium)
                .foregroundColor(CrossRole.white.color)
                .textSelectionEnabled()
            if let actionTitle, let action {
                DashButton(actionTitle, style: .tintedWhite, size: .extraSmall, action: action)
            }
        }
        .padding(.horizontal, points(DashSpacing.l))
        .padding(.vertical, points(DashSpacing.sm))
        .background {
            RoundedRectangle(cornerRadius: DashRadius.standard).fill(DashColor.toastBackground.light.crossColor)
        }
        .frame(maxWidth: 640)
    }
}

/// One item of the status bar ("tDASH", "HD", "Locked", ...). Text only on
/// Cross (no icon-only items, ADR 0002 A1); `tone` colours the text for a
/// state (locked green, unlocked red, mixing-only orange).
public struct StatusBarItem: Sendable, Hashable, Identifiable {
    public let id: String
    public let text: String
    public let help: String?
    public let tone: BadgeTone?

    public init(id: String, text: String, help: String? = nil, tone: BadgeTone? = nil) {
        self.id = id
        self.text = text
        self.help = help
        self.tone = tone
    }
}

/// The bottom status row (C3): `leading` (demo badge), sync text and a thin
/// progress bar on the left; items then `accessory` (dash-qt's unit selector,
/// the peers and sync buttons) on the right, in dash-qt's order.
public struct StatusBarView<Leading: View, Accessory: View>: View {
    let syncText: String
    let syncHelp: String?
    /// 0...1, or `nil` when unknown or done (no bar is drawn).
    let progress: Double?
    let items: [StatusBarItem]
    let leading: Leading
    let accessory: Accessory

    public init(
        syncText: String, syncHelp: String? = nil, progress: Double?, items: [StatusBarItem],
        @ViewBuilder leading: () -> Leading, @ViewBuilder accessory: () -> Accessory
    ) {
        self.syncText = syncText
        self.syncHelp = syncHelp
        self.progress = progress
        self.items = items
        self.leading = leading()
        self.accessory = accessory()
    }

    public var body: some View {
        VStack(spacing: 0) {
            Hairline()
            HStack(spacing: points(DashSpacing.m)) {
                leading
                syncLabel
                if let progress {
                    ProgressView(value: progress)
                        .frame(width: 120)
                }
                Spacer()
                ForEach(items) { item in
                    StatusBarText(item: item)
                }
                accessory
            }
            .padding(.horizontal, points(DashSpacing.l))
            .padding(.vertical, points(DashSpacing.xxs))
            .frame(minHeight: Double(CrossLayout.statusBarHeight))
        }
        .background(CrossRole.canvas.color)
    }

    @ViewBuilder
    private var syncLabel: some View {
        let text = Text(syncText)
            .dashFont(.caption1)
            .dashForeground(CrossRole.textSecondary)
            .lineLimit(1)
        if let syncHelp {
            text.help(syncHelp)
        } else {
            text
        }
    }
}

extension StatusBarView where Leading == EmptyView, Accessory == EmptyView {
    public init(syncText: String, progress: Double?, items: [StatusBarItem]) {
        self.syncText = syncText
        self.syncHelp = nil
        self.progress = progress
        self.items = items
        self.leading = EmptyView()
        self.accessory = EmptyView()
    }
}

public struct StatusBarText: View {
    let item: StatusBarItem

    public init(item: StatusBarItem) { self.item = item }

    public var body: some View {
        let text = Text(item.text)
            .dashFont(.caption1Medium)
            .dashForeground(item.tone?.foreground ?? CrossRole.textSecondary)
            .lineLimit(1)
            .fixedSize()
        if let help = item.help {
            text.help(help)
        } else {
            text
        }
    }
}
