// Overview additions: the iOS shortcut bar (IOS-025: four slots with
// state-dependent defaults, customizable from the context menu; M4/M5
// actions disabled) and the 24-hour backup reminder (IOS-005).
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

struct ShortcutBarView: View {
    let bar: ShortcutBarViewModel
    let perform: (ShortcutRoute) -> Void

    var body: some View {
        HStack(spacing: DashSpacing.m) {
            ForEach(bar.slots) { slot in
                Button {
                    if let route = bar.route(for: slot) { perform(route) }
                } label: {
                    VStack(spacing: DashSpacing.xs) {
                        Image(systemName: Self.symbol(slot.action))
                            .font(.system(size: 20))
                            .foregroundStyle(slot.isEnabled ? Color.dash.blue : Color.dash.secondaryText)
                        Text(slot.action.title)
                            .dashFont(.footnoteMedium)
                            .foregroundStyle(slot.isEnabled ? Color.dash.primaryText : Color.dash.secondaryText)
                            .lineLimit(1)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, DashSpacing.m)
                    .background(RoundedRectangle(cornerRadius: DashRadius.card).fill(Color.dash.secondaryBackground))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(!slot.isEnabled)
                .help(slot.helpText ?? slot.action.title)
                .contextMenu {
                    Menu(MacStrings.Overview.replaceShortcut) {
                        ForEach(bar.customizableActions, id: \.self) { action in
                            Button(action.title) { bar.setSlot(slot.index, to: action) }
                        }
                    }
                    Button(MacStrings.Overview.resetShortcuts) { bar.resetToDefaults() }
                }
                .accessibilityIdentifier("shortcut.\(slot.action.rawValue)")
            }
        }
        .task { bar.start() }
        .onDisappear { bar.stop() }
        .accessibilityIdentifier("shortcutBar")
    }

    static func symbol(_ action: ShortcutAction) -> String {
        switch action {
        case .backup: "lock.shield"
        case .receive: "arrow.down.left"
        case .send, .sendToAddress: "arrow.up.right"
        case .scanQR: "qrcode.viewfinder"
        case .buySell: "creditcard"
        case .explore: "map"
        case .spend: "cart"
        case .atm: "banknote"
        case .coinbase, .uphold, .topper: "building.columns"
        case .dashDEX: "arrow.left.arrow.right"
        case .crowdNode: "person.3"
        case .testnetFaucet: "drop"
        case .switchWallet: "wallet.pass"
        case .nodes: "server.rack"
        }
    }
}

/// IOS-005: shown once, 24 h after a new wallet first held funds, until the
/// phrase is backed up.
struct BackupReminderBanner: View {
    let reminder: BackupReminderViewModel
    let backUp: () -> Void

    var body: some View {
        if reminder.isDue {
            HStack(alignment: .top, spacing: DashSpacing.m) {
                Image(systemName: "exclamationmark.shield.fill")
                    .foregroundStyle(Color.dash.orange)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: DashSpacing.xxxs) {
                    Text(L10n.HomeM2.backupReminderTitle).dashFont(.subheadMedium)
                    Text(L10n.HomeM2.backupReminderMessage)
                        .dashFont(.footnote)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer()
                Button(L10n.HomeM2.later) { reminder.markShown() }
                Button(L10n.HomeM2.backupNow) {
                    reminder.markShown()
                    backUp()
                }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("backupReminder.backUp")
            }
            .padding(DashSpacing.l)
            .background(RoundedRectangle(cornerRadius: DashRadius.card).fill(Color.dash.orangeAlpha10))
            .accessibilityIdentifier("backupReminder")
        }
    }
}

extension MacAppModel {
    /// Performs a shortcut bar route; web links go to `openURL`.
    func perform(_ route: ShortcutRoute, openURL: OpenURLAction) {
        switch route {
        case .section(let item):
            main?.selection = item
        case .shell(let command):
            perform(command)
        case .scanQR:
            Task { await scanQRImage() }
        case .openURL(let url):
            openURL(url)
        case .switchWallet:
            windowOpener?(id: SceneID.wallets)
        }
    }

    /// Desktop "Scan QR": decodes a `dash:` code from an image file and
    /// opens it on the Send page.
    func scanQRImage() async {
        guard let file = await MacOpenPanel.chooseFile(title: L10n.HomeM2.scanQR) else { return }
        let importer = QRImageImport(decoder: MacQRImageDecoder(), clipboard: features?.m2.clipboard)
        do {
            guard let code = try importer.decode(file: file).first else { return }
            await open(uri: code)
        } catch {
            uriError = ErrorText.m2(error.code)
        }
    }
}
#endif
