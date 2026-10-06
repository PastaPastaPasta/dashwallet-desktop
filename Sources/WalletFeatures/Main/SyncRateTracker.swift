// Sync overlay figures (QT-027) shared by both UIs: blocks left, progress
// per hour and time left from the statuses seen so far, and dash-qt's rule
// for showing the overlay by itself.
import Foundation
import Observation
import WalletRuntime

/// Rates for the sync overlay, from the statuses seen so far: how fast the
/// active phase advances and when it should finish. Values stay `nil`
/// (shown as unknown) until two samples at least a minute apart exist.
@MainActor
@Observable
public final class SyncRateTracker {
    public struct Sample: Equatable, Sendable {
        public let date: Date
        public let height: UInt32
        public let progress: Double
    }

    public private(set) var samples: [Sample] = []
    /// Samples older than this are dropped (dash-qt averages over the last hour).
    public static let window: TimeInterval = 3600
    public static let minimumSpan: TimeInterval = 60

    public init() {}

    public func record(_ status: SyncStatus, at date: Date = Date()) {
        guard status.running, !status.isDone, let progress = status.progress,
              let height = Self.activeProgress(status)?.currentHeight else {
            samples = []
            return
        }
        // A new SPV run, phase or a rescan starts over.
        if let last = samples.last, height < last.height || progress < last.progress { samples = [] }
        samples.append(Sample(date: date, height: height, progress: progress))
        samples.removeAll { date.timeIntervalSince($0.date) > Self.window }
    }

    /// Progress gained per hour, 0...1 scale.
    public var progressPerHour: Double? {
        guard let (first, last) = span else { return nil }
        return (last.progress - first.progress) / last.date.timeIntervalSince(first.date) * 3600
    }

    /// Time left at the current rate.
    public func remaining(for status: SyncStatus) -> TimeInterval? {
        guard let rate = progressPerHour, rate > 0, let progress = status.progress else { return nil }
        return (1 - progress) / rate * 3600
    }

    private var span: (Sample, Sample)? {
        guard let first = samples.first, let last = samples.last,
              last.date.timeIntervalSince(first.date) >= Self.minimumSpan else { return nil }
        return (first, last)
    }

    /// The active phase's heights; blocks left = target − current.
    public static func activeProgress(_ status: SyncStatus) -> SyncPhaseProgress? {
        status.phases.first { $0.phase == status.activePhase } ?? status.phases.first { !$0.done }
    }

    public static func blocksLeft(_ status: SyncStatus) -> UInt32? {
        guard let phase = activeProgress(status), let current = phase.currentHeight,
              let target = phase.targetHeight else { return nil }
        return target > current ? target - current : 0
    }

    /// dash-qt shows the overlay by itself while the tip is more than 25
    /// minutes old (QT-027).
    public static func tipIsOld(_ status: SyncStatus, now: Date = Date()) -> Bool {
        guard status.running, !status.isDone else { return false }
        guard let tipDate = status.tipDate else { return true }
        return now.timeIntervalSince(tipDate) > 25 * 60
    }
}
