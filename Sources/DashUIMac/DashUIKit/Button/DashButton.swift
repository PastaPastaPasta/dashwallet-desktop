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
//  Vendored from DashUIKit e8d9243 (Button/DashButton.swift); see Sources/DashUIMac/VENDORED.md.
//  Changes: preview blocks removed; compiled on macOS only; icons render through DashIconImage
//  from the exported icon set; the loading spinner uses the small control size (the macOS default
//  spinner is 32 pt).
//

#if os(macOS)
import SwiftUI

@available(iOS 14, macOS 10.15, *)
public enum DashButtonSize: Sendable {
    case large, medium, small, extraSmall

    var gap: CGFloat {
        switch self {
        case .large: return 10
        case .medium: return 8
        case .small: return 6
        case .extraSmall: return 6
        }
    }

    var hPadding: CGFloat {
        switch self {
        case .large: return 20
        case .medium: return 16
        case .small: return 12
        case .extraSmall: return 8
        }
    }

    var vPadding: CGFloat {
        switch self {
        case .large: return 14
        case .medium: return 10
        case .small: return 6
        case .extraSmall: return 4
        }
    }

    var radius: CGFloat {
        switch self {
        case .large: return 16
        case .medium: return 14
        case .small: return 11
        case .extraSmall: return 9
        }
    }

    var fontSize: Font {
        switch self {
        case .large: return .system(size: 16)
        case .medium: return .system(size: 14)
        case .small: return .system(size: 13)
        case .extraSmall: return .system(size: 12)
        }
    }
}

@available(iOS 14, macOS 10.15, *)
public enum DashButtonStyle {
    case filledBlue, filledRed, strokeGray, tintedBlue, tintedGray, plainBlue, plainBlack, plainRed, filledWhiteBlue, tintedWhite, plainWhite

    func backgroundColor(isEnabled: Bool) -> Color {
        switch self {
        case .filledBlue:
            return isEnabled ? .dash.buttonFilledBlueBackground : .dash.buttonFilledBlueBackgroundDisabled
        case .filledRed:
            return isEnabled ? .dash.buttonFilledRedBackground : .dash.buttonFilledRedBackgroundDisabled
        case .strokeGray:
            return isEnabled ? .clear : .dash.buttonStrokeGrayBackgroundDisabled
        case .tintedBlue:
            return isEnabled ? .dash.buttonTintedBlueBackground : .dash.buttonTintedBlueBackgroundDisabled
        case .tintedGray:
            return isEnabled ? .dash.buttonTintedGrayBackground : .dash.buttonTintedGrayBackgroundDisabled
        case .plainBlue, .plainBlack, .plainRed, .plainWhite:
            return .clear
        case .filledWhiteBlue:
            return isEnabled ? .dash.buttonFilledWhiteBackground : .dash.buttonFilledWhiteBackgroundDisabled
        case .tintedWhite:
            return isEnabled ? .dash.buttonTintedWhiteBackground : .dash.buttonTintedWhiteBackgroundDisabled
        }
    }

    func foregroundColor(isEnabled: Bool) -> Color {
        switch self {
        case .filledBlue:
            return isEnabled ? .dash.buttonFilledBlueContent : .dash.buttonFilledBlueContentDisabled
        case .filledRed:
            return isEnabled ? .dash.buttonFilledRedContent : .dash.buttonFilledRedContentDisabled
        case .strokeGray:
            return isEnabled ? .dash.buttonStrokeGrayContent : .dash.buttonStrokeGrayContentDisabled
        case .tintedBlue:
            return isEnabled ? .dash.buttonTintedBlueContent : .dash.buttonTintedBlueContentDisabled
        case .tintedGray:
            return isEnabled ? .dash.buttonTintedGrayContent : .dash.buttonTintedGrayContentDisabled
        case .plainBlue:
            return isEnabled ? .dash.buttonPlainBlueContent : .dash.buttonPlainBlueContentDisabled
        case .plainBlack:
            return isEnabled ? .dash.buttonPlainBlackContent : .dash.buttonPlainBlackContentDisabled
        case .plainRed:
            return isEnabled ? .dash.buttonPlainRedContent : .dash.buttonPlainRedContentDisabled
        case .filledWhiteBlue:
            return isEnabled ? .dash.buttonFilledWhiteContent : .dash.buttonFilledWhiteContentDisabled
        case .tintedWhite:
            return isEnabled ? .dash.buttonTintedWhiteContent : .dash.buttonTintedWhiteContentDisabled
        case .plainWhite:
            return isEnabled ? .dash.buttonPlainWhiteContent : .dash.buttonPlainWhiteContentDisabled
        }
    }
}

@available(iOS 14, macOS 11, *)
public struct DashButton: View {

    public var text: String? = "Label"
    public var leadingIcon: DashIconSource? = nil
    public var trailingIcon: DashIconSource? = nil

    public var isEnabled: Bool = true
    public var isLoading: Bool = false
    public var fillsWidth: Bool = false

    public var size: DashButtonSize = .large
    public var style: DashButtonStyle = .filledBlue
    public var action: () -> Void = {}

    public init(
        text: String? = nil,
        leadingIcon: DashIconSource? = nil,
        trailingIcon: DashIconSource? = nil,
        isEnabled: Bool = true,
        isLoading: Bool = false,
        fillsWidth: Bool = false,
        size: DashButtonSize,
        style: DashButtonStyle,
        action: @escaping () -> Void = {}
    ) {
        self.text = text
        self.leadingIcon = leadingIcon
        self.trailingIcon = trailingIcon
        self.isEnabled = isEnabled
        self.isLoading = isLoading
        self.fillsWidth = fillsWidth
        self.size = size
        self.style = style
        self.action = action
    }

    public var body: some View {
        Button(action: action) {
            styledContent
        }
        .buttonStyle(.plain)
        .disabled(!isEnabled || isLoading)
    }

    private var styledContent: some View {
        Group {
            if style == .strokeGray {
                content
                    .padding(.horizontal, size.hPadding)
                    .padding(.vertical, size.vPadding)
                    .foregroundColor(style.foregroundColor(isEnabled: isEnabled))
                    .background(style.backgroundColor(isEnabled: isEnabled))
                    .clipShape(RoundedRectangle(cornerRadius: size.radius, style: .continuous))
                    .frame(maxWidth: fillsWidth ? .infinity : nil)
                    .overlay(
                        RoundedRectangle(cornerRadius: size.radius)
                            .inset(by: 0.5)
                            .stroke(Color.dash.buttonStrokeGrayStroke, lineWidth: 1)
                    )
            } else {
                content
                    .padding(.horizontal, size.hPadding)
                    .padding(.vertical, size.vPadding)
                    .foregroundColor(style.foregroundColor(isEnabled: isEnabled))
                    .frame(maxWidth: fillsWidth ? .infinity : nil)
                    .background(style.backgroundColor(isEnabled: isEnabled))
                    .clipShape(RoundedRectangle(cornerRadius: size.radius, style: .continuous))
            }
        }

    }

    private var content: some View {
        HStack(spacing: size.gap) {
            if let icon = leadingIcon {
                DashIconImage(icon, resizable: false)
            }

            if isLoading {
                ProgressView()
                    .controlSize(.small)
            } else if let text {
                Text(text)
                    .font(size.fontSize)
                    .fontWeight(.semibold)
            }

            if let icon = trailingIcon {
                DashIconImage(icon, resizable: false)
            }
        }
    }
}
#endif
