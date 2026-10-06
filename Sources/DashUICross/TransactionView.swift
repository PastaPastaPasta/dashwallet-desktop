// DashUIKit `TransactionView` (C9): one history row with the iOS transaction
// icon, a human title, the time with one status chip, and the amount
// (`AmountText`, never coloured by direction). The day-group card (C8) that
// holds the rows is `TransactionGroupCard`.
import DesignTokens
import SwiftCrossUI

/// The row's icon (UX-SPEC §2.7 `tx-*`).
public enum TransactionDirection: Sendable, Hashable {
    case incoming, outgoing, internalTransfer, mixing, mined, failed

    var icon: DashIconToken {
        switch self {
        case .incoming: .txReceived
        case .outgoing: .txSent
        case .internalTransfer: .txInternalTransfer
        case .mixing: .txMixing
        case .mined: .txMining
        case .failed: .txError
        }
    }
}

/// The chip in a row's subtitle ("Pending", "InstantSend", "Abandoned"…).
public struct TransactionChip: Sendable, Hashable {
    public let text: String
    public let tone: BadgeTone

    public init(_ text: String, tone: BadgeTone = .info) {
        self.text = text
        self.tone = tone
    }
}

public struct TransactionView: View {
    let direction: TransactionDirection
    let title: String
    let subtitle: String
    let amount: String
    let unit: AmountUnitDisplay
    let chip: TransactionChip?
    let status: String?
    let dimmed: Bool

    /// `amount` is the formatter's signed text; `status` is the orange
    /// trailing status (iOS "Locked"); `dimmed` shows the title in the
    /// secondary colour (conflicted, abandoned).
    public init(
        direction: TransactionDirection, title: String, subtitle: String, amount: String,
        unit: AmountUnitDisplay = .none, chip: TransactionChip? = nil, status: String? = nil, dimmed: Bool = false
    ) {
        self.direction = direction
        self.title = title
        self.subtitle = subtitle
        self.amount = amount
        self.unit = unit
        self.chip = chip
        self.status = status
        self.dimmed = dimmed
    }

    public var body: some View {
        HStack(spacing: points(DashSpacing.transactionRowSpacing)) {
            DashIcon(direction.icon, size: 30, width: 30)
            VStack(alignment: .leading, spacing: points(DashSpacing.xxs)) {
                Text(title)
                    .dashFont(.footnoteMedium)
                    .dashForeground(dimmed ? CrossRole.textSecondary : CrossRole.textPrimary)
                    .lineLimit(1)
                HStack(spacing: points(DashSpacing.s)) {
                    if !subtitle.isEmpty {
                        Text(subtitle)
                            .dashFont(.footnote)
                            .dashForeground(CrossRole.textSecondary)
                            .lineLimit(1)
                    }
                    if let chip {
                        DashBadge(chip.text, tone: chip.tone)
                    }
                }
            }
            Spacer()
            VStack(alignment: .trailing, spacing: points(DashSpacing.xxxs)) {
                AmountText(amount, unit: unit, style: .footnoteMedium)
                if let status {
                    Text(status)
                        .dashFont(.caption1Medium)
                        .dashForeground(CrossRole.warning)
                        .lineLimit(1)
                }
            }
        }
        .padding(.horizontal, points(DashSpacing.l))
        .padding(.vertical, points(DashSpacing.m))
        .frame(minHeight: Double(CrossLayout.txRowMinHeight))
    }
}

/// One day of history (C8): a header with the day and the weekday, then the
/// rows, on a card with radius 10. `content` is usually a `List` of
/// `TransactionView`s so rows stay selectable and named for AT-SPI.
public struct TransactionGroupCard<Content: View>: View {
    let day: String
    let weekday: String?
    let content: Content

    public init(day: String, weekday: String?, @ViewBuilder content: () -> Content) {
        self.day = day
        self.weekday = weekday
        self.content = content()
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(day)
                    .dashFont(.footnoteMedium)
                    .dashForeground(CrossRole.textPrimary)
                Spacer()
                if let weekday {
                    Text(weekday)
                        .dashFont(.footnote)
                        .dashForeground(CrossRole.textTertiary)
                }
            }
            .padding(.horizontal, points(DashSpacing.l))
            .frame(height: 38)
            content
        }
        .padding(.bottom, points(DashSpacing.xs))
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardBackground(radius: CrossLayout.groupRadius)
    }
}
