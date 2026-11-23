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
//  Vendored from DashUIKit e8d9243 (Components/MenuItem.swift); see Sources/DashUIMac/VENDORED.md.
//  Changes: preview blocks removed; compiled on macOS only; icons render through DashIconImage
//  from the exported icon set; the toggle accessory uses the switch style (macOS defaults to a
//  checkbox) and carries the row title as its accessibility label.
//

#if os(macOS)
import SwiftUI

/// Finite set of trailing accessories for `MenuItem`.
/// Add a new case here — not a per-call-site font/color override — when a
/// new trailing look is needed, to keep all rows consistent.
@available(iOS 14, macOS 11, *)
public enum MenuItemAccessory {
    case none
    case toggle(isOn: Binding<Bool>)
    case text(String)
    case button(DashButton)
    /// Dash amount with an optional pre-formatted fiat sub-line.
    /// The caller converts the fiat value via its own exchange infrastructure;
    /// the library only renders the string it receives.
    case balance(dash: Int64, sign: DashAmountSign = .negativeOnly, fiat: String? = nil)
}

@available(iOS 14, macOS 11, *)
public struct MenuItem: View {

    public var leadingIcon: DashIconSource?
    public var isEnabled: Bool
    public var disabledLeadingIcon: DashIconSource?
    public var title: String
    public var helpText: String?
    public var infoIcon: DashIconSource?
    public var accessory: MenuItemAccessory

    public init(
        leadingIcon: DashIconSource? = nil,
        isEnabled: Bool = true,
        disabledLeadingIcon: DashIconSource? = nil,
        title: String,
        helpText: String? = nil,
        infoIcon: DashIconSource? = nil,
        accessory: MenuItemAccessory = .none
    ) {
        self.leadingIcon = leadingIcon
        self.isEnabled = isEnabled
        self.disabledLeadingIcon = disabledLeadingIcon
        self.title = title
        self.helpText = helpText
        self.infoIcon = infoIcon
        self.accessory = accessory
    }

    public var body: some View {
        HStack(spacing: 10) {
            leading
            central
            Spacer()
            trailing
        }
        .padding(10)
    }

    @ViewBuilder
    private var leading: some View {
        let icon = isEnabled ? leadingIcon : (disabledLeadingIcon ?? leadingIcon)
        if let icon {
            DashIconImage(icon)
                .scaledToFit()
                .frame(width: 30, height: 30)
        }
    }

    private var central: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 6) {
                Text(title)
                    .dashFont(.subheadMedium)
                    .foregroundColor(isEnabled ? Color.dash.primaryText : Color.dash.secondaryText)

                if let icon = infoIcon {
                    DashIconImage(icon, resizable: false)
                        .frame(width: 20, height: 20, alignment: .center)
                }
            }

            if let helpText {
                Text(helpText)
                    .dashFont(.footnote)
                    .foregroundColor(isEnabled ? Color.dash.secondaryText : Color.dash.tertiaryText)
            }
        }
        .padding(.leading, 6)
    }

    @ViewBuilder
    private var trailing: some View {
        switch accessory {
        case .none:
            EmptyView()
        case .toggle(let isOn):
            Toggle(title, isOn: isOn)
                .toggleStyle(.switch)
                .labelsHidden()
                .disabled(!isEnabled)
        case .text(let value):
            Text(value)
                .dashFont(.subhead)
                .foregroundColor(Color.dash.secondaryText)
        case .button(let button):
            button
        case .balance(let dash, let sign, let fiat):
            VStack(alignment: .trailing, spacing: 1) {
                DashAmount(amount: dash, sign: sign)
                    .foregroundColor(Color.dash.primaryText)

                if dash != 0, dash != .max, dash != .min, let fiat {
                    Text(fiat)
                        .dashFont(.footnote)
                        .foregroundColor(Color.dash.secondaryText)
                }
            }
        }
    }
}
#endif
