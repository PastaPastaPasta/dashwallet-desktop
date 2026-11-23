// One component for every amount (UX-SPEC §5.1, C10): the formatted number, then the Dash glyph
// or the unit name. The view never formats numbers; it draws the string it is given.
#if os(macOS)
import DesignTokens
import SwiftUI

/// What follows the number.
public enum AmountUnitDisplay: Sendable, Hashable {
    /// The Dash currency glyph (DASH and tDASH); `spoken` is the unit name for assistive technology.
    case glyph(spoken: String)
    /// A unit with no glyph (mDASH, μDASH, duffs), written after a space.
    case name(String)
    /// The number alone.
    case none

    var spokenUnit: String? {
        switch self {
        case .glyph(let spoken): spoken
        case .name(let name): name
        case .none: nil
        }
    }
}

/// `[sign][number] [glyph | unit]`, in the current foreground style. Digits are tabular
/// (`monospacedDigit`) in a proportional face; amounts are never monospaced and never coloured by
/// direction.
public struct AmountText: View {
    public let text: String
    public let unit: AmountUnitDisplay
    public let size: Double
    public let weight: Font.Weight
    /// Glyph height relative to `size` (the hero uses 0.7, like iOS).
    public let glyphFactor: Double
    /// Tooltip, e.g. dash-qt's `.withUnit` string or why the value is unknown.
    public let help: String?

    public init(
        _ text: String, unit: AmountUnitDisplay, size: Double = DesignTokens.DashTextStyle.footnote.size,
        weight: Font.Weight = .medium, glyphFactor: Double = 1, help: String? = nil
    ) {
        self.text = text
        self.unit = unit
        self.size = size
        self.weight = weight
        self.glyphFactor = glyphFactor
        self.help = help
    }

    public var body: some View {
        HStack(alignment: .center, spacing: 2) {
            Text(text)
                .font(.system(size: size, weight: weight))
                .monospacedDigit()
                .lineLimit(1)
            switch unit {
            case .glyph:
                DashIconImage(.token(.dashCurrency), template: true)
                    .aspectRatio(contentMode: .fit)
                    .frame(width: size * glyphFactor, height: size * glyphFactor)
            case .name(let name):
                Text(" " + name)
                    .font(.system(size: size, weight: weight))
                    .lineLimit(1)
            case .none:
                EmptyView()
            }
        }
        .fixedSize()
        .help(help ?? "")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text([text, unit.spokenUnit].compactMap { $0 }.joined(separator: " ")))
    }
}

// MARK: Badges

/// The orange capsule that names a non-mainnet network on the hero and lock screen (C20): white
/// `caption2` bold with tracking 1.2 on orange at 90 %.
public struct NetworkCapsule: View {
    public let name: String

    public init(_ name: String) {
        self.name = name
    }

    public var body: some View {
        Text(name.uppercased())
            .font(.system(size: DesignTokens.DashTextStyle.caption2.size, weight: .bold))
            .tracking(1.2)
            .foregroundStyle(Color.role.textOnHero)
            .padding(.horizontal, DashSpacing.s)
            .padding(.vertical, 3)
            .background(Capsule().fill(Color.role.warning.opacity(DashOpacity.networkCapsule)))
            .accessibilityLabel(Text(name))
    }
}
#endif
