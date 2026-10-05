// One entry of the main window sidebar: icon, title, optional count badge.
#if os(macOS)
import DesignTokens
import SwiftUI

/// A sidebar entry. Inside `List(selection:)` leave `isSelected` false and let the list draw
/// the selection; in a custom sidebar pass `isSelected` to draw the Dash selection style.
public struct SidebarRow: View {
    public let title: String
    public let icon: DashIconSource?
    /// Trailing count or short status, e.g. "3".
    public let badgeText: String?
    public let isSelected: Bool

    public init(title: String, icon: DashIconSource? = nil, badgeText: String? = nil, isSelected: Bool = false) {
        self.title = title
        self.icon = icon
        self.badgeText = badgeText
        self.isSelected = isSelected
    }

    public var body: some View {
        HStack(spacing: DashSpacing.sm) {
            if let icon {
                DashIconImage(icon)
                    .scaledToFit()
                    .frame(width: 20, height: 20)
                    .accessibilityHidden(true)
            }
            Text(title)
                .font(DashTextStyle.subheadMedium.font)
                .foregroundStyle(isSelected ? Color.dash.blueText : Color.dash.primaryText)
                .lineLimit(1)
            Spacer(minLength: DashSpacing.xxs)
            if let badgeText {
                Text(badgeText)
                    .font(DashTextStyle.caption1Medium.font)
                    .foregroundStyle(Color.dash.blueText)
                    .padding(.horizontal, DashSpacing.xs)
                    .padding(.vertical, DashSpacing.xxxs)
                    .background(Capsule().fill(Color.dash.blueAlpha10))
            }
        }
        .padding(.horizontal, DashSpacing.s)
        .padding(.vertical, DashSpacing.xs)
        .background(
            RoundedRectangle(cornerRadius: DashRadius.small, style: .continuous)
                .fill(isSelected ? Color.dash.blueAlpha10 : .clear)
        )
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }
}
#endif
