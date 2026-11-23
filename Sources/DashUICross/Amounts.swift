// `AmountText` (C10) and the text rules of UX-SPEC §5: the compact style
// (trailing zeros trimmed, never rounded), the fixed-string middle
// truncation of addresses, and how a unit is shown.
import DesignTokens
import SwiftCrossUI

/// How the unit follows a number (UX-SPEC §5.1).
public enum AmountUnitDisplay: Sendable, Hashable {
    /// The Dash currency glyph (DASH and tDASH).
    case glyph
    /// A space and the unit name (mDASH, μDASH, duffs).
    case name(String)
    /// No unit.
    case none
}

/// `[number][2 pt][glyph]` or `[number] unit`. The view never formats
/// numbers: callers pass the formatter's text. Never coloured by direction.
public struct AmountText: View {
    let text: String
    let unit: AmountUnitDisplay
    let style: DashTextStyle
    let weight: DashFontWeight?
    let color: DashColor
    let glyphFactor: Double
    let help: String?

    public init(
        _ text: String, unit: AmountUnitDisplay, style: DashTextStyle = .footnoteMedium, weight: DashFontWeight? = nil,
        color: DashColor = CrossRole.textPrimary, glyphFactor: Double = 0.7, help: String? = nil
    ) {
        self.text = text
        self.unit = unit
        self.style = style
        self.weight = weight
        self.color = color
        self.glyphFactor = glyphFactor
        self.help = help
    }

    public var body: some View {
        let font = Font.system(size: style.size, weight: (weight ?? style.weight).crossWeight)
        let row = HStack(spacing: points(DashSpacing.xxxs)) {
            switch unit {
            case .glyph:
                Text(text).font(font).foregroundColor(color.color).lineLimit(1).fixedSize()
                DashIcon(.dashCurrency, size: (style.size * glyphFactor).rounded(), tint: .color(color))
            case .name(let name):
                Text("\(text) \(name)").font(font).foregroundColor(color.color).lineLimit(1).fixedSize()
            case .none:
                Text(text).font(font).foregroundColor(color.color).lineLimit(1).fixedSize()
            }
        }
        if let help {
            row.help(help)
        } else {
            row
        }
    }
}

/// Pure text rules for amounts and addresses (UX-SPEC §5.2, §5.5).
public enum AmountTextRules {
    /// `.compact`: the exact value with trailing fractional zeros removed down
    /// to `minimumDecimals`; digits are never rounded. Text without a decimal
    /// point (duffs) is returned as it is. "0.20000000" → "0.20",
    /// "-0.50000226" → "-0.50000226", "30.39698287" → "30.39698287".
    public static func compact(_ text: String, minimumDecimals: Int = 2) -> String {
        guard let dot = text.lastIndex(of: ".") else { return text }
        var fraction = Array(text[text.index(after: dot)...])
        // A suffix after the digits (a unit, a closing bracket) is kept apart.
        var suffix: [Character] = []
        while let last = fraction.last, !last.isNumber, last != "\u{2009}" {
            suffix.insert(fraction.removeLast(), at: 0)
        }
        while fraction.count > minimumDecimals, let last = fraction.last, last == "0" || last == "\u{2009}" {
            fraction.removeLast()
        }
        // A thin-space group separator left at the end is dropped too.
        while let last = fraction.last, last == "\u{2009}" { fraction.removeLast() }
        return String(text[...dot]) + String(fraction) + String(suffix)
    }

    /// `prefix(12) + "…" + suffix(12)` when longer than 24 characters (iOS
    /// confirm-sheet rule, UX-SPEC §5.5).
    public static func middleTruncated(_ text: String, keep: Int = 12) -> String {
        guard text.count > 2 * keep else { return text }
        return String(text.prefix(keep)) + "…" + String(text.suffix(keep))
    }
}

extension DashFontWeight {
    var crossWeight: Font.Weight {
        switch self {
        case .regular: .regular
        case .medium: .medium
        case .semibold: .semibold
        case .bold: .bold
        }
    }
}
