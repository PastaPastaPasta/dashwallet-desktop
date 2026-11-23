import Foundation

/// Corner radii in points, taken from DashUIKit components and dashwallet-ios. DashUIKit draws them with
/// continuous ("squircle") corners; toolkits without that style use plain circular corners.
public enum DashRadius {
    /// dashwallet-ios `Style.swift` `radius`: the legacy UIKit card and button corner.
    public static let standard: Double = 12
    /// DashUIKit `MenuViewModifier` menu cards, `Toast` and `SystemMessageView`.
    public static let card: Double = 20
    /// DashUIKit `AddressFieldView`.
    public static let textField: Double = 16
    /// DashUIKit `SearchBar` field.
    public static let searchField: Double = 14
    /// DashUIKit `TransactionView` icon tile.
    public static let transactionIcon: Double = 12
    /// DashUIKit `ConverterArrowBadge`.
    public static let badge: Double = 10
    /// DashUIKit `DualSwapAmountView` switcher and `TransactionView` secondary badge.
    public static let switcher: Double = 7
    /// DashUIKit `CoinSelector` badge and `DashPickerView`.
    public static let small: Double = 6
    /// DashUIKit `BottomSheet` grabber.
    public static let grabber: Double = 5
    /// Fully rounded ends (capsules, switch thumbs). Toolkits clamp it to half the shorter side.
    public static let capsule: Double = 1000
}
