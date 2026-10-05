// macOS app entry point and composition root. `--demo` (or `--fixture`,
// `--demo-scenario funded|fresh|locked`) runs over the demo services;
// otherwise the live WalletRuntime services are built for
// ~/Library/Application Support/org.dashfoundation.DashWallet/.
import Foundation
import MacUI
import SwiftUI

@main
struct DashWalletApp: App {
    @State private var model = MacAppComposition.makeModel(
        launch: LaunchOptions.parse(CommandLine.arguments, environment: ProcessInfo.processInfo.environment),
        live: LiveComposition.makeEnvironment)

    var body: some Scene {
        DashWalletScenes(model: model)
    }
}
