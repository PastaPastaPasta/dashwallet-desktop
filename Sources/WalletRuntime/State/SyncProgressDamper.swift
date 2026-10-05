import DashKit
import Foundation

/// Turns raw engine sync snapshots into the `SyncStatus` the UI shows
/// (iOS `SyncingActivityMonitor`). Pure: time comes in as an argument.
///
/// - Progress: the mean of the phase fractions (1 once caught up). The shown
///   value never moves more than `maxStep` (10 %) per snapshot and never goes
///   backwards within one SPV run; a new run (SPV stopped, then running
///   again) starts over from the raw value.
/// - Done (iOS rule 6): never from a progress fraction. `isDone` turns true
///   once the engine has reported `caughtUp` continuously for `peakDelay`
///   (3.25 s), so a momentary caught-up between phases does not flash
///   "synced". Once done it stays done while SPV runs and the raw progress
///   stays at or above `doneHoldThreshold`, so a new block does not flip the
///   UI back to "syncing"; it drops when SPV stops or real catching-up starts.
/// - Stalled: not caught up and no progress for `stallSeconds` (45 s), or a
///   `syncStalled` notice that no later progress has cleared.
struct SyncProgressDamper {
    static let maxStep = 0.10
    static let peakDelay: Duration = .milliseconds(3250)
    static let stallSeconds: UInt64 = 45
    static let doneHoldThreshold = 0.995

    private var shown: Double?
    private var caughtUpSince: Duration?
    private var done = false
    private var wasRunning = false
    private var stallNotice = false
    /// The last snapshot applied, so the status can be recomputed when the
    /// peak delay ends without asking the engine again.
    private(set) var lastSnapshot: DashKit.SyncSnapshot?

    /// Forgets everything (network switch).
    mutating func reset() {
        self = SyncProgressDamper()
    }

    /// The engine sent a `syncStalled` notice.
    mutating func noteStallNotice() {
        stallNotice = true
    }

    /// Applies `snapshot` taken at `now`. Returns the status and, while
    /// caught up but not yet done, the time at which to call `apply` again
    /// (with the same snapshot) to let `isDone` turn true.
    mutating func apply(_ snapshot: DashKit.SyncSnapshot, at now: Duration) -> (status: SyncStatus, recheckAt: Duration?) {
        lastSnapshot = snapshot
        let raw = Self.rawProgress(snapshot)

        if snapshot.running && !wasRunning {
            shown = nil
            done = false
            caughtUpSince = nil
        }
        wasRunning = snapshot.running

        // Damped, monotonic progress.
        if let current = shown {
            if raw > current { shown = min(raw, current + Self.maxStep) }
        } else {
            shown = raw
        }

        // Done gate.
        var recheckAt: Duration?
        if snapshot.running && snapshot.caughtUp {
            let since = caughtUpSince ?? now
            caughtUpSince = since
            if now - since >= Self.peakDelay {
                done = true
            } else if !done {
                recheckAt = since + Self.peakDelay
            }
        } else {
            caughtUpSince = nil
            if !snapshot.running || raw < Self.doneHoldThreshold { done = false }
        }
        if done { shown = 1 }

        // Stall.
        if let quiet = snapshot.secondsSinceProgress, quiet < Self.stallSeconds { stallNotice = false }
        if snapshot.caughtUp || !snapshot.running { stallNotice = false }
        let quietTooLong = (snapshot.secondsSinceProgress ?? 0) >= Self.stallSeconds
        let stalled = snapshot.running && !snapshot.caughtUp && !done && (quietTooLong || stallNotice)

        let status = SyncStatus(
            running: snapshot.running, phases: snapshot.phases.map(SyncPhaseProgress.init),
            activePhase: snapshot.activePhase.map(SyncPhase.init), tipHeight: snapshot.tipHeight,
            tipDate: snapshot.tipDate, chainLockHeight: snapshot.chainLockHeight,
            connectedPeers: snapshot.connectedPeers, progress: shown, isDone: done, isStalled: stalled)
        return (status, recheckAt)
    }

    /// Mean of the phase fractions; 1 when caught up, 0 with no phases.
    static func rawProgress(_ snapshot: DashKit.SyncSnapshot) -> Double {
        if snapshot.caughtUp { return 1 }
        guard !snapshot.phases.isEmpty else { return 0 }
        let total = snapshot.phases.reduce(0.0) { sum, phase in
            if phase.done { return sum + 1 }
            guard let current = phase.currentHeight, let target = phase.targetHeight, target > 0 else { return sum }
            return sum + min(1, Double(current) / Double(target))
        }
        return total / Double(snapshot.phases.count)
    }
}
