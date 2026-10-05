// The macOS app's root state: the composition-root environment, the main
// view model and the window-level choices menus make.
#if os(macOS)
import Foundation
import Observation
import SwiftUI
import WalletFeatures
import WalletRuntime

/// Scene identifiers for `openWindow(id:)`.
public enum SceneID {
    public static let main = "main"
    public static let addressBook = "address-book"
    public static let signVerify = "sign-verify"
    public static let about = "about"
}

public enum SignVerifyTab: Hashable, Sendable {
    case sign, verify
}

@MainActor
@Observable
public final class MacAppModel {
    public let launch: LaunchOptions
    /// `nil` when no wallet runtime could be built (`unavailableReason` says why).
    public let env: AppEnvironment?
    public let main: MainViewModel?
    public let unavailableReason: String?

    public var isOpenURIPresented = false
    public var showsMenuBarExtra: Bool
    /// Purpose the Address Book window opens on.
    public var addressBookPurpose: AddressPurpose = .send
    public var signVerifyTab: SignVerifyTab = .sign
    /// The last `dash:` URI the app could not parse, for the alert.
    public var uriError: String?

    private var started = false

    public init(environment: AppEnvironment, launch: LaunchOptions) {
        self.launch = launch
        self.env = environment
        self.main = MainViewModel(env: environment)
        self.unavailableReason = nil
        self.showsMenuBarExtra = launch.menuBarExtra
    }

    /// The runtime could not be built; the window explains why.
    public init(unavailableReason: String, launch: LaunchOptions) {
        self.launch = launch
        self.env = nil
        self.main = nil
        self.unavailableReason = unavailableReason
        self.showsMenuBarExtra = false
    }

    public var isDemo: Bool { launch.isDemo }

    /// Loads the active network and starts following changes. Idempotent.
    public func start() async {
        guard !started, let main else { return }
        started = true
        await main.start()
        await main.settings.load()
    }

    /// A `dash:` URL from Launch Services, drag and drop or File ▸ Open URI (QT-019, QT-150).
    public func open(uri text: String) async {
        guard let main else { return }
        await main.open(uri: text)
        uriError = main.errorMessage
    }

    /// Light/dark override for every window: the launch switch wins over the setting.
    public var colorScheme: ColorScheme? {
        switch launch.appearance ?? main?.settings.theme ?? .system {
        case .light: .light
        case .dark: .dark
        case .system: nil
        }
    }

    /// Window title with the demo tag (the network tag comes from the view model).
    public var windowTitle: String {
        let title = main?.windowTitle ?? L10n.Navigation.appName
        return isDemo ? "\(title) - [\(MacStrings.App.demoBadge.lowercased())]" : title
    }

    /// Display name of the current unit (`tDASH` off mainnet).
    public var unitName: String {
        guard let env, let main else { return "" }
        return env.amounts.unitName(main.settings.display.unit)
    }

    /// `amount` with the unit name in the display unit, thin-space separated.
    public func formatAmount(_ amount: Amount) -> String {
        guard let env, let main else { return "" }
        return env.amounts.format(amount, unit: main.settings.display.unit, style: .withUnit(plusSign: false, separators: .always))
    }

    public func makeAddressBook(purpose: AddressPurpose, selectionMode: Bool = false) -> AddressBookViewModel? {
        guard let env, let network = main?.network else { return nil }
        return AddressBookViewModel(env: env, network: network, purpose: purpose, selectionMode: selectionMode)
    }

    public func makeSignVerify() -> SignVerifyViewModel? {
        guard let env, let network = main?.network else { return nil }
        return SignVerifyViewModel(env: env, network: network)
    }
}
#endif
