// The UX-SPEC §2 tokens the Cross screens use: colour roles (§2.1), opacities,
// layout constants (§2.3) and radii (§2.4).
//
// Every role is an alias of a generated DesignTokens colour; no hex literal is
// written here. The names carry a `Cross` prefix because the shared
// `DashColor.Role` / `DashLayout` namespaces of UX-SPEC §2.8 belong to
// DesignTokens; this file mirrors their values for SwiftCrossUI and is replaced
// by aliases of them once they exist.
import DesignTokens
import SwiftCrossUI

/// Semantic colour roles (UX-SPEC §2.1).
public enum CrossRole {
    /// Window and page background (iOS `dw_secondaryBackground`): #F7F7F7 / #141519.
    public static let canvas = DashColor.App.secondaryBackgroundColor
    /// Cards, menu cards and dialogs: #FFFFFF / #1E1F24.
    public static let card = DashColor.secondaryBackground
    /// A card inside a card, table headers.
    public static let cardRaised = DashColor.App.tertiaryBackgroundColor
    /// The sidebar column on Linux/Windows.
    public static let sidebar = DashColor.App.secondaryBackgroundColor
    /// Balance hero band and lock screen.
    public static let hero = DashColor.App.dashNavigationBarBlueColor
    /// Primary buttons, selection, links, focus, switches on.
    public static let accent = DashColor.blue
    /// Hovered or selected rows, info badges, detail chips.
    public static let accentTint = DashColor.blueAlpha10
    /// A selected row under the pointer.
    public static let accentTintStrong = DashColor.blueAlpha20
    public static let textPrimary = DashColor.primaryText
    public static let textSecondary = DashColor.secondaryText
    public static let textTertiary = DashColor.tertiaryText
    public static let textOnHero = DashColor.whiteText
    public static let textLink = DashColor.blueText
    public static let separator = DashColor.App.separatorLineColor
    public static let fieldFill = DashColor.textFieldCryptoAddressBackground
    public static let fieldStroke = DashColor.buttonStrokeGrayStroke
    public static let success = DashColor.green
    public static let successTint = DashColor.greenAlpha10
    public static let danger = DashColor.red
    public static let dangerTint = DashColor.redAlpha10
    public static let warning = DashColor.orange
    public static let warningTint = DashColor.orangeAlpha10
    public static let caution = DashColor.yellow
    public static let neutralTint = DashColor.gray300Alpha20
    /// Scrim behind the sync overlay.
    public static let overlay = DashColor.App.modalDimmingColor
    public static let switchOn = DashColor.blue
    public static let progressTrack = DashColor.App.progressBackgroundColor
    /// Level-1 elevation on Cross (no shadow API in SwiftCrossUI 0.10): a hairline
    /// in light appearance, none in dark, where cards are lighter than the canvas.
    public static let cardBorder = DashColor(light: DashColor.gray300Alpha30.light, dark: DashColor.secondaryBackground.dark)
    /// Level-2 elevation (shortcut card over the hero, toasts).
    public static let floatingBorder = DashColor(light: DashColor.gray300Alpha30.light, dark: DashColor.whiteAlpha10.dark)
    public static let white = DashColor.white
}

/// Opacities used with `white` on the hero (UX-SPEC §2.1 notes).
public enum CrossOpacity {
    public static let heroCard = 0.12
    public static let heroDivider = 0.18
    public static let heroSecondaryText = 0.7
    public static let heroHint = 0.5
    /// Disabled shortcut items (C6).
    public static let disabled = 0.4
}

/// Layout constants (UX-SPEC §2.3, §2.4).
public enum CrossLayout {
    public static let pagePaddingH = 24
    public static let pagePaddingTop = 20
    public static let contentMaxWidth = 760.0
    public static let formMaxWidth = 640.0
    public static let sectionGap = 24
    public static let cardPadding = 16
    public static let rowMinHeight = 56
    public static let txRowMinHeight = 62
    public static let sidebarWidth = 220
    public static let statusBarHeight = 28
    /// The transaction day-group card (iOS `RoundedShape(…radii: 10)`).
    public static let groupRadius = 10
    public static let chipRadius = 7
    public static let sidebarRowRadius = 8
    /// Hero band height above the shortcut card's vertical middle (C4, §2.3 `heroHeight`).
    public static let heroBandHeight = 236.0
    public static let heroBandHeightHidden = 180.0
    public static let shortcutCardHeight = 92.0
    public static let shortcutCardMaxWidth = 520.0
    public static let breakdownMaxWidth = 600.0
}

extension DashColor {
    /// The token at `opacity` × its own alpha, in both appearances.
    public func opacity(_ opacity: Double) -> DashColor {
        DashColor(
            light: RGBA(red: light.red, green: light.green, blue: light.blue, alpha: light.alpha * opacity),
            dark: RGBA(red: dark.red, green: dark.green, blue: dark.blue, alpha: dark.alpha * opacity))
    }
}
