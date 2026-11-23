// One entry of the main window sidebar (UX-SPEC C1): 16 pt symbol, `subhead` title, optional count
// badge. Selected: accent pill with white text.
#if os(macOS)
import DesignTokens
import SwiftUI

/// A sidebar entry. Inside `List(selection:)` leave `isSelected` false: the row is a `Label`, so
/// the system sidebar draws the selection in the window's tint (Dash blue) and turns the row white.
/// In a custom sidebar pass `isSelected` to draw the same pill.
public struct SidebarRow: View {
    public let title: String
    public let icon: DashIconSource?
    /// Trailing count or short status, e.g. "3".
    public let badgeText: String?
    public let isSelected: Bool
    /// Icon colour of an unselected row in a custom sidebar; a system sidebar tints icons itself.
    public let iconColor: Color?

    public init(
        title: String, icon: DashIconSource? = nil, badgeText: String? = nil, isSelected: Bool = false,
        iconColor: Color? = nil
    ) {
        self.title = title
        self.icon = icon
        self.badgeText = badgeText
        self.isSelected = isSelected
        self.iconColor = iconColor
    }

    public var body: some View {
        HStack(spacing: DashSpacing.s) {
            Label {
                Text(title)
                    .font(DesignTokens.DashTextStyle.subhead.font)
                    .lineLimit(1)
            } icon: {
                if let icon {
                    let image = DashIconImage(icon)
                        .scaledToFit()
                        .frame(width: 16, height: 16)
                        .accessibilityHidden(true)
                    if isSelected {
                        image.foregroundStyle(Color.role.textOnHero)
                    } else if let iconColor {
                        image.foregroundStyle(iconColor)
                    } else {
                        image
                    }
                }
            }
            // In a system sidebar the list colours the row (white when selected); a custom
            // sidebar draws the selected colours itself.
            .foregroundStyle(isSelected ? AnyShapeStyle(Color.role.textOnHero) : AnyShapeStyle(.primary))
            Spacer(minLength: DashSpacing.xxs)
            if let badgeText {
                Text(badgeText)
                    .font(DesignTokens.DashTextStyle.caption1Medium.font)
                    .foregroundStyle(isSelected ? Color.role.textOnHero : Color.role.textLink)
                    .padding(.horizontal, DashSpacing.xs)
                    .padding(.vertical, DashSpacing.xxxs)
                    .background(Capsule().fill(isSelected ? Color.white.opacity(0.2) : Color.role.accentTint))
            }
        }
        .padding(.horizontal, isSelected ? DashSpacing.s : 0)
        .padding(.vertical, isSelected ? DashSpacing.xs : 0)
        .background(
            RoundedRectangle(cornerRadius: DashRadius.small + 2, style: .continuous)
                .fill(isSelected ? Color.role.accent : .clear)
        )
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }
}
#endif
