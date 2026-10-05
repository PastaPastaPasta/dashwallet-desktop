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
//  Vendored from DashUIKit e8d9243 (Components/ConverterCard/ConverterCard.swift); see Sources/DashUIMac/VENDORED.md.
//  Changes: preview blocks removed; compiled on macOS only; icons render through DashIconImage
//  from the exported icon set.
//

#if os(macOS)
import SwiftUI

// MARK: - ConverterCard

/// A two-row card (source → destination) with an arrow badge centered on the seam between the
/// rows. Each row is a `MenuItem` wrapped in rounded card chrome.
///
/// Pass `onSwap` to show a tappable `diagonal-up-down` button; omit it (or pass `nil`) for a
/// static `arrow-down` indicator when the card is non-swappable.
@available(iOS 14, macOS 11, *)
public struct ConverterCard: View {

    private enum Layout {
        static let cardSpacing: CGFloat = 5
    }

    private let fromItem: ConverterCardItem
    private let toItem: ConverterCardItem
    private let onSwap: (() -> Void)?

    @State private var topRowHeight: CGFloat = 74

    public init(
        fromItem: ConverterCardItem,
        toItem: ConverterCardItem,
        onSwap: (() -> Void)? = nil
    ) {
        self.fromItem = fromItem
        self.toItem = toItem
        self.onSwap = onSwap
    }

    private var orderedItems: [ConverterCardItem] { [fromItem, toItem] }

    public var body: some View {
        VStack(spacing: Layout.cardSpacing) {
            ForEach(Array(orderedItems.enumerated()), id: \.element.id) { index, item in
                ConverterCardRow(slot: index == 0 ? .top : .bottom) {
                    row(item: item)
                }
            }
        }
        .animation(.spring(response: 0.35, dampingFraction: 0.82), value: fromItem.id)
        .overlay(
            ConverterArrowBadge(onSwap: onSwap)
                // Anchor the badge's CENTER to the overlay's .top point, so the offset below
                // places the badge centre exactly on the seam — independent of the badge's height.
                .alignmentGuide(VerticalAlignment.top) { $0[VerticalAlignment.center] }
                .offset(y: seamY),
            alignment: .top
        )
        .onPreferenceChange(ConverterRowHeightKey.self) { heights in
            if let h = heights[.top], h > 0 { topRowHeight = h }
        }
    }

    /// Centre of the gap between the two rows. Depends only on the top row height and the stack
    /// spacing — never on the bottom row — so it stays correct when the rows differ in height.
    private var seamY: CGFloat { topRowHeight + Layout.cardSpacing / 2 }

    /// Row layout mirrors `MenuItem` (icon 30pt, 10pt padding) but stays flexible enough for a
    /// custom icon/trailing view and a multi-line subtitle.
    private func row(item: ConverterCardItem) -> some View {
        HStack(spacing: 10) {
            leading(item)

            VStack(alignment: .leading, spacing: 1) {
                Text(item.title)
                    .dashFont(.subheadMedium)
                    .foregroundColor(Color.dash.primaryText)

                if let subtitle = item.subtitle {
                    Text(subtitle)
                        .dashFont(.footnote)
                        .foregroundColor(Color.dash.secondaryText)
                        .lineLimit(item.subtitleLineLimit)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(.leading, 6)

            Spacer(minLength: 8)

            trailing(item)
        }
        .padding(10)
    }

    @ViewBuilder
    private func leading(_ item: ConverterCardItem) -> some View {
        if let iconView = item.iconView {
            iconView.frame(width: 30, height: 30)
        } else if let icon = item.icon {
            DashIconImage(icon)
                .scaledToFit()
                .frame(width: 30, height: 30)
        }
    }

    @ViewBuilder
    private func trailing(_ item: ConverterCardItem) -> some View {
        if let trailingView = item.trailingView {
            trailingView
        } else if item.showsBalance {
            VStack(alignment: .trailing, spacing: 1) {
                DashAmount(amount: item.dashBalance, sign: .none)
                    .foregroundColor(Color.dash.primaryText)

                if item.dashBalance != 0, let fiat = item.fiat {
                    Text(fiat)
                        .dashFont(.footnote)
                        .foregroundColor(Color.dash.secondaryText)
                }
            }
        }
    }
}
#endif
