// Thin adapters from the M1 query/command protocols to `EngineProtocol`:
// history, receive, coin control, address book. Each call goes to the open
// network; with none open it fails with `network_not_open`. Engine calls that
// have not landed yet fail with the engine's `not_implemented`.
import DashKit
import Foundation

/// `HistoryProviding` over engine `history_page` / `tx_detail` / `set_tx_label`.
public final class HistoryService: HistoryProviding {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func page(wallet: WalletID, query: HistoryQuery) async throws(ServiceError) -> HistoryPage {
        let id = try wallet.kit
        let kitQuery = query.kit
        return HistoryPage(try await context.call { engine, network throws(DashKitError) in
            try await engine.historyPage(on: network, wallet: id, query: kitQuery)
        })
    }

    public func detail(wallet: WalletID, txid: String) async throws(ServiceError) -> TransactionDetail {
        let id = try wallet.kit
        return TransactionDetail(try await context.call { engine, network throws(DashKitError) in
            try await engine.txDetail(on: network, wallet: id, txid: txid)
        })
    }

    public func setLabel(wallet: WalletID, txid: String, label: String?) async throws(ServiceError) {
        let id = try wallet.kit
        try await context.call { engine, network throws(DashKitError) in
            try await engine.setTxLabel(on: network, wallet: id, txid: txid, label: label)
        }
    }

    /// One stream per call. It yields the txids of each engine
    /// `HistoryChanged` for `wallet` on the open network; values the consumer
    /// has not taken yet are merged (an empty list, "reload all", absorbs the
    /// rest). An `EventBus` `.resynchronize` yields `[]`. The stream ends when
    /// the consumer stops iterating or the engine shuts down.
    public func changes(wallet: WalletID) -> AsyncStream<[String]> {
        let signal = TxidSignal()
        let subscription = context.engine.events.subscribe()
        let active = context.active
        let target = wallet.hex
        let pump = Task {
            for await event in subscription {
                switch event {
                case .historyChanged(let network, let id, let txids)
                where id.hex == target && network == active.network:
                    signal.send(txids)
                case .resynchronize:
                    signal.send([])
                default:
                    break
                }
            }
            signal.finish()
        }
        signal.onTermination { pump.cancel() }
        return signal.stream
    }
}

/// `ReceiveProviding` over engine `receive.rs`.
public final class ReceiveService: ReceiveProviding {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func currentAddress(wallet: WalletID) async throws(ServiceError) -> AddressInfo {
        let id = try wallet.kit
        return AddressInfo(try await context.call { engine, network throws(DashKitError) in
            try await engine.currentReceiveAddress(on: network, wallet: id)
        })
    }

    public func nextAddress(wallet: WalletID, label: String?) async throws(ServiceError) -> AddressInfo {
        let id = try wallet.kit
        return AddressInfo(try await context.call { engine, network throws(DashKitError) in
            try await engine.nextReceiveAddress(on: network, wallet: id, label: label)
        })
    }

    public func addresses(wallet: WalletID, filter: AddressFilter) async throws(ServiceError) -> [AddressInfo] {
        let id = try wallet.kit
        let kitFilter = filter.kit
        return try await context.call { engine, network throws(DashKitError) in
            try await engine.addresses(on: network, wallet: id, filter: kitFilter)
        }.map(AddressInfo.init)
    }

    public func createRequest(wallet: WalletID, amount: Amount?, label: String?, message: String?)
        async throws(ServiceError) -> ReceiveRequest
    {
        let id = try wallet.kit
        let kitAmount = amount?.kit
        return ReceiveRequest(try await context.call { engine, network throws(DashKitError) in
            try await engine.createReceiveRequest(
                on: network, wallet: id, amount: kitAmount, label: label, message: message)
        })
    }

    public func requests(wallet: WalletID) async throws(ServiceError) -> [ReceiveRequest] {
        let id = try wallet.kit
        return try await context.call { engine, network throws(DashKitError) in
            try await engine.receiveRequests(on: network, wallet: id)
        }.map(ReceiveRequest.init)
    }

    public func deleteRequest(wallet: WalletID, id requestID: UInt64) async throws(ServiceError) {
        let id = try wallet.kit
        try await context.call { engine, network throws(DashKitError) in
            try await engine.deleteReceiveRequest(on: network, wallet: id, id: requestID)
        }
    }
}

/// `CoinControlProviding` over engine `coins.rs`.
public final class CoinControlService: CoinControlProviding {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func utxos(wallet: WalletID, filter: UtxoFilter) async throws(ServiceError) -> [Utxo] {
        let id = try wallet.kit
        let kitFilter = filter.kit
        return try await context.call { engine, network throws(DashKitError) in
            try await engine.utxos(on: network, wallet: id, filter: kitFilter)
        }.map(Utxo.init)
    }

    public func lock(wallet: WalletID, outpoints: [OutPoint]) async throws(ServiceError) {
        let id = try wallet.kit
        let kitOutpoints = outpoints.map(\.kit)
        try await context.call { engine, network throws(DashKitError) in
            try await engine.lockOutpoints(on: network, wallet: id, outpoints: kitOutpoints)
        }
    }

    public func unlock(wallet: WalletID, outpoints: [OutPoint]) async throws(ServiceError) {
        let id = try wallet.kit
        let kitOutpoints = outpoints.map(\.kit)
        try await context.call { engine, network throws(DashKitError) in
            try await engine.unlockOutpoints(on: network, wallet: id, outpoints: kitOutpoints)
        }
    }

    public func lockedOutpoints(wallet: WalletID) async throws(ServiceError) -> [OutPoint] {
        let id = try wallet.kit
        return try await context.call { engine, network throws(DashKitError) in
            try await engine.lockedOutpoints(on: network, wallet: id)
        }.map(OutPoint.init)
    }
}

/// `AddressBookProviding` over engine `labels.rs`.
public final class AddressBookService: AddressBookProviding {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func entries(wallet: WalletID, purpose: AddressPurpose?, search: String?) async throws(ServiceError)
        -> [AddressBookEntry]
    {
        let id = try wallet.kit
        let kitPurpose = purpose?.kit
        return try await context.call { engine, network throws(DashKitError) in
            try await engine.addressBook(on: network, wallet: id, purpose: kitPurpose, search: search)
        }.map(AddressBookEntry.init)
    }

    public func save(wallet: WalletID, address: String, label: String, purpose: AddressPurpose, replace: Bool)
        async throws(ServiceError) -> AddressBookEntry
    {
        let id = try wallet.kit
        let kitPurpose = purpose.kit
        return AddressBookEntry(try await context.call { engine, network throws(DashKitError) in
            try await engine.saveAddressBookEntry(
                on: network, wallet: id, address: address, label: label, purpose: kitPurpose, replace: replace)
        })
    }

    public func delete(wallet: WalletID, address: String) async throws(ServiceError) {
        let id = try wallet.kit
        try await context.call { engine, network throws(DashKitError) in
            try await engine.deleteAddressBookEntry(on: network, wallet: id, address: address)
        }
    }
}
