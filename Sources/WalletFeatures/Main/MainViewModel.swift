// App composition: sidebar, routes, overlays and the network-scoped page
// view models (QT-011…014, QT-019, IOS-013, IOS-018).
import Foundation
import Observation
import WalletRuntime

@MainActor
@Observable
public final class MainViewModel {
    public let features: FeatureFlags
    public var selection: SidebarItem = .overview
    public var sheet: SheetRoute?
    public private(set) var network: DashNetwork?
    /// Lifecycle overlay state (IOS-018).
    public private(set) var transition: LifecycleTransition = .idle
    public private(set) var lockState: VaultLockState?
    /// `nil` until the wallets of the network are loaded.
    public private(set) var wallets: [WalletInfo]?
    public private(set) var selectedWalletID: WalletID?
    public private(set) var errorMessage: String?

    // Page view models of the active network; rebuilt when it changes.
    public private(set) var home: HomeViewModel?
    public private(set) var send: SendViewModel?
    public private(set) var receive: ReceiveViewModel?
    public private(set) var transactions: TransactionsViewModel?
    public let lock: LockViewModel
    public let settings: SettingsViewModel
    /// Present while the network has no wallet (QT-013).
    public private(set) var onboarding: OnboardingViewModel?

    public var visibleSidebarItems: [SidebarItem] { SidebarItem.visible(with: features) }
    /// Wallet selector only with two or more wallets (QT-014).
    public var showsWalletSelector: Bool { (wallets?.count ?? 0) >= 2 }
    public var needsOnboarding: Bool { wallets?.isEmpty ?? false }
    public var showsLockScreen: Bool { lockState == .locked }
    public var showsTransitionOverlay: Bool { transition != .idle }

    /// Sync rates for the sync overlay; the window records each sync status.
    public let syncRates = SyncRateTracker()
    /// The user asked for the sync overlay (status bar).
    public var syncOverlayRequested = false
    /// The user hid the sync overlay; it stays hidden until asked for again.
    public private(set) var syncOverlayHidden = false

    /// The sync overlay (QT-027): over the wallet while it catches up, when
    /// asked for, or by itself while the tip is more than 25 minutes old
    /// unless hidden.
    public var showsSyncOverlay: Bool {
        guard !needsOnboarding, !showsLockScreen else { return false }
        return SyncRateTracker.showsOverlay(home?.sync, requested: syncOverlayRequested, hidden: syncOverlayHidden)
    }

    public func hideSyncOverlay() {
        syncOverlayHidden = true
        syncOverlayRequested = false
    }

    /// "Dash Wallet - <wallet> - [testnet]"; the network tag appears on
    /// every non-mainnet network (QT-011, dash-qt quirk #1 fixed).
    public var windowTitle: String {
        var parts = [L10n.Navigation.appName]
        if let id = selectedWalletID, let wallet = wallets?.first(where: { $0.id == id }), !wallet.name.isEmpty {
            parts.append(wallet.name)
        }
        if let tag = network.flatMap(Self.networkTag) { parts.append(tag) }
        return parts.joined(separator: " - ")
    }

    private let env: AppEnvironment
    private var tasks: [Task<Void, Never>] = []

    public init(env: AppEnvironment, features: FeatureFlags = .m1) {
        self.env = env
        self.features = features
        self.lock = LockViewModel(auth: env.auth, vault: env.vault, timing: env.timing)
        self.settings = SettingsViewModel(env: env)
        self.lockState = env.auth.lockState
    }

    /// Loads the active network and its wallets and starts following changes.
    public func start() async {
        stopObserving()
        await reloadNetwork()
        await lock.load()
        lock.start()
        settings.start()
        let transitions = env.lifecycle.transitions()
        let walletChanges = env.walletState.changes()
        let lockChanges = env.auth.lockStateChanges()
        tasks.append(Task { [weak self] in
            for await transition in transitions {
                guard let self else { return }
                self.transition = transition
                if transition == .idle { await self.reloadNetwork() }
            }
        })
        tasks.append(Task { [weak self] in
            for await _ in walletChanges {
                guard let self else { return }
                await self.reloadWallets()
            }
        })
        tasks.append(Task { [weak self] in
            for await state in lockChanges {
                guard let self else { return }
                self.lockState = state
            }
        })
    }

    public func stop() {
        stopObserving()
        lock.stop()
        settings.stop()
        stopPages()
    }

    /// Performs a route a page view model raised.
    public func navigate(_ route: AppRoute) async {
        switch route {
        case .section(let item):
            guard item.isVisible(with: features) else { return }
            selection = item
        case .transaction(let txid):
            selection = .transactions
            await transactions?.reveal(txid: txid)
        case .send(let paymentURI):
            selection = .send
            send?.fill(from: paymentURI)
        }
    }

    /// A `dash:` URI from the OS, drag and drop or File ▸ Open URI (QT-019).
    public func open(uri text: String) async {
        do {
            let parsed = try env.uri.parsePaymentURI(text)
            errorMessage = nil
            await navigate(.send(parsed))
        } catch {
            errorMessage = L10n.Send.invalidAddress
        }
    }

    /// Cmd/Alt+N, numbered by the visible sidebar items (QT-012).
    public func selectShortcut(_ number: Int) {
        let items = visibleSidebarItems
        guard (1...items.count).contains(number) else { return }
        selection = items[number - 1]
    }

    public func selectWallet(_ id: WalletID) async {
        env.walletState.select(id)
        await reloadWallets()
    }

    // MARK: Private

    private func reloadNetwork() async {
        let active = await env.host.activeNetwork
        let rebuild = active != network || home == nil
        if rebuild {
            network = active
            rebuildPages()
        }
        await reloadWallets(forceRefresh: rebuild)
    }

    private func reloadWallets(forceRefresh: Bool = false) async {
        let previous = selectedWalletID
        wallets = env.walletState.wallets
        selectedWalletID = env.walletState.selectedWalletID
        if needsOnboarding {
            if onboarding == nil {
                let model = OnboardingViewModel(env: env)
                onboarding = model
                await model.load()
            }
        } else if case .done = onboarding?.step {
            onboarding = nil
        }
        if forceRefresh || previous != selectedWalletID { await refreshPages() }
    }

    private func rebuildPages() {
        stopPages()
        guard let network else {
            home = nil
            send = nil
            receive = nil
            transactions = nil
            return
        }
        home = HomeViewModel(env: env, network: network, features: features)
        send = SendViewModel(env: env, network: network)
        receive = ReceiveViewModel(env: env)
        transactions = TransactionsViewModel(env: env, features: features)
        home?.start()
    }

    /// Reloads the pages after the selected wallet changed.
    private func refreshPages() async {
        guard selectedWalletID != nil else { return }
        home?.start()
        transactions?.start()
        receive?.start()
        await home?.refresh()
        await transactions?.reload()
        await receive?.load()
    }

    private func stopPages() {
        home?.stop()
        transactions?.stop()
        receive?.stop()
    }

    private func stopObserving() {
        tasks.forEach { $0.cancel() }
        tasks = []
    }

    static func networkTag(_ network: DashNetwork) -> String? {
        switch network {
        case .mainnet: nil
        case .testnet: "[testnet]"
        case .regtest: "[regtest]"
        case .devnet(let name): "[devnet: \(name)]"
        }
    }
}
