// Wallet list, selection and balances (iOS `SwiftDashSDKWalletState`).
import DashKit
import Foundation
import Observation

/// Wallets and Core balance buckets of the open network (QT-034).
///
/// - `wallets` is `nil` until the first successful `wallet_infos` of a
///   session; if the engine cannot list wallets, `lastError` says why and
///   `wallets` stays `nil` (never an invented empty list).
/// - `balances` are the selected wallet's buckets: `nil` while no wallet is
///   selected or its balances have not been read (iOS rule 7).
/// - The selection survives reloads while the wallet exists; otherwise the
///   first wallet (engine creation order) is selected.
/// - Engine events: `WalletCreated` / `WalletRemoved` /
///   `SessionOpened` / `.resynchronize` reload the list (coalesced);
///   `Balances` re-reads that wallet's buckets only.
@MainActor
@Observable
public final class WalletState: WalletStateProviding, SessionObserving {
    public private(set) var wallets: [WalletInfo]?
    public private(set) var selectedWalletID: WalletID?
    public private(set) var balances: WalletBalances?
    /// The last failure to read wallets or balances; cleared by the next success.
    public private(set) var lastError: ServiceError?

    @ObservationIgnored private let engine: any EngineProtocol
    @ObservationIgnored private var network: DashKit.DashNetwork?
    @ObservationIgnored private let notifier = ChangeNotifier()
    @ObservationIgnored private let tasks = TaskBag()
    @ObservationIgnored private var reloader: Coalescer!
    /// Wallets whose balances changed since the last balance pass.
    @ObservationIgnored private var staleBalances: Set<WalletID> = []
    @ObservationIgnored private var balanceUpdater: Coalescer!

    public init(engine: any EngineProtocol) {
        self.engine = engine
        reloader = Coalescer { [weak self] in await self?.reload() }
        balanceUpdater = Coalescer { [weak self] in await self?.updateBalances() }
    }

    public func changes() -> AsyncStream<Void> {
        notifier.stream()
    }

    /// Selects `id` if it is one of `wallets`; the balances follow.
    public func select(_ id: WalletID) {
        guard selectedWalletID != id, wallets?.contains(where: { $0.id == id }) ?? false else { return }
        selectedWalletID = id
        balances = wallets?.first(where: { $0.id == id })?.balances
        notifier.notify()
        staleBalances.insert(id)
        balanceUpdater.request()
    }

    public func rename(_ id: WalletID, to name: String) async throws(ServiceError) {
        guard let network else { throw ServiceError(code: .networkNotOpen, detail: "no network is open") }
        let wallet = try id.kit
        let engine = engine
        try await serviceCall { () async throws(DashKitError) in
            try await engine.renameWallet(on: network, wallet: wallet, name: name)
        }
        reloader.request()
        await reloader.idle()
    }

    /// Returns once every requested reload has run (tests, diagnostics).
    public func settle() async {
        await reloader.idle()
        await balanceUpdater.idle()
    }

    // MARK: SessionObserving

    public func sessionDidStart(_ network: DashKit.DashNetwork) async {
        self.network = network
        clear()
        startEventPump()
        reloader.request()
        await reloader.idle()
    }

    public func sessionWillStop(_ network: DashKit.DashNetwork) async {
        self.network = nil
        tasks.cancelAll()
        reloader.cancel()
        balanceUpdater.cancel()
        clear()
        notifier.notify()
    }

    public func walletsDidChange(_ network: DashKit.DashNetwork) async {
        reloader.request()
        await reloader.idle()
    }

    // MARK: Private

    private func clear() {
        wallets = nil
        selectedWalletID = nil
        balances = nil
        lastError = nil
        staleBalances = []
    }

    private func startEventPump() {
        let subscription = engine.events.subscribe()
        tasks.set("events", Task { [weak self] in
            for await event in subscription {
                guard let self else { return }
                self.handle(event)
            }
        })
    }

    private func handle(_ event: EngineEvent) {
        guard let network else { return }
        switch event {
        case .walletCreated(let n, _) where n == network, .walletRemoved(let n, _) where n == network,
             .sessionOpened(let n) where n == network:
            reloader.request()
        case .balancesChanged(let n, let id) where n == network:
            staleBalances.insert(WalletID(id))
            balanceUpdater.request()
        case .resynchronize:
            reloader.request()
        default:
            break
        }
    }

    private func reload() async {
        guard let network else { return }
        let infos: [DashKit.WalletInfo]
        do {
            infos = try await engine.walletInfos(on: network)
        } catch {
            if self.network == network {
                lastError = ServiceError(error)
                notifier.notify()
            }
            return
        }
        guard self.network == network else { return }
        lastError = nil
        let list = infos.map(WalletInfo.init)
        wallets = list
        if let selected = selectedWalletID, list.contains(where: { $0.id == selected }) {
            // Keep the selection.
        } else {
            selectedWalletID = list.first?.id
        }
        balances = selectedWalletID.flatMap { id in list.first(where: { $0.id == id })?.balances }
        // `staleBalances` is kept: a `Balances` event that arrived while
        // `wallet_infos` ran may be newer than the list.
        notifier.notify()
    }

    private func updateBalances() async {
        guard let network else { return }
        let pending = staleBalances
        staleBalances = []
        var changed = false
        for id in pending {
            guard let wallet = try? id.kit else { continue }
            // `nil` until the wallet's scan has passed its birth height.
            let kitBalances: DashKit.WalletBalances?
            do {
                kitBalances = try await engine.balances(on: network, wallet: wallet)
            } catch {
                guard self.network == network else { return }
                lastError = ServiceError(error)
                changed = true
                continue
            }
            guard self.network == network else { return }
            let fresh = kitBalances.map(WalletBalances.init)
            if let index = wallets?.firstIndex(where: { $0.id == id }), let old = wallets?[index] {
                wallets?[index] = WalletInfo(
                    id: old.id, name: old.name, watchOnly: old.watchOnly, hasMnemonic: old.hasMnemonic, hd: old.hd,
                    birthHeight: old.birthHeight, createdAt: old.createdAt, balances: fresh)
            }
            if id == selectedWalletID, balances != fresh {
                balances = fresh
            }
            changed = true
        }
        if changed { notifier.notify() }
    }
}
