// Menus (QT-015…018 on macOS): File, Wallet (dash-qt's Settings menu),
// View (sidebar shortcuts), Window and Help.
#if os(macOS)
import SwiftUI
import WalletFeatures
import WalletRuntime

struct WalletCommands: Commands {
    let model: MacAppModel
    @Environment(\.openWindow) private var openWindow

    private var main: MainViewModel? { model.main }
    private var vault: VaultStatus? { main?.settings.vault }
    private var hasWallet: Bool { !(main?.wallets?.isEmpty ?? true) }

    var body: some Commands {
        CommandGroup(replacing: .appInfo) {
            Button(MacStrings.Menu.about) { openWindow(id: SceneID.about) }
        }

        // File (QT-015). Create/open/close wallet arrive with multi-wallet (M2).
        CommandGroup(replacing: .newItem) {
            Button(MacStrings.Menu.openURI) { model.isOpenURIPresented = true }
                .keyboardShortcut("u", modifiers: [.command, .shift])
                .disabled(main == nil)
            Button(MacStrings.Menu.backupWallet) {}
                .disabled(true)
                .help(MacStrings.Menu.backupUnavailable)
            Divider()
            Button(MacStrings.Menu.signMessage) {
                model.signVerifyTab = .sign
                openWindow(id: SceneID.signVerify)
            }
            .disabled(!hasWallet)
            Button(MacStrings.Menu.verifyMessage) {
                model.signVerifyTab = .verify
                openWindow(id: SceneID.signVerify)
            }
            .disabled(main == nil)
        }

        // dash-qt's Settings menu (QT-016); "Settings…" itself is the app menu item.
        CommandMenu(MacStrings.Menu.wallet) {
            Button(MacStrings.Menu.encryptWallet) { main?.sheet = .encryptWallet }
                .disabled(!hasWallet || vault?.encrypted != false)
            Button(MacStrings.Menu.changePassphrase) { main?.sheet = .changePassphrase }
                .disabled(vault?.encrypted != true)
            Button(MacStrings.Menu.showRecoveryPhrase) { main?.sheet = .showRecoveryPhrase }
                .disabled(!hasWallet)
            Divider()
            Button(MacStrings.Menu.lockWallet) {
                Task { await main?.lock.lock() }
            }
            .keyboardShortcut("l", modifiers: [.command, .control])
            .disabled(main?.lockState != .unlocked && main?.lockState != .unlockedMixingOnly)
            // Goes through the settings view model so Settings and Overview
            // (which follows the settings stream) stay in step.
            Toggle(MacStrings.Menu.discreetMode, isOn: Binding(
                get: { main?.settings.display.hideBalances ?? false },
                set: { main?.settings.setDiscreet($0) }))
            .keyboardShortcut("d", modifiers: [.command, .shift])
            .disabled(main == nil)
        }

        // Sidebar shortcuts, renumbered by the visible items (QT-012).
        CommandGroup(after: .sidebar) {
            if let main {
                Divider()
                ForEach(main.visibleSidebarItems) { item in
                    if let number = item.shortcutNumber(with: main.features), number <= 9 {
                        Button(item.title) { main.selectShortcut(number) }
                            .keyboardShortcut(KeyEquivalent(Character(String(number))), modifiers: .command)
                            .disabled(main.needsOnboarding)
                    }
                }
            }
        }

        // Window menu (QT-017).
        CommandGroup(before: .windowList) {
            Button(MacStrings.Menu.sendingAddresses) {
                model.addressBookPurpose = .send
                openWindow(id: SceneID.addressBook)
            }
            .disabled(!hasWallet)
            Button(MacStrings.Menu.receivingAddresses) {
                model.addressBookPurpose = .receive
                openWindow(id: SceneID.addressBook)
            }
            .disabled(!hasWallet)
            Divider()
        }

        CommandGroup(replacing: .help) {
            Link(MacStrings.Menu.help, destination: URL(string: "https://docs.dash.org/")!)
        }
    }
}
#endif
