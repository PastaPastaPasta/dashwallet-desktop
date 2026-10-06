// The Home balance hero (UX-SPEC §4.5, C4/C5): a full-width Dash-blue band with the network
// capsule, a status caption, the total in large bold white with the Dash glyph, an optional fiat
// line, the breakdown strip, and a card (the shortcut card) straddling its bottom edge. Every amount
// arrives formatted.
#if os(macOS)
import DesignTokens
import SwiftUI

/// One cell of the breakdown strip: dash-qt's Available / Pending / Immature.
public struct BalanceBreakdownCell: Identifiable, Hashable, Sendable {
    public let id: String
    public let title: String
    /// One-line explanation under the title.
    public let subtitle: String?
    /// The formatted amount, or nil when it is not known (drawn as "—" with `help`).
    public let amount: String?
    public let unit: AmountUnitDisplay
    /// Tooltip (dash-qt's), or the reason an unknown amount is unknown.
    public let help: String?

    public init(
        id: String, title: String, subtitle: String?, amount: String?, unit: AmountUnitDisplay, help: String?
    ) {
        self.id = id
        self.title = title
        self.subtitle = subtitle
        self.amount = amount
        self.unit = unit
        self.help = help
    }
}

/// The line between the capsule and the amount: "Syncing balance", "(out of sync)".
public struct BalanceHeroCaption: Hashable, Sendable {
    public let text: String
    public let help: String?
    /// Drawn in `caution` instead of white (dash-qt's out-of-sync warning).
    public let isWarning: Bool
    /// Pulses while syncing (not under Reduce Motion).
    public let pulses: Bool

    public init(text: String, help: String? = nil, isWarning: Bool = false, pulses: Bool = false) {
        self.text = text
        self.help = help
        self.isWarning = isWarning
        self.pulses = pulses
    }
}

public struct BalanceHero<Overlap: View>: View {
    public let network: String?
    public let caption: BalanceHeroCaption?
    /// The formatted total, the masked string in discreet mode, or nil when unknown.
    public let amount: String?
    public let unit: AmountUnitDisplay
    public let fiat: String?
    public let isHidden: Bool
    /// Text under an unknown amount ("Balance unavailable").
    public let unavailableText: String
    /// Hint under the amount ("Click to hide balance").
    public let hint: String?
    public let breakdown: [BalanceBreakdownCell]
    /// Click on the amount toggles discreet mode (iOS tap-to-hide).
    public let onToggleHidden: (() -> Void)?
    public let toggleLabel: String
    /// Height of the part of `overlap` that hangs below the band.
    public let overlapHeight: Double
    @ViewBuilder public let overlap: () -> Overlap

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var pulse = false

    public init(
        network: String?, caption: BalanceHeroCaption?, amount: String?, unit: AmountUnitDisplay,
        fiat: String? = nil, isHidden: Bool, unavailableText: String, hint: String? = nil,
        breakdown: [BalanceBreakdownCell], onToggleHidden: (() -> Void)?, toggleLabel: String,
        overlapHeight: Double = 46, @ViewBuilder overlap: @escaping () -> Overlap
    ) {
        self.network = network
        self.caption = caption
        self.amount = amount
        self.unit = unit
        self.fiat = fiat
        self.isHidden = isHidden
        self.unavailableText = unavailableText
        self.hint = hint
        self.breakdown = breakdown
        self.onToggleHidden = onToggleHidden
        self.toggleLabel = toggleLabel
        self.overlapHeight = overlapHeight
        self.overlap = overlap
    }

    public var body: some View {
        VStack(spacing: 0) {
            band
            overlap()
                .padding(.top, -overlapHeight)
                .padding(.horizontal, DashLayout.pagePaddingH)
        }
    }

    private var band: some View {
        VStack(spacing: DashSpacing.s) {
            // The capsule and caption rows keep their place so the layout never jumps.
            Group {
                if let network { NetworkCapsule(network) } else { Color.clear }
            }
            .frame(height: 20)
            captionView
                .frame(height: 16)
            amountRow
            if let fiat, !isHidden {
                Text(fiat)
                    .dashFont(.subhead)
                    .foregroundStyle(Color.role.textOnHeroSecondary)
            }
            if amount == nil {
                Text(unavailableText)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.textOnHeroSecondary)
            } else if let hint {
                Text(hint)
                    .dashFont(.caption1)
                    .foregroundStyle(Color.role.textOnHero.opacity(DashOpacity.heroHint))
            }
            if !breakdown.isEmpty {
                BalanceBreakdownStrip(cells: breakdown)
                    .padding(.top, DashSpacing.s)
            }
        }
        .padding(.top, DashSpacing.l)
        .padding(.bottom, overlapHeight + DashSpacing.xl)
        .padding(.horizontal, DashLayout.pagePaddingH)
        .frame(maxWidth: .infinity)
        .background(Color.role.hero)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("balanceHero")
    }

    @ViewBuilder
    private var captionView: some View {
        if let caption {
            Text(caption.text)
                .dashFont(.caption1)
                .foregroundStyle(caption.isWarning ? Color.role.caution : Color.role.textOnHero)
                .opacity(caption.pulses && !reduceMotion
                    ? (pulse ? DashMotion.syncingPulseHigh : DashMotion.syncingPulseLow) : 1)
                .help(caption.help ?? "")
                .onAppear {
                    guard caption.pulses, !reduceMotion else { return }
                    withAnimation(.easeInOut(duration: DashMotion.syncingPulse).repeatForever(autoreverses: true)) {
                        pulse = true
                    }
                }
        } else {
            Color.clear
        }
    }

    private var amountRow: some View {
        HStack(spacing: DashSpacing.m) {
            if isHidden, let onToggleHidden {
                Button(action: onToggleHidden) {
                    Image(systemName: "eye.slash")
                        .font(.system(size: 18, weight: .medium))
                        .foregroundStyle(Color.role.textOnHero)
                        .frame(width: 40, height: 40)
                        .background(Circle().fill(Color.black.opacity(DashOpacity.heroHiddenButton)))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text(toggleLabel))
            }
            Group {
                if let amount {
                    AmountText(
                        amount, unit: unit, size: DesignTokens.DashTextStyle.largeTitle.size, weight: .bold,
                        glyphFactor: 0.7)
                } else {
                    Text("—")
                        .font(.system(size: DesignTokens.DashTextStyle.largeTitle.size, weight: .bold))
                        .help(unavailableText)
                }
            }
            .foregroundStyle(Color.role.textOnHero)
            .contentShape(Rectangle())
            .onTapGesture { onToggleHidden?() }
            .accessibilityAddTraits(onToggleHidden == nil ? [] : .isButton)
            .accessibilityHint(onToggleHidden == nil ? Text("") : Text(toggleLabel))
            .accessibilityIdentifier("overview.total")
        }
        .frame(minHeight: 44)
    }
}

/// The white-12 % strip inside the hero (C5): cells side by side, stacked when the column is narrow.
public struct BalanceBreakdownStrip: View {
    public let cells: [BalanceBreakdownCell]

    public init(cells: [BalanceBreakdownCell]) {
        self.cells = cells
    }

    public var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 0) {
                ForEach(Array(cells.enumerated()), id: \.element.id) { index, cell in
                    if index > 0 {
                        Rectangle().fill(Color.role.heroDivider).frame(width: 1).padding(.vertical, DashSpacing.s)
                    }
                    BreakdownCellView(cell: cell)
                        .frame(minWidth: 230)
                }
            }
            .fixedSize(horizontal: false, vertical: true)
            VStack(spacing: 0) {
                ForEach(Array(cells.enumerated()), id: \.element.id) { index, cell in
                    if index > 0 {
                        Rectangle().fill(Color.role.heroDivider).frame(height: 1).padding(.horizontal, DashSpacing.m)
                    }
                    BreakdownCellView(cell: cell)
                }
            }
        }
        .frame(maxWidth: DashLayout.heroStripMaxWidth)
        .background(RoundedRectangle(cornerRadius: DashRadius.standard, style: .continuous).fill(Color.role.heroCard))
    }
}

private struct BreakdownCellView: View {
    let cell: BalanceBreakdownCell

    var body: some View {
        HStack(alignment: .center, spacing: DashSpacing.m) {
            VStack(alignment: .leading, spacing: 1) {
                Text(cell.title)
                    .dashFont(.footnoteMedium)
                    .foregroundStyle(Color.role.textOnHero)
                if let subtitle = cell.subtitle {
                    Text(subtitle)
                        .dashFont(.caption2)
                        .foregroundStyle(Color.role.textOnHeroSecondary)
                }
            }
            Spacer(minLength: DashSpacing.s)
            Group {
                if let amount = cell.amount {
                    AmountText(amount, unit: cell.unit, size: DesignTokens.DashTextStyle.footnote.size, weight: .medium)
                } else {
                    Text("—").dashFont(.footnoteMedium)
                }
            }
            .foregroundStyle(Color.role.textOnHero)
        }
        .padding(.horizontal, DashSpacing.l)
        .padding(.vertical, DashSpacing.m)
        .help(cell.help ?? "")
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("overview.balance.\(cell.id)")
    }
}
#endif
