// Home building blocks after dashwallet-iOS (UX-SPEC C6–C9): the shortcut card with its items,
// the History header, the day-group transaction card and the desktop transaction row.
#if os(macOS)
import DesignTokens
import SwiftUI

// MARK: Shortcut card (C6)

/// One card (radius 20, floating elevation) holding equal-width shortcut items.
public struct ShortcutCard<Content: View>: View {
    @ViewBuilder public let content: () -> Content

    public init(@ViewBuilder content: @escaping () -> Content) {
        self.content = content
    }

    public var body: some View {
        HStack(spacing: 0) {
            content()
        }
        .padding(DashSpacing.xxs)
        .frame(maxWidth: DashLayout.shortcutCardMaxWidth)
        .dashCard(padding: nil, elevation: .floating)
    }
}

/// A shortcut: the 46 pt iOS shortcut icon over a two-line `caption2` semibold caption.
public struct ShortcutItem: View {
    public let title: String
    public let icon: DashIconSource
    public let action: () -> Void
    @Environment(\.isEnabled) private var isEnabled
    @State private var isHovering = false

    public init(title: String, icon: DashIconSource, action: @escaping () -> Void) {
        self.title = title
        self.icon = icon
        self.action = action
    }

    public var body: some View {
        Button(action: action) {
            VStack(spacing: DashSpacing.xs) {
                iconView
                    .frame(width: DashLayout.shortcutIconSize, height: DashLayout.shortcutIconSize)
                Text(title)
                    .font(.system(size: DesignTokens.DashTextStyle.caption2.size + 1, weight: .semibold))
                    .foregroundStyle(Color.role.textPrimary)
                    .multilineTextAlignment(.center)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, DashSpacing.m)
            .background(
                RoundedRectangle(cornerRadius: DashRadius.card - DashSpacing.xxs, style: .continuous)
                    .fill(isHovering && isEnabled ? Color.role.accentTint : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(ShortcutPressStyle())
        .opacity(isEnabled ? 1 : DashOpacity.disabled)
        .onHover { isHovering = $0 }
    }

    /// An exported shortcut icon (30 pt circle in a 46 pt frame), or an SF Symbol drawn white in a
    /// blue circle of the same size.
    @ViewBuilder
    private var iconView: some View {
        if case .system(let name) = icon {
            Image(systemName: name)
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(Color.role.textOnHero)
                .frame(width: DashLayout.rowIconSize, height: DashLayout.rowIconSize)
                .background(Circle().fill(Color.role.accent))
        } else {
            DashIconImage(icon)
                .scaledToFit()
                .frame(width: DashLayout.rowIconSize, height: DashLayout.rowIconSize)
        }
    }
}

/// Scales a pressed shortcut to 0.93 with a spring (iOS `ShortcutCellButton`), unless Reduce Motion.
private struct ShortcutPressStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed && !reduceMotion ? DashMotion.pressScale : 1)
            .animation(
                .spring(response: DashMotion.pressSpringResponse, dampingFraction: DashMotion.pressSpringDamping),
                value: configuration.isPressed)
    }
}

// MARK: History header (C7)

/// "History" on the left; the sync progress (a button that opens the sync overlay) and a
/// "Filter" link on the right.
public struct HistoryHeader: View {
    public let title: String
    public let syncText: String?
    public let onSync: (() -> Void)?
    public let filterTitle: String?
    public let onFilter: (() -> Void)?

    public init(
        title: String, syncText: String? = nil, onSync: (() -> Void)? = nil, filterTitle: String? = nil,
        onFilter: (() -> Void)? = nil
    ) {
        self.title = title
        self.syncText = syncText
        self.onSync = onSync
        self.filterTitle = filterTitle
        self.onFilter = onFilter
    }

    public var body: some View {
        HStack(spacing: DashSpacing.m) {
            Text(title)
                .dashFont(.subhead)
                .foregroundStyle(Color.role.textSecondary)
                .accessibilityAddTraits(.isHeader)
            Spacer()
            if let syncText {
                Button(syncText) { onSync?() }
                    .buttonStyle(.dash(.plainBlack, .extraSmall))
                    .disabled(onSync == nil)
            }
            if let filterTitle, let onFilter {
                Button(action: onFilter) {
                    HStack(spacing: DashSpacing.xxs) {
                        Text(filterTitle)
                        DashIconImage(.token(.filter), template: true)
                            .scaledToFit()
                            .frame(width: 14, height: 14)
                    }
                }
                .buttonStyle(.dash(.plainBlue, .extraSmall))
            }
        }
        .padding(.horizontal, DashSpacing.xxs)
    }
}

// MARK: Day-group card (C8)

/// One day of transactions on a radius-10 card: the day on the left and the weekday on the right of
/// a 38 pt header, then the rows.
public struct TransactionGroupCard<Rows: View>: View {
    public let day: String
    public let weekday: String?
    @ViewBuilder public let rows: () -> Rows

    public init(day: String, weekday: String?, @ViewBuilder rows: @escaping () -> Rows) {
        self.day = day
        self.weekday = weekday
        self.rows = rows
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(day)
                    .dashFont(.footnoteMedium)
                    .foregroundStyle(Color.role.textPrimary)
                    .accessibilityAddTraits(.isHeader)
                Spacer()
                if let weekday {
                    Text(weekday)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.role.textTertiary)
                }
            }
            .padding(.horizontal, DashSpacing.l)
            .frame(height: DashLayout.dayHeaderHeight)
            VStack(spacing: 0) {
                rows()
            }
            .padding(.horizontal, DashSpacing.xs)
            .padding(.bottom, DashSpacing.xs)
        }
        .dashCard(radius: DashRadius.group, padding: nil, elevation: .card)
    }
}

// MARK: Transaction row (C9)

/// The iOS transaction row with a formatted amount: 30 pt icon (+ badge), title, time and one status
/// chip, and on the right an orange status, an optional "internal" glyph, the amount and a fiat line.
/// Hover tints the row; selection keeps a stronger tint.
public struct DashTransactionRow: View {
    public struct Chip: Hashable, Sendable {
        public let text: String
        public let isProblem: Bool

        public init(text: String, isProblem: Bool = false) {
            self.text = text
            self.isProblem = isProblem
        }
    }

    public let icon: DashIconSource
    public let title: String
    public let subtitle: String?
    public let chip: Chip?
    public let topText: String?
    public let amount: String
    public let unit: AmountUnitDisplay
    public let amountHelp: String?
    public let fiat: String?
    public let trailingStatus: String?
    /// The amount carries no direction (a payment to yourself, a mixing day).
    public let isInternal: Bool
    /// Title in the secondary colour (abandoned or conflicted transactions).
    public let isDimmed: Bool
    public let isSelected: Bool
    public let action: (() -> Void)?

    public init(
        icon: DashIconSource, title: String, subtitle: String?, chip: Chip? = nil, topText: String? = nil,
        amount: String, unit: AmountUnitDisplay, amountHelp: String? = nil, fiat: String? = nil,
        trailingStatus: String? = nil, isInternal: Bool = false, isDimmed: Bool = false, isSelected: Bool = false,
        action: (() -> Void)? = nil
    ) {
        self.icon = icon
        self.title = title
        self.subtitle = subtitle
        self.chip = chip
        self.topText = topText
        self.amount = amount
        self.unit = unit
        self.amountHelp = amountHelp
        self.fiat = fiat
        self.trailingStatus = trailingStatus
        self.isInternal = isInternal
        self.isDimmed = isDimmed
        self.isSelected = isSelected
        self.action = action
    }

    public var body: some View {
        if let action {
            Button(action: action) { content }
                .buttonStyle(.plain)
        } else {
            content
        }
    }

    private var content: some View {
        HStack(spacing: DashSpacing.transactionRowSpacing) {
            DashIconImage(icon)
                .scaledToFit()
                .frame(width: DashLayout.rowIconSize, height: DashLayout.rowIconSize)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) {
                if let topText {
                    Text(topText)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.role.textSecondary)
                }
                Text(title)
                    .dashFont(.footnoteMedium)
                    .foregroundStyle(isDimmed ? Color.role.textSecondary : Color.role.textPrimary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                HStack(spacing: DashSpacing.s) {
                    if let subtitle {
                        Text(subtitle)
                            .dashFont(.footnote)
                            .foregroundStyle(Color.role.textSecondary)
                    }
                    if let chip {
                        Text(chip.text)
                            .dashFont(.caption1Medium)
                            .foregroundStyle(chip.isProblem ? Color.role.danger : Color.role.textLink)
                            .padding(.horizontal, DashSpacing.xxs)
                            .background(
                                RoundedRectangle(cornerRadius: DashRadius.switcher, style: .continuous)
                                    .fill(chip.isProblem ? Color.role.dangerTint : Color.role.accentTint))
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            VStack(alignment: .trailing, spacing: 1) {
                HStack(spacing: DashSpacing.xxs) {
                    if let trailingStatus {
                        Text(trailingStatus)
                            .dashFont(.caption1Medium)
                            .foregroundStyle(Color.role.warning)
                    }
                    if isInternal {
                        Image(systemName: "arrow.triangle.2.circlepath")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(Color.role.textSecondary)
                            .accessibilityHidden(true)
                    }
                    AmountText(amount, unit: unit, help: amountHelp)
                        .foregroundStyle(Color.role.textPrimary)
                }
                if let fiat {
                    Text(fiat)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.role.textSecondary)
                }
            }
        }
        .padding(.vertical, DashSpacing.transactionRowVertical)
        .padding(.horizontal, DashSpacing.transactionRowHorizontal)
        .frame(minHeight: DashLayout.txRowMinHeight)
        .contentShape(Rectangle())
        .dashRowHighlight(isSelected: isSelected, radius: DashRadius.standard)
        .accessibilityElement(children: .combine)
    }
}
#endif
