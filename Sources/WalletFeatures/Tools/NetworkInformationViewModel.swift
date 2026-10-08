// Tools ▸ Information ▸ Network (QT-144): masternode and EvoNode counts,
// the best ChainLock, quorums, and the credit pool and InstantSend counters,
// which an SPV wallet cannot know ("—" with "Requires full-node data
// source"). The mempool rows stay in `InformationViewModel` (M2).
import Foundation
import Observation
import WalletRuntime

@MainActor
@Observable
public final class NetworkInformationViewModel {
    public private(set) var statistics: NetworkStatistics?
    public private(set) var available = true
    public private(set) var errorMessage: String?

    public var sections: [InformationSection] {
        guard let s = statistics else { return [] }
        typealias T = NetworkInformationText
        let none = L10n.Tools.none
        let fullNode = L10n.Tools.requiresFullNode
        func row(_ title: String, _ value: String?, fullNodeOnly: Bool = false) -> InformationRow {
            InformationRow(title: title, value: value ?? none, note: value == nil && fullNodeOnly ? fullNode : nil)
        }
        func count(_ c: MasternodeCount?) -> String? { c.map { T.enabledOfTotal($0.enabled, $0.total) } }
        let pool = s.creditPool
        let isLocks = s.instantSend
        return [
            InformationSection(title: T.masternodes, rows: [
                row(T.masternodeCount, count(s.masternodes)),
                row(T.evonodeCount, count(s.evonodes)),
            ]),
            InformationSection(title: T.chainLocks, rows: [
                row(T.bestChainLockHeight, s.bestChainLock.map { "\($0.height)" }),
                row(T.bestChainLockHash, s.bestChainLock?.blockHash),
            ]),
            InformationSection(title: T.quorums, rows: s.quorums.isEmpty
                ? [row(T.quorums, nil)]
                : s.quorums.map { q in
                    row(q.name, T.quorum(active: q.active, health: q.healthPercent, rotated: q.rotated))
                }),
            InformationSection(title: T.creditPool, rows: [
                row(T.creditPoolLastChange, pool.map { "\($0.lastBlockChange)" }, fullNodeOnly: true),
                row(T.creditPoolLocked, pool.map { amount($0.totalLocked) }, fullNodeOnly: true),
                row(T.creditPoolPending, pool.map { amount($0.pendingUnlocks) }, fullNodeOnly: true),
                row(T.creditPoolLimit, pool.map { amount($0.withdrawalLimit) }, fullNodeOnly: true),
            ]),
            InformationSection(title: T.instantSend, rows: [
                row(T.isVerified, isLocks.map { "\($0.verified)" }, fullNodeOnly: true),
                row(T.isUnverified, isLocks.map { "\($0.unverified)" }, fullNodeOnly: true),
                row(T.isAwaiting, isLocks.map { "\($0.awaitingTransaction)" }, fullNodeOnly: true),
                row(T.isUnprotected, isLocks.map { "\($0.unprotectedTransactions)" }, fullNodeOnly: true),
            ]),
        ]
    }

    private let provider: any NetworkStatisticsProviding
    private let sync: any SyncStatusProviding
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding
    private var tasks: [Task<Void, Never>] = []

    public init(
        provider: any NetworkStatisticsProviding, sync: any SyncStatusProviding, amounts: any AmountFormatting,
        settings: any SettingsProviding
    ) {
        self.provider = provider
        self.sync = sync
        self.amounts = amounts
        self.settings = settings
    }

    public convenience init(env: AppEnvironment, m3: M3Services) {
        self.init(
            provider: m3.networkStatistics, sync: env.sync, amounts: env.amounts, settings: env.settings)
    }

    private func amount(_ value: Amount) -> String { AmountText(amounts: amounts, settings: settings)(value) }

    /// Loads, then re-reads on sync changes (the masternode list is a sync
    /// phase).
    public func start() async {
        stop()
        await reload()
        let syncChanges = sync.changes()
        tasks.append(Task { [weak self] in
            for await _ in syncChanges { await self?.reload() }
        })
    }

    public func stop() {
        tasks.forEach { $0.cancel() }
        tasks = []
    }

    public func reload() async {
        do {
            statistics = try await provider.statistics()
            available = true
        } catch {
            if error.isNotImplemented {
                available = false
            } else {
                errorMessage = ErrorText.m3(error, amount: amount)
            }
        }
    }
}

/// dash-qt's Information ▸ Network labels (`debugwindow.ui`).
public enum NetworkInformationText {
    public static let masternodes = "Masternodes"
    public static let masternodeCount = "Masternode Count"
    public static let evonodeCount = "EvoNode Count"
    public static let chainLocks = "ChainLocks"
    public static let bestChainLockHeight = "Best ChainLock height"
    public static let bestChainLockHash = "Best ChainLock hash"
    public static let quorums = "Quorums"
    public static let creditPool = "Credit Pool"
    public static let creditPoolLastChange = "Last block change"
    public static let creditPoolLocked = "Total locked"
    public static let creditPoolPending = "Pending unlocks"
    public static let creditPoolLimit = "Withdrawal limit"
    public static let instantSend = "InstantSend"
    public static let isVerified = "Verified locks"
    public static let isUnverified = "Unverified locks"
    public static let isAwaiting = "Awaiting transaction"
    public static let isUnprotected = "Unprotected transactions"
    public static func enabledOfTotal(_ enabled: Int, _ total: Int) -> String { "\(total) (\(enabled) enabled)" }
    public static func quorum(active: Int, health: Double?, rotated: Bool) -> String {
        var text = "\(active) active"
        if let health { text += String(format: " (%.1f%% health)", health) }
        if rotated { text += ", rotated" }
        return text
    }
}
