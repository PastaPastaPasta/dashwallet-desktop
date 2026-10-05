import DashKit
import Foundation
import Observation

/// SPV sync status of the open network (iOS `SwiftDashSDKSPVCoordinator`).
///
/// On every engine sync signal it re-queries `syncSnapshot` (coalesced) and
/// runs it through `SyncProgressDamper`. `status` stays `nil` until the first
/// snapshot arrives; if the engine cannot produce one, `lastError` says why
/// (e.g. `not_implemented`) and `status` stays `nil` — it is never invented.
@MainActor
@Observable
public final class SPVCoordinator: SyncStatusProviding, SessionObserving {
    public private(set) var status: SyncStatus?
    /// The last failure to read the sync state; cleared by the next success.
    public private(set) var lastError: ServiceError?

    @ObservationIgnored private let engine: any EngineProtocol
    @ObservationIgnored private let clock: any RuntimeClock
    @ObservationIgnored private var network: DashKit.DashNetwork?
    @ObservationIgnored private var damper = SyncProgressDamper()
    @ObservationIgnored private let broadcaster = StateBroadcaster<SyncStatus>()
    @ObservationIgnored private let tasks = TaskBag()
    @ObservationIgnored private var coalescer: Coalescer!

    public init(engine: any EngineProtocol, clock: any RuntimeClock = SystemClock()) {
        self.engine = engine
        self.clock = clock
        coalescer = Coalescer { [weak self] in await self?.refresh() }
    }

    public func changes() -> AsyncStream<SyncStatus> {
        broadcaster.stream()
    }

    public func peers() async throws(ServiceError) -> [PeerInfo] {
        let network = try requireNetwork()
        let engine = engine
        return try await serviceCall { () async throws(DashKitError) in try await engine.peers(on: network) }
            .map(PeerInfo.init)
    }

    public func rotatePeers() async throws(ServiceError) {
        let network = try requireNetwork()
        let engine = engine
        try await serviceCall { () async throws(DashKitError) in try await engine.rotatePeers(on: network) }
    }

    public func rescan(from start: RescanStart) async throws(ServiceError) {
        let network = try requireNetwork()
        let engine = engine
        try await serviceCall { () async throws(DashKitError) in try await engine.rescan(on: network, from: start.kit) }
    }

    /// Returns once every requested refresh has run (tests, diagnostics).
    public func settle() async {
        await coalescer.idle()
    }

    // MARK: SessionObserving

    public func sessionDidStart(_ network: DashKit.DashNetwork) async {
        self.network = network
        damper.reset()
        status = nil
        lastError = nil
        broadcaster.clear()
        startEventPump()
        coalescer.request()
        await coalescer.idle()
    }

    public func sessionWillStop(_ network: DashKit.DashNetwork) async {
        self.network = nil
        tasks.cancelAll()
        coalescer.cancel()
        damper.reset()
        status = nil
        broadcaster.clear()
    }

    public func walletsDidChange(_ network: DashKit.DashNetwork) async {}

    // MARK: Private

    private func requireNetwork() throws(ServiceError) -> DashKit.DashNetwork {
        guard let network else { throw ServiceError(code: .networkNotOpen, detail: "no network is open") }
        return network
    }

    private func startEventPump() {
        let subscription = engine.events.subscribe()
        tasks.set("events", Task { [weak self] in
            for await event in subscription {
                guard let self else { return }
                self.handle(event)
            }
        })
    }

    private func handle(_ event: EngineEvent) {
        guard let network, event.network == nil || event.network == network else { return }
        switch event {
        case .syncChanged, .spvStateChanged, .peersChanged, .syncProgress, .sessionOpened, .resynchronize:
            coalescer.request()
        case .notice(_, .syncStalled, _):
            damper.noteStallNotice()
            coalescer.request()
        default:
            break
        }
    }

    private func refresh() async {
        guard let network else { return }
        let snapshot: DashKit.SyncSnapshot
        do {
            snapshot = try await engine.syncSnapshot(on: network)
        } catch {
            if self.network == network { lastError = ServiceError(error) }
            return
        }
        guard self.network == network else { return }
        lastError = nil
        apply(snapshot)
    }

    private func apply(_ snapshot: DashKit.SyncSnapshot) {
        let (next, recheckAt) = damper.apply(snapshot, at: clock.now())
        if next != status {
            status = next
            broadcaster.send(next)
        }
        guard let recheckAt else {
            tasks.set("peak", nil)
            return
        }
        let delay = recheckAt - clock.now()
        let clock = clock
        tasks.set("peak", Task { [weak self] in
            do { try await clock.sleep(for: delay) } catch { return }
            self?.reapplyLastSnapshot()
        })
    }

    /// The peak delay ended: recompute from the last snapshot.
    private func reapplyLastSnapshot() {
        guard network != nil, let snapshot = damper.lastSnapshot else { return }
        apply(snapshot)
    }
}
