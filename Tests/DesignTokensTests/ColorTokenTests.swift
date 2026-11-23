import Foundation
import Testing
@testable import DesignTokens

/// Every colour token in both namespaces, labelled with its namespace.
private let allColorTokens: [(name: String, color: DashColor)] =
    DashColor.allTokens.map { ("DashColor.\($0.key)", $0.value) }
    + DashColor.App.allTokens.map { ("DashColor.App.\($0.key)", $0.value) }

private func expectColor(_ color: RGBA, hex: String, alpha: Double = 1, sourceLocation: SourceLocation = #_sourceLocation) {
    #expect(color.hex == hex, sourceLocation: sourceLocation)
    #expect(abs(color.alpha - alpha) < 1e-9, "alpha \(color.alpha) != \(alpha)", sourceLocation: sourceLocation)
}

@Suite("Colour tokens")
struct ColorTokenTests {
    @Test("Dash blue is #008DE4 in both catalogs")
    func dashBlue() {
        expectColor(DashColor.App.dashBlueColor.light, hex: "#008DE4")
        expectColor(DashColor.App.dashBlueColor.dark, hex: "#008DE4")
        expectColor(DashColor.blue.light, hex: "#008DE4")
        expectColor(DashColor.blue.dark, hex: "#008DE4")
        expectColor(DashColor.buttonFilledBlueBackground.light, hex: "#008DE4")
        expectColor(DashColor.blueAlpha10.light, hex: "#008DE4", alpha: 0.1)
        #expect(DashColor.App.dashBlueColor.light == RGBA(hex: 0x008DE4))
    }

    @Test("App text and background palette matches SharedAssets")
    func appPalette() {
        expectColor(DashColor.App.label.light, hex: "#0A0B0D")
        expectColor(DashColor.App.label.dark, hex: "#FFFFFF")
        expectColor(DashColor.App.secondaryTextColor.light, hex: "#525C66")
        expectColor(DashColor.App.secondaryTextColor.dark, hex: "#A4ABB3")
        expectColor(DashColor.App.tertiaryTextColor.light, hex: "#75808A")
        expectColor(DashColor.App.tertiaryTextColor.dark, hex: "#FFFFFF", alpha: 0.6)
        expectColor(DashColor.App.backgroundColor.light, hex: "#FFFFFF")
        expectColor(DashColor.App.backgroundColor.dark, hex: "#1E1F24")
        expectColor(DashColor.App.secondaryBackgroundColor.light, hex: "#F7F7F7")
        expectColor(DashColor.App.secondaryBackgroundColor.dark, hex: "#141519")
        expectColor(DashColor.App.tertiaryBackgroundColor.light, hex: "#FAFAFA")
        expectColor(DashColor.App.tertiaryBackgroundColor.dark, hex: "#1D2023")
        expectColor(DashColor.App.darkBlueColor.light, hex: "#011F5F")
        expectColor(DashColor.App.blueGradientStartColor.light, hex: "#00BBE4")
        expectColor(DashColor.App.modalDimmingColor.light, hex: "#04040F", alpha: 0.4)
    }

    @Test("App gray ramp")
    func appGrayRamp() {
        expectColor(DashColor.App.gray50.light, hex: "#F5F6F7")
        expectColor(DashColor.App.gray100.light, hex: "#EBEDEE")
        expectColor(DashColor.App.gray200.light, hex: "#CED2D5")
        expectColor(DashColor.App.gray300.light, hex: "#B0B6BC")
        expectColor(DashColor.App.gray400.light, hex: "#75808A")
        expectColor(DashColor.App.gray500.light, hex: "#525C66")
        expectColor(DashColor.App.black800.light, hex: "#1E1F24")
        expectColor(DashColor.App.black900.light, hex: "#141519")
    }

    @Test("Mixed component formats: integer components and an unset dark slot")
    func componentFormats() {
        // QuaternaryFillColor light uses 8-bit integer strings ("112"), dark uses floats ("0.455").
        let fill = DashColor.App.quaternaryFillColor
        #expect(abs(fill.light.red - 112.0 / 255) < 1e-12)
        #expect(abs(fill.light.blue - 114.0 / 255) < 1e-12)
        #expect(abs(fill.light.alpha - 0.07) < 1e-12)
        #expect(abs(fill.dark.red - 0.455) < 1e-12)
        #expect(abs(fill.dark.alpha - 0.18) < 1e-12)
        // LightBlueButtonColor has a dark entry with no colour; Xcode falls back to the light value.
        #expect(DashColor.App.lightBlueButtonColor.dark == DashColor.App.lightBlueButtonColor.light)
    }

    @Test("Duplicate colour-set names resolve the way actool does (first in sorted order)")
    func duplicateResolution() {
        // SharedAssets has Colors/Black/Black (#000000) and Colors/Gray/Black (#0A0B0D); actool ships the former.
        expectColor(DashColor.App.black.light, hex: "#000000")
        // Colors/Black/Black1000Alpha5 (with a dark variant) wins over Colors/Gray/Black/Black1000Alpha5.
        expectColor(DashColor.App.black1000Alpha5.light, hex: "#0A0B0D", alpha: 0.05)
        expectColor(DashColor.App.black1000Alpha5.dark, hex: "#FFFFFF", alpha: 0.05)
    }

    @Test("DashUIKit semantic tokens")
    func designSystemTokens() {
        expectColor(DashColor.primaryText.light, hex: "#0A0A0D")
        expectColor(DashColor.primaryText.dark, hex: "#FFFFFF", alpha: 0.9)
        expectColor(DashColor.secondaryText.light, hex: "#525C66")
        expectColor(DashColor.secondaryText.dark, hex: "#FFFFFF", alpha: 0.8)
        expectColor(DashColor.primaryBackground.light, hex: "#F5F5F7")
        expectColor(DashColor.primaryBackground.dark, hex: "#000000")
        expectColor(DashColor.secondaryBackground.light, hex: "#FFFFFF")
        expectColor(DashColor.secondaryBackground.dark, hex: "#1E1F24")
        expectColor(DashColor.green.light, hex: "#3DB58A")
        expectColor(DashColor.red.light, hex: "#EB3842")
        expectColor(DashColor.orange.light, hex: "#FA9169")
        expectColor(DashColor.buttonFilledRedBackground.light, hex: "#EB3842")
        expectColor(DashColor.successText.light, hex: "#3DB58A")
        expectColor(DashColor.errorText.light, hex: "#EB3842")
    }

    @Test("Namespaces are complete")
    func counts() {
        #expect(DashColor.allTokens.count == 158)
        #expect(DashColor.App.allTokens.count == 118)
        #expect(DashColor.assetNames["blue"] == "Blue")
        #expect(DashColor.App.assetNames["dashBlueColor"] == "DashBlueColor")
        #expect(Set(DashColor.allTokens.keys) == Set(DashColor.assetNames.keys))
        #expect(Set(DashColor.App.allTokens.keys) == Set(DashColor.App.assetNames.keys))
    }

    @Test("No generated colour is purple-ish (HSV hue 260-320 degrees with saturation > 0.25)")
    func noPurple() {
        let offenders = allColorTokens.compactMap { token -> String? in
            let hits = DashAppearance.allCases.compactMap { appearance -> String? in
                let c = token.color.resolved(for: appearance)
                let (hue, saturation, _) = c.hsv
                return (260...320).contains(hue) && saturation > 0.25 ? "\(appearance) \(c)" : nil
            }
            return hits.isEmpty ? nil : "\(token.name) (\(hits.joined(separator: ", ")))"
        }
        #expect(offenders.isEmpty, "purple-ish tokens: \(offenders.sorted().joined(separator: "; "))")
    }

    @Test("No token is named after a purple shade")
    func noPurpleNames() {
        let pattern = ["purple", "violet", "indigo", "lavender", "magenta", "lilac", "plum", "mauve"]
        let offenders = allColorTokens.map(\.name).filter { name in pattern.contains { name.lowercased().contains($0) } }
        #expect(offenders.isEmpty, "purple-named tokens: \(offenders.sorted().joined(separator: ", "))")
    }
}

@Suite("RGBA and DashColor helpers")
struct ColorTypeTests {
    @Test func hexRoundTrip() {
        let c = RGBA(hex: 0x0A0B0D, alpha: 0.5)
        #expect(c.hex == "#0A0B0D")
        #expect(c.hexWithAlpha == "#0A0B0D80")
        #expect(c.description == "#0A0B0D80")
        #expect(RGBA(hex: 0xFFFFFF).description == "#FFFFFF")
    }

    @Test func hsvOfKnownColours() {
        let blue = RGBA(hex: 0x008DE4).hsv
        #expect(abs(blue.hue - 202.89) < 0.01)
        #expect(abs(blue.saturation - 1) < 1e-9)
        let purple = RGBA(hex: 0x8000FF).hsv
        #expect(abs(purple.hue - 270.12) < 0.01)
        let red = RGBA(hex: 0xFF0040).hsv
        #expect(abs(red.hue - 344.94) < 0.01)
        #expect(RGBA(hex: 0x777777).hsv.saturation == 0)
    }

    @Test func resolvedAppearance() {
        let c = DashColor(light: RGBA(hex: 0xFFFFFF), dark: RGBA(hex: 0x000000))
        #expect(c.resolved(for: .light).hex == "#FFFFFF")
        #expect(c.resolved(for: .dark).hex == "#000000")
        let same = DashColor(RGBA(hex: 0x123456))
        #expect(same.light == same.dark)
    }
}
