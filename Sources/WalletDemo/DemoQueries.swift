// History, receive, coin control, address book and messages over
// `DemoWorld`, with the engine's checks and error codes (m1-engine.md
// §2.5–2.10).
import Foundation
import WalletRuntime

final class DemoHistory: HistoryProviding {
    let world: DemoWorld

    init(world: DemoWorld) {
        self.world = world
    }

    func page(wallet: WalletID, query: HistoryQuery) async throws(ServiceError) -> HistoryPage {
        try await world.ledger(wallet).page(query)
    }

    func detail(wallet: WalletID, txid: String) async throws(ServiceError) -> TransactionDetail {
        try Self.checkTxid(txid)
        guard let detail = try await world.ledger(wallet).detail(txid: txid) else { throw .demo(.historyTxNotFound) }
        return detail
    }

    func setLabel(wallet: WalletID, txid: String, label: String?) async throws(ServiceError) {
        try Self.checkTxid(txid)
        try await setLabel(wallet: wallet, txid: txid, to: label)
    }

    @MainActor
    private func setLabel(wallet: WalletID, txid: String, to label: String?) throws(ServiceError) {
        try world.update(wallet) { (ledger: inout DemoLedger) throws(ServiceError) in
            ledger.txLabels[txid] = label?.isEmpty == false ? label : nil
        }
        world.notifyHistory(wallet, txids: [txid])
    }

    func changes(wallet: WalletID) -> AsyncStream<[String]> {
        let source = world.historyChanges.stream()
        return AsyncStream { continuation in
            let task = Task {
                for await (changed, txids) in source where changed == wallet { continuation.yield(txids) }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// The engine takes 64 lowercase hex characters only.
    static func checkTxid(_ txid: String) throws(ServiceError) {
        guard txid.count == 64, txid.allSatisfy({ $0.isHexDigit && !$0.isUppercase }) else {
            throw .demo(.invalidArgument, "txid")
        }
    }
}

final class DemoReceive: ReceiveProviding {
    let world: DemoWorld
    let uri: any URIHandling

    init(world: DemoWorld, uri: any URIHandling) {
        self.world = world
        self.uri = uri
    }

    func currentAddress(wallet: WalletID) async throws(ServiceError) -> AddressInfo {
        let ledger = try await world.ledger(wallet)
        return ledger.addressInfo(receiving: try ledger.currentReceiveIndex())
    }

    func nextAddress(wallet: WalletID, label: String?) async throws(ServiceError) -> AddressInfo {
        try await issue(wallet: wallet, label: label)
    }

    @MainActor
    private func issue(wallet: WalletID, label: String?) throws(ServiceError) -> AddressInfo {
        let now = world.now()
        return try world.update(wallet) { (ledger: inout DemoLedger) throws(ServiceError) -> AddressInfo in
            let index = try ledger.issueReceiveAddress()
            let address = ledger.receiveAddresses[index]
            if let label, !label.isEmpty, !ledger.addressBook.contains(where: { $0.address == address }) {
                ledger.addressBook.append(AddressBookEntry(address: address, label: label, purpose: .receive, createdAt: now))
            }
            return ledger.addressInfo(receiving: index)
        }
    }

    func addresses(wallet: WalletID, filter: AddressFilter) async throws(ServiceError) -> [AddressInfo] {
        let ledger = try await world.ledger(wallet)
        let receiving = (0..<DemoLedger.receiveCount).map { ledger.addressInfo(receiving: $0) }
        let change = (0..<DemoLedger.changeCount).map { ledger.addressInfo(change: $0) }
        return (receiving + change).filter { info in
            (filter.chain == nil || filter.chain == info.chain) && (filter.used == nil || filter.used == info.used)
        }
    }

    func createRequest(
        wallet: WalletID, amount: Amount?, label: String?, message: String?
    ) async throws(ServiceError) -> ReceiveRequest {
        if let amount, amount.duffs < 0 || amount.duffs > DemoDraft.maxMoney { throw .demo(.invalidArgument, "amount") }
        let requested = amount.flatMap { $0.duffs == 0 ? nil : $0 }
        let address = try await issue(wallet: wallet, label: label).address
        let text = try uri.buildPaymentURI(address: address, amount: requested, label: label, message: message)
        return try await addRequest(
            wallet: wallet, address: address, amount: requested, label: label, message: message, uri: text)
    }

    @MainActor
    private func addRequest(
        wallet: WalletID, address: String, amount: Amount?, label: String?, message: String?, uri: String
    ) throws(ServiceError) -> ReceiveRequest {
        let now = world.now()
        return try world.update(wallet) { (ledger: inout DemoLedger) throws(ServiceError) -> ReceiveRequest in
            ledger.addRequest(address: address, amount: amount, label: label, message: message, uri: uri, now: now)
        }
    }

    /// Newest first.
    func requests(wallet: WalletID) async throws(ServiceError) -> [ReceiveRequest] {
        try await world.ledger(wallet).requests.sorted { $0.createdAt > $1.createdAt }
    }

    func deleteRequest(wallet: WalletID, id: UInt64) async throws(ServiceError) {
        try await deleteRequest(wallet: wallet, requestID: id)
    }

    @MainActor
    private func deleteRequest(wallet: WalletID, requestID: UInt64) throws(ServiceError) {
        try world.update(wallet) { (ledger: inout DemoLedger) throws(ServiceError) in
            guard ledger.requests.contains(where: { $0.id == requestID }) else { throw .demo(.receiveRequestNotFound) }
            ledger.requests.removeAll { $0.id == requestID }
        }
    }
}

final class DemoCoinControl: CoinControlProviding {
    let world: DemoWorld

    init(world: DemoWorld) {
        self.world = world
    }

    /// Every unspent output, largest first.
    func utxos(wallet: WalletID, filter: UtxoFilter) async throws(ServiceError) -> [Utxo] {
        guard !filter.fullyMixedOnly else { throw .demo(.notImplemented, "CoinJoin rounds are not tracked yet") }
        let ledger = try await world.ledger(wallet)
        return ledger.coins.sorted { $0.amount > $1.amount }.compactMap { coin in
            let locked = ledger.lockedOutpoints.contains(coin.outpoint)
            if locked && !filter.includeLocked { return nil }
            if let minimum = filter.minimumConfirmations, coin.confirmations < minimum { return nil }
            let reserved = ledger.reserved.contains(coin.outpoint)
            return Utxo(
                outpoint: coin.outpoint, address: coin.address, amount: Amount(duffs: coin.amount),
                confirmations: coin.confirmations, date: coin.confirmations > 0 ? coin.date : nil,
                instantLocked: coin.instantLocked, chainLocked: coin.confirmations > 0, userLocked: locked,
                reserved: reserved, label: ledger.addressBook.first { $0.address == coin.address }?.label,
                isChange: coin.isChange, coinJoinDenominated: false, coinJoinRounds: nil,
                spendable: coin.trusted && !locked && !reserved)
        }
    }

    func lock(wallet: WalletID, outpoints: [OutPoint]) async throws(ServiceError) {
        try await change(wallet, outpoints, lock: true)
    }

    func unlock(wallet: WalletID, outpoints: [OutPoint]) async throws(ServiceError) {
        try await change(wallet, outpoints, lock: false)
    }

    /// Active locks, in the order they were made is not kept: sorted by outpoint.
    func lockedOutpoints(wallet: WalletID) async throws(ServiceError) -> [OutPoint] {
        try await world.ledger(wallet).lockedOutpoints.sorted { ($0.txid, $0.vout) < ($1.txid, $1.vout) }
    }

    @MainActor
    private func change(_ wallet: WalletID, _ outpoints: [OutPoint], lock: Bool) throws(ServiceError) {
        try world.update(wallet) { (ledger: inout DemoLedger) throws(ServiceError) in
            if lock {
                let unspent = Set(ledger.coins.map(\.outpoint))
                if let missing = outpoints.first(where: { !unspent.contains($0) }) {
                    throw .demo(.coinsOutpointNotFound, "\(missing.txid):\(missing.vout)")
                }
                ledger.lockedOutpoints.formUnion(outpoints)
            } else {
                ledger.lockedOutpoints.subtract(outpoints)
            }
        }
    }
}

final class DemoAddressBook: AddressBookProviding {
    let world: DemoWorld
    let addresses: any URIHandling

    init(world: DemoWorld, addresses: any URIHandling) {
        self.world = world
        self.addresses = addresses
    }

    /// Sorted by label (case-insensitive, unlabelled last), then address;
    /// `search` is dash-qt's wildcard match (`*`, `?`) anywhere in the label
    /// or address.
    func entries(wallet: WalletID, purpose: AddressPurpose?, search: String?) async throws(ServiceError)
        -> [AddressBookEntry]
    {
        let book = try await world.ledger(wallet).addressBook
        let matcher = search.flatMap { $0.isEmpty ? nil : Self.wildcard($0) }
        return book.filter { entry in
            (purpose == nil || entry.purpose == purpose)
                && (matcher.map { $0(entry.label) || $0(entry.address) } ?? true)
        }
        .sorted { lhs, rhs in
            switch (lhs.label.isEmpty, rhs.label.isEmpty) {
            case (false, true): return true
            case (true, false): return false
            default:
                let order = lhs.label.localizedCaseInsensitiveCompare(rhs.label)
                return order == .orderedSame ? lhs.address < rhs.address : order == .orderedAscending
            }
        }
    }

    func save(
        wallet: WalletID, address: String, label: String, purpose: AddressPurpose, replace: Bool
    ) async throws(ServiceError) -> AddressBookEntry {
        guard case .core = addresses.classifyAddress(address) else { throw .demo(.labelsInvalidAddress) }
        return try await save(wallet: wallet, entry: AddressBookEntry(
            address: address, label: label, purpose: purpose, createdAt: nil), replace: replace)
    }

    @MainActor
    private func save(wallet: WalletID, entry: AddressBookEntry, replace: Bool) throws(ServiceError) -> AddressBookEntry {
        let now = world.now()
        return try world.update(wallet) { (ledger: inout DemoLedger) throws(ServiceError) -> AddressBookEntry in
            let own = ledger.isOwn(entry.address)
            if entry.purpose == .send, own { throw .demo(.labelsOwnAddress) }
            if entry.purpose == .receive, !own { throw .demo(.invalidArgument, "not one of the wallet's addresses") }
            if let index = ledger.addressBook.firstIndex(where: { $0.address == entry.address }) {
                let old = ledger.addressBook[index]
                guard replace, old.purpose == entry.purpose else { throw .demo(.labelsDuplicateAddress) }
                let updated = AddressBookEntry(
                    address: old.address, label: entry.label, purpose: old.purpose, createdAt: old.createdAt)
                ledger.addressBook[index] = updated
                return updated
            }
            let added = AddressBookEntry(address: entry.address, label: entry.label, purpose: entry.purpose, createdAt: now)
            ledger.addressBook.append(added)
            return added
        }
    }

    func delete(wallet: WalletID, address: String) async throws(ServiceError) {
        try await delete(wallet: wallet, entry: address)
    }

    @MainActor
    private func delete(wallet: WalletID, entry address: String) throws(ServiceError) {
        try world.update(wallet) { (ledger: inout DemoLedger) throws(ServiceError) in
            guard let entry = ledger.addressBook.first(where: { $0.address == address }) else {
                throw .demo(.labelsEntryNotFound)
            }
            guard entry.purpose == .send else { throw .demo(.labelsReceiveEntryNotDeletable) }
            ledger.addressBook.removeAll { $0.address == address }
        }
    }

    /// dash-qt's case-insensitive wildcard match anywhere in the text.
    static func wildcard(_ pattern: String) -> @Sendable (String) -> Bool {
        let escaped = NSRegularExpression.escapedPattern(for: pattern.lowercased())
            .replacingOccurrences(of: "\\*", with: ".*").replacingOccurrences(of: "\\?", with: ".")
        let regex = try? NSRegularExpression(pattern: escaped)
        return { text in
            let lower = text.lowercased()
            guard let regex else { return lower.contains(pattern.lowercased()) }
            return regex.firstMatch(in: lower, range: NSRange(lower.startIndex..., in: lower)) != nil
        }
    }
}

/// Verification is the engine's. The sample wallets' addresses are random
/// and no key for them exists, so signing fails after the checks the engine
/// makes first (address is the wallet's, grant), with
/// `message.address_no_key`, instead of inventing a signature that only the
/// demo would accept.
final class DemoMessages: MessageSigning {
    let world: DemoWorld

    init(world: DemoWorld) {
        self.world = world
    }

    func sign(wallet: WalletID, address: String, message: String, grant: AuthGrant) async throws(ServiceError)
        -> String
    {
        guard case .signMessage = grant.purpose else {
            throw .demo(.vaultGrantPurposeMismatch, "signing a message needs a signMessage grant")
        }
        try await check(wallet: wallet, address: address, grant: grant)
        throw .demo(.messageAddressNoKey, "demo wallets hold no keys")
    }

    @MainActor
    private func check(wallet: WalletID, address: String, grant: AuthGrant) throws(ServiceError) {
        let ledger = try world.ledger(wallet)
        guard ledger.isOwn(address) else { throw .demo(.messageAddressNotMine) }
        try world.check(grant, .signMessage, wallet: wallet, refuse: .message, locked: .messageVaultLocked)
        try world.redeem(grant, .signMessage, wallet: wallet, refuse: .message)
    }

    func verify(address: String, message: String, signature: String) throws(ServiceError) {
        try EngineFunctions.verifyMessage(
            address: address, message: message, signature: signature, network: world.activeNetwork.current)
    }
}
