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
//  Vendored from DashUIKit e8d9243 (Components/BottomSheet.swift); see Sources/DashUIMac/VENDORED.md.
//  Changes (macOS port): chrome for a macOS `.sheet` instead of an iOS bottom sheet. No grabber
//  and no detents (macOS sheets size to their content); no NavigationView wrapper; dismissal goes
//  through `\.dismiss`, and Escape closes the sheet while dismissal is enabled.
//  `selfSizingSheet(...)` and `BottomSheetHeightPreferenceKey` are not ported.
//

#if os(macOS)
import SwiftUI

/// Sheet chrome: a navigation bar (optional back button, title, close button) above the content.
///
/// Present it with `.sheet { BottomSheet(...) { … } }`.
public struct BottomSheet<Content: View>: View {
    @Environment(\.dismiss) private var dismiss

    public var title: String = ""
    @Binding public var showBackButton: Bool
    public var onBackButtonPressed: (() -> Void)? = nil
    /// Controls every dismissal affordance owned by the sheet. When `false`, the close button is
    /// disabled, Escape does nothing and interactive dismissal is blocked.
    @Binding public var isDismissalEnabled: Bool
    public var showsCloseButton: Bool = true
    /// Overrides the close button action. The callback is responsible for dismissing the sheet.
    public var onClose: (() -> Void)? = nil
    /// `true` (default): the content fills the sheet. `false`: the sheet takes the content's
    /// natural height.
    public var fillsHeight: Bool = true
    /// Fill behind the whole sheet: header and content alike.
    public var background: Color = .dash.primaryBackground
    @ViewBuilder public var content: () -> Content

    public init(
        title: String = "",
        showBackButton: Binding<Bool>,
        onBackButtonPressed: (() -> Void)? = nil,
        isDismissalEnabled: Binding<Bool> = .constant(true),
        showsCloseButton: Bool = true,
        onClose: (() -> Void)? = nil,
        fillsHeight: Bool = true,
        background: Color = .dash.primaryBackground,
        @ViewBuilder content: @escaping () -> Content
    ) {
        self.title = title
        self._showBackButton = showBackButton
        self.onBackButtonPressed = onBackButtonPressed
        self._isDismissalEnabled = isDismissalEnabled
        self.showsCloseButton = showsCloseButton
        self.onClose = onClose
        self.fillsHeight = fillsHeight
        self.background = background
        self.content = content
    }

    public var body: some View {
        VStack(spacing: 0) {
            header
            contentSection
        }
        .background(background)
        .interactiveDismissDisabled(!isDismissalEnabled)
        .onExitCommand(perform: close)
    }

    private var header: some View {
        NavigationBar(
            leading: {
                if showBackButton {
                    NavigationBarElement.back.button { onBackButtonPressed?() }
                }
            },
            central: {
                Text(title)
                    .dashFont(.calloutMedium)
                    .foregroundColor(.dash.primaryText)
                    .accessibilityAddTraits(.isHeader)
            },
            trailing: {
                if showsCloseButton {
                    NavigationBarElement.close.button(action: close)
                        .disabled(!isDismissalEnabled)
                        .opacity(isDismissalEnabled ? 1 : 0.35)
                }
            }
        )
    }

    @ViewBuilder
    private var contentSection: some View {
        if fillsHeight {
            content()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            content()
                .frame(maxWidth: .infinity)
        }
    }

    private func close() {
        BottomSheetDismissalAction.perform(isEnabled: isDismissalEnabled, onClose: onClose, dismiss: { dismiss() })
    }
}

public extension BottomSheet {
    /// A sheet that takes its content's natural height (`fillsHeight: false`). macOS sizes sheets
    /// to their content, so no detent modifier is needed.
    static func selfSizing(
        title: String = "",
        showBackButton: Binding<Bool>,
        onBackButtonPressed: (() -> Void)? = nil,
        isDismissalEnabled: Binding<Bool> = .constant(true),
        showsCloseButton: Bool = true,
        onClose: (() -> Void)? = nil,
        background: Color = .dash.primaryBackground,
        @ViewBuilder content: @escaping () -> Content
    ) -> BottomSheet<Content> {
        BottomSheet(
            title: title,
            showBackButton: showBackButton,
            onBackButtonPressed: onBackButtonPressed,
            isDismissalEnabled: isDismissalEnabled,
            showsCloseButton: showsCloseButton,
            onClose: onClose,
            fillsHeight: false,
            background: background,
            content: content
        )
    }
}

enum BottomSheetDismissalAction {
    static func perform(isEnabled: Bool, onClose: (() -> Void)?, dismiss: () -> Void) {
        guard isEnabled else { return }

        if let onClose {
            onClose()
        } else {
            dismiss()
        }
    }
}
#endif
