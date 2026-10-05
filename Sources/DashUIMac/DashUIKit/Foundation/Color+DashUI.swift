//
//  Created by Roman Chornyi
//  Copyright © 2026 Dash Core Group. All rights reserved.
//
//  Licensed under the MIT License (the "License");
//  you may not use this file except in compliance with the License.
//  You may obtain a copy of the License at
//
//  https://opensource.org/licenses/MIT
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
//
//
//  Vendored from DashUIKit e8d9243 (Foundation/Color+DashUI.swift); see Sources/DashUIMac/VENDORED.md.
//  Changes: every token resolves from DesignTokens (`DashColor`) instead of the Media.xcassets
//  colour sets; `purple` is dropped (DesignTokens excludes it); `shadow` uses a dynamic NSColor.
//

#if os(macOS)
import AppKit
import DesignTokens
import SwiftUI

public extension Color {
    static var dash: DashColors.Type { DashColors.self }
}

/// `Color.dash.<token>`: the DashUIKit colour tokens, each resolved from the matching
/// DesignTokens `DashColor` so it follows the view's light/dark appearance.
public enum DashColors {

    // MARK: Text

    public static var primaryText: Color { Color(dash: DashColor.primaryText) }
    public static var secondaryText: Color { Color(dash: DashColor.secondaryText) }
    public static var tertiaryText: Color { Color(dash: DashColor.tertiaryText) }
    public static var whiteText: Color { Color(dash: DashColor.whiteText) }
    public static var blueText: Color { Color(dash: DashColor.blueText) }
    public static var successText: Color { Color(dash: DashColor.successText) }
    public static var errorText: Color { Color(dash: DashColor.errorText) }

    // MARK: Background

    public static var primaryBackground: Color { Color(dash: DashColor.primaryBackground) }
    public static var secondaryBackground: Color { Color(dash: DashColor.secondaryBackground) }
    public static var tertiaryBackground: Color { Color(dash: DashColor.tertiaryBackground) }
    public static var blueBackground: Color { Color(dash: DashColor.blueBackground) }

    // MARK: Badge

    public static var badgeBackground1: Color { Color(dash: DashColor.badgeBackground1) }
    public static var badgeBackground2: Color { Color(dash: DashColor.badgeBackground2) }
    public static var badgeBackgroundCont1: Color { Color(dash: DashColor.badgeBackgroundCont1) }
    public static var badgeBackgroundCont2: Color { Color(dash: DashColor.badgeBackgroundCont2) }

    // MARK: Bottom Nav

    public static var bottomNavBackground: Color { Color(dash: DashColor.bottomNavBackground) }

    // MARK: Buttons

    public static var buttonFilledBlueBackground: Color { Color(dash: DashColor.buttonFilledBlueBackground) }
    public static var buttonFilledBlueBackgroundDisabled: Color { Color(dash: DashColor.buttonFilledBlueBackgroundDisabled) }
    public static var buttonFilledBlueContent: Color { Color(dash: DashColor.buttonFilledBlueContent) }
    public static var buttonFilledBlueContentDisabled: Color { Color(dash: DashColor.buttonFilledBlueContentDisabled) }

    public static var buttonFilledOrangeBackground: Color { Color(dash: DashColor.buttonFilledOrangeBackground) }
    public static var buttonFilledOrangeBackgroundDisabled: Color { Color(dash: DashColor.buttonFilledOrangeBackgroundDisabled) }
    public static var buttonFilledOrangeContent: Color { Color(dash: DashColor.buttonFilledOrangeContent) }
    public static var buttonFilledOrangeContentDisabled: Color { Color(dash: DashColor.buttonFilledOrangeContentDisabled) }

    public static var buttonFilledRedBackground: Color { Color(dash: DashColor.buttonFilledRedBackground) }
    public static var buttonFilledRedBackgroundDisabled: Color { Color(dash: DashColor.buttonFilledRedBackgroundDisabled) }
    public static var buttonFilledRedContent: Color { Color(dash: DashColor.buttonFilledRedContent) }
    public static var buttonFilledRedContentDisabled: Color { Color(dash: DashColor.buttonFilledRedContentDisabled) }

    public static var buttonFilledWhiteBackground: Color { Color(dash: DashColor.buttonFilledWhiteBackground) }
    public static var buttonFilledWhiteBackgroundDisabled: Color { Color(dash: DashColor.buttonFilledWhiteBackgroundDisabled) }
    public static var buttonFilledWhiteContent: Color { Color(dash: DashColor.buttonFilledWhiteContent) }
    public static var buttonFilledWhiteContentDisabled: Color { Color(dash: DashColor.buttonFilledWhiteContentDisabled) }

    public static var buttonPlainBlackContent: Color { Color(dash: DashColor.buttonPlainBlackContent) }
    public static var buttonPlainBlackContentDisabled: Color { Color(dash: DashColor.buttonPlainBlackContentDisabled) }

    public static var buttonPlainBlueContent: Color { Color(dash: DashColor.buttonPlainBlueContent) }
    public static var buttonPlainBlueContentDisabled: Color { Color(dash: DashColor.buttonPlainBlueContentDisabled) }

    public static var buttonPlainRedContent: Color { Color(dash: DashColor.buttonPlainRedContent) }
    public static var buttonPlainRedContentDisabled: Color { Color(dash: DashColor.buttonPlainRedContentDisabled) }

    public static var buttonPlainWhiteContent: Color { Color(dash: DashColor.buttonPlainWhiteContent) }
    public static var buttonPlainWhiteContentDisabled: Color { Color(dash: DashColor.buttonPlainWhiteContentDisabled) }

    public static var buttonStrokeGrayBackgroundDisabled: Color { Color(dash: DashColor.buttonStrokeGrayBackgroundDisabled) }
    public static var buttonStrokeGrayContent: Color { Color(dash: DashColor.buttonStrokeGrayContent) }
    public static var buttonStrokeGrayContentDisabled: Color { Color(dash: DashColor.buttonStrokeGrayContentDisabled) }
    public static var buttonStrokeGrayStroke: Color { Color(dash: DashColor.buttonStrokeGrayStroke) }

    public static var buttonTintedBlueBackground: Color { Color(dash: DashColor.buttonTintedBlueBackground) }
    public static var buttonTintedBlueBackgroundDisabled: Color { Color(dash: DashColor.buttonTintedBlueBackgroundDisabled) }
    public static var buttonTintedBlueContent: Color { Color(dash: DashColor.buttonTintedBlueContent) }
    public static var buttonTintedBlueContentDisabled: Color { Color(dash: DashColor.buttonTintedBlueContentDisabled) }

    public static var buttonTintedGrayBackground: Color { Color(dash: DashColor.buttonTintedGrayBackground) }
    public static var buttonTintedGrayBackgroundDisabled: Color { Color(dash: DashColor.buttonTintedGrayBackgroundDisabled) }
    public static var buttonTintedGrayContent: Color { Color(dash: DashColor.buttonTintedGrayContent) }
    public static var buttonTintedGrayContentDisabled: Color { Color(dash: DashColor.buttonTintedGrayContentDisabled) }

    public static var buttonTintedWhiteBackground: Color { Color(dash: DashColor.buttonTintedWhiteBackground) }
    public static var buttonTintedWhiteBackgroundDisabled: Color { Color(dash: DashColor.buttonTintedWhiteBackgroundDisabled) }
    public static var buttonTintedWhiteContent: Color { Color(dash: DashColor.buttonTintedWhiteContent) }
    public static var buttonTintedWhiteContentDisabled: Color { Color(dash: DashColor.buttonTintedWhiteContentDisabled) }

    // MARK: Grabber

    public static var grabberFill: Color { Color(dash: DashColor.grabberFill) }

    // MARK: List

    public static var listGiftCardNumberBackground: Color { Color(dash: DashColor.listGiftCardNumberBackground) }

    // MARK: Nav

    public static var navBackButton: Color { Color(dash: DashColor.navBackButton) }

    // MARK: Overlay

    public static var backgroundOverlay: Color { Color(dash: DashColor.backgroundOverlay) }

    // MARK: Search

    public static var searchBackground: Color { Color(dash: DashColor.searchBackground) }
    public static var searchClearIcon: Color { Color(dash: DashColor.searchClearIcon) }
    public static var searchIcon: Color { Color(dash: DashColor.searchIcon) }
    public static var searchPlaceholder: Color { Color(dash: DashColor.searchPlaceholder) }
    public static var searchTextEntered: Color { Color(dash: DashColor.searchTextEntered) }

    // MARK: Segment Control

    public static var segmentControlBackground: Color { Color(dash: DashColor.segmentControlBackground) }
    public static var segmentControlBackgroundGroup: Color { Color(dash: DashColor.segmentControlBackgroundGroup) }
    public static var segmentControlContNotSelected: Color { Color(dash: DashColor.segmentControlContNotSelected) }
    public static var segmentControlContSelected: Color { Color(dash: DashColor.segmentControlContSelected) }
    public static var segmentControlDivider: Color { Color(dash: DashColor.segmentControlDivider) }

    // MARK: Select

    public static var selectBackgroundSelected: Color { Color(dash: DashColor.selectBackgroundSelected) }
    public static var selectStrokeDefault: Color { Color(dash: DashColor.selectStrokeDefault) }
    public static var selectStrokeSelected: Color { Color(dash: DashColor.selectStrokeSelected) }

    // MARK: Shortcut Bar

    public static var shortcutBarBackground: Color { Color(dash: DashColor.shortcutBarBackground) }

    // MARK: Status Bar

    public static var statusBarElements: Color { Color(dash: DashColor.statusBarElements) }

    // MARK: Stepper

    public static var stepperBorder: Color { Color(dash: DashColor.stepperBorder) }
    public static var stepperBorderDisabled: Color { Color(dash: DashColor.stepperBorderDisabled) }
    public static var stepperElement: Color { Color(dash: DashColor.stepperElement) }
    public static var stepperElementDisabled: Color { Color(dash: DashColor.stepperElementDisabled) }

    // MARK: Switch

    public static var switchThumbFill: Color { Color(dash: DashColor.switchThumbFill) }
    public static var switchTrackFillOff: Color { Color(dash: DashColor.switchTrackFillOff) }
    public static var switchTrackFillOffDisabled: Color { Color(dash: DashColor.switchTrackFillOffDisabled) }
    public static var switchTrackFillOn: Color { Color(dash: DashColor.switchTrackFillOn) }

    // MARK: TextField

    public static var textFieldCryptoAddressBackground: Color { Color(dash: DashColor.textFieldCryptoAddressBackground) }
    public static var textFieldCryptoAddressIcon: Color { Color(dash: DashColor.textFieldCryptoAddressIcon) }

    // MARK: Toast

    public static var toastBackground: Color { Color(dash: DashColor.toastBackground) }
    public static var toastText: Color { Color(dash: DashColor.toastText) }

    // MARK: Toolbar

    public static var toolbarRoundBorder: Color { Color(dash: DashColor.toolbarRoundBorder) }
    public static var toolbarRoundContent: Color { Color(dash: DashColor.toolbarRoundContent) }

    // MARK: Top Intro

    public static var topIntroButtonBackground: Color { Color(dash: DashColor.topIntroButtonBackground) }
    public static var topIntroButtonContent: Color { Color(dash: DashColor.topIntroButtonContent) }

    // MARK: Tokens

    // MARK: Tokens / Blue

    public static var blue: Color { Color(dash: DashColor.blue) }
    public static var blueAlpha5: Color { Color(dash: DashColor.blueAlpha5) }
    public static var blueAlpha10: Color { Color(dash: DashColor.blueAlpha10) }
    public static var blueAlpha20: Color { Color(dash: DashColor.blueAlpha20) }
    public static var blueAlpha30: Color { Color(dash: DashColor.blueAlpha30) }
    public static var blueAlpha40: Color { Color(dash: DashColor.blueAlpha40) }
    public static var blueAlpha50: Color { Color(dash: DashColor.blueAlpha50) }
    public static var blueAlpha90: Color { Color(dash: DashColor.blueAlpha90) }

    // MARK: Tokens / Gray

    public static var black: Color { Color(dash: DashColor.black) }
    public static var black1000Alpha5: Color { Color(dash: DashColor.black1000Alpha5) }
    public static var black1000Alpha8: Color { Color(dash: DashColor.black1000Alpha8) }
    public static var black1000Alpha10: Color { Color(dash: DashColor.black1000Alpha10) }
    public static var black1000Alpha15: Color { Color(dash: DashColor.black1000Alpha15) }
    public static var black1000Alpha20: Color { Color(dash: DashColor.black1000Alpha20) }
    public static var black1000Alpha30: Color { Color(dash: DashColor.black1000Alpha30) }
    public static var black1000Alpha40: Color { Color(dash: DashColor.black1000Alpha40) }
    public static var black1000Alpha50: Color { Color(dash: DashColor.black1000Alpha50) }
    public static var black1000Alpha60: Color { Color(dash: DashColor.black1000Alpha60) }
    public static var black1000Alpha70: Color { Color(dash: DashColor.black1000Alpha70) }
    public static var black1000Alpha80: Color { Color(dash: DashColor.black1000Alpha80) }
    public static var black1000Alpha90: Color { Color(dash: DashColor.black1000Alpha90) }
    public static var black800: Color { Color(dash: DashColor.black800) }
    public static var black900: Color { Color(dash: DashColor.black900) }
    public static var gray50: Color { Color(dash: DashColor.gray50) }
    public static var gray100: Color { Color(dash: DashColor.gray100) }
    public static var gray200: Color { Color(dash: DashColor.gray200) }
    public static var gray300: Color { Color(dash: DashColor.gray300) }
    public static var gray300Alpha5: Color { Color(dash: DashColor.gray300Alpha5) }
    public static var gray300Alpha10: Color { Color(dash: DashColor.gray300Alpha10) }
    public static var gray300Alpha20: Color { Color(dash: DashColor.gray300Alpha20) }
    public static var gray300Alpha30: Color { Color(dash: DashColor.gray300Alpha30) }
    public static var gray300Alpha40: Color { Color(dash: DashColor.gray300Alpha40) }
    public static var gray300Alpha50: Color { Color(dash: DashColor.gray300Alpha50) }
    public static var gray300Alpha60: Color { Color(dash: DashColor.gray300Alpha60) }
    public static var gray300Alpha70: Color { Color(dash: DashColor.gray300Alpha70) }
    public static var gray300Alpha80: Color { Color(dash: DashColor.gray300Alpha80) }
    public static var gray300Alpha90: Color { Color(dash: DashColor.gray300Alpha90) }
    public static var gray400: Color { Color(dash: DashColor.gray400) }
    public static var gray400Alpha10: Color { Color(dash: DashColor.gray400Alpha10) }
    public static var gray400Alpha13: Color { Color(dash: DashColor.gray400Alpha13) }
    public static var gray400Alpha25: Color { Color(dash: DashColor.gray400Alpha25) }
    public static var gray500: Color { Color(dash: DashColor.gray500) }

    // MARK: Tokens / Green

    public static var green: Color { Color(dash: DashColor.green) }
    public static var greenAlpha10: Color { Color(dash: DashColor.greenAlpha10) }

    // MARK: Tokens / Light Blue

    public static var lightBlue: Color { Color(dash: DashColor.lightBlue) }
    public static var lightBlueAlpha10: Color { Color(dash: DashColor.lightBlueAlpha10) }

    // MARK: Tokens / Orange

    public static var orange: Color { Color(dash: DashColor.orange) }
    public static var orangeAlpha10: Color { Color(dash: DashColor.orangeAlpha10) }

    // MARK: Tokens / Red

    public static var red: Color { Color(dash: DashColor.red) }
    public static var redAlpha5: Color { Color(dash: DashColor.redAlpha5) }
    public static var redAlpha10: Color { Color(dash: DashColor.redAlpha10) }

    // MARK: Tokens / White

    public static var white: Color { Color(dash: DashColor.white) }
    public static var whiteAlpha5: Color { Color(dash: DashColor.whiteAlpha5) }
    public static var whiteAlpha10: Color { Color(dash: DashColor.whiteAlpha10) }
    public static var whiteAlpha15: Color { Color(dash: DashColor.whiteAlpha15) }
    public static var whiteAlpha20: Color { Color(dash: DashColor.whiteAlpha20) }
    public static var whiteAlpha30: Color { Color(dash: DashColor.whiteAlpha30) }
    public static var whiteAlpha40: Color { Color(dash: DashColor.whiteAlpha40) }
    public static var whiteAlpha50: Color { Color(dash: DashColor.whiteAlpha50) }
    public static var whiteAlpha60: Color { Color(dash: DashColor.whiteAlpha60) }
    public static var whiteAlpha70: Color { Color(dash: DashColor.whiteAlpha70) }
    public static var whiteAlpha80: Color { Color(dash: DashColor.whiteAlpha80) }
    public static var whiteAlpha90: Color { Color(dash: DashColor.whiteAlpha90) }

    // MARK: Tokens / Yellow

    public static var yellow: Color { Color(dash: DashColor.yellow) }
    public static var yellowAlpha10: Color { Color(dash: DashColor.yellowAlpha10) }

    // MARK: Tokens / Custom

    public static var topper: Color { Color(dash: DashColor.topper) }
    public static var uphold: Color { Color(dash: DashColor.uphold) }

    // MARK: Shadow

    /// DashUIKit's card shadow: a faint blue-grey in light appearance, none in dark.
    public static var shadow: Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            if appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua {
                return .clear
            }
            return NSColor(srgbRed: 0.72, green: 0.76, blue: 0.8, alpha: 0.1)
        })
    }
}
#endif
