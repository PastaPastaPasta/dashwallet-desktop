// Command-line options of `dash-wallet` (SwiftCrossUI app). GTK does not
// parse argv here (SwiftCrossUI runs the GApplication with no arguments).
// The app's own options are read first; everything else goes to WalletRuntime's
// LaunchArgumentsParser, which applies dash-qt's rules (QT-006): `-testnet`,
// `-datadir=<dir>`, `-choosedatadir`, `-windowtitle=<name>`, `-splash`,
// `-resetguisettings`, `-min`, `-lang`, trailing `dash:` URIs.
import CrossUI
import Foundation
import WalletDemo
import WalletFeatures
import WalletRuntime

struct AppLaunchOptions: Sendable, Equatable {
    enum Mode: Sendable, Equatable {
        /// The engine-backed wallet.
        case live
        /// Sample data, nothing leaves the process.
        case demo(DemoScenario)
        /// DashUICross component gallery.
        case gallery
    }

    var mode: Mode = .live
    /// `--network <mainnet|testnet|regtest|devnet-NAME>`, or dash-qt's
    /// `-testnet`/`-regtest`/`-devnet=`/`-chain=`. Unset: the live app
    /// reopens the last network (first run: mainnet); the demo uses testnet.
    var networkName: String?
    /// `--connect HOST:PORT` (repeatable): SPV peers instead of DNS seeds.
    var spvPeers: [String] = []
    /// `--dapi URL` (repeatable): DAPI endpoints; regtest and devnets have no defaults.
    var dapiAddresses: [String] = []
    /// `--datadir <path>` (or dash-qt's `-datadir=<path>`) overrides the per-OS data root.
    var dataDirectory: String?
    /// `--page <name>`: the page shown first (screenshots, smoke tests).
    var page: String?
    /// `--appearance light|dark`: forces the window's appearance over the
    /// Theme setting (screenshots of both appearances).
    var appearance: AppTheme?
    var showHelp = false
    var problems: [String] = []
    /// dash-qt's options, as WalletRuntime parsed them.
    var shell = LaunchOptions()

    static let usage = """
        Usage: dash-wallet [--demo [funded|locked|onboarding|offline]] [--gallery]
                           [--network mainnet|testnet|regtest|devnet-NAME] [--datadir PATH]
                           [--connect HOST:PORT]... [--dapi URL]... [--page NAME]
                           [--appearance light|dark]
                           [dash-qt options] [URI]
          --demo       run on in-memory sample data (DWD_DEMO=1 does the same);
                       locked asks for the passphrase "demo", onboarding starts
                       with no wallet, offline has no peers and is syncing
          --gallery    show the DashUICross component gallery
          --network    network to open (default: the last one opened, first run
                       mainnet; the demo uses testnet)
          --datadir    data directory root (default: $XDG_DATA_HOME/dashwallet on
                       Linux, %APPDATA%\\Dash\\DashWallet on Windows)
          --connect    SPV peer to use instead of DNS seeds (repeatable)
          --dapi       DAPI endpoint (repeatable; needed for regtest and devnets)
          --page       first page: \(pages.joined(separator: ", "))
          --appearance light or dark, over the Theme setting (default: the setting)
          dash-qt options: -testnet -regtest -devnet=NAME -chain=CHAIN -datadir=DIR
                       -choosedatadir -windowtitle=NAME -splash=0 -resetguisettings
                       -min -lang=LANG (see Help ▸ Command-line options)
        """

    static let pages = SidebarItem.allCases.prefix(4).map(\.rawValue) + ToolPage.pageNames

    init() {}

    init(arguments: [String], environment: [String: String], parser: any LaunchArgumentsParsing = LaunchArgumentsParser()) {
        if environment["DWD_DEMO"] == "1" { mode = .demo(.funded) }
        var iterator = arguments.dropFirst().makeIterator()
        var pending: String?
        var forwarded: [String] = []
        func next() -> String? {
            if let value = pending {
                pending = nil
                return value
            }
            return iterator.next()
        }
        while let argument = next() {
            switch argument {
            case "--demo":
                mode = .demo(.funded)
                if let value = iterator.next() {
                    if let scenario = DemoScenario(name: value) { mode = .demo(scenario) } else { pending = value }
                }
            case "--gallery":
                mode = .gallery
            case "--network":
                if let value = next() { networkName = value } else { problems.append("--network needs a value") }
            case "--connect":
                if let value = next() { spvPeers.append(value) } else { problems.append("--connect needs a value") }
            case "--dapi":
                if let value = next() { dapiAddresses.append(value) } else { problems.append("--dapi needs a value") }
            case "--datadir":
                if let value = next() { dataDirectory = value } else { problems.append("--datadir needs a value") }
            case "--page":
                if let value = next(), Self.pages.contains(value) {
                    page = value
                } else {
                    problems.append("--page needs one of: \(Self.pages.joined(separator: ", "))")
                }
            case "--appearance":
                switch next() {
                case "light": appearance = .light
                case "dark": appearance = .dark
                default: problems.append("--appearance needs light or dark")
                }
            case "--help", "-h":
                showHelp = true
            default:
                forwarded.append(argument)
            }
        }
        do {
            shell = try parser.parse(forwarded)
        } catch {
            problems.append("\(error.detail): \(Self.text(for: error.code))")
            return
        }
        if shell.showHelp { showHelp = true }
        if shell.showVersion {
            // The version comes from the engine, which is not open while the
            // arguments are read; Help ▸ About shows it.
            problems.append("-version is not supported; Help ▸ About shows the version")
        }
        if networkName == nil, let network = shell.network { networkName = Self.name(of: network) }
        if dataDirectory == nil, let directory = shell.dataDirectory { dataDirectory = directory.path }
    }

    static func text(for code: ServiceErrorCode) -> String {
        switch code {
        case .launchUnknownOption: "unknown option"
        case .launchInvalidValue: "invalid value"
        case .launchOptionAfterURI: "options may not follow a URI"
        default: code.rawValue
        }
    }

    static func name(of network: DashNetwork) -> String {
        switch network {
        case .mainnet: "mainnet"
        case .testnet: "testnet"
        case .regtest: "regtest"
        case .devnet(let name): "devnet-\(name)"
        }
    }
}
