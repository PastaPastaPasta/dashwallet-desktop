// DashUIKit `TransactionView`: one history row with a direction tile, title,
// subtitle and signed amount.
import DesignTokens
import SwiftCrossUI

public enum TransactionDirection: Sendable, Hashable {
    case incoming, outgoing, internalTransfer

    var glyph: String {
        switch self {
        case .incoming: "+"
        case .outgoing: "-"
        case .internalTransfer: "="
        }
    }

    var tint: DashColor {
        switch self {
        case .incoming: .green
        case .outgoing: .blue
        case .internalTransfer: .gray400
        }
    }

    var tileBackground: DashColor {
        switch self {
        case .incoming: .greenAlpha10
        case .outgoing: .blueAlpha10
        case .internalTransfer: .gray300Alpha10
        }
    }
}

public struct TransactionView: View {
    let direction: TransactionDirection
    let title: String
    let subtitle: String
    let amount: String
    let detail: String?

    public init(direction: TransactionDirection, title: String, subtitle: String, amount: String, detail: String? = nil) {
        self.direction = direction
        self.title = title
        self.subtitle = subtitle
        self.amount = amount
        self.detail = detail
    }

    public var body: some View {
        HStack(spacing: points(DashSpacing.transactionRowSpacing)) {
            Text(direction.glyph)
                .dashFont(.headline)
                .dashForeground(direction.tint)
                .frame(width: 30, height: 30)
                .background(
                    RoundedRectangle(cornerRadius: DashRadius.transactionIcon).fill(direction.tileBackground.color))
            VStack(alignment: .leading, spacing: points(DashSpacing.xxxs)) {
                Text(title)
                    .dashFont(.subheadMedium)
                    .dashForeground(.primaryText)
                    .lineLimit(1)
                Text(subtitle)
                    .dashFont(.footnote)
                    .dashForeground(.secondaryText)
                    .lineLimit(1)
            }
            Spacer()
            VStack(alignment: .trailing, spacing: points(DashSpacing.xxxs)) {
                Text(amount)
                    .dashFont(.subheadMedium)
                    .dashForeground(direction == .incoming ? .green : .primaryText)
                    .lineLimit(1)
                if let detail {
                    Text(detail)
                        .dashFont(.caption1)
                        .dashForeground(.tertiaryText)
                        .lineLimit(1)
                }
            }
        }
        .padding(.horizontal, points(DashSpacing.transactionRowHorizontal))
        .padding(.vertical, points(DashSpacing.xs))
    }
}
