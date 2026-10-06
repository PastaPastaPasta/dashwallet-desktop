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

/// The iOS shortcut card (UX-SPEC C6): the view model's four slots as items
/// with the 46 pt shortcut icons; right-click replaces a slot (iOS long-press).
struct ShortcutBarView: View {
    let bar: ShortcutBarViewModel
    let perform: (ShortcutRoute) -> Void

    var body: some View {
        ShortcutCard {
            ForEach(bar.slots) { slot in
                ShortcutItem(title: slot.action.title, icon: Self.icon(slot.action)) {
                    if let route = bar.route(for: slot) { perform(route) }
                }
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

    /// dashwallet-iOS shortcut-bar icons where they are exported; the
    /// others draw their SF Symbol in a blue circle of the same size.
    static func icon(_ action: ShortcutAction) -> DashIconSource {
        switch action {
        case .backup: .token(.shortcutBackup)
        case .receive: .token(.shortcutReceive)
        case .send: .token(.shortcutSend)
        case .sendToAddress: .token(.shortcutSendToAddress)
        case .scanQR: .token(.shortcutScanQR)
        case .explore: .token(.shortcutExplore)
        case .buySell: .system("creditcard")
        case .spend: .system("cart")
        case .atm: .system("banknote")
        case .coinbase, .uphold, .topper: .system("building.columns")
        case .dashDEX: .system("arrow.left.arrow.right")
        case .crowdNode: .system("person.3")
        case .testnetFaucet: .system("drop")
        case .switchWallet: .system("wallet.pass")
        case .nodes: .system("server.rack")
        }
    }
}

/// IOS-005: shown once, 24 h after a new wallet first held funds, until the
/// phrase is backed up. A DashUIKit system message with Back up / Later.
struct BackupReminderBanner: View {
    let reminder: BackupReminderViewModel
    let backUp: () -> Void

    var body: some View {
        if reminder.isDue {
            SystemMessageView(
                title: L10n.HomeM2.backupReminderTitle, subtitle: L10n.HomeM2.backupReminderMessage,
                icon: .token(.messageShield), backgroundColor: Color.role.warningTint,
                buttonName: L10n.HomeM2.backupNow,
                onAction: {
                    reminder.markShown()
                    backUp()
                },
                secondaryButtonName: L10n.HomeM2.later, onSecondaryAction: { reminder.markShown() })
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
