// Builds the AppEnvironment for `--demo`.
import Foundation
import WalletFeatures
import WalletRuntime

enum DemoEnvironment {
    @MainActor
    static func make(network: DashNetwork, scenario: DemoScenario) -> AppEnvironment {
        let world = DemoWorld(network: network, scenario: scenario)
        let box = world.networkBox
        let uri = CoreURIHandling(box: box)
        let settings = DemoSettings(world: world)
        return AppEnvironment(
            host: DemoHost(world: world), lifecycle: DemoLifecycle(world: world),
            walletState: DemoWalletState(world: world), sync: DemoSync(world: world), vault: DemoVault(world: world),
            auth: DemoAuth(world: world), sender: DemoSender(world: world), history: DemoHistory(world: world),
            receive: DemoReceive(world: world, uri: uri), coinControl: DemoCoinControl(world: world),
            addressBook: DemoAddressBook(world: world), messages: CoreMessageSigning(box: box), uri: uri,
            amounts: CoreAmountFormatting(box: box), settings: settings, preferences: settings,
            screenCapture: nil, developerMode: true)
    }
}
