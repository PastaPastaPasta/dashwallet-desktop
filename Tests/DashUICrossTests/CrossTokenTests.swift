import DesignTokens
import Foundation
import ImageFormats
import Testing

@testable import DashUICross

@Suite("Cross colour roles and text rules (UX-SPEC §2, §5)")
@MainActor
struct CrossTokenTests {
    @Test("canvas and card are the iOS surfaces, never pure black")
    func test_IOS_019_surfaces() {
        #expect(CrossRole.canvas.light.hex == "#F7F7F7")
        #expect(CrossRole.canvas.dark.hex == "#141519")
        #expect(CrossRole.card.light.hex == "#FFFFFF")
        #expect(CrossRole.card.dark.hex == "#1E1F24")
        #expect(CrossRole.accent.light.hex == "#008DE4")
        #expect(CrossRole.accent.dark.hex == "#008DE4")
    }

    @Test("no role is purple or violet")
    func test_QT_012_noPurpleRoles() {
        let roles: [DashColor] = [
            CrossRole.canvas, CrossRole.card, CrossRole.cardRaised, CrossRole.hero, CrossRole.accent,
            CrossRole.accentTint, CrossRole.textLink, CrossRole.success, CrossRole.danger, CrossRole.warning,
            CrossRole.caution, CrossRole.overlay, CrossRole.switchOn,
        ]
        for role in roles {
            for color in [role.light, role.dark] where color.hsv.saturation > 0.2 {
                #expect(!(255..<330).contains(color.hsv.hue), "\(color) is purple/violet")
            }
        }
    }

    @Test(".compact trims trailing zeros to two decimals and never rounds")
    func test_QT_036_compactAmounts() {
        #expect(AmountTextRules.compact("0.20000000") == "0.20")
        #expect(AmountTextRules.compact("+1.25000000") == "+1.25")
        #expect(AmountTextRules.compact("-0.50000226") == "-0.50000226")
        #expect(AmountTextRules.compact("30.39698287") == "30.39698287")
        #expect(AmountTextRules.compact("12\u{2009}345.10000000") == "12\u{2009}345.10")
        #expect(AmountTextRules.compact("1.00000000 tDASH") == "1.00 tDASH")
        #expect(AmountTextRules.compact("[0.10000000]") == "[0.10]")
        #expect(AmountTextRules.compact("2500") == "2500")
        #expect(AmountTextRules.compact("0.50000") == "0.50")
    }

    @Test("addresses longer than 24 characters are cut in the middle, 12 + 12")
    func test_QT_088_middleTruncation() {
        let address = "yQkAZdnn922YucpQ9DZedU1uFY4abcdEF"
        #expect(AmountTextRules.middleTruncated(address) == "yQkAZdnn922Y…" + String(address.suffix(12)))
        #expect(String(address.suffix(12)) == "U1uFY4abcdEF")
        #expect(AmountTextRules.middleTruncated(address).count == 25)
        #expect(AmountTextRules.middleTruncated("short") == "short")
    }

    @Test("the GTK theme CSS names the Dash blue accent, built from the tokens")
    func test_QT_012_toolkitAccent() {
        let css = ToolkitTheme.css(for: .light)
        #expect(css.contains("@define-color accent_bg_color rgba(0,141,228,1.0)"))
        #expect(ToolkitTheme.css(for: .dark).contains("switch:checked"))
    }

    @Test("tinting keeps the alpha channel and replaces the colour")
    func test_IOS_019_iconTint() {
        let image = ImageFormats.Image<ImageFormats.RGBA>(width: 1, height: 2, bytes: [10, 20, 30, 0, 40, 50, 60, 128])
        let tinted = IconCache.tinted(image, with: RGBA(red: 1, green: 1, blue: 1))
        #expect(tinted.bytes == [255, 255, 255, 0, 255, 255, 255, 128])
        let flipped = IconCache.verticallyFlipped(image)
        #expect(flipped.bytes == [40, 50, 60, 128, 10, 20, 30, 0])
    }

    @Test("every exported icon the Cross components use is bundled and decodes")
    func test_IOS_019_bundledIcons() {
        let tokens: [DashIconToken] = [
            .dashCurrency, .txReceived, .txSent, .txInternalTransfer, .txMixing, .txMining, .txError, .receive, .send,
            .scanQR, .backup, .tabHome, .arrowDown, .votingList, .dashLogo, .toastInfo, .toastSuccess, .toastWarning,
            .toastError, .toastCopied, .security, .wallet, .addressBook,
        ]
        for token in tokens {
            #expect(IconCache.image(token, appearance: .light, tint: .original, flipped: false) != nil, "\(token)")
        }
    }

    @Test("Inter is bundled with its licence")
    func test_IOS_019_interBundled() throws {
        let directory = try #require(ToolkitTheme.fontDirectory)
        let files = try FileManager.default.contentsOfDirectory(atPath: directory.path)
        for weight in ["Regular", "Medium", "SemiBold", "Bold"] {
            #expect(files.contains("Inter-\(weight).ttf"))
        }
        #expect(files.contains("OFL.txt"))
    }
}
