// M1 service contracts: SPV sync status (iOS `SwiftDashSDKSPVCoordinator`).
import Foundation

public enum SyncPhase: Sendable, Hashable, CaseIterable {
    case headers
    case filterHeaders
    case filters
    case masternodes
}

public struct SyncPhaseProgress: Sendable, Hashable {
    public let phase: SyncPhase
    public let currentHeight: UInt32?
    public let targetHeight: UInt32?
    public let done: Bool

    public init(phase: SyncPhase, currentHeight: UInt32?, targetHeight: UInt32?, done: Bool) {
        self.phase = phase
        self.currentHeight = currentHeight
        self.targetHeight = targetHeight
        self.done = done
    }
}

/// What the status bar, sync overlay and Home banner show (QT-024/025/027,
/// IOS-023). `progress` is damped by the coordinator (10 % max step).
public struct SyncStatus: Sendable, Hashable {
    public let running: Bool
    public let phases: [SyncPhaseProgress]
    public let activePhase: SyncPhase?
    public let tipHeight: UInt32?
    public let tipDate: Date?
    public let chainLockHeight: UInt32?
    public let connectedPeers: UInt32
    /// 0...1; `nil` before the first engine snapshot.
    public let progress: Double?
    /// The gate for "synced" UI (iOS rule 6): engine `caught_up`.
    public let isDone: Bool
    /// No progress for 45 s while not done; offer "Change peers".
    public let isStalled: Bool

    public init(
        running: Bool, phases: [SyncPhaseProgress], activePhase: SyncPhase?, tipHeight: UInt32?,
        tipDate: Date?, chainLockHeight: UInt32?, connectedPeers: UInt32, progress: Double?,
        isDone: Bool, isStalled: Bool
    ) {
        self.running = running
        self.phases = phases
        self.activePhase = activePhase
        self.tipHeight = tipHeight
        self.tipDate = tipDate
        self.chainLockHeight = chainLockHeight
        self.connectedPeers = connectedPeers
        self.progress = progress
        self.isDone = isDone
        self.isStalled = isStalled
    }
}

public struct PeerInfo: Sendable, Hashable {
    public let address: String
    public let userAgent: String?
    public let protocolVersion: UInt32?
    public let bestHeight: UInt32?
    public let pingMilliseconds: UInt32?
    public let connectedSince: Date?
    public let inbound: Bool

    public init(
        address: String, userAgent: String?, protocolVersion: UInt32?, bestHeight: UInt32?,
        pingMilliseconds: UInt32?, connectedSince: Date?, inbound: Bool
    ) {
        self.address = address
        self.userAgent = userAgent
        self.protocolVersion = protocolVersion
        self.bestHeight = bestHeight
        self.pingMilliseconds = pingMilliseconds
        self.connectedSince = connectedSince
        self.inbound = inbound
    }
}

public enum RescanStart: Sendable, Hashable {
    case walletBirth
    case genesis
    case height(UInt32)
}

@MainActor
public protocol SyncStatusProviding: AnyObject {
    /// `nil` before the first engine snapshot of the active network.
    var status: SyncStatus? { get }
    /// Current status followed by every change.
    func changes() -> AsyncStream<SyncStatus>
    func peers() async throws(ServiceError) -> [PeerInfo]
    func rotatePeers() async throws(ServiceError)
    func rescan(from start: RescanStart) async throws(ServiceError)
}
