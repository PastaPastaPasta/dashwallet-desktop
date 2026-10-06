// macOS app entry point. MacAppComposition is the composition root:
// `--demo` (or `--fixture`, `--demo-scenario funded|fresh|locked`) runs over
// the demo services; otherwise the live WalletRuntime services are built for
// ~/Library/Application Support/org.dashfoundation.DashWallet/ (`--datadir`
// overrides it, `--network` picks the first network).
import AppKit
import Foundation
import MacUI
import SwiftUI

@main
struct DashWalletApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        DashWalletScenes(model: delegate.model)
    }
}

/// Owns the app's root model (built once, when AppKit creates the delegate)
/// and releases the engine before the process exits: quitting waits until
/// `MacAppModel.shutdown()` has stopped SPV and closed the session.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model = MacAppComposition.makeModel(
        launch: LaunchOptions.parse(CommandLine.arguments, environment: ProcessInfo.processInfo.environment))

    /// Opens the network as soon as the app runs, not only when the main
    /// window first appears (the menu bar companion works without it).
    func applicationDidFinishLaunching(_ notification: Notification) {
        Task { await model.start() }
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
