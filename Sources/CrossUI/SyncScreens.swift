// dash-qt's sync overlay (QT-027) and the connected peers with "Change
// Peers" (QT-024 "Show Peers", QT-147), as pages over the window content.
// SwiftCrossUI has no modal overlay that AT-SPI sees inside the window, so
// both replace the content until hidden or closed.
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

/// The overlay's card: why balances may be wrong and how far the sync is.
struct SyncOverlayScreen: View {
    let state: CrossAppState
    let status: SyncStatus

    var body: some View {
        let state = state
        let rates = state.main.syncRates
        VStack(alignment: .leading, spacing: Int(DashSpacing.m)) {
            Spacer()
            DashCard {
                SectionHeader(L10n.SyncOverlay.title, style: .title3)
                Text(L10n.SyncOverlay.body).dashFont(.footnote).dashForeground(.secondaryText)
                KeyValueRow(L10n.SyncOverlay.status, state.main.home?.syncText ?? L10n.Common.unknown)
                KeyValueRow(L10n.SyncOverlay.blocksLeft, SyncRateTracker.blocksLeft(status).map { "\($0)" } ?? L10n.Common.unknown)
                KeyValueRow(L10n.SyncOverlay.lastBlockTime, Format.date(status.tipDate))
                KeyValueRow(L10n.SyncOverlay.progress, status.progress.map(Self.percent) ?? L10n.Common.unknown)
                KeyValueRow(L10n.SyncOverlay.progressPerHour, rates.progressPerHour.map(Self.percent) ?? L10n.Common.unknown)
                KeyValueRow(L10n.SyncOverlay.timeLeft, rates.remaining(for: status).map(Self.duration) ?? L10n.Common.unknown)
                HStack {
                    Spacer()
                    DashButton(L10n.SyncOverlay.hide, style: .tintedBlue) { state.main.hideSyncOverlay() }
                }
            }
            .frame(maxWidth: 560)
            Spacer()
        }
        .padding(Int(DashSpacing.xxl))
        .frame(maxWidth: .infinity)
    }

    static func percent(_ value: Double) -> String {
        String(format: "%.2f%%", value * 100)
    }

    /// Whole minutes, at least one: "2 h 5 min", "3 d 4 h".
    static func duration(_ seconds: TimeInterval) -> String {
        let minutes = max(1, Int((seconds / 60).rounded()))
        let days = minutes / 1440
        let hours = (minutes % 1440) / 60
        let rest = minutes % 60
        if days > 0 { return "\(days) d \(hours) h" }
        if hours > 0 { return "\(hours) h \(rest) min" }
        return "\(rest) min"
    }
}

/// The connected peers and "Change Peers".
struct PeersScreen: View {
    let state: CrossAppState

    @State var model: PeersViewModel

    init(state: CrossAppState) {
        self.state = state
        _model = State(wrappedValue: PeersViewModel(sync: state.env.sync))
    }

    var body: some View {
        let state = state
        let model = model
        Page(L10n.Peers.title) {
            HStack(spacing: Int(DashSpacing.s)) {
                DashButton(
                    L10n.Peers.changePeers, style: .tintedBlue, size: .small, isEnabled: !model.rotating,
                    help: L10n.Peers.changePeersHelp
                ) { Task { await model.rotate() } }
                DashButton(CrossStrings.close, style: .strokeGray, size: .small) { state.showsPeers = false }
            }
            if let error = model.error {
                Toast(error, kind: .error)
            }
            if let peers = model.peers {
                if peers.isEmpty {
                    Text(L10n.Peers.none).dashFont(.footnote).dashForeground(.secondaryText)
                } else {
                    let names = peers.map(Self.rowName)
                    // ADR 0002: the list scrolls inside a fixed-height ScrollView.
                    ScrollView {
                        List(peers.map(PeerRow.init), selection: bind({ nil as String? }, { _ in })) { row in
                            MenuItem(title: row.peer.address, subtitle: Self.details(row.peer))
                        }
                        .accessibleRowNames(names)
                    }
                    .frame(height: 320)
                }
            } else if model.error == nil {
                Text(CrossStrings.loading).dashFont(.footnote).dashForeground(.secondaryText)
            }
        }
        .task { await model.load() }
    }

    /// User agent, best height, ping and direction; unknown fields say so
    /// (dash-spv reports only addresses today).
    static func details(_ peer: PeerInfo) -> String {
        [
            "\(L10n.Peers.userAgent): \(peer.userAgent ?? L10n.Common.unknown)",
            "\(L10n.Peers.height): \(peer.bestHeight.map(String.init) ?? L10n.Common.unknown)",
            "\(L10n.Peers.ping): \(peer.pingMilliseconds.map { "\($0) ms" } ?? L10n.Common.unknown)",
            peer.inbound ? L10n.Peers.inbound : L10n.Peers.outbound,
        ].joined(separator: "  ")
    }

    static func rowName(_ peer: PeerInfo) -> String {
        "\(peer.address), \(details(peer))"
    }
}

struct PeerRow: Identifiable, Hashable {
    let peer: PeerInfo
    var id: String { peer.address }

    static func == (lhs: PeerRow, rhs: PeerRow) -> Bool { lhs.peer.address == rhs.peer.address }
    func hash(into hasher: inout Hasher) { hasher.combine(peer.address) }
}
