import Foundation

/// Spacing scale in points.
///
/// The steps are the paddings and stack spacings DashUIKit uses (2, 4, 6, 8, 10, 12, 16, 20, 24, 40 pt; counted
/// across its component sources), plus named values that components take directly from DashUIKit or
/// dashwallet-ios.
public enum DashSpacing {
    public static let none: Double = 0
    public static let xxxs: Double = 2
    public static let xxs: Double = 4
    public static let xs: Double = 6
    public static let s: Double = 8
    public static let sm: Double = 10
    public static let m: Double = 12
    public static let l: Double = 16
    public static let xl: Double = 20
    public static let xxl: Double = 24
    public static let xxxl: Double = 40

    /// dashwallet-ios `Style.swift` `stackSpacing`.
    public static let stack: Double = 15
    /// Horizontal inset of DashUIKit `NavigationBar` and of the `EnterAmountView` content.
    public static let screenHorizontal: Double = 20
    /// Inner padding of DashUIKit `MenuViewModifier` (the rounded menu card).
    public static let menuCardInner: Double = 6
    /// DashUIKit `TransactionView` row: horizontal and vertical padding, icon-to-text spacing.
    public static let transactionRowHorizontal: Double = 10
    public static let transactionRowVertical: Double = 12
    public static let transactionRowSpacing: Double = 16
}
