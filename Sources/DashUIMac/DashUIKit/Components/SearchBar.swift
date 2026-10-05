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
//  Vendored from DashUIKit e8d9243 (Components/SearchBar.swift); see Sources/DashUIMac/VENDORED.md.
//  Changes (macOS port): one focus-driven implementation (the iOS 14 fallback is gone); the field
//  uses the plain text-field style so AppKit draws no bezel; Escape clears and leaves the field;
//  the clear button is a 24 pt target (pointer, not finger); icons come from the exported set.
//

#if os(macOS)
import DesignTokens
import SwiftUI

/// Rounded search field with a magnifying glass, a clear button while text is present, and a
/// "Cancel" button while the field has focus.
public struct SearchBar: View {
    private enum Layout {
        static let fieldHeight: CGFloat = 40
        static let fieldCornerRadius: CGFloat = 14
        static let fieldHorizontalPadding: CGFloat = 14
        static let fieldSpacing: CGFloat = 10
        static let clearTapArea: CGFloat = 24
        static let cancelHorizontalPadding: CGFloat = 12
        static let cancelVerticalPadding: CGFloat = 6
        static let animationDuration: TimeInterval = 0.25
    }

    @Binding private var text: String
    private let placeholder: String

    @FocusState private var isFocused: Bool

    public init(
        text: Binding<String>,
        placeholder: String? = nil
    ) {
        self._text = text
        self.placeholder = placeholder
            ?? NSLocalizedString("Search", bundle: .module, comment: "DashUIKit")
    }

    public var body: some View {
        HStack(spacing: 0) {
            HStack(spacing: Layout.fieldSpacing) {
                DashIconImage(.token(.searchMagnifyingGlass))
                    .scaledToFit()
                    .frame(maxHeight: 15)
                    .accessibilityHidden(true)
                searchField
                clearButton
            }
            .padding(.horizontal, Layout.fieldHorizontalPadding)
            .frame(height: Layout.fieldHeight)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.dash.searchBackground)
            .clipShape(RoundedRectangle(cornerRadius: Layout.fieldCornerRadius, style: .continuous))

            if isFocused {
                cancelButton
                    .transition(.move(edge: .trailing).combined(with: .opacity))
            }
        }
        .animation(.easeInOut(duration: Layout.animationDuration), value: isFocused)
    }

    private var searchField: some View {
        TextField(
            text: $text,
            prompt: Text(placeholder)
                .font(Font.dash.subhead)
                .foregroundStyle(Color.dash.black1000Alpha30)
        ) {
            Text(placeholder)
        }
        .textFieldStyle(.plain)
        .font(Font.dash.subhead)
        .foregroundStyle(Color.dash.primaryText)
        .focused($isFocused)
        .onExitCommand(perform: cancel)
    }

    @ViewBuilder
    private var clearButton: some View {
        if !text.isEmpty {
            Button(
                action: { text = "" },
                label: {
                    DashIconImage(.token(.searchClear))
                        .scaledToFit()
                        .frame(maxHeight: 15)
                        .frame(width: Layout.clearTapArea, height: Layout.clearTapArea)
                        .contentShape(Rectangle())
                }
            )
            .buttonStyle(.plain)
            .accessibilityLabel(Text(NSLocalizedString("Clear", bundle: .module, comment: "DashUIKit")))
        }
    }

    private var cancelButton: some View {
        Button(
            action: cancel,
            label: {
                Text(NSLocalizedString("Cancel", bundle: .module, comment: "DashUIKit"))
                    .font(Font.dash.footnote.weight(.semibold))
                    .foregroundStyle(Color.dash.primaryText)
                    .padding(.horizontal, Layout.cancelHorizontalPadding)
                    .padding(.vertical, Layout.cancelVerticalPadding)
            }
        )
        .buttonStyle(.plain)
    }

    private func cancel() {
        text = ""
        isFocused = false
    }
}
#endif
