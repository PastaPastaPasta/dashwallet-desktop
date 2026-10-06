// Composition root of the SwiftCrossUI app (Linux/Windows; AppKitBackend when
// built on macOS for development). DefaultBackend picks GtkBackend on Linux,
// WinUIBackend on Windows and AppKitBackend on macOS.
import CrossUI
import DashUICross
import DefaultBackend
import DesignTokens
import Foundation
import PlatformServices
import PlatformServicesDesktop
import SwiftCrossUI
import WalletDemo
import WalletFeatures
import WalletRuntime

#if os(macOS)
    import AppKit
#elseif os(Linux)
    import CGtk
    import Gtk
    import GtkBackend
#endif

@main
struct DashWalletCrossApp: App {
    let options: AppLaunchOptions
    /// The window state for the menu bar and window title (QT-011, QT-015…018).
    let host = CrossAppHost()
    /// Shuts the live engine down when the app quits.
    let quitHook = QuitHook()

    init() {
        let options = AppLaunchOptions(
            arguments: CommandLine.arguments, environment: ProcessInfo.processInfo.environment)
        if options.showHelp || !options.problems.isEmpty {
            for problem in options.problems { FileHandle.standardError.write(Data("dash-wallet: \(problem)\n".utf8)) }
            FileHandle.standardError.write(Data((AppLaunchOptions.usage + "\n").utf8))
            exit(options.showHelp && options.problems.isEmpty ? 0 : 64)
        }
        self.options = options
        #if os(Linux)
            // AT-SPI and desktop shells show this instead of the executable
            // name (ADR 0002, gap A7).
            g_set_application_name(L10n.Navigation.appName)
        #endif
        if case .demo(let scenario) = options.mode {
            let network = options.networkName.flatMap(Self.runtimeNetwork) ?? .testnet
            let clipboard = AppOSServices.clipboard
            let (env, m2) = DemoEnvironment.makeWithM2(
                scenario: scenario, network: network, launchOptions: options.shell,
                platform: DemoPlatformServices(clipboard: clipboard.service))
            let state = CrossAppState(
                env: env, m2: m2, main: MainViewModel(env: env),
                capabilities: CrossPlatformCapabilities(clipboard: clipboard.available, tray: false),
                notice: CrossDemoText.notice)
            state.appUsage = AppLaunchOptions.usage
            Self.open(page: options.page, in: state)
            host.state = state
        }
        quitHook.install()
    }

    var body: some Scene {
        let host = host
        // A closure value, not a literal: SwiftCrossUI's CommandsBuilder only
        // takes CommandMenu literals, and the menus are built from the model.
        let menus: () -> Commands = { ShellMenus.commands(for: host.state, quit: { exit(0) }) }
        return WindowGroup(host.state?.shell.windowTitle ?? L10n.Navigation.appName) {
            switch options.mode {
            case .gallery:
                DashUICrossGallery()
            case .demo:
                if let state = host.state { WalletRootView(state: state) }
            case .live:
                LiveStartView(options: options, quitHook: quitHook, host: host)
            }
        }
        .defaultSize(width: 1100, height: 760)
        .commands(menus)
    }

    /// Applies `--page`: a sidebar section or one of the tool pages.
    @MainActor
    static func open(page: String?, in state: CrossAppState) {
        guard let page else { return }
        if let item = SidebarItem(rawValue: page) {
            state.main.selection = item
        } else if let tool = ToolPage(pageName: page) {
            state.open(tool)
        }
    }

    static func runtimeNetwork(_ name: String) -> DashNetwork? {
        switch name {
        case "mainnet": .mainnet
        case "testnet": .testnet
        case "regtest": .regtest
        default: name.hasPrefix("devnet-") ? .devnet(name: String(name.dropFirst("devnet-".count))) : nil
        }
    }
}

enum CrossDemoText {
    static let notice = "Demo mode: sample data, nothing is sent (passphrase: demo)"
}

/// Runs `LiveSession.shutdownBeforeExit()` on the way out:
/// - macOS (AppKitBackend): on `NSApplication.willTerminateNotification`.
/// - Linux (GtkBackend): on the GApplication's "shutdown" signal, which
///   `g_application_run` emits after the main loop ends (the last window
///   was closed or the application quit), before it returns.
///   The window's "destroy" signal is not used: in GTK 4 it is emitted only
///   when the window is disposed, and SwiftCrossUI 0.10 keeps a reference to
///   every window it created (`GtkBackend.windows`, plus the wrapper's own
///   `g_object_ref`), so a closed window is never disposed while the app
///   runs. That is why the Xvfb run of 2026-10-05 never printed
///   "engine shut down" (RESULTS.md).
/// - Windows (WinUIBackend): not hooked yet; the process exits without the
///   engine's orderly shutdown.
/// File ▸ Exit stops the engine through the shutdown page (QT-008) first;
/// the hook then has nothing left to do.
@MainActor
final class QuitHook {
    var session: LiveSession?
    private var shutdownSignalConnected = false

    func install() {
        #if os(macOS)
            NotificationCenter.default.addObserver(
                forName: NSApplication.willTerminateNotification, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.session?.shutdownBeforeExit() }
            }
        #endif
    }

    func appClosing() {
        guard let session else { return }
        FileHandle.standardError.write(Data("dash-wallet: shutting the engine down\n".utf8))
        session.shutdownBeforeExit()
    }

    #if os(Linux)
        /// Connects `appClosing()` to the "shutdown" signal of the
        /// GtkApplication that owns `window` (once). The hook stays retained
        /// by the connection for the rest of the process.
        func connectApplicationShutdown(of window: Gtk.ApplicationWindow) {
            guard !shutdownSignalConnected else { return }
            let windowPointer = UnsafeMutableRawPointer(window.widgetPointer).assumingMemoryBound(to: GtkWindow.self)
            guard let application = gtk_window_get_application(windowPointer) else { return }
            shutdownSignalConnected = true
            let handler: @convention(c) (UnsafeMutableRawPointer?, UnsafeMutableRawPointer?) -> Void = { _, data in
                guard let data else { return }
                let hook = Unmanaged<QuitHook>.fromOpaque(data).takeUnretainedValue()
                MainActor.assumeIsolated { hook.appClosing() }
            }
            g_signal_connect_data(
                application, "shutdown", unsafeBitCast(handler, to: GCallback.self),
                Unmanaged.passRetained(self).toOpaque(), nil, GConnectFlags(rawValue: 0))
        }
    #endif
}

/// Live start: the data-directory chooser when asked for (`-choosedatadir`,
/// `-resetguisettings`), then the engine and the runtime, the unreadable-
/// settings question (QT-007) and the splash (QT-005) while the last (or
/// requested) network opens, then the wallet. Failures are shown here.
struct LiveStartView: View {
    let options: AppLaunchOptions
    let quitHook: QuitHook
    let host: CrossAppHost

    @State var session: LiveSession?
    @State var chooser: DataDirectoryChooserViewModel?
    @State var dataRoot: URL?
    @State var launching = false
    @State var launchFailure: String?
    @State var failure: String?

    var body: some View {
        content.hookApplicationShutdown(quitHook)
    }

    @ViewBuilder
    private var content: some View {
        if let session {
            if case .settingsUnreadable(let files) = session.startup.stage {
                SettingsUnreadableScreen(model: session.startup, files: files, onAbort: { exit(1) })
            } else if launching, session.startup.showsSplash {
                SplashScreen(model: session.startup)
            } else {
                VStack(spacing: 0) {
                    if let launchFailure {
                        Toast(launchFailure, kind: .error).padding(Int(DashSpacing.s))
                    }
                    WalletRootView(state: session.state)
                }
            }
        } else if let chooser {
            DataDirectoryChooserScreen(model: chooser) { outcome in
                switch outcome {
                case .accepted(let url): Task { await open(root: url) }
                case .cancelled: exit(0)
                case .choosing: break
                }
            }
        } else {
            VStack(alignment: .leading, spacing: Int(DashSpacing.m)) {
                SectionHeader(L10n.Navigation.appName, style: .title2)
                if let dataRoot { KeyValueRow("Data directory", dataRoot.path) }
                if let failure {
                    Toast(failure, kind: .error)
                    Text("Run `dash-wallet --demo` to try the screens on sample data.")
                        .dashFont(.footnote)
                        .textSelectionEnabled()
                } else {
                    Text("Opening the wallet data…")
                }
                Spacer()
            }
            .padding(Int(DashSpacing.xxxl))
            .task { await start() }
        }
    }

    private func start() async {
        guard session == nil, chooser == nil, failure == nil else { return }
        if let directory = options.dataDirectory {
            await open(root: URL(fileURLWithPath: directory, isDirectory: true), fixed: true)
            return
        }
        if options.shell.chooseDataDirectory || options.shell.resetGUISettings {
            // QT-004: the default is offered as it is (not created yet); the
            // choice applies to this run, pass --datadir to reuse it.
            chooser = DataDirectoryChooserViewModel(
                defaultDirectory: DesktopDataDirectory.root(), inspector: FileSystemDataDirectoryInspector())
            return
        }
        let root: URL
        do {
            root = try DesktopDataLocation().defaultDataRoot()
        } catch {
            failure = "Could not create the data directory: \(error.localizedDescription)"
            return
        }
        await open(root: root, fixed: false)
    }

    /// Opens the engine on `root` (a `--datadir` or chooser directory is
    /// prepared first), then launches the network.
    private func open(root chosen: URL, fixed: Bool = true) async {
        var network: DashNetwork?
        if let name = options.networkName {
            guard let named = DashWalletCrossApp.runtimeNetwork(name) else {
                failure = "Unknown network \(name)."
                chooser = nil
                return
            }
            network = named
        }
        let root: URL
        do {
            if fixed {
                root = try FixedDataLocation(root: chosen).defaultDataRoot()
                try DesktopDataDirectory.prepare(root)
            } else {
                root = chosen
            }
        } catch {
            failure = "Could not create the data directory: \(error.localizedDescription)"
            chooser = nil
            return
        }
        dataRoot = root
        chooser = nil
        let session: LiveSession
        do {
            session = try LiveSession(
                dataRoot: root, network: network,
                options: NetworkOptions(dapiAddresses: options.dapiAddresses, spvPeers: options.spvPeers),
                launch: options)
        } catch {
            Self.log(error)
            failure = "The wallet engine could not open \(root.path) (\(error.code.rawValue))."
            return
        }
        quitHook.session = session
        DashWalletCrossApp.open(page: options.page, in: session.state)
        launching = true
        self.session = session
        host.state = session.state
        // Not awaited here: setting `session` replaces the view that owns this
        // `.task`, and the network must keep opening after that.
        let uris = options.shell.uris
        Task { @MainActor in
            do throws(ServiceError) {
                try await session.launch(uris: uris)
            } catch {
                Self.log(error)
                launchFailure = "The network could not be opened (\(error.code.rawValue))."
            }
            launching = false
        }
    }
}

extension LiveStartView {
    /// Writes an error's code and detail to stderr; the page shows only the code.
    static func log(_ error: ServiceError) {
        FileHandle.standardError.write(Data("dash-wallet: \(error.code.rawValue): \(error.detail)\n".utf8))
    }
}

extension View {
    /// Tells `hook` when the GTK application that owns this view's window
    /// shuts down (Linux only).
    @ViewBuilder
    func hookApplicationShutdown(_ hook: QuitHook) -> some View {
        #if os(Linux)
            inspectWindow { window in
                hook.connectApplicationShutdown(of: window)
            }
        #else
            self
        #endif
    }
}
