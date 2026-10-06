// The macOS app's root state: the composition-root environment, the main
// view model, the M2 view models and the window-level choices menus make.
#if os(macOS)
import Foundation
import Observation
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

/// Scene identifiers for `openWindow(id:)`.
public enum SceneID {
    public static let main = "main"
    public static let addressBook = "address-book"
    public static let signVerify = "sign-verify"
    public static let about = "about"
    public static let options = "options"
    public static let tools = "tools"
    public static let coinControl = "coin-control"
    public static let psbt = "psbt"
    public static let wallets = "wallets"
    public static let addWallet = "add-wallet"
    public static let commandLine = "command-line"
    public static let shutdown = "shutdown"
}

public enum SignVerifyTab: Hashable, Sendable {
    case sign, verify
}

/// Builds the live services once the data directory is known (QT-004).
public typealias LiveServicesFactory = @MainActor (URL) throws(ServiceError) -> MacAppServices

/// What the composition root hands the model: the environment, the M2
/// services and how to start and stop them.
@MainActor
public struct MacAppServices {
    public var environment: AppEnvironment
    public var m2: M2Services?
    public var lifecycle: RuntimeLifecycle?

    public init(environment: AppEnvironment, m2: M2Services?, lifecycle: RuntimeLifecycle?) {
        self.environment = environment
        self.m2 = m2
        self.lifecycle = lifecycle
    }
}

@MainActor
@Observable
public final class MacAppModel {
    public let launch: LaunchOptions
    /// `nil` when no wallet runtime could be built (`unavailableReason` says
    /// why) or while the data-directory chooser is open.
    public private(set) var env: AppEnvironment?
    public private(set) var main: MainViewModel?
    /// The M2 view models; `nil` without M2 services.
    public private(set) var features: MacFeatureModels?
    public private(set) var unavailableReason: String?
    /// The first-run data-directory chooser (QT-004), before the runtime exists.
    public private(set) var dataDirectoryChooser: DataDirectoryChooserViewModel?

    public var shell: ShellModel? { features?.shell }

    public var isOpenURIPresented = false
    /// The menu bar companion. Changes (the Options toggle, or the user
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
    /// Why saving a preference failed (shown in Options).
    public private(set) var preferencesError: String?
    /// Purpose the Address Book window opens on.
    public var addressBookPurpose: AddressPurpose = .send
    public var signVerifyTab: SignVerifyTab = .sign
    /// The tab the Tools window shows (Window ▸ Information/Console/Peers/Repair).
    public var toolsTab: ToolsTab = .information
    /// The last `dash:` URI the app could not parse, for the alert.
    public var uriError: String?
    /// The Peers sheet (status bar connections item, QT-024).
    public var isPeersPresented = false
    /// Settings ▸ Unlock Wallet (QT-016).
    public var isUnlockPresented = false
    /// File ▸ Backup Wallet (QT-110).
    public var isBackupPresented = false
    /// Why opening the network at launch failed (`nil` while it works).
    public private(set) var launchError: ServiceError?
    /// File ▸ Create Wallet: the onboarding flow over the existing vault.
    public private(set) var addWallet: OnboardingViewModel?

    @ObservationIgnored private var lifecycle: RuntimeLifecycle?
    @ObservationIgnored private var liveFactory: LiveServicesFactory?
    @ObservationIgnored private var started = false
    @ObservationIgnored private var shutDown = false
    /// SwiftUI's window opener, captured from the first window that appears,
    /// so menu, Dock and AppKit callbacks can open scenes.
    @ObservationIgnored public var windowOpener: OpenWindowAction?

    /// - Parameter lifecycle: starts and stops the services; `nil` when they
    ///   run without one (demo).
    public init(
        environment: AppEnvironment, m2: M2Services? = nil, launch: LaunchOptions, lifecycle: RuntimeLifecycle? = nil
    ) {
        self.launch = launch
        self.menuBarExtraShown = launch.menuBarExtra && environment.preferences.preferences.showsMenuBarExtra != false
        install(MacAppServices(environment: environment, m2: m2, lifecycle: lifecycle))
    }

    /// The runtime could not be built; the window explains why.
    public init(unavailableReason: String, launch: LaunchOptions) {
        self.launch = launch
        self.unavailableReason = unavailableReason
        self.menuBarExtraShown = false
    }

    /// First run (or `-choosedatadir`): the data-directory chooser opens
    /// first; `factory` builds the live services for the chosen directory.
    public init(chooser: DataDirectoryChooserViewModel, launch: LaunchOptions, factory: @escaping LiveServicesFactory) {
        self.launch = launch
        self.dataDirectoryChooser = chooser
        self.liveFactory = factory
        self.menuBarExtraShown = false
    }

    private func install(_ services: MacAppServices) {
        env = services.environment
        lifecycle = services.lifecycle
        main = MainViewModel(env: services.environment, m2: services.m2)
        features = services.m2.map { MacFeatureModels(env: services.environment, m2: $0) }
        unavailableReason = nil
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

    // MARK: Lifecycle

    /// Starts following the runtime, then opens the last network (live
    /// runtime only). The view models observe the lifecycle first, so the
    /// "Starting…" overlay shows while the network opens. Idempotent; waits
    /// while the data-directory chooser is open.
    public func start() async {
        guard !started, let main else { return }
        started = true
        await main.start()
        await main.settings.load()
        await features?.start()
        if let error = launch.argumentError {
            uriError = ErrorText.m2(error.code)
        }
        if launch.runtime.showHelp || launch.runtime.showVersion {
            await presentNow(.commandLineOptions)
        }
        await openNetwork()
        // `dash:` URIs handed over on the command line (QT-019).
        for uri in launch.runtime.uris where uri.lowercased().hasPrefix("dash:") {
            await open(uri: uri)
        }
    }

    /// dash-qt's Dock menu (QT-029): Show / Hide, Send, Receive, Sign and
    /// Verify message, Options and the Tools window tabs.
    public var dockMenuItems: [MacDockMenuItem] {
        guard main != nil else { return [] }
        let hasWallet = !(main?.needsOnboarding ?? true)
        return [
            MacDockMenuItem(title: MacStrings.Dock.showHide) { [weak self] in self?.perform(.showHideWindow) },
            .separator,
            MacDockMenuItem(title: MacStrings.MenuBar.send, isEnabled: hasWallet) { [weak self] in
                self?.perform(.section(.send))
            },
            MacDockMenuItem(title: MacStrings.Dock.receive, isEnabled: hasWallet) { [weak self] in
                self?.perform(.section(.receive))
            },
            .separator,
            MacDockMenuItem(title: L10n.Shell.signMessage, isEnabled: isEnabled(.signMessage)) { [weak self] in
                self?.perform(.signMessage)
            },
            MacDockMenuItem(title: L10n.Shell.verifyMessage, isEnabled: isEnabled(.verifyMessage)) { [weak self] in
                self?.perform(.verifyMessage)
            },
            .separator,
            MacDockMenuItem(title: L10n.Shell.options, isEnabled: features != nil) { [weak self] in
                self?.perform(.options)
            },
            MacDockMenuItem(title: L10n.Shell.information, isEnabled: features != nil) { [weak self] in
                self?.perform(.tools(.information))
            },
            MacDockMenuItem(title: L10n.Shell.console, isEnabled: features != nil) { [weak self] in
                self?.perform(.tools(.console))
            },
            MacDockMenuItem(title: L10n.Shell.peers, isEnabled: features != nil) { [weak self] in
                self?.perform(.tools(.peers))
            },
            MacDockMenuItem(title: L10n.Shell.repair, isEnabled: features != nil) { [weak self] in
                self?.perform(.tools(.repair))
            },
        ]
    }

    /// The chooser's OK: builds the live services for the directory, stores
    /// the choice, and starts.
    public func acceptDataDirectory(_ directory: URL, remember: (URL) -> Void) async {
        guard let factory = liveFactory else { return }
        do {
            let services = try factory(directory)
            remember(directory)
            dataDirectoryChooser = nil
            liveFactory = nil
            menuBarExtraShown = launch.menuBarExtra && services.environment.preferences.preferences.showsMenuBarExtra != false
            install(services)
            await start()
        } catch {
            unavailableReason = MacAppComposition.unavailableText(error, dataDirectory: directory)
            dataDirectoryChooser = nil
        }
    }

    /// Retries opening the network after `launchError`.
    public func retryLaunch() async {
        guard started, launchError != nil else { return }
        await openNetwork()
    }

    /// Stops the observers and releases the engine (stops SPV and closes the
    /// session first) behind the shutdown window (QT-008). Called once when
    /// the app quits; later calls do nothing.
    public func shutdown() async {
        guard !shutDown else { return }
        shutDown = true
        main?.stop()
        features?.stop()
        if let features, lifecycle != nil {
            windowOpener?(id: SceneID.shutdown)
            await features.shutdown.quit()
            return
        }
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

    // MARK: Shell commands (QT-015…018, QT-029)

    /// Runs a menu, Dock or menu-bar command through the shell model and
    /// presents what it asks for.
    public func perform(_ command: ShellCommand) {
        Task { await performNow(command) }
    }

    func performNow(_ command: ShellCommand) async {
        if case .section(let item) = command {
            openWindow(SceneID.main)
            main?.selection = item
            return
        }
        guard let shell else {
            present(command)
            return
        }
        await shell.perform(command)
        // Close Wallet / Close All ask on the main window; errors show there.
        if shell.confirmation != nil || shell.errorMessage != nil { openWindow(SceneID.main) }
        if let pending = shell.pendingPresentation {
            shell.presentationHandled()
            await presentNow(pending)
        }
    }

    private func present(_ command: ShellCommand) {
        Task { await presentNow(command) }
    }

    /// Opens the window, sheet or panel `command` stands for.
    func presentNow(_ command: ShellCommand) async {
        switch command {
        case .createWallet:
            guard let env else { return }
            let onboarding = OnboardingViewModel(env: env)
            addWallet = onboarding
            await onboarding.load()
            openWindow(SceneID.addWallet)
        case .backupWallet:
            openWindow(SceneID.main)
            isBackupPresented = true
        case .restoreWallet:
            guard let wallets = features?.wallets,
                let url = await MacOpenPanel.chooseFile(title: L10n.Shell.restoreWallet)
            else { return }
            openWindow(SceneID.wallets)
            await wallets.importFile(url)
        case .openURI:
            openWindow(SceneID.main)
            isOpenURIPresented = true
        case .signMessage:
            signVerifyTab = .sign
            openWindow(SceneID.signVerify)
        case .verifyMessage:
            signVerifyTab = .verify
            openWindow(SceneID.signVerify)
        case .loadPSBTFromFile:
            guard let psbt = features?.psbt,
                let url = await MacOpenPanel.chooseFile(title: L10n.Shell.loadPSBTFromFileTip)
            else { return }
            await psbt.load(file: url)
            openWindow(SceneID.psbt)
        case .loadPSBTFromClipboard:
            guard let psbt = features?.psbt else { return }
            await psbt.loadFromClipboard()
            openWindow(SceneID.psbt)
        case .encryptWallet:
            openWindow(SceneID.main)
            main?.sheet = .encryptWallet
        case .changePassphrase:
            openWindow(SceneID.main)
            main?.sheet = .changePassphrase
        case .showRecoveryPhrase:
            openWindow(SceneID.main)
            main?.sheet = .showRecoveryPhrase
        case .unlockWallet:
            openWindow(SceneID.main)
            isUnlockPresented = true
        case .lockWallet:
            await main?.lock.lock()
        case .toggleDiscreetMode:
            guard let settings = main?.settings else { return }
            settings.setDiscreet(!settings.display.hideBalances)
        case .options:
            openWindow(SceneID.options)
        case .minimize:
            MacApplication.minimizeKeyWindow()
        case .sendingAddresses:
            addressBookPurpose = .send
            openWindow(SceneID.addressBook)
        case .receivingAddresses:
            addressBookPurpose = .receive
            openWindow(SceneID.addressBook)
        case .tools(let tab):
            toolsTab = tab
            openWindow(SceneID.tools)
        case .commandLineOptions:
            openWindow(SceneID.commandLine)
        case .about:
            openWindow(SceneID.about)
        case .showHideWindow:
            MacApplication.toggleVisibility()
        case .exit:
            MacApplication.terminate()
        case .section(let item):
            openWindow(SceneID.main)
            main?.selection = item
        case .openWallet, .closeWallet, .closeAllWallets, .showAutomaticBackups, .migrateWallet, .openDebugLog,
            .openConfigurationFile, .coinJoinInformation:
            // The shell runs these itself, or they are disabled (no
            // migration, no single debug log, no dash.conf, no CoinJoin yet).
            break
        }
    }

    /// Whether `command` can run now; without the M2 shell only the M1
    /// commands the app always had are enabled.
    public func isEnabled(_ command: ShellCommand) -> Bool {
        shell?.isEnabled(command) ?? false
    }

    private func openWindow(_ id: String) {
        windowOpener?(id: id)
        if id != SceneID.shutdown { MacApplication.activate() }
    }

    /// File ▸ Create Wallet finished or was closed.
    public func addWalletClosed() {
        addWallet = nil
    }

    // MARK: Display

    /// Light/dark override for every window: the launch switch wins over the setting.
    public var colorScheme: ColorScheme? {
        switch launch.appearance ?? main?.settings.theme ?? .system {
        case .light: .light
        case .dark: .dark
        case .system: nil
        }
    }

    /// `Dash Wallet - <--windowtitle> - <wallet> - [network]` (QT-011) with
    /// the demo tag.
    public var windowTitle: String {
        let title = shell?.windowTitle ?? main?.windowTitle ?? L10n.Navigation.appName
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
