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
//  Vendored from DashUIKit e8d9243 (Foundation/LineHeight+DashUI.swift); see Sources/DashUIMac/VENDORED.md.
//  Changes: AppKit branch only; preview removed.
//

#if os(macOS)
import AppKit
import SwiftUI

public extension View {
    /// Applies a design line height to text rendered with a fixed-size system font.
    ///
    /// `Font.system(size:)` renders with the typeface's natural line height, which is smaller than
    /// the line height of each `Font.dash.*` token (footnote is 13pt with an 18pt line height).
    /// `lineSpacing` opens the gap between wrapped lines, and vertical padding gives single lines
    /// the same extra height while keeping the text vertically centred.
    ///
    /// - Parameters:
    ///   - size: the font point size (the same value as the `Font.dash.*` token).
    ///   - lineHeight: the target line height from the design spec.
    func dashLineHeight(size: CGFloat, lineHeight: CGFloat) -> some View {
        let natural = Self.naturalLineHeight(forSize: size)
        let delta = max(lineHeight - natural, 0)
        return self
            .lineSpacing(delta)
            .padding(.vertical, delta / 2)
    }

    /// Natural line height of the system font at `size`. For SF it does not depend on weight.
    private static func naturalLineHeight(forSize size: CGFloat) -> CGFloat {
        let font = NSFont.systemFont(ofSize: size)
        return font.ascender - font.descender
    }
}
#endif
