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
//  Vendored from DashUIKit e8d9243 (Foundation/Icon_DashUI.swift); see Sources/DashUIMac/VENDORED.md.
//  Changes: each case maps to the DesignTokens `DashIconToken` exported to Resources/Icons
//  instead of a Media.xcassets imageset. Cases with no exported icon are dropped (VENDORED.md).
//

#if os(macOS)
import DesignTokens
import SwiftUI

/// A named DashUIKit icon, backed by an exported `DashIconToken`.
///
/// ```swift
/// DashIconImage(DashIcon.Menu.send.source)
/// ```
public protocol DashIconAsset {
    /// The exported icon this case renders.
    var token: DashIconToken { get }
}

public extension DashIconAsset {
    /// The icon as a `DashIconSource`, for components that take one.
    var source: DashIconSource { .token(token) }
}

/// Namespace for the DashUIKit icons, grouped the same way DashUIKit's asset catalog is.
public enum DashIcon {

    public enum Common: CaseIterable, DashIconAsset {
        case arrowDown
        case checkmark
        case chevronDownCurrencySelect
        case diagonalUpDown
        case enterAmountDash
        case iconDashCurrency

        public var token: DashIconToken {
            switch self {
            case .arrowDown: .arrowDown
            case .checkmark: .checkmark
            case .chevronDownCurrencySelect: .chevronDown
            case .diagonalUpDown: .swapUpDown
            case .enterAmountDash: .enterAmountDash
            case .iconDashCurrency: .dashCurrency
            }
        }
    }

    public enum Icons: CaseIterable, DashIconAsset {
        case copyOutline

        public var token: DashIconToken {
            switch self {
            case .copyOutline: .copy
            }
        }
    }

    public enum Checkbox: CaseIterable, DashIconAsset {
        case checkmarkChecked
        case checkmarkUnchecked

        public var token: DashIconToken {
            switch self {
            case .checkmarkChecked: .checkboxChecked
            case .checkmarkUnchecked: .checkboxUnchecked
            }
        }
    }

    public enum SearchBar: CaseIterable, DashIconAsset {
        case magnifyingglassIcon
        case xmarkIcon

        public var token: DashIconToken {
            switch self {
            case .magnifyingglassIcon: .searchMagnifyingGlass
            case .xmarkIcon: .searchClear
            }
        }
    }

    public enum Menu: CaseIterable, DashIconAsset {
        case addressBook
        case advancedSecurity
        case appearance
        case autohideBalance
        case backup
        case buySell
        case clipboard
        case connections
        case convert
        case csvExport
        case explore
        case extendPublicKey
        case file
        case importPrivateKey
        case infoRect
        case invitation
        case localCurrency
        case logout
        case masternodeKeys
        case mixing
        case networkMonitor
        case notification
        case pin
        case qr
        case receive
        case receiveDisabled
        case recoveryPhrase
        case rescanBlockchain
        case resetWallet
        case scanQR
        case security
        case send
        case sendAccount
        case sendAddress
        case sendDisabled
        case settings
        case shield
        case spendingConfirmation
        case support
        case tools
        case touchID
        case transfer
        case userSearch
        case usernameVoting
        case wallet

        public var token: DashIconToken {
            switch self {
            case .addressBook: .addressBook
            case .advancedSecurity: .advancedSecurity
            case .appearance: .appearance
            case .autohideBalance: .autohideBalance
            case .backup: .backup
            case .buySell: .buySell
            case .clipboard: .clipboard
            case .connections: .connections
            case .convert: .convert
            case .csvExport: .csvExport
            case .explore: .explore
            case .extendPublicKey: .extendedPublicKey
            case .file: .file
            case .importPrivateKey: .importPrivateKey
            case .infoRect: .about
            case .invitation: .invitation
            case .localCurrency: .localCurrency
            case .logout: .logout
            case .masternodeKeys: .masternodeKeys
            case .mixing: .coinjoinMixing
            case .networkMonitor: .networkMonitor
            case .notification: .notifications
            case .pin: .pin
            case .qr: .showQR
            case .receive: .receive
            case .receiveDisabled: .receiveDisabled
            case .recoveryPhrase: .recoveryPhrase
            case .rescanBlockchain: .rescanBlockchain
            case .resetWallet: .resetWallet
            case .scanQR: .scanQR
            case .security: .security
            case .send: .send
            case .sendAccount: .sendToContact
            case .sendAddress: .sendToAddress
            case .sendDisabled: .sendDisabled
            case .settings: .settings
            case .shield: .shield
            case .spendingConfirmation: .spendingConfirmation
            case .support: .support
            case .tools: .tools
            case .touchID: .biometrics
            case .transfer: .transfer
            case .userSearch: .userSearch
            case .usernameVoting: .usernameVoting
            case .wallet: .wallet
            }
        }
    }

    public enum NavigationBar: CaseIterable, DashIconAsset {
        case back
        case close
        case info
        case plus

        public var token: DashIconToken {
            switch self {
            case .back: .navBack
            case .close: .navClose
            case .info: .navInfo
            case .plus: .navPlus
            }
        }
    }

    public enum SystemMessage: CaseIterable, DashIconAsset {
        case infoRectSmall
        case shieldSmall
        case timerSmall
        case unmixedFunds
        case warningTriangle

        public var token: DashIconToken {
            switch self {
            case .infoRectSmall: .messageInfo
            case .shieldSmall: .messageShield
            case .timerSmall: .messageTimer
            case .unmixedFunds: .messageUnmixedFunds
            case .warningTriangle: .messageWarning
            }
        }
    }

    public enum Toast: CaseIterable, DashIconAsset {
        case copied
        case error
        case info
        case noWifi
        case success
        case warning

        public var token: DashIconToken {
            switch self {
            case .copied: .toastCopied
            case .error: .toastError
            case .info: .toastInfo
            case .noWifi: .toastNoInternet
            case .success: .toastSuccess
            case .warning: .toastWarning
            }
        }
    }

    public enum Other: CaseIterable, DashIconAsset {
        case textFieldClear
        case textFieldQR

        public var token: DashIconToken {
            switch self {
            case .textFieldClear: .textFieldClear
            case .textFieldQR: .textFieldQR
            }
        }
    }

    public enum AdditionalInfo: CaseIterable, DashIconAsset {
        case error
        case received
        case sent

        public var token: DashIconToken {
            switch self {
            case .error: .txDetailError
            case .received: .txDetailReceived
            case .sent: .txDetailSent
            }
        }
    }

    public enum Transaction: CaseIterable, DashIconAsset {
        case allTrans
        case contactRequestApprove
        case contactRequestSent
        case convert
        case error
        case internalTransfer
        case mining
        case mixing
        case received
        case sent

        public var token: DashIconToken {
            switch self {
            case .allTrans: .txAll
            case .contactRequestApprove: .txContactRequestApproved
            case .contactRequestSent: .txContactRequestSent
            case .convert: .txConvert
            case .error: .txError
            case .internalTransfer: .txInternalTransfer
            case .mining: .txMining
            case .mixing: .txMixing
            case .received: .txReceived
            case .sent: .txSent
            }
        }
    }
}
#endif
