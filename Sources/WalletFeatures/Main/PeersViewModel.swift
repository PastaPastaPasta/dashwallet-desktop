// dash-qt's peers list (QT-024 "Show Peers") with "Change Peers" (QT-147),
// shared by the macOS sheet and the SwiftCrossUI page.
import Foundation
import Observation
import WalletRuntime

@MainActor
@Observable
public final class PeersViewModel {
    /// `nil` until the first load answers.
    public private(set) var peers: [PeerInfo]?
    public private(set) var error: String?
    public private(set) var rotating = false

    private let sync: any SyncStatusProviding

    public init(sync: any SyncStatusProviding) {
        self.sync = sync
    }

    public func load() async {
        do {
            peers = try await sync.peers()
            error = nil
        } catch {
            self.error = ErrorText.common(error.code)
        }
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
}
