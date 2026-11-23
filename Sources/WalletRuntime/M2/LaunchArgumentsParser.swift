// Command-line parsing (QT-006), dash-qt's rules for the options that mean
// something to an SPV wallet (dash src/qt/bitcoin.cpp SetupUIArgs and the
// chain options of chainparamsbase).
import Foundation

/// `LaunchArgumentsParsing` with dash-qt's syntax:
///
/// - options start with `-` or `--`; values follow `=` (`-lang=de_DE`);
/// - boolean options accept `-min`, `-min=1`, `-min=0` and `-nomin`;
/// - every argument that does not start with `-` is a URI (or a file the OS
///   handed over); no option may follow one (`launch.option_after_uri`,
///   dash-qt "Command line contains unexpected token");
/// - unknown options, including node-only ones (`-server`, `-prune`, …),
///   are `launch.unknown_option`; bad values are `launch.invalid_value`;
/// - `-psn_…` (old Finder launches) and Xcode's `-NSDocumentRevisionsDebugMode
///   <value>` are skipped.
public struct LaunchArgumentsParser: LaunchArgumentsParsing {
    private enum Kind {
        case flag, value
    }

    private static let options: [(name: String, kind: Kind, help: String)] = [
        ("choosedatadir", .flag, "-choosedatadir"),
        ("chain", .value, "-chain=<chain>"),
        ("datadir", .value, "-datadir=<dir>"),
        ("devnet", .value, "-devnet=<name>"),
        ("help", .flag, "-help"),
        ("lang", .value, "-lang=<lang>"),
        ("min", .flag, "-min"),
        ("regtest", .flag, "-regtest"),
        ("resetguisettings", .flag, "-resetguisettings"),
        ("splash", .flag, "-splash"),
        ("testnet", .flag, "-testnet"),
        ("version", .flag, "-version"),
        ("windowtitle", .value, "-windowtitle=<name>"),
    ]

    /// Directory a relative `-datadir` is resolved against.
    private let currentDirectory: URL

    public init(currentDirectory: URL = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)) {
        self.currentDirectory = currentDirectory
    }

    public var optionNames: [String] {
        Self.options.map(\.help)
    }

    public func parse(_ arguments: [String]) throws(ServiceError) -> LaunchOptions {
        var options = LaunchOptions()
        var testnet = false
        var regtest = false
        var devnet: String?
        var chain: String?
        var index = arguments.startIndex
        while index < arguments.endIndex {
            let argument = arguments[index]
            index += 1
            if argument.hasPrefix("-psn_") { continue }
            if argument == "-NSDocumentRevisionsDebugMode" {
                index += 1
                continue
            }
            guard argument.hasPrefix("-"), argument.count > 1 else {
                options.uris.append(argument)
                continue
            }
            if !options.uris.isEmpty {
                throw ServiceError(code: .launchOptionAfterURI, detail: argument)
            }
            var body = Substring(argument.dropFirst(argument.hasPrefix("--") ? 2 : 1))
            var value: String?
            if let equals = body.firstIndex(of: "=") {
                value = String(body[body.index(after: equals)...])
                body = body[..<equals]
            }
            var name = String(body)
            var negated = false
            if Self.kind(of: name) == nil, name.hasPrefix("no"), Self.kind(of: String(name.dropFirst(2))) == .flag {
                name = String(name.dropFirst(2))
                negated = true
            }
            guard let kind = Self.kind(of: name) else {
                throw ServiceError(code: .launchUnknownOption, detail: argument)
            }
            switch kind {
            case .flag:
                var on = try Self.boolValue(value, option: argument)
                if negated { on.toggle() }
                switch name {
                case "choosedatadir": options.chooseDataDirectory = on
                case "help": options.showHelp = on
                case "min": options.startMinimized = on
                case "regtest": regtest = on
                case "resetguisettings": options.resetGUISettings = on
                case "splash": options.showSplash = on
                case "testnet": testnet = on
                case "version": options.showVersion = on
                default: break
                }
            case .value:
                guard !negated, let value, !value.isEmpty else {
                    throw ServiceError(code: .launchInvalidValue, detail: "\(argument) needs a value")
                }
                switch name {
                case "chain": chain = value
                case "datadir": options.dataDirectory = Self.directory(value, relativeTo: currentDirectory)
                case "devnet":
                    guard value.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-" || $0 == "_") })
                    else {
                        throw ServiceError(code: .launchInvalidValue, detail: argument)
                    }
                    devnet = value
                case "lang":
                    guard value.allSatisfy({ $0.isASCII && ($0.isLetter || $0 == "_" || $0 == "-") }) else {
                        throw ServiceError(code: .launchInvalidValue, detail: argument)
                    }
                    options.language = value
                case "windowtitle": options.windowTitleSuffix = value
                default: break
                }
            }
        }
        options.network = try Self.network(testnet: testnet, regtest: regtest, devnet: devnet, chain: chain)
        return options
    }

    private static func kind(of name: String) -> Kind? {
        options.first { $0.name == name }?.kind
    }

    /// `nil` (bare flag) and `1` are on, `0` is off (Core `InterpretBool`
    /// treats any other number as on and text as off; dash-qt users only
    /// write 0 and 1, so anything else is reported).
    private static func boolValue(_ value: String?, option: String) throws(ServiceError) -> Bool {
        switch value {
        case nil, "", "1": return true
        case "0": return false
        default:
            if let number = Int(value!) { return number != 0 }
            throw ServiceError(code: .launchInvalidValue, detail: option)
        }
    }

    private static func directory(_ path: String, relativeTo base: URL) -> URL {
        let expanded = (path as NSString).expandingTildeInPath
        if expanded.hasPrefix("/") {
            return URL(fileURLWithPath: expanded, isDirectory: true).standardizedFileURL
        }
        return base.appendingPathComponent(expanded, isDirectory: true).standardizedFileURL
    }

    /// Core allows one of `-testnet`, `-regtest`, `-devnet` and `-chain`
    /// ("Invalid combination of -regtest, -testnet, -devnet and -chain").
    private static func network(testnet: Bool, regtest: Bool, devnet: String?, chain: String?) throws(ServiceError)
        -> DashNetwork?
    {
        var chosen: [DashNetwork] = []
        if testnet { chosen.append(.testnet) }
        if regtest { chosen.append(.regtest) }
        if let chain {
            switch chain {
            case "main": chosen.append(.mainnet)
            case "test": chosen.append(.testnet)
            case "regtest": chosen.append(.regtest)
            case "devnet":
                guard let devnet else {
                    throw ServiceError(code: .launchInvalidValue, detail: "-chain=devnet needs -devnet=<name>")
                }
                chosen.append(.devnet(name: devnet))
            default:
                throw ServiceError(code: .launchInvalidValue, detail: "-chain=\(chain)")
            }
        }
        if let devnet, chain != "devnet" { chosen.append(.devnet(name: devnet)) }
        guard chosen.count <= 1 else {
            throw ServiceError(code: .launchInvalidValue, detail: "more than one network option")
        }
        return chosen.first
    }
}
