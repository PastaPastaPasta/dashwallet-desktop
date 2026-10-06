// macOS app entry point. MacAppComposition is the composition root:
// `--demo` (or `--fixture`, `--demo-scenario funded|fresh|locked|offline`)
// runs over the demo services; otherwise the live WalletRuntime and M2
// services are built for ~/Library/Application Support/org.dashfoundation.DashWallet/
// (`--datadir` / `-datadir=` overrides it, `--network` / `-testnet` picks the
// first network, `-choosedatadir` opens the data-directory chooser). dash-qt's
// GUI options (`-min`, `-splash=0`, `-windowtitle=…`, `-resetguisettings`,
// `-help`) are read too.
import AppKit
import Foundation
import MacUI
import PlatformServicesMac
import SwiftUI

@main
struct DashWalletApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        DashWalletScenes(model: delegate.model)
    }
}

/// Owns the app's root model (built once, when AppKit creates the delegate),
/// builds the Dock menu, and releases the engine before the process exits:
/// quitting waits until `MacAppModel.shutdown()` has stopped SPV and closed
/// the session behind the shutdown window (QT-008).
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model = MacAppComposition.makeModel(
        launch: LaunchOptions.parse(CommandLine.arguments, environment: ProcessInfo.processInfo.environment))
    private let dockMenu = MacDockMenu()

    /// Opens the network as soon as the app runs, not only when the main
    /// window first appears (the menu bar companion works without it).
    /// `-min` starts with the main window minimized (QT-006).
    func applicationDidFinishLaunching(_ notification: Notification) {
        Task { await model.start() }
        if model.launch.runtime.startMinimized {
            DispatchQueue.main.async {
                NSApplication.shared.windows.first { $0.isVisible }?.miniaturize(nil)
            }
        }
    }

    /// dash-qt's Dock menu (QT-029).
    func applicationDockMenu(_ sender: NSApplication) -> NSMenu? {
        let items = model.dockMenuItems
        return items.isEmpty ? nil : dockMenu.menu(items)
    }

    /// How long quitting waits for the engine before exiting anyway.
    static let shutdownTimeout: Duration = .seconds(15)
    private var terminationReplied = false

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        Task {
            await model.shutdown()
            replyTermination(sender)
        }
        // A shutdown that never finishes must not keep the app from quitting.
        Task {
            try? await Task.sleep(for: Self.shutdownTimeout)
            replyTermination(sender)
        }
        return .terminateLater
    }

    private func replyTermination(_ sender: NSApplication) {
        guard !terminationReplied else { return }
        terminationReplied = true
        sender.reply(toApplicationShouldTerminate: true)
    }

    /// With the menu bar companion on, closing the window keeps the app
    /// running (QT-028); without it, closing the last window quits.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        !model.showsMenuBarExtra
    }
}
