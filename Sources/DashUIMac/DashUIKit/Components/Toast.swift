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
//  Vendored from DashUIKit e8d9243 (Components/Toast.swift); see Sources/DashUIMac/VENDORED.md.
//  Changes (macOS port): the UIKit `UIVisualEffectView` blur becomes a SwiftUI dark
//  `.ultraThinMaterial`; icons come from the exported icon set; preview removed.
//

#if os(macOS)
import DesignTokens
import SwiftUI

// MARK: - ToastStyle

public enum ToastStyle: CaseIterable, Sendable {
    case warning, info, error, success, copied, loading, noInternet

    /// The exported icon, or nil for `loading`, which draws `LoadingSpinner` instead.
    var icon: DashIconToken? {
        switch self {
        case .warning: .toastWarning
        case .info: .toastInfo
        case .error: .toastError
        case .success: .toastSuccess
        case .copied: .toastCopied
        case .loading: nil
        case .noInternet: .toastNoInternet
        }
    }
}

// MARK: - Toast

/// A dark, blurred notification capsule with a leading status icon and an optional close button.
public struct Toast: View {

    private let style: ToastStyle
    private let message: String
    private let onDismiss: (() -> Void)?

    public init(
        style: ToastStyle,
        message: String,
        onDismiss: (() -> Void)? = nil
    ) {
        self.style = style
        self.message = message
        self.onDismiss = onDismiss
    }

    public var body: some View {
        HStack(alignment: .top, spacing: 8) {
            HStack(alignment: .top, spacing: 0) {
                leadingIcon
                    .frame(width: 24, height: 24)

                Text(message)
                    .dashFont(.footnoteMedium)
                    .foregroundColor(Color.dash.toastText)
                    .padding(.vertical, 3)
                    .padding(.leading, 8)
            }
            .padding(.trailing, 8)

            if let onDismiss {
                Spacer(minLength: 10)

                Button(action: onDismiss) {
                    XmarkIcon(color: Color.dash.toastText)
                        .padding(8)
                        .background(Circle().fill(Color.dash.whiteAlpha10))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text(NSLocalizedString("Close", bundle: .module, comment: "DashUIKit")))
            }
        }
        .padding(.leading, 12)
        .padding(.trailing, 8)
        .padding(.vertical, 8)
        .background(
            ZStack {
                Rectangle()
                    .fill(.ultraThinMaterial)
                    .environment(\.colorScheme, .dark)
                Color.dash.toastBackground
            }
        )
        .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var leadingIcon: some View {
        if let icon = style.icon {
            DashIconImage(.token(icon))
                .scaledToFit()
                .frame(width: 16, height: 16)
        } else {
            // The design-system spinner; it applies its own per-spoke opacity on top of toastText.
            LoadingSpinner(size: 16, color: Color.dash.toastText)
        }
    }
}
#endif
