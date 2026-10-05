// Composition root of the SwiftCrossUI app (Linux/Windows; AppKitBackend when
// built on macOS for development). DefaultBackend picks GtkBackend on Linux,
// WinUIBackend on Windows and AppKitBackend on macOS.
import CrossUI
import DashUICross
import DefaultBackend
import DesignTokens
import Foundation
import PlatformServicesDesktop
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

@main
struct DashWalletCrossApp: App {
    let options: LaunchOptions
    let demoState: CrossAppState?

    init() {
        let options = LaunchOptions(arguments: CommandLine.arguments, environment: ProcessInfo.processInfo.environment)
        if options.showHelp || !options.problems.isEmpty {
            for problem in options.problems { FileHandle.standardError.write(Data("dash-wallet: \(problem)\n".utf8)) }
            FileHandle.standardError.write(Data((LaunchOptions.usage + "\n").utf8))
            exit(options.showHelp && options.problems.isEmpty ? 0 : 64)
        }
        self.options = options
        if case .demo(let scenario) = options.mode {
            let network = Self.runtimeNetwork(options.networkName) ?? .testnet
            let env = DemoEnvironment.make(network: network, scenario: scenario)
            let main = MainViewModel(env: env)
            Self.open(page: options.page, in: main)
            demoState = CrossAppState(env: env, main: main, notice: CrossDemoText.notice)
        } else {
            demoState = nil
        }
    }

    var body: some Scene {
        WindowGroup(L10n.Navigation.appName) {
            switch options.mode {
            case .gallery:
                DashUICrossGallery()
            case .demo:
                if let demoState { WalletRootView(state: demoState) }
            case .live:
                LiveStartView(options: options)
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

/// Live start: opens the engine on the data directory, then builds the live
/// environment. Until the runtime adapters exist that second step fails with
/// `notImplemented`, and this view says so instead of showing the wallet.
struct LiveStartView: View {
    let options: LaunchOptions

    @State var report: LiveEngineReport?
    @State var state: CrossAppState?
    @State var failure: String?

    var body: some View {
        if let state {
            WalletRootView(state: state)
        } else {
            VStack(alignment: .leading, spacing: Int(DashSpacing.m)) {
                SectionHeader(L10n.Navigation.appName, style: .title2)
                if let report {
                    KeyValueRow("Data directory", report.dataRoot.path)
                    KeyValueRow("Network", report.networkName)
                    KeyValueRow("Engine", report.coreVersion)
                    KeyValueRow("Wallets on disk", report.walletCount.map(String.init) ?? L10n.Common.unknown)
                    if let engineFailure = report.failure {
                        Toast("The engine could not open the wallet data: \(engineFailure)", kind: .error)
                    }
                } else {
                    Text("Opening the wallet data…")
                }
                if let failure {
                    Toast(failure, kind: .warning)
                    Text("Run `dash-wallet --demo` to try the screens on sample data.")
                        .dashFont(.footnote)
                        .textSelectionEnabled()
                }
                Spacer()
            }
            .padding(Int(DashSpacing.xxxl))
            .task { await start() }
        }
    }

    private func start() async {
        guard report == nil else { return }
        guard let network = DashWalletCrossApp.runtimeNetwork(options.networkName),
            let engineNetwork = LiveEngineProbe.network(named: options.networkName)
        else {
            failure = "Unknown network \(options.networkName)."
            return
        }
        let root = options.dataDirectory.map { URL(fileURLWithPath: $0, isDirectory: true) }
            ?? DesktopDataDirectory.root()
        do {
            try DesktopDataDirectory.prepare(root)
        } catch {
            failure = "Could not create \(root.path): \(error.localizedDescription)"
            return
        }
        report = await LiveEngineProbe.run(dataRoot: root, network: engineNetwork)
        do {
            let env = try LiveEnvironment.make(dataRoot: root, network: network)
            let main = MainViewModel(env: env)
            DashWalletCrossApp.open(page: options.page, in: main)
            state = CrossAppState(env: env, main: main)
        } catch {
            failure = "The wallet screens need the WalletRuntime adapters, which this build does not have yet (\(error.code))."
        }
    }
}
