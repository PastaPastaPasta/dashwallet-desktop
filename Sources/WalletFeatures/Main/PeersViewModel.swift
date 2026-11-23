// dash-qt's peers list (QT-024 "Show Peers") with "Change Peers" (QT-147),
// shared by the macOS sheet and the SwiftCrossUI page. With a
// `PeerModerating` service (M2) it also disconnects, bans and unbans.
import Foundation
import Observation
import WalletRuntime

/// dash-qt's ban durations (QT-147).
public enum BanDuration: Sendable, Hashable, CaseIterable {
    case hour, day, week, year

    public var duration: Duration {
        switch self {
        case .hour: .seconds(3600)
        case .day: .seconds(86_400)
        case .week: .seconds(604_800)
        case .year: .seconds(31_536_000)
        }
    }

    public var title: String {
        switch self {
        case .hour: L10n.Tools.banHour
        case .day: L10n.Tools.banDay
        case .week: L10n.Tools.banWeek
        case .year: L10n.Tools.banYear
        }
    }
}

@MainActor
@Observable
public final class PeersViewModel {
    /// `nil` until the first load answers.
    public private(set) var peers: [PeerInfo]?
    /// Banned subnets; dash-qt shows the table only when it is not empty.
    public private(set) var banned: [BannedPeer] = []
    public private(set) var error: String?
    public private(set) var rotating = false

    /// Disconnect / Ban / Unban are offered (an M2 moderation service exists).
    public var canModerate: Bool { moderation != nil }
    public var showsBannedList: Bool { !banned.isEmpty }

    private let sync: any SyncStatusProviding
    private let moderation: (any PeerModerating)?

    public init(sync: any SyncStatusProviding, moderation: (any PeerModerating)? = nil) {
        self.sync = sync
        self.moderation = moderation
    }

    public func load() async {
        do {
            peers = try await sync.peers()
            error = nil
        } catch {
            self.error = ErrorText.common(error.code)
        }
        await loadBanned()
    }

    /// Drops the current peers for new ones, then reloads the list.
    public func rotate() async {
        rotating = true
        defer { rotating = false }
        do {
            try await sync.rotatePeers()
            await load()
        } catch {
            self.error = ErrorText.common(error.code)
        }
    }

    public func disconnect(_ address: String) async {
        await moderate { (moderation) throws(ServiceError) in try await moderation.disconnect(address: address) }
    }

    public func ban(_ address: String, for duration: BanDuration) async {
        await moderate { (moderation) throws(ServiceError) in
            try await moderation.ban(address: address, for: duration.duration)
        }
    }

    public func unban(_ subnet: String) async {
        await moderate { (moderation) throws(ServiceError) in try await moderation.unban(subnet: subnet) }
    }

    private func loadBanned() async {
        guard let moderation else { return }
        do {
            banned = try await moderation.bannedPeers()
        } catch {
            banned = []
            if error.code != .notImplemented { self.error = ErrorText.m2(error.code) }
        }
    }

    private func moderate(_ body: (any PeerModerating) async throws(ServiceError) -> Void) async {
        guard let moderation else { return }
        do {
            try await body(moderation)
            error = nil
        } catch {
            self.error = ErrorText.m2(error.code)
            return
        }
        await load()
    }
}
