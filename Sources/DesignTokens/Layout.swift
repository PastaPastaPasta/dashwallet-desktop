import Foundation

/// Layout constants of the desktop windows (UX-SPEC §2.3), in points.
public enum DashLayout {
    /// Horizontal page inset inside the detail column.
    public static let pagePaddingH: Double = 24
    /// Top inset under the toolbar.
    public static let pagePaddingTop: Double = 20
    /// Home, Receive and settings-style pages: a centred column of at most this width.
    public static let contentMaxWidth: Double = 760
    /// Send, Sign/Verify, forms and wizards.
    public static let formMaxWidth: Double = 640
    /// Space between cards and sections on a page.
    public static let sectionGap: Double = 24
    /// Padding inside cards. Menu cards use `DashSpacing.menuCardInner` plus the row padding instead.
    public static let cardPadding: Double = 16
    /// Minimum height of a menu row (dashwallet-ios `SecurityMenuScreen`).
    public static let rowMinHeight: Double = 56
    /// Minimum height of a transaction row: 30 pt icon, 12 pt padding above and below, two text lines.
    public static let txRowMinHeight: Double = 62
    /// Height of a day header inside a transaction group card (dashwallet-ios `HomeView`).
    public static let dayHeaderHeight: Double = 38
    public static let sidebarMinWidth: Double = 200
    public static let sidebarIdealWidth: Double = 220
    public static let sidebarMaxWidth: Double = 280
    /// The Home hero band including the half of the shortcut card that overlaps it.
    public static let heroHeight: Double = 236
    /// The hero band while balances are hidden (no breakdown strip).
    public static let heroHeightHidden: Double = 180
    /// Widest breakdown strip inside the hero.
    public static let heroStripMaxWidth: Double = 600
    /// Below this window width the breakdown strip stacks its cells.
    public static let heroStackBelowWidth: Double = 980
    /// Widest shortcut card.
    public static let shortcutCardMaxWidth: Double = 520
    /// Frame of one shortcut icon.
    public static let shortcutIconSize: Double = 46
    /// Transaction, menu and avatar icons.
    public static let rowIconSize: Double = 30
    /// Sheet widths: confirm sheets and alerts with fields, detail sheets, wizards.
    public static let sheetWidthSmall: Double = 480
    public static let sheetWidthMedium: Double = 640
    public static let sheetWidthLarge: Double = 760
    /// Height of a sheet header (back, title, close).
    public static let sheetHeaderHeight: Double = 64
    public static let windowMinWidth: Double = 920
    public static let windowMinHeight: Double = 600
    public static let statusBarHeight: Double = 28
    /// Width of the sync progress bar in the status bar.
    public static let statusProgressWidth: Double = 120
    /// Label column of a wide `DetailRow`.
    public static let detailLabelWidth: Double = 160
}

/// Animation durations and press scales (UX-SPEC §2.6). Toolkits without animation ignore them.
public enum DashMotion {
    /// dashwallet-ios `Style.swift` `kAnimationDuration`.
    public static let standard: Double = 0.35
    /// Hiding or showing the hero balance (`HomeBalanceView`).
    public static let balanceToggle: Double = 0.3
    /// Lock screen and overlays fading in or out.
    public static let overlay: Double = 0.2
    /// Background fade on hover.
    public static let hover: Double = 0.12
    /// Scale of a pressed shortcut item (`ShortcutCellButton`).
    public static let pressScale: Double = 0.93
    /// Scale and opacity of a pressed navigation button.
    public static let navPressScale: Double = 0.88
    public static let navPressOpacity: Double = 0.7
    /// Spring response and damping of the press animation.
    public static let pressSpringResponse: Double = 0.4
    public static let pressSpringDamping: Double = 0.5
    /// The "Syncing balance" caption pulses between these opacities.
    public static let syncingPulseLow: Double = 0.3
    public static let syncingPulseHigh: Double = 0.7
    public static let syncingPulse: Double = 0.8
    /// How long a toast stays: most toasts, and the "Copied" toast.
    public static let toastDuration: Double = 3
    public static let toastCopiedDuration: Double = 1.5
}

/// A shadow that both toolkits read: macOS draws it, toolkits without shadows draw a 1 px
/// `DashColor.Role.cardBorder` instead (UX-SPEC §2.5).
public struct DashElevation: Sendable, Hashable {
    public let radius: Double
    public let y: Double
    /// Shadow colour (clear in dark appearance).
    public let color: DashColor
    /// Draw a hairline border in dark appearance, where the shadow is clear.
    public let borderInDark: Bool

    public init(radius: Double, y: Double, color: DashColor, borderInDark: Bool) {
        self.radius = radius
        self.y = y
        self.color = color
        self.borderInDark = borderInDark
    }

    /// Cards on a page (DashUIKit `MenuViewModifier`).
    public static let card = DashElevation(radius: 10, y: 5, color: DashColor.Role.shadow, borderInDark: false)
    /// Settings menu cards (dashwallet-ios `SecurityMenuScreen`).
    public static let menuCard = DashElevation(radius: 20, y: 5, color: DashColor.Role.shadow, borderInDark: false)
    /// The shortcut card over the hero, toasts and popovers.
    public static let floating = DashElevation(radius: 10, y: 5, color: DashColor.Role.shadow, borderInDark: true)
}

extension DashRadius {
    /// The day-group transaction card (dashwallet-ios `RoundedShape(…radii: 10)`).
    public static let group: Double = 10
}
