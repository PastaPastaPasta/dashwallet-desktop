// The runtime half of the composition root (docs/contracts/m1-swift.md §1).
import DashKit
import Foundation

/// Every WalletRuntime adapter for one app run, wired to one engine. Each
/// app's `@main` builds one (`live(…)`) and hands it to WalletFeatures'
/// `AppEnvironment`; there are no singletons.
///
/// Wiring:
/// - `WalletHost` owns the engine; `LifecycleQueue` is the only caller of
///   its `start`/`stop` and notifies the session observers in this order on
///   start (reverse on stop): `settings` (records the last network), `auth`
///   (lock state), `walletState` (wallets, balances), `sync` (SPV status).
/// - The query/command services read the open network from the host's
///   `ActiveNetwork` at each call.
/// - `VaultService` forwards every status it gets to `auth`, and `auth`
///   reads "require authentication for every payment" from `settings`.
///
/// Call `shutdown()` before the app exits: it stops SPV, closes the session
/// and releases the engine on the engine's actor, not the main thread
/// (review L2).
@MainActor
public final class WalletRuntimeServices {
    public let host: WalletHost
    public let lifecycle: LifecycleQueue
    public let walletState: WalletState
    public let sync: SPVCoordinator
    public let vault: VaultService
    public let auth: AuthenticationGate
    public let sender: TransactionSender
    public let history: HistoryService
    public let receive: ReceiveService
    public let coinControl: CoinControlService
    public let addressBook: AddressBookService
    public let messages: MessageService
    public let uri: URIService
    public let amounts: EngineAmountFormatter
    public let settings: SettingsStore

    /// Builds the services on the Rust engine.
    ///
    /// - Parameters:
    ///   - dataRoot: directory with one sub-directory per network; also holds
    ///     `settings.json` and `global.json`.
    ///   - workerThreads: engine tokio workers; `nil` = one per core.
    ///   - networkOptions: endpoints for a network, read each time it starts.
    ///   - fallbackNetwork: network the pure functions (units, URIs) use
    ///     before any session was opened: the last network, else this.
    public static func live(
        dataRoot: URL,
        workerThreads: UInt32? = nil,
        networkOptions: @escaping @Sendable (DashNetwork) -> NetworkOptions = { _ in NetworkOptions() },
        fallbackNetwork: DashNetwork = .mainnet
    ) throws(ServiceError) -> WalletRuntimeServices {
        let engine = try serviceCall { () throws(DashKitError) in
            try EngineClient(dataRoot: dataRoot, workerThreads: workerThreads)
        }
        let settings = SettingsStore(directory: dataRoot)
        return WalletRuntimeServices(
            engine: engine, settings: settings, networkOptions: networkOptions,
            fallbackNetwork: settings.lastNetwork ?? fallbackNetwork)
    }

    /// Builds the services on any engine (tests use a fake).
    public init(
        engine: any EngineProtocol,
        settings: SettingsStore,
        networkOptions: @escaping @Sendable (DashNetwork) -> NetworkOptions = { _ in NetworkOptions() },
        fallbackNetwork: DashNetwork = .mainnet,
        clock: any RuntimeClock = SystemClock(),
        authorizeTimeout: Duration = AuthenticationGate.defaultAuthorizeTimeout
    ) {
        let host = WalletHost(engine: engine)
        let context = EngineContext(engine: engine, active: host.active, fallbackNetwork: fallbackNetwork.kit)
        let auth = AuthenticationGate(
            engine: engine, clock: clock, authorizeTimeout: authorizeTimeout,
            requireAuthenticationForEveryPayment: { [weak settings] in
                settings?.requireAuthenticationForEveryPayment ?? true
            })
        let walletState = WalletState(engine: engine)
        let sync = SPVCoordinator(engine: engine, clock: clock)
        self.host = host
        self.settings = settings
        self.auth = auth
        self.walletState = walletState
        self.sync = sync
        lifecycle = LifecycleQueue(
            host: host, options: networkOptions, observers: [settings, auth, walletState, sync])
        vault = VaultService(context: context, statusObserver: { [weak auth] status in
            await auth?.apply(status)
        })
        sender = TransactionSender(context: context)
        history = HistoryService(context: context)
        receive = ReceiveService(context: context)
        coinControl = CoinControlService(context: context)
        addressBook = AddressBookService(context: context)
        messages = MessageService(context: context)
        uri = URIService(context: context)
        amounts = EngineAmountFormatter(context: context)
    }

    /// Opens the last network (`settings.lastNetwork`), else `defaultNetwork`.
    public func launch(defaultNetwork: DashNetwork = .mainnet) async throws(ServiceError) {
        try await lifecycle.start(network: settings.lastNetwork ?? defaultNetwork)
    }

    /// Stops the open network and releases the engine. Call once, at quit.
    public func shutdown() async throws(ServiceError) {
        try await lifecycle.shutdown()
    }
}
