import Foundation
import Testing
@testable import DesignTokens

/// The UX-SPEC §2.1 roles and §2.3–2.6 layout constants.
@Suite("Colour roles and layout tokens")
struct RoleTokenTests {
    private func expect(_ color: RGBA, _ hex: String, alpha: Double = 1, sourceLocation: SourceLocation = #_sourceLocation) {
        #expect(color.hex == hex, sourceLocation: sourceLocation)
        #expect(abs(color.alpha - alpha) < 1e-9, "alpha \(color.alpha) != \(alpha)", sourceLocation: sourceLocation)
    }

    @Test("Surfaces are the iOS canvas and card, never DashUIKit's black page")
    func surfaces() {
        expect(DashColor.Role.canvas.light, "#F7F7F7")
        expect(DashColor.Role.canvas.dark, "#141519")
        expect(DashColor.Role.card.light, "#FFFFFF")
        expect(DashColor.Role.card.dark, "#1E1F24")
        expect(DashColor.Role.cardRaised.dark, "#1D2023")
        #expect(DashColor.Role.canvas != DashColor.primaryBackground)
        for (name, color) in DashColor.Role.all where color.dark.alpha == 1 {
            #expect(color.dark.hex != "#000000", "\(name) is pure black in dark")
        }
    }

    @Test("Accent and hero are Dash blue")
    func accent() {
        expect(DashColor.Role.accent.light, "#008DE4")
        expect(DashColor.Role.accent.dark, "#008DE4")
        expect(DashColor.Role.hero.dark, "#008DE4")
        expect(DashColor.Role.switchOn.dark, "#008DE4")
        expect(DashColor.Role.accentTint.light, "#008DE4", alpha: 0.1)
    }

    @Test("Dark overrides replace DashUIKit's broken dark values")
    func darkOverrides() {
        // DashUIKit: switch track off is opaque white in dark; the role is white at 20 %.
        expect(DashColor.Role.switchOff.dark, "#FFFFFF", alpha: 0.2)
        expect(DashColor.Role.switchOff.light, "#B0B6BC")
        // DashUIKit's stroke-gray content is near-black in dark; the role follows the primary text.
        #expect(DashColor.Role.strokeGrayContent == DashColor.primaryText)
        #expect(DashColor.Role.plainContentDisabled == DashColor.tertiaryText)
        #expect(DashColor.Role.shadow.dark.alpha == 0)
    }

    @Test("Every role resolves and has a unique name")
    func everyRoleResolves() {
        let names = DashColor.Role.all.map(\.name)
        #expect(Set(names).count == names.count)
        #expect(names.count == 34)
        for (name, color) in DashColor.Role.all {
            for appearance in DashAppearance.allCases {
                let value = color.resolved(for: appearance)
                #expect((0...1).contains(value.alpha), "\(name) alpha out of range")
                // No purple in the palette (hue 255°–315° with visible saturation).
                let hsv = value.hsv
                #expect(!(hsv.saturation > 0.25 && (255..<315).contains(hsv.hue)), "\(name) is purple")
            }
        }
    }

    @Test("Opacity, layout, motion and elevation constants")
    func layoutConstants() {
        #expect(DashOpacity.heroCard == 0.12)
        #expect(DashOpacity.heroDivider == 0.18)
        #expect(DashLayout.contentMaxWidth == 760)
        #expect(DashLayout.formMaxWidth == 640)
        #expect(DashLayout.rowMinHeight == 56)
        #expect(DashLayout.txRowMinHeight == 62)
        #expect(DashLayout.statusBarHeight == 28)
        #expect(DashLayout.windowMinWidth == 920 && DashLayout.windowMinHeight == 600)
        #expect(DashRadius.group == 10)
        #expect(DashMotion.standard == 0.35)
        #expect(DashMotion.pressScale == 0.93)
        #expect(DashElevation.card.radius == 10 && DashElevation.card.y == 5)
        #expect(DashElevation.menuCard.radius == 20)
        #expect(DashElevation.floating.borderInDark)
    }

    /// WCAG 2.1 contrast of `fg` over `bg` (both composited onto opaque `base` first).
    private func contrast(_ fg: RGBA, _ bg: RGBA, base: RGBA = RGBA(hex: 0xFFFFFF)) -> Double {
        func over(_ c: RGBA, _ under: RGBA) -> RGBA {
            RGBA(red: c.red * c.alpha + under.red * (1 - c.alpha),
                 green: c.green * c.alpha + under.green * (1 - c.alpha),
                 blue: c.blue * c.alpha + under.blue * (1 - c.alpha))
        }
        func luminance(_ c: RGBA) -> Double {
            func channel(_ v: Double) -> Double { v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4) }
            return 0.2126 * channel(c.red) + 0.7152 * channel(c.green) + 0.0722 * channel(c.blue)
        }
        let back = over(bg, base)
        let (a, b) = (luminance(over(fg, back)), luminance(back))
        return (max(a, b) + 0.05) / (min(a, b) + 0.05)
    }

    @Test("The AA variants reach 4.5:1 on the pairs UX-SPEC §2.1a lists")
    func aaVariants() {
        let white = RGBA(hex: 0xFFFFFF)
        let aa = DashColor.RoleAA.self
        let role = DashColor.Role.self
        let heroStrip = RGBA(hex: 0xFFFFFF, alpha: DashOpacityAA.heroCard)
        let pairs: [(String, Double)] = [
            ("white on accent", contrast(white, aa.accent.light)),
            ("textLink on card (light)", contrast(aa.textLink.light, role.card.light)),
            ("textLink on accentTint (light)", contrast(aa.textLink.light, role.accentTint.light, base: role.card.light)),
            ("textLink on accentTint (dark)", contrast(aa.textLink.dark, role.accentTint.dark, base: role.card.dark)),
            ("hero strip subtitle",
             contrast(RGBA(hex: 0xFFFFFF, alpha: DashOpacityAA.heroSecondaryText), heroStrip, base: aa.hero.light)),
            ("textTertiary on canvas (light)", contrast(aa.textTertiary.light, role.canvas.light)),
            ("textTertiary on card (dark)", contrast(aa.textTertiary.dark, role.card.dark, base: role.card.dark)),
            ("warning on canvas (light)", contrast(aa.warning.light, role.canvas.light)),
            ("warning on card (dark)", contrast(aa.warning.dark, role.card.dark)),
            ("white on the network capsule", contrast(white, aa.networkCapsule.light)),
            ("danger on canvas (light)", contrast(aa.danger.light, role.canvas.light)),
            ("danger on dangerTint (light)", contrast(aa.danger.light, role.dangerTint.light, base: role.card.light)),
            ("danger on canvas (dark)", contrast(aa.danger.dark, role.canvas.dark)),
            ("danger on dangerTint (dark)", contrast(aa.danger.dark, role.dangerTint.dark, base: role.card.dark)),
        ]
        for (name, ratio) in pairs {
            #expect(ratio >= 4.5, "\(name): \(ratio)")
        }
    }
}
