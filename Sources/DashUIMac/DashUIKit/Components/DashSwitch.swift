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
//  Vendored from DashUIKit e8d9243 (Components/DashSwitch.swift); see Sources/DashUIMac/VENDORED.md.
//  Changes (macOS port): the system switch toggle style tinted with `switchTrackFillOn`; an
//  optional accessibility label because the visible label is hidden.
//

#if os(macOS)
import SwiftUI

/// A DS-tinted switch without a visible label.
public struct DashSwitch: View {

    @Binding private var isOn: Bool
    private let accessibilityLabel: String

    /// - Parameter accessibilityLabel: the name VoiceOver reads for the switch.
    public init(isOn: Binding<Bool>, accessibilityLabel: String = "") {
        self._isOn = isOn
        self.accessibilityLabel = accessibilityLabel
    }

    public var body: some View {
        Toggle(accessibilityLabel, isOn: $isOn)
            .toggleStyle(.switch)
            .labelsHidden()
            .tint(Color.dash.switchTrackFillOn)
    }
}
#endif
