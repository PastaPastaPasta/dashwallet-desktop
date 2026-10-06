// dash-qt's sync overlay (QT-027) and peers list (QT-024 "Show Peers").
#if os(macOS)
import DashUIMac
import DesignTokens
import Observation
import SwiftUI
import WalletFeatures
import WalletRuntime

/// The modal-looking card over the wallet while it catches up (QT-027).
struct SyncOverlayView: View {
    let status: SyncStatus
    let rates: SyncRateTracker
    let syncText: String
    let hide: () -> Void

    /// A card on the dimmed window (UX-SPEC §4.6): dash-qt's text and facts,
    /// the progress bar, and iOS's per-phase progress.
    var body: some View {
        ZStack {
            Color.role.overlay.ignoresSafeArea()
            VStack(alignment: .leading, spacing: DashSpacing.l) {
                VStack(alignment: .leading, spacing: DashSpacing.xs) {
                    Text(MacStrings.SyncOverlay.title)
                        .dashFont(.title2)
                        .foregroundStyle(Color.role.textPrimary)
                    Text(MacStrings.SyncOverlay.body)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.role.textSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                VStack(spacing: 0) {
                    DetailRow(MacStrings.SyncOverlay.status, value: syncText)
                    DetailRow(MacStrings.SyncOverlay.blocksLeft, value: value(SyncRateTracker.blocksLeft(status).map { "\($0)" }))
                    DetailRow(MacStrings.SyncOverlay.lastBlockTime, value: value(status.tipDate.map {
                        $0.formatted(date: .abbreviated, time: .shortened)
                    }))
                    DetailRow(MacStrings.SyncOverlay.progress, value: value(status.progress.map { Self.percent($0) })) {
                        DashProgressBar(value: status.progress).frame(width: 120)
                    }
                    DetailRow(MacStrings.SyncOverlay.progressPerHour, value: value(rates.progressPerHour.map { Self.percent($0) }))
                    DetailRow(MacStrings.SyncOverlay.timeLeft, value: value(rates.remaining(for: status).flatMap(Self.duration)))
                }
                .dashCard(radius: DashRadius.standard, padding: DashSpacing.xs, elevation: nil, fill: Color.role.cardRaised)
                .accessibilityIdentifier("syncOverlay.details")
                if !status.phases.isEmpty {
                    VStack(alignment: .leading, spacing: DashSpacing.s) {
                        Text(MacStrings.SyncPhases.title)
                            .dashFont(.subheadMedium)
                            .foregroundStyle(Color.role.textSecondary)
                        ForEach(status.phases, id: \.phase) { phase in
                            HStack(spacing: DashSpacing.m) {
                                Text(Self.phaseName(phase.phase))
                                    .dashFont(.footnote)
                                    .foregroundStyle(Color.role.textPrimary)
                                    .frame(width: 130, alignment: .leading)
                                DashProgressBar(value: Self.fraction(phase))
                                Text(Self.heights(phase))
                                    .dashFont(.caption1)
                                    .monospacedDigit()
                                    .foregroundStyle(phase.done ? Color.role.success : Color.role.textSecondary)
                                    .frame(width: 140, alignment: .trailing)
                            }
                        }
                        Text(MacStrings.Status.connections(status.connectedPeers))
                            .dashFont(.caption1)
                            .foregroundStyle(Color.role.textTertiary)
                    }
                }
                HStack {
                    Spacer()
                    Button(MacStrings.SyncOverlay.hide, action: hide)
                        .buttonStyle(.dash(.tintedGray, .medium))
                        .keyboardShortcut(.cancelAction)
                        .accessibilityIdentifier("syncOverlay.hide")
                }
            }
            .padding(DashSpacing.xxl)
            .frame(width: DashLayout.sheetWidthSmall + 40)
            .dashCard(padding: nil, elevation: .floating)
        }
        .accessibilityIdentifier("syncOverlay")
    }

    /// An unknown value is "—", never a guess.
    private func value(_ text: String?) -> String { text ?? "—" }

    /// Nil while a height is unknown (the bar is then indeterminate).
    static func fraction(_ phase: SyncPhaseProgress) -> Double? {
        if phase.done { return 1 }
        guard let current = phase.currentHeight, let target = phase.targetHeight, target > 0 else { return nil }
        return Double(current) / Double(target)
    }

    static func heights(_ phase: SyncPhaseProgress) -> String {
        if phase.done { return "✓" }
        return "\(phase.currentHeight.map(String.init) ?? "—") / \(phase.targetHeight.map(String.init) ?? "—")"
    }

    static func phaseName(_ phase: SyncPhase) -> String {
        switch phase {
        case .headers: L10n.Home.headers
        case .filterHeaders: L10n.Home.filterHeaders
        case .filters: L10n.Home.filters
        case .masternodes: L10n.Home.masternodes
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
    @State private var model: PeersViewModel
    @Environment(\.dismiss) private var dismiss

    init(sync: any SyncStatusProviding) {
        _model = State(initialValue: PeersViewModel(sync: sync))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Peers.title)
                .dashFont(.title3)
                .foregroundStyle(Color.role.textPrimary)
            Group {
                if let peers = model.peers {
                    Table(peers.map(PeerRow.init)) {
                        TableColumn(MacStrings.Peers.address) { Text($0.peer.address).monospacedDigit() }
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
                        if peers.isEmpty { Text(MacStrings.Peers.none).foregroundStyle(Color.role.textSecondary) }
                    }
                } else if model.error == nil {
                    ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
            .frame(minHeight: 220)
            if let error = model.error {
                Text(error).dashFont(.footnote).foregroundStyle(Color.role.danger)
            }
            HStack {
                Button(MacStrings.Peers.changePeers) { Task { await model.rotate() } }
                    .buttonStyle(.dash(.tintedBlue, .medium))
                    .disabled(model.rotating)
                    .help(MacStrings.Peers.changePeersHelp)
                Spacer()
                Button(MacStrings.Common.close) { dismiss() }
                    .buttonStyle(.dash(.tintedGray, .medium))
                    .keyboardShortcut(.cancelAction)
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 680, height: 380)
        .dashCanvas()
        .task { await model.load() }
        .accessibilityIdentifier("peers")
    }
}

private struct PeerRow: Identifiable {
    let peer: PeerInfo
    var id: String { peer.address }
}
#endif
