// Builds an AppEnvironment from the demo services (screenshots, UI tests,
// trying the app without funds).
#if os(macOS)
import Foundation
import PlatformServices
import WalletFeatures
import WalletRuntime

public enum DemoEnvironment {
    /// A complete environment over one fresh `DemoStore`. Developer mode is
    /// on so regtest appears in the network lists.
    @MainActor
    public static func make(
        scenario: DemoScenario, screenCapture: (any ScreenCaptureGuard)? = nil, timing: Timing = Timing()
    ) -> AppEnvironment {
        let store = DemoStore(scenario: scenario, now: timing.now)
        let uri = DemoURIHandler(network: store.activeNetwork)
        let settings = DemoSettings(store: store)
        return AppEnvironment(
            host: DemoHost(store: store), lifecycle: DemoLifecycle(store: store),
            walletState: DemoWalletState(store: store), sync: DemoSyncStatus(store: store),
            vault: DemoVault(store: store), auth: DemoAuthentication(store: store), sender: DemoSender(store: store),
            history: DemoHistory(store: store), receive: DemoReceive(store: store, uri: uri),
            coinControl: DemoCoinControl(store: store), addressBook: DemoAddressBook(store: store),
            messages: DemoMessages(store: store), uri: uri, amounts: DemoAmounts(network: store.activeNetwork),
            settings: settings, preferences: settings, screenCapture: screenCapture, timing: timing,
            developerMode: true)
    }
}
#endif
