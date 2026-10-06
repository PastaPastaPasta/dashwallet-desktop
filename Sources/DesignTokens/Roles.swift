import Foundation

/// Opacity constants. dashwallet-ios draws the hero's breakdown strip and its dividers as white at a
/// fixed opacity (`HomeBalanceView`); the values are tokens so screens never carry inline literals.
public enum DashOpacity {
    /// Fill of the breakdown strip inside the blue hero (`Color.dash.white.opacity(0.12)` on iOS).
    public static let heroCard: Double = 0.12
    /// Dividers between the strip's cells.
    public static let heroDivider: Double = 0.18
    /// Secondary text on the hero (strip subtitles, fiat line).
    public static let heroSecondaryText: Double = 0.7
    /// The "Click to hide balance" hint under the hero amount.
    public static let heroHint: Double = 0.5
    /// A disabled shortcut item or icon.
    public static let disabled: Double = 0.4
    /// The capsule that names a non-mainnet network on the hero and lock screen.
    public static let networkCapsule: Double = 0.9
    /// The round "hidden balance" button left of the masked hero amount (black at this opacity).
    public static let heroHiddenButton: Double = 0.2
}

extension DashColor {
    /// Semantic colour roles for the desktop surfaces (UX-SPEC §2.1). Every role is an alias of a
    /// generated token, chosen where the two catalogs disagree or where a DashUIKit dark value is wrong
    /// for a desktop window (pure-black page, white switch track, light-grey "tertiary background").
    /// Screens use these roles; components may still read the generated tokens directly.
    public enum Role {
        /// Window and page background: the iOS app's `dw_secondaryBackground` (#F7F7F7 / #141519).
        public static let canvas = DashColor.App.secondaryBackgroundColor
        /// Cards, menu cards, sheets and popover content (#FFFFFF / #1E1F24).
        public static let card = DashColor.secondaryBackground
        /// A card nested inside a card and table headers (#FAFAFA / #1D2023).
        public static let cardRaised = DashColor.App.tertiaryBackgroundColor
        /// The balance hero band and the lock screen.
        public static let hero = DashColor.App.dashNavigationBarBlueColor
        /// The breakdown strip inside the hero; draw it at `DashOpacity.heroCard`.
        public static let heroCard = DashColor.white
        /// Primary buttons, selection, links, focus ring, switches when on.
        public static let accent = DashColor.blue
        /// Selected-row tint, info badges, detail chips.
        public static let accentTint = DashColor.blueAlpha10
        /// A selected row under the pointer.
        public static let accentTintStrong = DashColor.blueAlpha20
        public static let textPrimary = DashColor.primaryText
        public static let textSecondary = DashColor.secondaryText
        public static let textTertiary = DashColor.tertiaryText
        /// All text on `hero`.
        public static let textOnHero = DashColor.whiteText
        /// Plain links and inline actions.
        public static let textLink = DashColor.blueText
        /// Hairlines between rows.
        public static let separator = DashColor.App.separatorLineColor
        /// Text-field and search-field fill.
        public static let fieldFill = DashColor.textFieldCryptoAddressBackground
        /// Focused or hovered field border.
        public static let fieldStroke = DashColor.buttonStrokeGrayStroke
        public static let success = DashColor.green
        public static let successTint = DashColor.greenAlpha10
        public static let danger = DashColor.red
        public static let dangerTint = DashColor.redAlpha10
        public static let warning = DashColor.orange
        public static let warningTint = DashColor.orangeAlpha10
        /// Stalled sync, stale rate.
        public static let caution = DashColor.yellow
        public static let cautionTint = DashColor.yellowAlpha10
        /// Neutral badge background.
        public static let neutralTint = DashColor.gray300Alpha20
        /// Scrim behind modal overlays.
        public static let overlay = DashColor.App.modalDimmingColor
        public static let toastFill = DashColor.toastBackground
        /// Toggle track when on. DashUIKit's `switchTrackFillOn` is white in dark appearance.
        public static let switchOn = DashColor.blue
        /// Toggle track when off: DashUIKit's grey in light, white at 20 % in dark (its dark value is
        /// opaque white).
        public static let switchOff = DashColor(light: DashColor.switchTrackFillOff.light, dark: DashColor.whiteAlpha20.dark)
        /// Track of a progress bar.
        public static let progressTrack = DashColor(
            light: DashColor.App.progressBackgroundColor.light, dark: DashColor.whiteAlpha10.dark)
        /// 1 px card border where shadows are not drawn (dark mode, and SwiftCrossUI).
        public static let cardBorder = DashColor(light: DashColor.gray300Alpha30.light, dark: DashColor.whiteAlpha10.dark)
        /// Content of `DashButton.strokeGray` on desktop: DashUIKit's token is near-black in dark.
        public static let strokeGrayContent = DashColor.primaryText
        /// Disabled content of the plain button styles: DashUIKit's tokens are near-black in dark.
        public static let plainContentDisabled = DashColor.tertiaryText
        /// Card shadow: blue-grey at 10 % in light, none in dark. DashUIKit defines this colour in code
        /// (`Color.dash.shadow`), not in an asset catalog, so it is the one role with its own value.
        public static let shadow = DashColor(
            light: RGBA(red: 0.72, green: 0.76, blue: 0.8, alpha: 0.1), dark: RGBA(red: 0, green: 0, blue: 0, alpha: 0))

        /// Every role with its name, for tests and the gallery.
        public static let all: [(name: String, color: DashColor)] = [
            ("canvas", canvas), ("card", card), ("cardRaised", cardRaised), ("hero", hero), ("heroCard", heroCard),
            ("accent", accent), ("accentTint", accentTint), ("accentTintStrong", accentTintStrong),
            ("textPrimary", textPrimary), ("textSecondary", textSecondary), ("textTertiary", textTertiary),
            ("textOnHero", textOnHero), ("textLink", textLink), ("separator", separator), ("fieldFill", fieldFill),
            ("fieldStroke", fieldStroke), ("success", success), ("successTint", successTint), ("danger", danger),
            ("dangerTint", dangerTint), ("warning", warning), ("warningTint", warningTint), ("caution", caution),
            ("cautionTint", cautionTint), ("neutralTint", neutralTint), ("overlay", overlay),
            ("toastFill", toastFill), ("switchOn", switchOn), ("switchOff", switchOff),
            ("progressTrack", progressTrack), ("cardBorder", cardBorder), ("strokeGrayContent", strokeGrayContent),
            ("plainContentDisabled", plainContentDisabled), ("shadow", shadow),
        ]
    }
}
