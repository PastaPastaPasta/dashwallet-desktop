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
//  Vendored from DashUIKit e8d9243 (Components/EnterAmount/DashBalanceView.swift); see Sources/DashUIMac/VENDORED.md.
//  Changes: preview blocks removed; compiled on macOS only; icons render through DashIconImage
//  from the exported icon set.
//

#if os(macOS)
import SwiftUI

// MARK: - DashBalanceView

/// Trailing balance for the convert source row: a symbol-free Dash amount followed by the Dash
/// symbol, with the fiat value beneath. Both strings come from the view model
/// (`dashBalanceFormatted` / `dashBalanceFiat`); pass `fiat: nil` to hide the fiat line.
@available(iOS 14, macOS 11, *)
public struct DashBalanceView: View {
    /// Symbol-free formatted balance, e.g. "1.5".
    public let balance: String
    /// Formatted fiat value; `nil` hides the line (e.g. zero balance).
    public var fiat: String?

    public init(balance: String, fiat: String? = nil) {
        self.balance = balance
        self.fiat = fiat
    }

    public var body: some View {
        VStack(alignment: .trailing, spacing: 0) {
            HStack(spacing: 6) {
                Text(balance)
                    .dashFont(.subhead)
                    .lineLimit(1)

                dashSymbol
            }
            .foregroundColor(Color.dash.primaryText)

            if let fiat {
                Text(fiat)
                    .dashFont(.caption1)
                    .foregroundColor(Color.dash.primaryText)
            }
        }
    }

    private var dashSymbol: some View {
        DashIconImage(DashIcon.Common.iconDashCurrency.source)
            .aspectRatio(contentMode: .fit)
            .frame(width: 12, height: 10)
    }
}
#endif
