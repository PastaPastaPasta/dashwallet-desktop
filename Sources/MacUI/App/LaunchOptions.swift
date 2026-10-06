// Command-line switches of the macOS app.
#if os(macOS)
import Foundation
import WalletDemo
import WalletFeatures
import WalletRuntime

/// What the app was asked to run.
public struct LaunchOptions: Sendable, Equatable {
    /// `--demo` / `--fixture` (funded) or `--demo-scenario <name>`; `nil` runs the real wallet.
    public var demoScenario: DemoScenario?
    /// `--appearance light|dark` overrides the theme setting (screenshots).
    public var appearance: AppTheme?
    /// `--no-menu-bar-extra` hides the menu bar companion (UI tests).
    public var menuBarExtra: Bool
    /// `--datadir <path>`: the live runtime's data root instead of
    /// `~/Library/Application Support/org.dashfoundation.DashWallet/`.
    public var dataDirectory: URL?
    /// `--network mainnet|testnet|regtest`: the network opened when no
    /// network was used before (afterwards the last network wins).
    public var network: DashNetwork?
    /// How every network is reached: `--peer host:port` (SPV peer, repeatable,
    /// like dash-qt's `-connect`), `--dapi <url>` (repeatable) and
    /// `--quorum-url <url>`. Empty means the network's defaults; regtest and
    /// devnets have none and need them.
    public var networkOptions: NetworkOptions

    public init(
        demoScenario: DemoScenario? = nil, appearance: AppTheme? = nil, menuBarExtra: Bool = true,
        dataDirectory: URL? = nil, network: DashNetwork? = nil, networkOptions: NetworkOptions = NetworkOptions()
    ) {
        self.demoScenario = demoScenario
        self.appearance = appearance
        self.menuBarExtra = menuBarExtra
        self.dataDirectory = dataDirectory
        self.network = network
        self.networkOptions = networkOptions
    }

    public var isDemo: Bool { demoScenario != nil }

    /// Reads the switches above; unknown arguments (Xcode's `-NS…` pairs,
    /// file paths) are ignored. `DWD_DEMO=1` in the environment also selects demo mode.
    public static func parse(_ arguments: [String], environment: [String: String] = [:]) -> LaunchOptions {
        var options = LaunchOptions()
        if environment["DWD_DEMO"] == "1" { options.demoScenario = .funded }
        var index = 1
        while index < arguments.count {
            let argument = arguments[index]
            let value = index + 1 < arguments.count ? arguments[index + 1] : nil
            switch argument {
            case "--demo", "--fixture":
                options.demoScenario = options.demoScenario ?? .funded
            case "--demo-scenario":
                if let value, let scenario = DemoScenario(name: value) {
                    options.demoScenario = scenario
                    index += 1
                }
            case "--appearance":
                if let value, let theme = AppTheme(rawValue: value) {
                    options.appearance = theme
                    index += 1
                }
            case "--no-menu-bar-extra":
                options.menuBarExtra = false
            case "--datadir":
                if let value, !value.isEmpty {
                    options.dataDirectory = URL(fileURLWithPath: (value as NSString).expandingTildeInPath, isDirectory: true)
                    index += 1
                }
            case "--peer", "--dapi", "--quorum-url":
                if let value, !value.isEmpty {
                    switch argument {
                    case "--peer": options.networkOptions.spvPeers.append(value)
                    case "--dapi": options.networkOptions.dapiAddresses.append(value)
                    default: options.networkOptions.quorumURL = value
                    }
                    index += 1
                }
            case "--network":
                if let value, let network = Self.network(named: value) {
                    options.network = network
                    index += 1
                }
            default:
                break
            }
            index += 1
        }
        return options
    }

    /// The networks `--network` accepts; devnets need a name and are not offered.
    static func network(named name: String) -> DashNetwork? {
        switch name.lowercased() {
        case "mainnet", "main": .mainnet
        case "testnet", "test": .testnet
        case "regtest": .regtest
        default: nil
        }
    }
}
#endif
