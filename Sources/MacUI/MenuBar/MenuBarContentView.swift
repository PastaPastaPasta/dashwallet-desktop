// Menu bar companion (IOS-117, QT-028/029): balance (masked in discreet
// mode), last transaction, receive QR and address with an optional request
// amount, pay from clipboard, and dash-qt's tray shortcuts (Send, Receive,
// Transactions, Sign/Verify, Options, Tools).
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
            // The network once, as the capsule (UX-SPEC §4.19); the demo tag
            // is in the window title.
            HStack {
                Text(L10n.Navigation.appName)
                    .dashFont(.headline)
                    .foregroundStyle(Color.role.textPrimary)
                Spacer()
                if let network = model.main?.network, network != .mainnet {
                    NetworkCapsule(L10n.Settings.networkName(network))
                }
            }
            if let home = model.main?.home {
                VStack(alignment: .leading, spacing: DashSpacing.xxxs) {
                    // HomeViewModel masks the digits in discreet mode.
                    Group {
                        if let total = home.formattedTotal {
                            AmountText(
                                formatted: total, size: DesignTokens.DashTextStyle.title3.size, weight: .bold, glyphFactor: 0.8)
                        } else {
                            Text("—").dashFont(.title3).help(L10n.UX.balanceUnavailable)
                        }
                    }
                    .foregroundStyle(Color.role.textPrimary)
                    .accessibilityIdentifier("menuBar.balance")
                    Text(home.syncText)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.role.textSecondary)
                }
                // Hidden in discreet mode, like the Overview list.
                if home.recentVisible, let last = home.recent.first {
                    VStack(alignment: .leading, spacing: 0) {
                        Text(MacStrings.MenuBar.lastTransaction)
                            .dashFont(.caption1)
                            .foregroundStyle(Color.role.textSecondary)
                        DashTransactionRow(
                            recent: last, unit: model.main?.settings.display.unit ?? .dash, unitName: model.unitName,
                            action: { show(.transaction(txid: last.id.txid)) })
                    }
                    .accessibilityIdentifier("menuBar.lastTransaction")
                }
            }
            if let companion = model.features?.companion, model.main?.needsOnboarding == false {
                Divider()
                CompanionRequestView(companion: companion)
                Button(MacStrings.MenuBar.payFromClipboard, systemImage: "doc.on.clipboard") {
                    companion.payFromClipboard()
                }
                .accessibilityIdentifier("menuBar.payFromClipboard")
                if let error = companion.errorMessage {
                    Text(error).dashFont(.caption1).foregroundStyle(Color.role.danger)
                }
            } else if let receive = model.main?.receive {
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
            Button(MacStrings.MenuBar.send, systemImage: "arrow.up.right") { show(.section(.send)) }
                .disabled(model.main?.needsOnboarding ?? true)
            Button(MacStrings.MenuBar.transactions, systemImage: "list.bullet.rectangle") { show(.section(.transactions)) }
                .disabled(model.main?.needsOnboarding ?? true)
            if model.features != nil {
                Button(L10n.Shell.signMessage, systemImage: "signature") { model.perform(.signMessage) }
                    .disabled(!model.isEnabled(.signMessage))
                Button(L10n.Shell.options, systemImage: "gearshape") { model.perform(.options) }
                Button(L10n.Shell.information, systemImage: "info.circle") { model.perform(.tools(.information)) }
            }
            Button(MacStrings.Menu.openWallet) {
                openWindow(id: SceneID.main)
                MacApplication.activate()
            }
            Button(MacStrings.Menu.quit) { MacApplication.terminate() }
        }
        .buttonStyle(.dash(.plainBlack, .small))
        .padding(DashSpacing.l)
        .frame(width: 280)
        .dashCanvas()
        .onChange(of: companionRoute) { _, route in
            guard let route else { return }
            model.features?.companion.routeHandled()
            show(route)
        }
        .task { await model.features?.companion.load() }
    }

    /// Follows the companion's pay-from-clipboard route into Send.
    private var companionRoute: AppRoute? { model.features?.companion.route }

    /// Brings the main window forward on `route`.
    private func show(_ route: AppRoute) {
        openWindow(id: SceneID.main)
        MacApplication.activate()
        Task { await model.main?.navigate(route) }
    }
}
/// IOS-117: the receiving address with an optional amount, as a `dash:`
/// URI and its QR.
private struct CompanionRequestView: View {
    let companion: MenuBarCompanionViewModel
    @State private var amount = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.s) {
            Text(MacStrings.MenuBar.receive).dashFont(.subheadMedium).foregroundStyle(Color.role.textPrimary)
            if let qr = companion.qr {
                HStack {
                    Spacer()
                    QRView(size: qr.size, modules: qr.modules, accessibilityLabel: MacStrings.Receive.qrLabel)
                        .frame(width: 150, height: 150)
                    Spacer()
                }
            }
            if let address = companion.address {
                Text(address)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.textPrimary)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(address)
                    .accessibilityIdentifier("menuBar.address")
            }
            TextField(MacStrings.MenuBar.requestAmount, text: $amount)
                .textFieldStyle(.dash)
                .onChange(of: amount) { _, text in companion.setRequestAmount(text) }
                .accessibilityIdentifier("menuBar.requestAmount")
            if let error = companion.requestAmountError {
                Text(error).dashFont(.caption1).foregroundStyle(Color.role.danger)
            }
            Button(MacStrings.MenuBar.copyRequest, systemImage: "doc.on.doc") {
                if let uri = companion.requestURI ?? companion.address { MacPasteboard.copy(uri) }
            }
            .buttonStyle(.dash(.tintedBlue, .small))
            .disabled(companion.address == nil)
        }
    }
}
#endif
