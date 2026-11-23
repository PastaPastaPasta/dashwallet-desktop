// M2 service contracts: the Tools window (Information, Console, Peers,
// Repair), node warnings and log export (engine tools.rs, console.rs,
// desktop.rs; m2-swift.md §2.7).
import Foundation

public struct MasternodeCount: Sendable, Hashable {
    public let total: Int
    public let enabled: Int

    public init(total: Int, enabled: Int) {
        self.total = total
        self.enabled = enabled
    }
}

public struct ChainLockInfo: Sendable, Hashable {
    public let height: UInt32
    public let blockHash: String
    public let blockDate: Date?

    public init(height: UInt32, blockHash: String, blockDate: Date?) {
        self.height = height
        self.blockHash = blockHash
        self.blockDate = blockDate
    }
}

/// Tools → Information (QT-143). `nil` = unknown or full-node only: show "—"
/// (with "Requires full-node data source" for the mempool rows).
public struct NodeInformation: Sendable, Hashable {
    public let clientVersion: String
    public let userAgent: String
    public let dataDirectory: URL
    public let startupDate: Date
    public let network: DashNetwork
    public let connectionsIn: Int
    public let connectionsOut: Int
    public let localAddresses: [String]
    public let tipHeight: UInt32?
    public let tipDate: Date?
    public let tipHash: String?
    public let bestChainLock: ChainLockInfo?
    public let masternodes: MasternodeCount?
    public let evonodes: MasternodeCount?
    public let mempoolTransactionCount: Int?
    public let mempoolUsageBytes: UInt64?

    public init(
        clientVersion: String, userAgent: String, dataDirectory: URL, startupDate: Date, network: DashNetwork,
        connectionsIn: Int, connectionsOut: Int, localAddresses: [String], tipHeight: UInt32?, tipDate: Date?,
        tipHash: String?, bestChainLock: ChainLockInfo?, masternodes: MasternodeCount?, evonodes: MasternodeCount?,
        mempoolTransactionCount: Int?, mempoolUsageBytes: UInt64?
    ) {
        self.clientVersion = clientVersion
        self.userAgent = userAgent
        self.dataDirectory = dataDirectory
        self.startupDate = startupDate
        self.network = network
        self.connectionsIn = connectionsIn
        self.connectionsOut = connectionsOut
        self.localAddresses = localAddresses
        self.tipHeight = tipHeight
        self.tipDate = tipDate
        self.tipHash = tipHash
        self.bestChainLock = bestChainLock
        self.masternodes = masternodes
        self.evonodes = evonodes
        self.mempoolTransactionCount = mempoolTransactionCount
        self.mempoolUsageBytes = mempoolUsageBytes
    }
}

/// Alert banner warnings (QT-040), most severe first.
public enum NodeWarning: Sendable, Hashable {
    case prereleaseBuild
    case uncleanShutdown
    case syncStalled
    case clockSkew
    case platformContextUnavailable
}

/// Information tab and alert banner (owner S1 adapter over R1). Re-query on
/// sync changes; both are in-memory reads in the engine.
public protocol NodeInformationProviding: AnyObject, Sendable {
    func information() async throws(ServiceError) -> NodeInformation
    func warnings() async throws(ServiceError) -> [NodeWarning]
}

public struct BannedPeer: Sendable, Hashable {
    public let subnet: String
    public let bannedUntil: Date

    public init(subnet: String, bannedUntil: Date) {
        self.subnet = subnet
        self.bannedUntil = bannedUntil
    }
}

/// Peers tab moderation (QT-147). The peer list itself is
/// `SyncStatusProviding.peers()` (M1).
public protocol PeerModerating: AnyObject, Sendable {
    func disconnect(address: String) async throws(ServiceError)
    /// dash-qt offers 1 hour, 1 day, 1 week and 1 year.
    func ban(address: String, for duration: Duration) async throws(ServiceError)
    func unban(subnet: String) async throws(ServiceError)
    func bannedPeers() async throws(ServiceError) -> [BannedPeer]
}

public struct RescanProgress: Sendable, Hashable {
    public let fromHeight: UInt32
    public let currentHeight: UInt32?
    public let targetHeight: UInt32?
    public let startedAt: Date

    public init(fromHeight: UInt32, currentHeight: UInt32?, targetHeight: UInt32?, startedAt: Date) {
        self.fromHeight = fromHeight
        self.currentHeight = currentHeight
        self.targetHeight = targetHeight
        self.startedAt = startedAt
    }
}

/// Repair tab and sync info (QT-117/148, IOS-113). Starting a rescan is
/// `SyncStatusProviding.rescan(from:)` (M1).
public protocol RepairProviding: AnyObject, Sendable {
    /// `nil` when no rescan runs.
    func rescanProgress() async throws(ServiceError) -> RescanProgress?
    /// `false` when none ran.
    func cancelRescan() async throws(ServiceError) -> Bool
    /// "Reset chain data and resync": the adapter stops SPV, deletes the
    /// chain data and starts SPV again on the lifecycle queue.
    func resetChainData() async throws(ServiceError)
    func setBirthHeight(wallet: WalletID, height: UInt32) async throws(ServiceError)
}

public struct ConsoleCommandInfo: Sendable, Hashable {
    public let name: String
    public let category: String
    public let sensitive: Bool
    public let available: Bool

    public init(name: String, category: String, sensitive: Bool, available: Bool) {
        self.name = name
        self.category = category
        self.sensitive = sensitive
        self.available = available
    }
}

/// Outcome of one console line (QT-145). Errors other than these (parse,
/// RPC, not available) are thrown with their `console.*` code; an RPC error
/// carries `parameters["rpc_code"]` and its Core message in `detail`, which
/// the console prints as "message (code N)" (RPC output, not UI copy).
public enum ConsoleResult: Sendable, Hashable {
    case output(text: String, isJSON: Bool)
    /// Authorize `purpose` for `wallet`, then run the same line with the grant.
    case authorizationRequired(GrantPurpose, wallet: WalletID?)
}

/// The local console (owner S1 adapter over R1).
public protocol ConsoleExecuting: AnyObject, Sendable {
    func commands() async throws(ServiceError) -> [ConsoleCommandInfo]
    /// The line as echoed and kept in history (sensitive arguments → "(…)").
    func redact(_ line: any SecretBuffer) throws(ServiceError) -> String
    /// `line` is a zeroing buffer because it may hold a passphrase.
    func execute(_ line: any SecretBuffer, wallet: WalletID?, grant: AuthGrant?) async throws(ServiceError)
        -> ConsoleResult
}

/// Log export (IOS-112, owner S1): Rust logs of every network plus the
/// Swift log, zipped to `file`.
public protocol LogExporting: AnyObject, Sendable {
    func exportLogs(to file: URL) async throws(ServiceError) -> URL
}
