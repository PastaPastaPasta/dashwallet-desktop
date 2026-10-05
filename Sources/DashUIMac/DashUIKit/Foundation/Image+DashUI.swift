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
//  Vendored from DashUIKit e8d9243 (Foundation/Image+DashUI.swift); see Sources/DashUIMac/VENDORED.md.
//  Changes: `.uiImage` becomes `.nsImage`; new `.token` case for the exported icon set; icons
//  render through the `DashIconImage` view, which picks the light or dark file from the environment.
//

#if os(macOS)
import AppKit
import DesignTokens
import SwiftUI

/// Where an icon comes from.
public enum DashIconSource {
    /// An SF Symbol.
    case system(_ name: String)
    /// An image in an asset catalog (`nil` bundle = the main bundle).
    case custom(_ name: String, bundle: Bundle? = nil)
    /// An icon of the exported Dash icon set (Resources/Icons).
    case token(DashIconToken)
    /// A runtime image.
    case nsImage(_ image: NSImage)
}

public extension Image {
    /// Resolves a non-token icon source into a plain `Image`. Token icons need the environment's
    /// appearance and icon library, so they render through `DashIconImage` instead; for a token
    /// this initialiser loads the light variant from the bundled library and yields an empty image
    /// when the file is missing.
    init(dash source: DashIconSource) {
        switch source {
        case .system(let name):
            self = Image(systemName: name)
        case .custom(let name, let bundle):
            self = Image(name, bundle: bundle)
        case .token(let token):
            self = Image(nsImage: DashIconLibrary.bundled.image(for: token, appearance: .light) ?? NSImage())
        case .nsImage(let image):
            self = Image(nsImage: image)
        }
    }
}

/// Renders a `DashIconSource`, choosing the dark variant of token icons in dark appearance.
///
/// `resizable` mirrors `Image.resizable()`: a resizable icon fills the frame its parent gives it
/// (combine with `.scaledToFit()`); a fixed one keeps its point size. `template` renders the
/// icon as a mask tinted by the foreground style.
public struct DashIconImage: View {
    private let source: DashIconSource
    private let resizable: Bool
    private let template: Bool?

    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.dashIconLibrary) private var library

    /// - Parameters:
    ///   - template: `true` forces template rendering, `false` forces the original colours,
    ///     `nil` keeps the icon's own setting (exported template icons tint by default).
    public init(_ source: DashIconSource, resizable: Bool = true, template: Bool? = nil) {
        self.source = source
        self.resizable = resizable
        self.template = template
    }

    public var body: some View {
        let base = image.renderingMode(renderingMode)
        if resizable {
            base.resizable()
        } else {
            base
        }
    }

    private var image: Image {
        switch source {
        case .token(let token):
            // A missing file renders as an empty image of zero size, not as a placeholder glyph.
            Image(nsImage: library.image(for: token, appearance: DashAppearance(colorScheme)) ?? NSImage())
        default:
            Image(dash: source)
        }
    }

    private var renderingMode: Image.TemplateRenderingMode? {
        switch template {
        case .some(true): .template
        case .some(false): .original
        case .none: nil
        }
    }
}
#endif
