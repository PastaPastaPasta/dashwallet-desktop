// Tools ▸ Information (QT-143) and the node alert banner (QT-040).
import Foundation
import Observation
import WalletRuntime

public struct InformationRow: Sendable, Hashable, Identifiable {
    public var id: String { title }
    public let title: String
    /// "—" when unknown or full-node only.
    public let value: String
    /// "Requires full-node data source" for rows an SPV wallet cannot fill.
    public let note: String?
}

public struct InformationSection: Sendable, Hashable, Identifiable {
    public var id: String { title }
    public let title: String
    public let rows: [InformationRow]
}

extension NodeWarning {
    /// Banner text (QT-040).
    public var text: String {
        switch self {
        case .prereleaseBuild: L10n.Tools.prereleaseBuild
        case .uncleanShutdown: L10n.Tools.uncleanShutdown
        case .syncStalled: L10n.Tools.syncStalled
        case .clockSkew: L10n.Tools.clockSkew
        case .platformContextUnavailable: L10n.Tools.platformContextUnavailable
        }
    }
}

@MainActor
@Observable
public final class InformationViewModel {
    public private(set) var information: NodeInformation?
    public private(set) var warnings: [NodeWarning] = []
    public private(set) var errorMessage: String?

    /// The most severe warning, for the banner over the main window.
    public var bannerText: String? { warnings.first?.text }

    /// General, Network, Block chain, Memory Pool, Masternodes (dash-qt's
    /// General and Network sub-tabs, without the full-node-only sections).
    public var sections: [InformationSection] {
        guard let info = information else { return [] }
        let none = L10n.Tools.none
        typealias T = L10n.Tools
        let fullNode = T.requiresFullNode
        return [
            InformationSection(title: T.general, rows: [
                InformationRow(title: T.clientVersion, value: info.clientVersion, note: nil),
                InformationRow(title: T.userAgent, value: info.userAgent, note: nil),
                InformationRow(title: T.datadir, value: info.dataDirectory.path, note: nil),
                InformationRow(title: T.startupTime, value: dateText(info.startupDate), note: nil),
            ]),
            InformationSection(title: T.network, rows: [
                InformationRow(title: T.name, value: T.networkName(info.network), note: nil),
                InformationRow(
                    title: T.connections,
                    value: T.connectionCount(
                        total: info.connectionsIn + info.connectionsOut, inbound: info.connectionsIn,
                        outbound: info.connectionsOut),
                    note: nil),
                InformationRow(
                    title: T.localAddresses,
                    value: info.localAddresses.isEmpty ? none : info.localAddresses.joined(separator: ", "), note: nil),
            ]),
            InformationSection(title: T.blockChain, rows: [
                InformationRow(title: T.blockHeight, value: info.tipHeight.map(String.init) ?? none, note: nil),
                InformationRow(title: T.lastBlockTime, value: info.tipDate.map(dateText) ?? none, note: nil),
                InformationRow(title: T.lastBlockHash, value: info.tipHash ?? none, note: nil),
            ]),
            InformationSection(title: T.memoryPool, rows: [
                InformationRow(
                    title: T.mempoolCount, value: info.mempoolTransactionCount.map(String.init) ?? none,
                    note: info.mempoolTransactionCount == nil ? fullNode : nil),
                InformationRow(
                    title: T.mempoolUsage, value: info.mempoolUsageBytes.map(Self.megabytes) ?? none,
                    note: info.mempoolUsageBytes == nil ? fullNode : nil),
            ]),
            InformationSection(title: T.masternodes, rows: [
                InformationRow(
                    title: T.masternodes,
                    value: info.masternodes.map { T.masternodeCount(total: $0.total, enabled: $0.enabled) } ?? none,
                    note: nil),
                InformationRow(
                    title: T.evonodes,
                    value: info.evonodes.map { T.masternodeCount(total: $0.total, enabled: $0.enabled) } ?? none,
                    note: nil),
                InformationRow(
                    title: T.chainLocks,
                    value: info.bestChainLock.map { lock in
                        [String(lock.height), lock.blockDate.map(dateText), lock.blockHash].compactMap { $0 }
                            .joined(separator: ", ")
                    } ?? none,
                    note: nil),
            ]),
        ]
    }

    private let nodeInformation: any NodeInformationProviding
    private let sync: any SyncStatusProviding
    private let timing: Timing
    private var task: Task<Void, Never>?

    public init(nodeInformation: any NodeInformationProviding, sync: any SyncStatusProviding, timing: Timing) {
        self.nodeInformation = nodeInformation
        self.sync = sync
        self.timing = timing
    }

    public convenience init(env: AppEnvironment, m2: M2Services) {
        self.init(nodeInformation: m2.nodeInformation, sync: env.sync, timing: env.timing)
    }

    public func load() async {
        do {
            information = try await nodeInformation.information()
            warnings = try await nodeInformation.warnings()
            errorMessage = nil
        } catch {
            errorMessage = ErrorText.m2(error.code)
        }
    }

    /// Re-reads both on every sync change (in-memory engine reads) until `stop()`.
    public func start() {
        stop()
        let changes = sync.changes()
        task = Task { [weak self] in
            for await _ in changes {
                await self?.load()
            }
        }
    }

    public func stop() {
        task?.cancel()
        task = nil
    }

    private func dateText(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = timing.timeZone
        formatter.dateFormat = "EEE MMM d HH:mm:ss yyyy"
        return formatter.string(from: date)
    }

    /// dash-qt `formatBytes`-style MB with one decimal.
    nonisolated static func megabytes(_ bytes: UInt64) -> String {
        String(format: "%.1f MB", Double(bytes) / 1_000_000)
    }
}
