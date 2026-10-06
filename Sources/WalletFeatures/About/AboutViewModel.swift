// About, command-line options and log export (IOS-107, IOS-112, QT-153).
import Foundation
import Observation
import WalletRuntime

/// One row of the command-line options dialog.
public struct CommandLineOption: Sendable, Hashable, Identifiable {
    public var id: String { name }
    public let name: String
    public let text: String
}

/// Log export as a small state machine.
public enum LogExportState: Sendable, Hashable {
    case idle
    case exporting
    case exported(URL)
    case failed(String)
}

@MainActor
@Observable
public final class AboutViewModel {
    public static let documentationURL = URL(string: "https://docs.dash.org/")!
    public static let githubURL = URL(string: "https://github.com/dashpay")!
    public static let supportURL = URL(string: "mailto:support@dash.org")!

    public private(set) var information: NodeInformation?
    public private(set) var network: DashNetwork?
    public private(set) var logExport: LogExportState = .idle
    public private(set) var errorMessage: String?

    public var title: String { L10n.HomeM2.aboutTitle }
    /// "Version <client version>"; `nil` until loaded.
    public var versionText: String? { information.map { L10n.HomeM2.version($0.clientVersion) } }
    public var networkName: String? { network.map(L10n.Settings.networkName) }
    public var dataDirectory: URL? { information?.dataDirectory ?? network.map(host.dataDirectory(for:)) }
    public var licenseText: String { L10n.HomeM2.license }

    /// `--help` as a two-column table (QT-153).
    public var commandLineOptions: [CommandLineOption] {
        launchArguments.optionNames.map { line in
            let key = line.drop { $0 == "-" }.prefix { $0 != "=" && $0 != " " }
            return CommandLineOption(name: line, text: L10n.HomeM2.optionDescription(String(key)))
        }
    }

    private let nodeInformation: any NodeInformationProviding
    private let logs: any LogExporting
    private let launchArguments: any LaunchArgumentsParsing
    private let host: any WalletHosting

    public init(
        nodeInformation: any NodeInformationProviding, logs: any LogExporting,
        launchArguments: any LaunchArgumentsParsing, host: any WalletHosting
    ) {
        self.nodeInformation = nodeInformation
        self.logs = logs
        self.launchArguments = launchArguments
        self.host = host
    }

    public convenience init(env: AppEnvironment, m2: M2Services) {
        self.init(nodeInformation: m2.nodeInformation, logs: m2.logs, launchArguments: m2.launchArguments, host: env.host)
    }

    public func load() async {
        network = await host.activeNetwork
        do {
            information = try await nodeInformation.information()
            errorMessage = nil
        } catch {
            information = nil
            errorMessage = ErrorText.m2(error.code)
        }
    }

    /// Zips every log to `file` (IOS-112).
    public func exportLogs(to file: URL) async {
        guard logExport != .exporting else { return }
        logExport = .exporting
        do {
            logExport = .exported(try await logs.exportLogs(to: file))
        } catch {
            logExport = .failed(ErrorText.m2(error.code))
        }
    }
}
