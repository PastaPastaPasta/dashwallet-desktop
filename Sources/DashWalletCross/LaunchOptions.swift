// Command-line options of `dash-wallet` (SwiftCrossUI app). GTK does not
// parse argv here (SwiftCrossUI runs the GApplication with no arguments).
import Foundation

struct LaunchOptions: Sendable, Equatable {
    enum Mode: Sendable, Equatable {
        /// The engine-backed wallet.
        case live
        /// Sample data, nothing leaves the process.
        case demo(DemoScenario)
        /// DashUICross component gallery.
        case gallery
    }

    var mode: Mode = .live
    /// `--network <mainnet|testnet|regtest|devnet-NAME>`. Unset: the live app
    /// reopens the last network (first run: mainnet); the demo uses testnet.
    var networkName: String?
    /// `--connect HOST:PORT` (repeatable): SPV peers instead of DNS seeds.
    var spvPeers: [String] = []
    /// `--dapi URL` (repeatable): DAPI endpoints; regtest and devnets have no defaults.
    var dapiAddresses: [String] = []
    /// `--datadir <path>` overrides the per-OS data root.
    var dataDirectory: String?
    /// `--page <name>`: the page shown first (screenshots, smoke tests).
    var page: String?
    var showHelp = false
    var problems: [String] = []

    static let usage = """
        Usage: dash-wallet [--demo [funded|locked|onboarding]] [--gallery]
                           [--network mainnet|testnet|regtest|devnet-NAME] [--datadir PATH]
                           [--connect HOST:PORT]... [--dapi URL]... [--page NAME]
          --demo       run on in-memory sample data (DWD_DEMO=1 does the same)
          --gallery    show the DashUICross component gallery
          --network    network to open (default: the last one opened, first run
                       mainnet; the demo uses testnet)
          --datadir    data directory root (default: $XDG_DATA_HOME/dashwallet on
                       Linux, %APPDATA%\\Dash\\DashWallet on Windows)
          --connect    SPV peer to use instead of DNS seeds (repeatable)
          --dapi       DAPI endpoint (repeatable; needed for regtest and devnets)
          --page       first page: overview, send, receive, transactions,
                       address-book, sign-verify, settings
        """

    static let pages = ["overview", "send", "receive", "transactions", "address-book", "sign-verify", "settings"]

    init() {}

    init(arguments: [String], environment: [String: String]) {
        if environment["DWD_DEMO"] == "1" { mode = .demo(.funded) }
        var iterator = arguments.dropFirst().makeIterator()
        var pending: String?
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
                    switch value {
                    case "funded": mode = .demo(.funded)
                    case "locked": mode = .demo(.locked)
                    case "onboarding": mode = .demo(.onboarding)
                    default: pending = value
                    }
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
            case "--help", "-h":
                showHelp = true
            default:
                // macOS adds "-NSDocumentRevisionsDebugMode YES" etc. when launched from Xcode.
                if argument.hasPrefix("-NS") {
                    _ = next()
                } else {
                    problems.append("unknown option \(argument)")
                }
            }
        }
    }
}
