// dash-qt's sync overlay (QT-027) as a page over the window content.
// SwiftCrossUI has no modal overlay that AT-SPI sees inside the window, so
// it replaces the content until hidden. The peers list moved to Tools ▸
// Peers (ToolsWindowScreen.swift).
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
        // UX-SPEC §4.6: a centred card on the scrim.
        VStack(alignment: .leading, spacing: Int(DashSpacing.m)) {
            Spacer()
            DashCard(padding: Int(DashSpacing.xxl)) {
                SectionHeader(L10n.SyncOverlay.title, style: .title2)
                Text(L10n.SyncOverlay.body).dashFont(.subhead).dashForeground(CrossRole.textSecondary)
                if let progress = status.progress {
                    ProgressView(value: progress)
                }
                KeyValueRow(L10n.SyncOverlay.status, state.main.home?.syncText ?? L10n.Common.unknown)
                KeyValueRow(L10n.SyncOverlay.blocksLeft, SyncRateTracker.blocksLeft(status).map { "\($0)" } ?? L10n.Common.unknown)
                KeyValueRow(L10n.SyncOverlay.lastBlockTime, Format.date(status.tipDate))
                KeyValueRow(L10n.SyncOverlay.progress, status.progress.map(Self.percent) ?? L10n.Common.unknown)
                KeyValueRow(L10n.SyncOverlay.progressPerHour, rates.progressPerHour.map(Self.percent) ?? L10n.Common.unknown)
                KeyValueRow(L10n.SyncOverlay.timeLeft, rates.remaining(for: status).map(Self.duration) ?? L10n.Common.unknown)
                HStack {
                    Spacer()
                    DashButton(L10n.SyncOverlay.hide, style: .tintedGray) { state.main.hideSyncOverlay() }
                }
            }
            .frame(maxWidth: 600)
            Spacer()
        }
        .padding(Int(DashSpacing.xxl))
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(CrossRole.overlay.color)
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
