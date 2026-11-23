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
//  Vendored from DashUIKit e8d9243 (Components/EnterAmount/EnterAmountView.swift); see Sources/DashUIMac/VENDORED.md.
//  Changes: preview blocks removed; compiled on macOS only.
//

#if os(macOS)
import SwiftUI

// MARK: - EnterAmountStyle

/// Controls which visual layout `EnterAmountView` renders.
@available(iOS 14, macOS 11, *)
public enum EnterAmountStyle {
    /// Single-amount input: Max button + `SwapAmountView` + `DashPickerView`. (default)
    case single
    /// Dual-amount stack: two amounts that animate and swap positions when `isPrimarySelected` changes.
    case dualSwap
}

// MARK: - EnterAmountView

@available(iOS 14, macOS 11, *)
public struct EnterAmountView: View {

    // MARK: State A (.single) — existing public API

    @Binding public var value: String
    @Binding public var selectedCurrency: CurrencyOption
    public var options: [CurrencyOption]
    public var onMax: (() -> Void)?
    public var onCurrencyTap: (() -> Void)?
    public var onPaste: (() -> Void)?
    public var hidesSelectedOption: Bool = false

    // MARK: Style selector

    /// Defaults to `.single` so all existing call sites compile unchanged.
    public var style: EnterAmountStyle = .single

    // MARK: State B (.dualSwap) — new API

    /// Amount string for the "primary" logical value (e.g. Dash). Shown in the large slot when
    /// `isPrimarySelected == true`.
    public var primaryAmount: String = ""
    /// Amount string for the "secondary" logical value (e.g. fiat). Shown in the large slot when
    /// `isPrimarySelected == false`.
    public var secondaryAmount: String = ""
    /// Currency for the primary logical value — drives Dash logo / symbol rendering.
    public var primaryCurrency: CurrencyOption = .dash
    /// Currency for the secondary logical value.
    public var secondaryCurrency: CurrencyOption = .fiat("USD")
    /// When `true`, `primaryAmount` occupies the large slot; when `false`, `secondaryAmount` does.
    public var isPrimarySelected: Bool = true
    /// Hides the currency-select chevron when `true`.
    public var isCurrencySelectorHidden: Bool = false
    /// Optional trailing input-type switcher. Pass `nil` (default) to omit the switcher entirely.
    public var currencyCodes: [String]? = nil
    /// The currently active currency code shown highlighted in the switcher.
    public var selectedCurrencyCode: String? = nil
    /// Called when the user taps either amount row to trigger a swap.
    public var onSwap: (() -> Void)? = nil
    /// Called when the user selects a currency code from the trailing switcher.
    public var onSelectInputType: ((String) -> Void)? = nil

    // MARK: Inits

    /// Initialises a `.single`-style view. Keeps existing call sites source-compatible.
    public init(
        value: Binding<String>,
        selectedCurrency: Binding<CurrencyOption>,
        options: [CurrencyOption],
        onMax: (() -> Void)? = nil,
        onCurrencyTap: (() -> Void)? = nil,
        onPaste: (() -> Void)? = nil,
        hidesSelectedOption: Bool = false
    ) {
        self._value = value
        self._selectedCurrency = selectedCurrency
        self.options = options
        self.onMax = onMax
        self.onCurrencyTap = onCurrencyTap
        self.onPaste = onPaste
        self.hidesSelectedOption = hidesSelectedOption
    }

    /// Initialises a `.dualSwap`-style view.
    ///
    /// - Parameters:
    ///   - primaryAmount: The host-formatted amount for the primary (Dash / first) input type.
    ///   - secondaryAmount: The host-formatted amount for the secondary (fiat / second) input type.
    ///   - primaryCurrency: Currency option for `primaryAmount`; controls Dash logo display.
    ///   - secondaryCurrency: Currency option for `secondaryAmount`; controls symbol display.
    ///   - isPrimarySelected: When `true`, `primaryAmount` occupies the large slot.
    ///   - isCurrencySelectorHidden: Set to `true` to hide the chevron selector.
    ///   - currencyCodes: If non-nil, a trailing vertical switcher is shown with these labels.
    ///   - selectedCurrencyCode: The code shown highlighted in the switcher.
    ///   - onMax: If provided, a Max button appears in the leading column.
    ///   - onSwap: Fired when the user taps either amount to request a position swap.
    ///   - onCurrencyTap: Fired when the user taps the chevron beside the secondary amount.
    ///   - onSelectInputType: Fired when the user picks a code from the trailing switcher.
    public init(
        primaryAmount: String,
        secondaryAmount: String,
        primaryCurrency: CurrencyOption,
        secondaryCurrency: CurrencyOption,
        isPrimarySelected: Bool,
        isCurrencySelectorHidden: Bool = false,
        currencyCodes: [String]? = nil,
        selectedCurrencyCode: String? = nil,
        onMax: (() -> Void)? = nil,
        onSwap: (() -> Void)? = nil,
        onCurrencyTap: (() -> Void)? = nil,
        onPaste: (() -> Void)? = nil,
        onSelectInputType: ((String) -> Void)? = nil
    ) {
        self._value = .constant("")
        self._selectedCurrency = .constant(.dash)
        self.options = []
        self.style = .dualSwap
        self.primaryAmount = primaryAmount
        self.secondaryAmount = secondaryAmount
        self.primaryCurrency = primaryCurrency
        self.secondaryCurrency = secondaryCurrency
        self.isPrimarySelected = isPrimarySelected
        self.isCurrencySelectorHidden = isCurrencySelectorHidden
        self.currencyCodes = currencyCodes
        self.selectedCurrencyCode = selectedCurrencyCode
        self.onMax = onMax
        self.onSwap = onSwap
        self.onCurrencyTap = onCurrencyTap
        self.onPaste = onPaste
        self.onSelectInputType = onSelectInputType
    }

    // MARK: Body

    public var body: some View {
        Group {
            if style == .dualSwap {
                dualSwapLayout
            } else {
                singleLayout
            }
        }
    }

    // MARK: Single layout (unchanged)

    private var singleLayout: some View {
        HStack(alignment: .center, spacing: 40) {
            if onMax != nil {
                maxButton
            } else {
                Color.clear.frame(width: 40, height: 40)
            }
            amountView
            currencyPickerView
        }
    }

    // MARK: Dual-swap layout

    /// Three equal-width columns: leading (Max or placeholder) — center (animated amounts) — trailing
    /// (switcher or placeholder). Equal fixed side widths keep the amount stack truly centered
    /// regardless of which controls are present.
    private var dualSwapLayout: some View {
        HStack(alignment: .center, spacing: 0) {
            // Leading side column
            Group {
                if onMax != nil {
                    maxButton
                } else {
                    Color.clear
                }
            }
            .frame(width: DualSwapLayout.sideColumnWidth)

            // Center — animated dual amount stack
            DualSwapAmountView(
                primaryAmount: primaryAmount,
                secondaryAmount: secondaryAmount,
                primaryCurrency: primaryCurrency,
                secondaryCurrency: secondaryCurrency,
                isPrimarySelected: isPrimarySelected,
                isCurrencySelectorHidden: isCurrencySelectorHidden,
                onSwap: onSwap ?? {},
                onSelectCurrency: onCurrencyTap ?? {},
                onPaste: onPaste
            )
            .frame(maxWidth: .infinity)

            // Trailing side column
            Group {
                if let codes = currencyCodes, !codes.isEmpty {
                    DualInputTypeSwitcher(
                        codes: codes,
                        selected: selectedCurrencyCode ?? (codes.first ?? ""),
                        onSelect: onSelectInputType ?? { _ in }
                    )
                } else {
                    Color.clear
                }
            }
            .frame(width: DualSwapLayout.sideColumnWidth)
        }
    }

    // MARK: Shared sub-views

    private var maxButton: some View {
        Button {
            onMax?()
        } label: {
            ZStack {
                Circle()
                    .fill(Color.dash.blueAlpha5)
                    .frame(width: 40, height: 40)

                Text(NSLocalizedString("Max", bundle: .module, comment: ""))
                    .dashFont(.caption2)
                    .foregroundColor(Color.dash.blue)
            }
        }
    }

    private var amountView: some View {
        SwapAmountView(
            amount: value,
            symbol: selectedCurrency.symbol,
            showDashLogo: selectedCurrency == .dash,
            showCurrencyButton: selectedCurrency.isFiat,
            onCurrencyTap: onCurrencyTap,
            onPaste: onPaste
        )
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var visibleOptions: [CurrencyOption] {
        guard hidesSelectedOption else { return options }
        return options.filter { $0 != selectedCurrency }
    }

    private var currencyPickerView: some View {
        DashPickerView(
            options: visibleOptions,
            title: { $0.displayName },
            selected: $selectedCurrency
        )
    }
}
#endif
