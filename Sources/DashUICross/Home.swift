// The Overview's iOS components (UX-SPEC §4.5): the blue balance hero (C4)
// with its breakdown strip (C5), the shortcut card that straddles the hero's
// lower edge (C6) and the "History" header (C7).
import DesignTokens
import SwiftCrossUI

/// One cell of the breakdown strip (dash-qt Available / Pending / Immature).
public struct BalanceCell: Sendable, Hashable, Identifiable {
    public var id: String { title }
    public let title: String
    public let subtitle: String
    public let amount: String
    public let help: String?

    public init(title: String, subtitle: String, amount: String, help: String? = nil) {
        self.title = title
        self.subtitle = subtitle
        self.amount = amount
        self.help = help
    }
}

/// The hero's content (C4): network capsule, status caption, the total in
/// `largeTitle` bold white with the Dash glyph, an optional hint, and the
/// breakdown strip. Drawn on `CrossRole.hero` by `HeroBand`.
public struct BalanceHero: View {
    let network: String?
    let caption: String?
    let captionIsWarning: Bool
    let captionHelp: String?
    let amount: String
    let unit: AmountUnitDisplay
    let amountHelp: String?
    let hint: String?
    let cells: [BalanceCell]
    let cellUnit: AmountUnitDisplay

    public init(
        network: String?, caption: String?, captionIsWarning: Bool = false, captionHelp: String? = nil, amount: String,
        unit: AmountUnitDisplay, amountHelp: String? = nil, hint: String? = nil, cells: [BalanceCell],
        cellUnit: AmountUnitDisplay
    ) {
        self.network = network
        self.caption = caption
        self.captionIsWarning = captionIsWarning
        self.captionHelp = captionHelp
        self.amount = amount
        self.unit = unit
        self.amountHelp = amountHelp
        self.hint = hint
        self.cells = cells
        self.cellUnit = cellUnit
    }

    public var body: some View {
        VStack(spacing: points(DashSpacing.s)) {
            // The capsule row keeps its height so the layout never jumps (§4.5).
            HStack {
                if let network { NetworkCapsule(network) }
            }
            .frame(height: 22)
            captionView
            AmountText(
                amount, unit: unit, style: .largeTitle, weight: .bold, color: CrossRole.textOnHero, glyphFactor: 0.7,
                help: amountHelp)
            if let hint {
                Text(hint)
                    .dashFont(.caption1)
                    .foregroundColor(CrossRole.white.opacity(CrossOpacity.heroHint).color)
            }
            if !cells.isEmpty {
                BalanceBreakdownStrip(cells: cells, unit: cellUnit)
                    .padding(.top, points(DashSpacing.xs))
            }
        }
        .padding(.horizontal, CrossLayout.pagePaddingH)
        .frame(maxWidth: .infinity)
    }

    @ViewBuilder
    private var captionView: some View {
        let color = captionIsWarning ? CrossRole.caution : CrossRole.white.opacity(CrossOpacity.heroSecondaryText)
        let text = Text(caption ?? " ")
            .dashFont(.footnoteMedium)
            .foregroundColor(color.color)
            .frame(height: 18)
        if let captionHelp {
            text.help(captionHelp)
        } else {
            text
        }
    }
}

/// The breakdown strip (C5): equal cells on white 12 %, separated by white
/// 18 % hairlines; title, one-line explanation and the amount.
public struct BalanceBreakdownStrip: View {
    let cells: [BalanceCell]
    let unit: AmountUnitDisplay

    public init(cells: [BalanceCell], unit: AmountUnitDisplay) {
        self.cells = cells
        self.unit = unit
    }

    public var body: some View {
        HStack(spacing: 0) {
            ForEach(Array(cells.enumerated()), id: \.element.id) { item in
                if item.offset > 0 {
                    Rectangle()
                        .fill(CrossRole.white.opacity(CrossOpacity.heroDivider).color)
                        .frame(width: 1, height: 32)
                }
                BreakdownCell(cell: item.element, unit: unit)
            }
        }
        .padding(.vertical, points(DashSpacing.m))
        .frame(maxWidth: CrossLayout.breakdownMaxWidth)
        .background(
            RoundedRectangle(cornerRadius: DashRadius.standard)
                .fill(CrossRole.white.opacity(CrossOpacity.heroCard).color))
    }
}

struct BreakdownCell: View {
    let cell: BalanceCell
    let unit: AmountUnitDisplay

    var body: some View {
        let content = HStack(spacing: points(DashSpacing.s)) {
            VStack(alignment: .leading, spacing: points(DashSpacing.xxxs)) {
                Text(cell.title)
                    .dashFont(.footnoteMedium)
                    .foregroundColor(CrossRole.textOnHero.color)
                Text(cell.subtitle)
                    .dashFont(.caption2)
                    .foregroundColor(CrossRole.white.opacity(CrossOpacity.heroSecondaryText).color)
            }
            Spacer()
            AmountText(cell.amount, unit: unit, style: .footnoteMedium, color: CrossRole.textOnHero)
        }
        .padding(.horizontal, points(DashSpacing.l))
        .frame(maxWidth: .infinity)
        if let help = cell.help {
            content.help(help)
        } else {
            content
        }
    }
}

/// The hero band: `hero` content on the blue band, then `overlap` (the
/// shortcut card) centred on the band's lower edge. `bandHeight` is the blue
/// height including the upper half of `overlap`.
public struct HeroBand<Hero: View, Overlap: View>: View {
    let bandHeight: Double
    let overlapHeight: Double
    let hero: Hero
    let overlap: Overlap

    public init(
        bandHeight: Double = CrossLayout.heroBandHeight, overlapHeight: Double = CrossLayout.shortcutCardHeight,
        @ViewBuilder hero: () -> Hero, @ViewBuilder overlap: () -> Overlap
    ) {
        self.bandHeight = bandHeight
        self.overlapHeight = overlapHeight
        self.hero = hero()
        self.overlap = overlap()
    }

    public var body: some View {
        VStack(spacing: 0) {
            hero
                .frame(maxWidth: .infinity)
                .frame(height: bandHeight - overlapHeight / 2)
            overlap
                .frame(height: overlapHeight)
        }
        .frame(maxWidth: .infinity)
        .background(alignment: .top) {
            Rectangle()
                .fill(CrossRole.hero.color)
                .frame(height: bandHeight)
                .frame(maxWidth: .infinity)
        }
    }
}

/// One shortcut (C6): a 40 pt icon over a `caption2` semibold title.
public struct ShortcutItem: Sendable, Hashable, Identifiable {
    public let id: Int
    public let title: String
    public let icon: DashIconToken
    public let isEnabled: Bool
    public let help: String?

    public init(id: Int, title: String, icon: DashIconToken, isEnabled: Bool = true, help: String? = nil) {
        self.id = id
        self.title = title
        self.icon = icon
        self.isEnabled = isEnabled
        self.help = help
    }
}

/// The shortcut card (C6): one white card, four equal items, hover tint.
public struct ShortcutCard: View {
    let items: [ShortcutItem]
    let action: @MainActor @Sendable (ShortcutItem) -> Void

    public init(items: [ShortcutItem], action: @escaping @MainActor @Sendable (ShortcutItem) -> Void) {
        self.items = items
        self.action = action
    }

    public var body: some View {
        let action = action
        HStack(spacing: points(DashSpacing.xxs)) {
            ForEach(items) { item in
                ShortcutButton(item: item) { action(item) }
            }
        }
        .padding(points(DashSpacing.xxs))
        .frame(maxWidth: CrossLayout.shortcutCardMaxWidth)
        .frame(height: CrossLayout.shortcutCardHeight)
        .cardBackground(border: CrossRole.floatingBorder)
    }
}

struct ShortcutButton: View {
    let item: ShortcutItem
    let action: @MainActor @Sendable () -> Void

    @State var hovering = false

    var body: some View {
        let enabled = item.isEnabled
        let button = Button(action: action) {
            VStack(spacing: points(DashSpacing.xs)) {
                DashIcon(item.icon, size: 36, width: 36, tint: enabled ? .original : .color(CrossRole.textTertiary))
                Text(item.title)
                    .font(.system(size: DashTextStyle.caption2.size, weight: .semibold))
                    .foregroundColor((enabled ? CrossRole.textPrimary : CrossRole.textTertiary).color)
                    .lineLimit(2)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, points(DashSpacing.s))
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .accessibilityLabel(item.title)
        .frame(maxWidth: .infinity)
        .background {
            if hovering && enabled {
                RoundedRectangle(cornerRadius: DashRadius.card - 4).fill(CrossRole.accentTint.color)
            }
        }
        .onHover { hovering = $0 }
        if let help = item.help {
            button.help(help)
        } else {
            button
        }
    }
}

/// "History" with the sync progress and the Filter action (C7).
public struct HistoryHeader: View {
    let title: String
    let syncText: String?
    let filterTitle: String?
    let onSync: (@MainActor @Sendable () -> Void)?
    let onFilter: (@MainActor @Sendable () -> Void)?

    public init(
        title: String, syncText: String?, filterTitle: String?, onSync: (@MainActor @Sendable () -> Void)? = nil,
        onFilter: (@MainActor @Sendable () -> Void)? = nil
    ) {
        self.title = title
        self.syncText = syncText
        self.filterTitle = filterTitle
        self.onSync = onSync
        self.onFilter = onFilter
    }

    public var body: some View {
        HStack(spacing: points(DashSpacing.s)) {
            Text(title)
                .dashFont(.subhead)
                .dashForeground(CrossRole.textSecondary)
            Spacer()
            if let syncText, let onSync {
                DashButton(syncText, style: .plainBlue, size: .small, action: onSync)
            }
            if let filterTitle, let onFilter {
                DashButton(filterTitle, style: .plainBlue, size: .small, icon: .filter, action: onFilter)
            }
        }
    }
}
