// dash-qt's sync overlay (QT-027) and peers list (QT-024 "Show Peers").
#if os(macOS)
import DashUIMac
import DesignTokens
import Observation
import SwiftUI
import WalletFeatures
import WalletRuntime

/// Rates for the sync overlay, from the statuses seen so far: how fast the
/// active phase advances and when it should finish. Values stay `nil`
/// (shown as unknown) until two samples at least a minute apart exist.
@MainActor
@Observable
final class SyncRateTracker {
    struct Sample: Equatable {
        let date: Date
        let height: UInt32
        let progress: Double
    }

    private(set) var samples: [Sample] = []
    /// Samples older than this are dropped (dash-qt averages over the last hour).
    static let window: TimeInterval = 3600
    static let minimumSpan: TimeInterval = 60

    func record(_ status: SyncStatus, at date: Date = Date()) {
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
    var progressPerHour: Double? {
        guard let (first, last) = span else { return nil }
        return (last.progress - first.progress) / last.date.timeIntervalSince(first.date) * 3600
    }

    /// Time left at the current rate.
    func remaining(for status: SyncStatus) -> TimeInterval? {
        guard let rate = progressPerHour, rate > 0, let progress = status.progress else { return nil }
        return (1 - progress) / rate * 3600
    }

    private var span: (Sample, Sample)? {
        guard let first = samples.first, let last = samples.last,
              last.date.timeIntervalSince(first.date) >= Self.minimumSpan else { return nil }
        return (first, last)
    }

    /// The active phase's heights; blocks left = target − current.
    static func activeProgress(_ status: SyncStatus) -> SyncPhaseProgress? {
        status.phases.first { $0.phase == status.activePhase } ?? status.phases.first { !$0.done }
    }

    static func blocksLeft(_ status: SyncStatus) -> UInt32? {
        guard let phase = activeProgress(status), let current = phase.currentHeight,
              let target = phase.targetHeight else { return nil }
        return target > current ? target - current : 0
    }

    /// dash-qt shows the overlay by itself while the tip is more than 25
    /// minutes old (QT-027).
    static func tipIsOld(_ status: SyncStatus, now: Date = Date()) -> Bool {
        guard status.running, !status.isDone else { return false }
        guard let tipDate = status.tipDate else { return true }
        return now.timeIntervalSince(tipDate) > 25 * 60
    }
}

/// The modal-looking card over the wallet while it catches up (QT-027).
struct SyncOverlayView: View {
    let status: SyncStatus
    let rates: SyncRateTracker
    let syncText: String
    let hide: () -> Void

    var body: some View {
        ZStack {
            Color.dash.backgroundOverlay.ignoresSafeArea()
            VStack(alignment: .leading, spacing: DashSpacing.m) {
                Text(MacStrings.SyncOverlay.title).dashFont(.title3)
                Text(MacStrings.SyncOverlay.body)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.secondaryText)
                    .fixedSize(horizontal: false, vertical: true)
                Grid(alignment: .leading, horizontalSpacing: DashSpacing.l, verticalSpacing: DashSpacing.s) {
                    row(MacStrings.SyncOverlay.status, syncText)
                    row(MacStrings.SyncOverlay.blocksLeft, SyncRateTracker.blocksLeft(status).map { "\($0)" })
                    row(MacStrings.SyncOverlay.lastBlockTime, status.tipDate.map {
                        $0.formatted(date: .abbreviated, time: .shortened)
                    })
                    row(MacStrings.SyncOverlay.progress, status.progress.map { Self.percent($0) })
                    row(MacStrings.SyncOverlay.progressPerHour, rates.progressPerHour.map { Self.percent($0) })
                    row(MacStrings.SyncOverlay.timeLeft, rates.remaining(for: status).flatMap(Self.duration))
                }
                .accessibilityIdentifier("syncOverlay.details")
                HStack {
                    Spacer()
                    Button(MacStrings.SyncOverlay.hide, action: hide)
                        .keyboardShortcut(.cancelAction)
                        .accessibilityIdentifier("syncOverlay.hide")
                }
            }
            .padding(DashSpacing.xl)
            .frame(width: 460)
            .background(RoundedRectangle(cornerRadius: DashRadius.card).fill(Color.dash.secondaryBackground))
        }
        .accessibilityIdentifier("syncOverlay")
    }

    private func row(_ label: String, _ value: String?) -> some View {
        GridRow {
            Text(label)
                .dashFont(.footnoteMedium)
                .foregroundStyle(Color.dash.secondaryText)
            Text(value ?? L10n.Common.unknown)
                .dashFont(.footnote)
                .foregroundStyle(Color.dash.primaryText)
                .textSelection(.enabled)
        }
    }

    static func percent(_ value: Double) -> String {
        (value * 100).formatted(.number.precision(.fractionLength(2))) + "%"
    }

    static func duration(_ seconds: TimeInterval) -> String? {
        let formatter = DateComponentsFormatter()
        formatter.unitsStyle = .full
        formatter.maximumUnitCount = 2
        formatter.allowedUnits = [.day, .hour, .minute]
        return formatter.string(from: max(seconds, 60))
    }
}

/// Connected peers with "Change peers" (QT-024 "Show Peers", QT-147).
struct PeersSheet: View {
    let sync: any SyncStatusProviding
    @State private var peers: [PeerInfo]?
    @State private var error: String?
    @State private var rotating = false
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Peers.title).dashFont(.title3)
            Group {
                if let peers {
                    Table(peers.map(PeerRow.init)) {
                        TableColumn(MacStrings.Peers.address) { Text($0.peer.address).monospaced() }
                        TableColumn(MacStrings.Peers.userAgent) { Text($0.peer.userAgent ?? L10n.Common.unknown) }
                        TableColumn(MacStrings.Peers.height) { row in
                            Text(row.peer.bestHeight.map { "\($0)" } ?? L10n.Common.unknown)
                        }
                        .width(80)
                        TableColumn(MacStrings.Peers.ping) { row in
                            Text(row.peer.pingMilliseconds.map { "\($0) ms" } ?? L10n.Common.unknown)
                        }
                        .width(70)
                        TableColumn(MacStrings.Peers.direction) { row in
                            Text(row.peer.inbound ? MacStrings.Peers.inbound : MacStrings.Peers.outbound)
                        }
                        .width(80)
                    }
                    .overlay {
                        if peers.isEmpty { Text(MacStrings.Peers.none).foregroundStyle(Color.dash.secondaryText) }
                    }
                } else if error == nil {
                    ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
            .frame(minHeight: 220)
            if let error {
                Text(error).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
            }
            HStack {
                Button(MacStrings.Peers.changePeers) { Task { await rotate() } }
                    .disabled(rotating)
                    .help(MacStrings.Peers.changePeersHelp)
                Spacer()
                Button(MacStrings.Common.close) { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 680, height: 380)
        .task { await load() }
        .accessibilityIdentifier("peers")
    }

    private func load() async {
        do {
            peers = try await sync.peers()
            error = nil
        } catch {
            self.error = ErrorText.common(error.code)
        }
    }

    private func rotate() async {
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

private struct PeerRow: Identifiable {
    let peer: PeerInfo
    var id: String { peer.address }
}
#endif
