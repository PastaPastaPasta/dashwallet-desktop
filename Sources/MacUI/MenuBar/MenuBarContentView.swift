// Menu bar companion (IOS-117, QT-028/029): balance (masked in discreet
// mode), receive QR and address, and shortcuts into the app.
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

struct MenuBarContentView: View {
    let model: MacAppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            HStack {
                Text(L10n.Navigation.appName).dashFont(.headline)
                Spacer()
                if let network = model.main?.network, network != .mainnet {
                    Badge(L10n.Settings.networkName(network), tone: .info)
                }
                if model.isDemo {
                    Badge(MacStrings.App.demoBadge, tone: .warning)
                }
            }
            if let home = model.main?.home {
                VStack(alignment: .leading, spacing: DashSpacing.xxxs) {
                    Text(MacStrings.Overview.balance)
                        .dashFont(.caption1)
                        .foregroundStyle(Color.dash.secondaryText)
                    // HomeViewModel masks the digits in discreet mode.
                    Text(home.formattedTotal ?? L10n.Common.unknown)
                        .font(.system(.title3, design: .monospaced).weight(.semibold))
                        .accessibilityIdentifier("menuBar.balance")
                    Text(home.syncText)
                        .dashFont(.caption1)
                        .foregroundStyle(Color.dash.secondaryText)
                }
            }
            if let receive = model.main?.receive {
                Divider()
                Text(MacStrings.MenuBar.receive).dashFont(.subheadMedium)
                HStack {
                    Spacer()
                    QuickReceiveView(receive: receive)
                    Spacer()
                }
                Button(MacStrings.Common.copyAddress, systemImage: "doc.on.doc") {
                    if let address = receive.copyAddress() { MacPasteboard.copy(address) }
                }
                .disabled(receive.copyAddress() == nil)
            }
            Divider()
            Button(MacStrings.Menu.openWallet) {
                openWindow(id: SceneID.main)
                MacApplication.activate()
            }
            Button(MacStrings.Menu.quit) { MacApplication.terminate() }
        }
        .buttonStyle(.borderless)
        .padding(DashSpacing.l)
        .frame(width: 280)
    }
}
#endif
