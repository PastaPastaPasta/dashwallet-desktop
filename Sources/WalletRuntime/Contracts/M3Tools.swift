// M3 service contract: Tools → Information → Network sub-tab (engine
// network_stats.rs; m3-swift.md §2.4). Owner of the adapter: R1. `nil` =
// full node only (show "—" with "Requires full-node data source").
import Foundation

public struct CreditPoolInfo: Sendable, Hashable {
    public let lastBlockChange: UInt32
    public let totalLocked: Amount
    public let pendingUnlocks: Amount
    public let withdrawalLimit: Amount

    public init(lastBlockChange: UInt32, totalLocked: Amount, pendingUnlocks: Amount, withdrawalLimit: Amount) {
        self.lastBlockChange = lastBlockChange
        self.totalLocked = totalLocked
        self.pendingUnlocks = pendingUnlocks
        self.withdrawalLimit = withdrawalLimit
    }
}

public struct InstantSendCounters: Sendable, Hashable {
    public let verified: Int
    public let unverified: Int
    public let awaitingTransaction: Int
    public let unprotectedTransactions: Int

    public init(verified: Int, unverified: Int, awaitingTransaction: Int, unprotectedTransactions: Int) {
        self.verified = verified
        self.unverified = unverified
        self.awaitingTransaction = awaitingTransaction
        self.unprotectedTransactions = unprotectedTransactions
    }
}

/// "N active (x.x% health)" per LLMQ type.
public struct QuorumSummary: Sendable, Hashable {
    public let name: String
    public let type: UInt8
    public let active: Int
    public let healthPercent: Double?
    public let rotated: Bool

    public init(name: String, type: UInt8, active: Int, healthPercent: Double?, rotated: Bool) {
        self.name = name
        self.type = type
        self.active = active
        self.healthPercent = healthPercent
        self.rotated = rotated
    }
}

public struct NetworkStatistics: Sendable, Hashable {
    public let creditPool: CreditPoolInfo?
    public let instantSend: InstantSendCounters?
    public let masternodes: MasternodeCount?
    public let evonodes: MasternodeCount?
    public let bestChainLock: ChainLockInfo?
    public let quorums: [QuorumSummary]

    public init(
        creditPool: CreditPoolInfo?, instantSend: InstantSendCounters?, masternodes: MasternodeCount?,
        evonodes: MasternodeCount?, bestChainLock: ChainLockInfo?, quorums: [QuorumSummary]
    ) {
        self.creditPool = creditPool
        self.instantSend = instantSend
        self.masternodes = masternodes
        self.evonodes = evonodes
        self.bestChainLock = bestChainLock
        self.quorums = quorums
    }
}

/// The Network sub-tab (QT-144). Re-query on sync and masternode changes.
public protocol NetworkStatisticsProviding: AnyObject, Sendable {
    func statistics() async throws(ServiceError) -> NetworkStatistics
}
