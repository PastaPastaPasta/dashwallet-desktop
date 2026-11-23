import Foundation

/// Size metrics of DashUIKit `DashButton`, one value per `DashButtonSize` case
/// (Sources/DashUIKit/Button/DashButton.swift).
public struct DashButtonMetrics: Sendable, Hashable {
    public let horizontalPadding: Double
    public let verticalPadding: Double
    /// Spacing between the icon, label and loading indicator.
    public let gap: Double
    public let cornerRadius: Double
    /// Label font size; DashUIKit uses the regular-weight system font at this size.
    public let fontSize: Double

    public init(horizontalPadding: Double, verticalPadding: Double, gap: Double, cornerRadius: Double, fontSize: Double) {
        self.horizontalPadding = horizontalPadding
        self.verticalPadding = verticalPadding
        self.gap = gap
        self.cornerRadius = cornerRadius
        self.fontSize = fontSize
    }

    public static let large = DashButtonMetrics(horizontalPadding: 20, verticalPadding: 14, gap: 10, cornerRadius: 16, fontSize: 16)
    public static let medium = DashButtonMetrics(horizontalPadding: 16, verticalPadding: 10, gap: 8, cornerRadius: 14, fontSize: 14)
    public static let small = DashButtonMetrics(horizontalPadding: 12, verticalPadding: 6, gap: 6, cornerRadius: 11, fontSize: 13)
    public static let extraSmall = DashButtonMetrics(horizontalPadding: 8, verticalPadding: 4, gap: 6, cornerRadius: 9, fontSize: 12)
}
