import Foundation

/// Font weight of a text style, with its CSS / OpenType numeric value.
public enum DashFontWeight: Int, Sendable, Hashable, CaseIterable {
    case regular = 400
    case medium = 500
    case semibold = 600
    case bold = 700
}

/// One step of the Dash type scale: size, weight, line height and tracking, all in points.
///
/// Transcribed from DashUIKit `DashTextStyle` (Sources/DashUIKit/Foundation/DashTextStyle.swift).
/// `Resources/Tokens/tokens.json` holds the same scale as parsed by `scripts/gen-tokens.swift`;
/// `DesignTokensTests` fails when the two disagree.
///
/// DashUIKit declares a public type with the same name. Code that imports both modules (DashUIMac)
/// must qualify this one as `DesignTokens.DashTextStyle`.
public struct DashTextStyle: Sendable, Hashable {
    /// Token name as used in DashUIKit (`largeTitle`, `footnoteMedium`, …).
    public let name: String
    public let size: Double
    public let weight: DashFontWeight
    /// Design line height. DashUIKit applies it through `.dashFont(_:)`; other toolkits set it directly.
    public let lineHeight: Double
    /// Letter spacing in points. DashUIKit sets none, so every style is 0 here: on Apple platforms the system
    /// font applies its own size-dependent tracking; other platforms render the font's default spacing.
    public let tracking: Double

    public init(name: String, size: Double, weight: DashFontWeight, lineHeight: Double, tracking: Double = 0) {
        self.name = name
        self.size = size
        self.weight = weight
        self.lineHeight = lineHeight
        self.tracking = tracking
    }
}

extension DashTextStyle {
    public static let largeTitle = DashTextStyle(name: "largeTitle", size: 34, weight: .bold, lineHeight: 41)
    public static let title1 = DashTextStyle(name: "title1", size: 28, weight: .bold, lineHeight: 34)
    public static let title2 = DashTextStyle(name: "title2", size: 22, weight: .bold, lineHeight: 28)
    public static let title3 = DashTextStyle(name: "title3", size: 20, weight: .bold, lineHeight: 25)
    public static let title3Medium = DashTextStyle(name: "title3Medium", size: 20, weight: .medium, lineHeight: 25)
    public static let headline = DashTextStyle(name: "headline", size: 17, weight: .bold, lineHeight: 22)
    public static let body = DashTextStyle(name: "body", size: 17, weight: .regular, lineHeight: 22)
    public static let callout = DashTextStyle(name: "callout", size: 16, weight: .regular, lineHeight: 21)
    public static let calloutMedium = DashTextStyle(name: "calloutMedium", size: 16, weight: .semibold, lineHeight: 21)
    public static let subhead = DashTextStyle(name: "subhead", size: 15, weight: .regular, lineHeight: 20)
    public static let subheadMedium = DashTextStyle(name: "subheadMedium", size: 15, weight: .medium, lineHeight: 20)
    public static let footnote = DashTextStyle(name: "footnote", size: 13, weight: .regular, lineHeight: 18)
    public static let footnoteMedium = DashTextStyle(name: "footnoteMedium", size: 13, weight: .medium, lineHeight: 18)
    public static let caption1 = DashTextStyle(name: "caption1", size: 12, weight: .regular, lineHeight: 16)
    public static let caption1Medium = DashTextStyle(name: "caption1Medium", size: 12, weight: .medium, lineHeight: 16)
    public static let caption2 = DashTextStyle(name: "caption2", size: 11, weight: .regular, lineHeight: 13)

    /// The full scale, largest first, in DashUIKit declaration order.
    public static let allStyles: [DashTextStyle] = [
        .largeTitle, .title1, .title2, .title3, .title3Medium, .headline, .body, .callout, .calloutMedium,
        .subhead, .subheadMedium, .footnote, .footnoteMedium, .caption1, .caption1Medium, .caption2,
    ]
}
