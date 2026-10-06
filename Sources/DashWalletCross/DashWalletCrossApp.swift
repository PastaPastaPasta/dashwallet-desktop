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
import WalletFeatures
import WalletRuntime

#if os(macOS)
    import AppKit
#elseif os(Linux)
    import CGtk
    import GtkBackend
#endif

@main
struct DashWalletCrossApp: App {
    let options: LaunchOptions
    let demoState: CrossAppState?
    /// Shuts the live engine down when the app quits.
    let quitHook = QuitHook()

    init() {
        let options = LaunchOptions(arguments: CommandLine.arguments, environment: ProcessInfo.processInfo.environment)
        if options.showHelp || !options.problems.isEmpty {
            for problem in options.problems { FileHandle.standardError.write(Data("dash-wallet: \(problem)\n".utf8)) }
            FileHandle.standardError.write(Data((LaunchOptions.usage + "\n").utf8))
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
            let env = DemoEnvironment.make(network: network, scenario: scenario)
            let main = MainViewModel(env: env)
            Self.open(page: options.page, in: main)
            demoState = CrossAppState(env: env, main: main, notice: CrossDemoText.notice)
        } else {
            demoState = nil
        }
        quitHook.install()
    }

    var body: some Scene {
        WindowGroup(L10n.Navigation.appName) {
            switch options.mode {
            case .gallery:
                DashUICrossGallery()
            case .demo:
                if let demoState { WalletRootView(state: demoState) }
            case .live:
                LiveStartView(options: options, quitHook: quitHook)
            }
        }
        .defaultSize(width: 1100, height: 760)
    }

    /// Applies `--page`.
    static func open(page: String?, in main: MainViewModel) {
        switch page {
        case nil: break
        case "address-book": main.sheet = .sendingAddresses
        case "sign-verify": main.sheet = .signMessage
        case "settings": main.sheet = .settings
        case let name?:
            if let item = SidebarItem(rawValue: name) { main.selection = item }
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
/// - Linux (GtkBackend): when the window is destroyed. Closing the last
///   window ends the GTK application right after that.
/// - Windows (WinUIBackend): not hooked yet; the process exits without the
///   engine's orderly shutdown.
@MainActor
final class QuitHook {
    var session: LiveSession?

    func install() {
        #if os(macOS)
            NotificationCenter.default.addObserver(
                forName: NSApplication.willTerminateNotification, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.session?.shutdownBeforeExit() }
            }
        #endif
    }

    func windowDestroyed() {
        session?.shutdownBeforeExit()
    }
}

/// Live start: resolves the data directory, opens the engine and builds the
/// runtime, then shows the wallet while the last (or requested) network
/// opens. Failures are shown on this page.
struct LiveStartView: View {
    let options: LaunchOptions
    let quitHook: QuitHook

    @State var state: CrossAppState?
    @State var dataRoot: URL?
    @State var launchFailure: String?
    @State var failure: String?

    var body: some View {
        content.hookWindowDestroy(quitHook)
    }

    @ViewBuilder
    private var content: some View {
        if let state {
            VStack(spacing: 0) {
                if let launchFailure {
                    Toast(launchFailure, kind: .error).padding(Int(DashSpacing.s))
                }
                WalletRootView(state: state)
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
        guard state == nil, failure == nil else { return }
        var network: DashNetwork?
        if let name = options.networkName {
            guard let named = DashWalletCrossApp.runtimeNetwork(name) else {
                failure = "Unknown network \(name)."
                return
            }
            network = named
        }
        let root: URL
        do {
            if let directory = options.dataDirectory {
                root = try FixedDataLocation(root: URL(fileURLWithPath: directory, isDirectory: true)).defaultDataRoot()
                try DesktopDataDirectory.prepare(root)
            } else {
                root = try DesktopDataLocation().defaultDataRoot()
            }
        } catch {
            failure = "Could not create the data directory: \(error.localizedDescription)"
            return
        }
        dataRoot = root
        let session: LiveSession
        do {
            session = try LiveSession(
                dataRoot: root, network: network,
                options: NetworkOptions(dapiAddresses: options.dapiAddresses, spvPeers: options.spvPeers))
        } catch {
            Self.log(error)
            failure = "The wallet engine could not open \(root.path) (\(error.code.rawValue))."
            return
        }
        quitHook.session = session
        DashWalletCrossApp.open(page: options.page, in: session.state.main)
        state = session.state
        do {
            try await session.launch()
        } catch {
            Self.log(error)
            launchFailure = "The network could not be opened (\(error.code.rawValue))."
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
    /// Tells `hook` when the GTK window is destroyed (Linux only).
    @ViewBuilder
    func hookWindowDestroy(_ hook: QuitHook) -> some View {
        #if os(Linux)
            inspectWindow { window in
                window.onDestroy = { [weak hook] _ in
                    MainActor.assumeIsolated { hook?.windowDestroyed() }
                }
            }
        #else
            self
        #endif
    }
}
