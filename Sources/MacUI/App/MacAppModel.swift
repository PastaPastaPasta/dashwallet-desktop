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
    /// The menu bar companion. Changes (the Settings toggle, or the user
    /// dragging the item out of the menu bar) are saved in the UI
    /// preferences; `--no-menu-bar-extra` hides it for this run without
    /// saving anything.
    public var showsMenuBarExtra: Bool {
        get { menuBarExtraShown }
        set {
            menuBarExtraShown = newValue
            saveMenuBarExtra(newValue)
        }
    }
    private var menuBarExtraShown: Bool
    /// Why saving a preference failed (shown in Settings).
    public private(set) var preferencesError: String?
    /// Purpose the Address Book window opens on.
    public var addressBookPurpose: AddressPurpose = .send
    public var signVerifyTab: SignVerifyTab = .sign
    /// The last `dash:` URI the app could not parse, for the alert.
    public var uriError: String?
    /// The Peers sheet (status bar connections item, QT-024).
    public var isPeersPresented = false
    /// The user hid the sync overlay; it stays hidden until asked for again.
    public var syncOverlayHidden = false
    /// The user asked for the sync overlay (status bar sync item).
    public var syncOverlayRequested = false
    /// Sync rates for the overlay, fed from the sync status.
    let syncRates = SyncRateTracker()
    /// Why opening the network at launch failed (`nil` while it works).
    public private(set) var launchError: ServiceError?

    @ObservationIgnored private let lifecycle: RuntimeLifecycle?
    @ObservationIgnored private var started = false
    @ObservationIgnored private var shutDown = false

    /// - Parameter lifecycle: starts and stops the services; `nil` when they
    ///   run without one (demo).
    public init(environment: AppEnvironment, launch: LaunchOptions, lifecycle: RuntimeLifecycle? = nil) {
        self.launch = launch
        self.env = environment
        self.main = MainViewModel(env: environment)
        self.unavailableReason = nil
        self.menuBarExtraShown = launch.menuBarExtra && environment.preferences.preferences.showsMenuBarExtra != false
        self.lifecycle = lifecycle
    }

    /// The runtime could not be built; the window explains why.
    public init(unavailableReason: String, launch: LaunchOptions) {
        self.launch = launch
        self.env = nil
        self.main = nil
        self.unavailableReason = unavailableReason
        self.menuBarExtraShown = false
        self.lifecycle = nil
    }

    public var isDemo: Bool { launch.isDemo }

    private func saveMenuBarExtra(_ shown: Bool) {
        guard let preferences = env?.preferences else { return }
        var stored = preferences.preferences
        guard (stored.showsMenuBarExtra ?? true) != shown else { return }
        stored.showsMenuBarExtra = shown
        do {
            try preferences.update(stored)
            preferencesError = nil
        } catch {
            preferencesError = ErrorText.common(error.code)
        }
    }

    /// Starts following the runtime, then opens the last network (live
    /// runtime only). The view models observe the lifecycle first, so the
    /// "Starting…" overlay shows while the network opens. Idempotent.
    public func start() async {
        guard !started, let main else { return }
        started = true
        await main.start()
        await main.settings.load()
        await openNetwork()
    }

    /// Retries opening the network after `launchError`.
    public func retryLaunch() async {
        guard started, launchError != nil else { return }
        await openNetwork()
    }

    /// Stops the observers and releases the engine (stops SPV and closes the
    /// session first). Called once when the app quits; later calls do nothing.
    public func shutdown() async {
        guard !shutDown else { return }
        shutDown = true
        main?.stop()
        do {
            try await lifecycle?.shutdown()
        } catch {
            // The process is exiting; the engine drops what is left.
            launchError = error
        }
    }

    private func openNetwork() async {
        guard let lifecycle else { return }
        do {
            try await lifecycle.launch()
            launchError = nil
        } catch {
            launchError = error
        }
    }

    /// A `dash:` URL from Launch Services, drag and drop or File ▸ Open URI (QT-019, QT-150).
    public func open(uri text: String) async {
        guard let main else { return }
        await main.open(uri: text)
        uriError = main.errorMessage
    }

    /// The sync overlay (QT-027): while catching up, when asked for, or by
    /// itself while the tip is more than 25 minutes old unless hidden.
    public var showsSyncOverlay: Bool {
        guard let main, !main.needsOnboarding, !main.showsLockScreen, let status = main.home?.sync,
              status.running, !status.isDone else { return false }
        return syncOverlayRequested || (!syncOverlayHidden && SyncRateTracker.tipIsOld(status))
    }

    public func hideSyncOverlay() {
        syncOverlayHidden = true
        syncOverlayRequested = false
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
