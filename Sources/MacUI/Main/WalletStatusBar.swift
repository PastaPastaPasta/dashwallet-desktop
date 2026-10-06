// dash-qt status bar (QT-020…025): sync text and progress on the left; unit
// selector, HD, lock, connections and sync state on the right. Items with
// no view-model data (proxy, governance clock) are left out.
#if os(macOS)
import DashUIMac
import SwiftUI
import WalletFeatures
import WalletRuntime

struct WalletStatusBar: View {
    let model: MacAppModel
    let main: MainViewModel

    private var sync: SyncStatus? { main.home?.sync }

    var body: some View {
        StatusBar(message: message, progress: progress, items: items)
            .contextMenu {
                // Unit selector menu (QT-020).
                ForEach(DisplayUnit.allCases, id: \.self) { unit in
                    Button(model.env?.amounts.unitName(unit) ?? "") { main.settings.setUnit(unit) }
                }
                Divider()
                Button(MacStrings.Peers.show) { model.isPeersPresented = true }
                    .disabled(sync == nil)
            }
            .accessibilityIdentifier("statusBar")
    }

    /// Sync text; while catching up, how far behind the tip date is (QT-025).
    private var message: String? {
        guard let home = main.home else { return nil }
        guard let sync, sync.running, !sync.isDone, let tipDate = sync.tipDate else { return home.syncText }
        let behind = Date().timeIntervalSince(tipDate)
        guard behind > 60 else { return home.syncText }
        let formatter = DateComponentsFormatter()
        formatter.unitsStyle = .full
        formatter.maximumUnitCount = 2
        formatter.allowedUnits = [.year, .weekOfMonth, .day, .hour, .minute]
        return formatter.string(from: behind).map { "\(home.syncText) – \(MacStrings.Status.behind($0))" }
            ?? home.syncText
    }

    /// The bar only while syncing; unknown progress is not drawn.
    private var progress: Double? {
        guard let sync, sync.running, !sync.isDone else { return nil }
        return sync.progress
    }

    private var items: [StatusBarItem] {
        var items: [StatusBarItem] = []
        let unit = main.settings.display.unit
        items.append(StatusBarItem(
            id: "unit", text: model.unitName, accessibilityLabel: model.unitName, help: MacStrings.Status.unitHelp,
            action: { main.settings.setUnit(Self.next(after: unit)) }))
        if let id = main.selectedWalletID, let wallet = main.wallets?.first(where: { $0.id == id }), wallet.hd {
            items.append(StatusBarItem(
                id: "hd", icon: .system("key.horizontal"), text: "HD", accessibilityLabel: MacStrings.Status.hdEnabled,
                help: MacStrings.Status.hdEnabled, tone: .success))
        }
        // dash-qt hides the lock icon for an unencrypted wallet (QT-022).
        switch main.lockState {
        case .unlocked:
            items.append(StatusBarItem(
                id: "lock", icon: .system("lock.open"), accessibilityLabel: MacStrings.Status.unlocked,
                help: MacStrings.Status.unlocked, tone: .success))
        case .unlockedMixingOnly:
            items.append(StatusBarItem(
                id: "lock", icon: .system("lock.open"), accessibilityLabel: MacStrings.Status.mixingOnly,
                help: MacStrings.Status.mixingOnly, tone: .warning))
        case .locked:
            items.append(StatusBarItem(
                id: "lock", icon: .system("lock"), accessibilityLabel: MacStrings.Status.locked,
                help: MacStrings.Status.locked, tone: .neutral))
        case .noVault, .noKeys, .unencrypted, nil:
            break
        }
        if let sync {
            let peers = MacStrings.Status.connections(sync.connectedPeers)
            items.append(StatusBarItem(
                id: "peers", icon: .system(Self.connectionsSymbol(sync.connectedPeers)), text: "\(sync.connectedPeers)",
                accessibilityLabel: peers, help: "\(peers). \(MacStrings.Peers.show)",
                tone: sync.connectedPeers == 0 ? .error : .neutral,
                action: { model.isPeersPresented = true }))
            // While catching up a click opens the sync overlay (QT-027).
            items.append(StatusBarItem(
                id: "sync", icon: .system(sync.isDone ? "checkmark.circle.fill" : "arrow.triangle.2.circlepath"),
                accessibilityLabel: sync.isDone ? MacStrings.Status.synced : MacStrings.Status.syncing,
                help: sync.isDone ? main.home?.syncText : "\(main.home?.syncText ?? ""). \(MacStrings.SyncOverlay.show)",
                tone: sync.isDone ? .success : .info,
                action: sync.isDone ? nil : { main.syncOverlayRequested = true }))
        }
        return items
    }

    static func next(after unit: DisplayUnit) -> DisplayUnit {
        let all = DisplayUnit.allCases
        let index = all.firstIndex(of: unit) ?? 0
        return all[(index + 1) % all.count]
    }

    /// Connection icon: none, 1–3 peers, more. (dash-qt has five levels;
    /// SF Symbols offers three distinct wifi states.)
    static func connectionsSymbol(_ count: UInt32) -> String {
        switch count {
        case 0: "wifi.slash"
        case 1...3: "wifi.exclamationmark"
        default: "wifi"
        }
    }
}
#endif
