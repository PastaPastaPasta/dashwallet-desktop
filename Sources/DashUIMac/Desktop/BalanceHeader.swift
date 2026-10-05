// Home balance card: total, fiat line, hide/show toggle and a breakdown (available, pending, …).
// All amounts arrive formatted; the view only lays them out.
#if os(macOS)
import DesignTokens
import SwiftUI

/// The wallet balance hero.
public struct BalanceHeader: View {
    /// One labelled amount of the breakdown row.
    public struct Line: Identifiable, Hashable, Sendable {
        public var id: String { label }
        public let label: String
        public let amount: String

        public init(label: String, amount: String) {
            self.label = label
            self.amount = amount
        }
    }

    public let title: String
    public let amount: String
    public let unit: String?
    public let fiat: String?
    /// Discreet mode: amounts are replaced by `hiddenPlaceholder`.
    public let isHidden: Bool
    public let hiddenPlaceholder: String
    /// Shows the show/hide button when set.
    public let onToggleHidden: (() -> Void)?
    public let breakdown: [Line]

    public init(
        title: String,
        amount: String,
        unit: String? = nil,
        fiat: String? = nil,
        isHidden: Bool = false,
        hiddenPlaceholder: String = "••••••",
        onToggleHidden: (() -> Void)? = nil,
        breakdown: [Line] = []
    ) {
        self.title = title
        self.amount = amount
        self.unit = unit
        self.fiat = fiat
        self.isHidden = isHidden
        self.hiddenPlaceholder = hiddenPlaceholder
        self.onToggleHidden = onToggleHidden
        self.breakdown = breakdown
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            HStack(spacing: DashSpacing.s) {
                Text(title)
                    .dashFont(.subheadMedium)
                    .foregroundStyle(Color.dash.secondaryText)
                if let onToggleHidden {
                    Button(action: onToggleHidden) {
                        DashIconImage(.token(isHidden ? .eyeClosed : .eyeOpen))
                            .scaledToFit()
                            .frame(width: 18, height: 18)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(Text(isHidden
                        ? NSLocalizedString("Show balance", bundle: .module, comment: "BalanceHeader")
                        : NSLocalizedString("Hide balance", bundle: .module, comment: "BalanceHeader")))
                }
            }

            VStack(alignment: .leading, spacing: DashSpacing.xxs) {
                HStack(alignment: .firstTextBaseline, spacing: DashSpacing.s) {
                    Text(isHidden ? hiddenPlaceholder : amount)
                        .font(DashTextStyle.title1.font)
                        .foregroundStyle(Color.dash.primaryText)
                        .lineLimit(1)
                        .minimumScaleFactor(0.5)
                    if let unit, !isHidden {
                        Text(unit)
                            .font(DashTextStyle.title3Medium.font)
                            .foregroundStyle(Color.dash.secondaryText)
                    }
                }
                if let fiat {
                    Text(isHidden ? hiddenPlaceholder : fiat)
                        .dashFont(.subhead)
                        .foregroundStyle(Color.dash.secondaryText)
                }
            }
            .accessibilityElement(children: .combine)

            if !breakdown.isEmpty {
                HStack(alignment: .top, spacing: DashSpacing.xxl) {
                    ForEach(breakdown) { line in
                        VStack(alignment: .leading, spacing: DashSpacing.xxxs) {
                            Text(line.label)
                                .dashFont(.caption1)
                                .foregroundStyle(Color.dash.secondaryText)
                            Text(isHidden ? hiddenPlaceholder : line.amount)
                                .dashFont(.footnoteMedium)
                                .foregroundStyle(Color.dash.primaryText)
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
            }
        }
        .padding(DashSpacing.xl)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: DashRadius.card, style: .continuous)
                .fill(Color.dash.secondaryBackground)
        )
        .shadow(color: Color.dash.shadow, radius: 10, x: 0, y: 5)
    }
}
#endif
