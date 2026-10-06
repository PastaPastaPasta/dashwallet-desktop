// One demo wallet's chain data: coins, transactions, addresses, address book
// and payment requests. The coins are the wallet's unspent outputs and the
// balances are their sum, so a payment made in demo mode spends coins,
// leaves change and shows up in the history like one made on the engine.
import Foundation
import WalletRuntime

/// An unspent output of a demo wallet.
struct DemoCoin: Sendable, Hashable {
    let outpoint: OutPoint
    let address: String
    let amount: Int64
    let confirmations: UInt32
    let date: Date?
    let instantLocked: Bool
    let isChange: Bool

    /// Confirmed, InstantSend-locked or own change: what the engine spends.
    var trusted: Bool { confirmations > 0 || instantLocked || isChange }
}

/// A transaction the wallet took part in.
struct DemoTransaction: Sendable {
    let txid: String
    let type: TxType
    let date: Date
    let confirmations: UInt32
    let instantLocked: Bool
    /// What the wallet's balance changed by, fee included.
    let net: Int64
    let fee: Int64?
    /// The counterparty (sends) or the receiving address (receives); `nil`
    /// when dash-qt shows none (received from an unknown sender).
    let address: String?
    let inputs: [TxInput]
    let outputs: [TxOutput]
    var message: String?
}

struct DemoLedger: Sendable {
    static let tipHeight: UInt32 = 1_234_567
    /// Target spacing, 2.5 minutes.
    static let blockSeconds: TimeInterval = 150
    /// Addresses per chain the demo derives; asking for more is `receive.gap_limit`.
    static let receiveCount = 40
    static let changeCount = 20

    let network: DashNetwork
    private(set) var transactions: [DemoTransaction] = []
    var coins: [DemoCoin] = []
    /// Inputs of prepared transactions that are neither sent nor released.
    var reserved: Set<OutPoint> = []
    var lockedOutpoints: Set<OutPoint> = []
    let receiveAddresses: [String]
    let changeAddresses: [String]
    private var nextChange = 0
    /// Receive addresses handed out by `nextAddress` or a payment request.
    private var issued: Set<Int> = []
    var txLabels: [String: String] = [:]
    var addressBook: [AddressBookEntry] = []
    var requests: [ReceiveRequest] = []
    private var nextRequestID: UInt64 = 1
    var rng: DemoRandom

    init(seed: String, network: DashNetwork) {
        self.network = network
        var rng = DemoRandom(text: seed + "|" + network.description)
        receiveAddresses = (0..<Self.receiveCount).map { _ in rng.address(on: network) }
        changeAddresses = (0..<Self.changeCount).map { _ in rng.address(on: network) }
        self.rng = rng
    }

    // MARK: Sample history

    /// One row of the scripted history: age, dash-qt type, DASH paid (negative)
    /// or received, counterparty.
    private struct Script {
        let hoursAgo: Double
        let type: TxType
        let dash: Double
        let contact: String?
        var instantLocked = true
        var confirmations: UInt32?
    }

    /// The fee every scripted payment pays (a one-input, two-output transaction at 1000 duff/kB).
    static let scriptFee: Int64 = 226

    /// A funded wallet with about four months of history: 25 transactions,
    /// five saved contacts and one open payment request.
    static func funded(seed: String, network: DashNetwork, now: Date) -> DemoLedger {
        var ledger = DemoLedger(seed: seed, network: network)
        let scripts: [Script] = [
            Script(hoursAgo: 2_700, type: .recvWithAddress, dash: 10.0, contact: "Opening balance"),
            Script(hoursAgo: 2_400, type: .sendToAddress, dash: -0.6, contact: "Alice"),
            Script(hoursAgo: 2_150, type: .recvWithAddress, dash: 5.0, contact: "Salary"),
            Script(hoursAgo: 1_900, type: .sendToAddress, dash: -1.75, contact: "Exchange"),
            Script(hoursAgo: 1_700, type: .recvWithAddress, dash: 2.5, contact: "Bob"),
            Script(hoursAgo: 1_500, type: .sendToAddress, dash: -0.12, contact: "Hosting"),
            Script(hoursAgo: 1_330, type: .sendToAddress, dash: -0.25, contact: "Carol"),
            Script(hoursAgo: 1_200, type: .recvWithAddress, dash: 5.0, contact: "Salary"),
            Script(hoursAgo: 1_060, type: .sendToAddress, dash: -0.9, contact: "Rent share"),
            Script(hoursAgo: 950, type: .recvWithAddress, dash: 0.4, contact: "Carol"),
            Script(hoursAgo: 820, type: .sendToAddress, dash: -0.08, contact: "Hosting"),
            Script(hoursAgo: 760, type: .dustReceive, dash: 0.00000547, contact: nil),
            Script(hoursAgo: 700, type: .recvWithAddress, dash: 5.0, contact: "Salary"),
            Script(hoursAgo: 610, type: .sendToAddress, dash: -2.1, contact: "Exchange"),
            Script(hoursAgo: 500, type: .recvFromOther, dash: 0.05, contact: nil),
            Script(hoursAgo: 430, type: .sendToAddress, dash: -0.333, contact: "Alice"),
            Script(hoursAgo: 360, type: .recvWithAddress, dash: 5.0, contact: "Salary"),
            Script(hoursAgo: 290, type: .sendToAddress, dash: -1.0, contact: "Exchange"),
            Script(hoursAgo: 210, type: .recvWithAddress, dash: 0.75, contact: "Bob"),
            Script(hoursAgo: 140, type: .sendToSelf, dash: 0, contact: nil),
            Script(hoursAgo: 90, type: .sendToAddress, dash: -0.12, contact: "Hosting"),
            Script(hoursAgo: 44, type: .recvWithAddress, dash: 3.0, contact: "Bob"),
            Script(hoursAgo: 20, type: .sendToAddress, dash: -0.5, contact: "Alice"),
            Script(hoursAgo: 0.6, type: .recvWithAddress, dash: 1.25, contact: "Coffee refund", confirmations: 1),
            Script(hoursAgo: 0.1, type: .recvWithAddress, dash: 0.2, contact: nil, instantLocked: false, confirmations: 0),
        ]
        let contacts = ["Alice", "Bob", "Carol", "Exchange", "Hosting", "Rent share", "Salary", "Coffee refund",
                        "Opening balance"]
        var contactAddress: [String: String] = [:]
        for name in contacts { contactAddress[name] = ledger.rng.address(on: network) }

        var receiveCursor = 0
        for script in scripts {
            let date = now.addingTimeInterval(-script.hoursAgo * 3600)
            let confirmations = script.confirmations
                ?? UInt32(min(Double(tipHeight), script.hoursAgo * 3600 / blockSeconds))
            let duffs = Int64((script.dash * 1e8).rounded())
            let counterparty = script.contact.flatMap { contactAddress[$0] } ?? ledger.rng.address(on: network)
            switch script.type {
            case .sendToAddress:
                _ = ledger.spend(
                    paying: [(counterparty, -duffs)], fee: scriptFee, type: .sendToAddress, date: date,
                    confirmations: confirmations, instantLocked: script.instantLocked)
            case .sendToSelf:
                let own = ledger.receiveAddresses[receiveCursor % receiveCount]
                _ = ledger.spend(
                    paying: [(own, 0)], fee: 2_260, type: .sendToSelf, date: date, confirmations: confirmations,
                    instantLocked: script.instantLocked)
            default:
                let own = ledger.receiveAddresses[receiveCursor % receiveCount]
                receiveCursor += 1
                ledger.receive(
                    duffs, at: own, from: script.type == .recvFromOther ? nil : counterparty, type: script.type,
                    date: date, confirmations: confirmations, instantLocked: script.instantLocked)
            }
        }

        let saved: [(String, String)] = [
            ("Alice", "Alice"), ("Bob", "Bob"), ("Carol", "Carol"), ("Exchange", "Exchange deposit"),
            ("Hosting", "Hosting"),
        ]
        ledger.addressBook = saved.map { key, label in
            AddressBookEntry(address: contactAddress[key]!, label: label, purpose: .send, createdAt: now)
        } + [
            AddressBookEntry(address: ledger.receiveAddresses[0], label: "Opening balance", purpose: .receive, createdAt: now),
            AddressBookEntry(address: ledger.receiveAddresses[2], label: "Salary", purpose: .receive, createdAt: now),
        ]
        let requestAddress = ledger.receiveAddresses[receiveCursor]
        ledger.issued.insert(receiveCursor)
        ledger.requests = [
            ReceiveRequest(
                id: 1, createdAt: now.addingTimeInterval(-86_400 * 3), address: requestAddress,
                amount: Amount(duffs: 25_000_000), label: "Invoice 1042", message: "Design work",
                uri: "dash:\(requestAddress)?amount=0.25&label=Invoice%201042&message=Design%20work"),
        ]
        ledger.nextRequestID = 2
        return ledger
    }

    // MARK: Coins and balances

    var balances: WalletBalances {
        let confirmed = coins.filter { $0.confirmations > 0 || $0.instantLocked }.reduce(Int64(0)) { $0 + $1.amount }
        let total = coins.reduce(Int64(0)) { $0 + $1.amount }
        return WalletBalances(
            confirmed: Amount(duffs: confirmed), unconfirmed: Amount(duffs: total - confirmed), immature: .zero,
            locked: .zero, total: Amount(duffs: total))
    }

    /// Coins a payment may use: trusted, not user-locked, not reserved.
    var spendableCoins: [DemoCoin] {
        coins.filter { $0.trusted && !lockedOutpoints.contains($0.outpoint) && !reserved.contains($0.outpoint) }
            .sorted { $0.amount > $1.amount }
    }

    func isOwn(_ address: String) -> Bool {
        receiveAddresses.contains(address) || changeAddresses.contains(address)
    }

    mutating func takeChangeAddress() -> String {
        defer { nextChange += 1 }
        return changeAddresses[nextChange % Self.changeCount]
    }

    // MARK: Recording transactions

    /// An incoming payment of `duffs` to the wallet's `address`.
    mutating func receive(
        _ duffs: Int64, at address: String, from sender: String?, type: TxType, date: Date, confirmations: UInt32,
        instantLocked: Bool
    ) {
        let txid = rng.hex(bytes: 32)
        let input = TxInput(
            previousOutput: OutPoint(txid: rng.hex(bytes: 32), vout: UInt32(rng.next() % 3)), address: sender,
            amount: nil, isMine: false)
        let output = TxOutput(vout: 0, address: address, amount: Amount(duffs: duffs), isMine: true, isChange: false, dataHex: nil)
        coins.append(DemoCoin(
            outpoint: OutPoint(txid: txid, vout: 0), address: address, amount: duffs, confirmations: confirmations,
            date: date, instantLocked: instantLocked, isChange: false))
        transactions.append(DemoTransaction(
            txid: txid, type: type, date: date, confirmations: confirmations, instantLocked: instantLocked, net: duffs,
            fee: nil, address: type == .recvFromOther ? nil : address, inputs: [input], outputs: [output]))
    }

    /// Spends the largest coins first for `payments` plus `fee`, adds the
    /// change, and records the transaction. Returns `nil` (and changes
    /// nothing) when the coins do not cover it.
    mutating func spend(
        paying payments: [(address: String, duffs: Int64)], fee: Int64, type: TxType, date: Date,
        confirmations: UInt32, instantLocked: Bool, inputs chosen: [DemoCoin]? = nil, txid: String? = nil,
        changeAddress: String? = nil, message: String? = nil
    ) -> String? {
        let needed = payments.reduce(fee) { $0 + $1.duffs }
        var selected: [DemoCoin] = []
        if let chosen {
            selected = chosen
        } else {
            var sum: Int64 = 0
            for coin in spendableCoins where sum < needed {
                selected.append(coin)
                sum += coin.amount
            }
        }
        let input = selected.reduce(Int64(0)) { $0 + $1.amount }
        guard input >= needed else { return nil }
        let txid = txid ?? rng.hex(bytes: 32)
        let spent = Set(selected.map(\.outpoint))
        coins.removeAll { spent.contains($0.outpoint) }
        reserved.subtract(spent)
        var outputs: [TxOutput] = []
        for (index, payment) in payments.enumerated() {
            let mine = isOwn(payment.address)
            let value = type == .sendToSelf && payment.duffs == 0 ? input - fee : payment.duffs
            outputs.append(TxOutput(
                vout: UInt32(index), address: payment.address, amount: Amount(duffs: value), isMine: mine,
                isChange: false, dataHex: nil))
            if mine {
                coins.append(DemoCoin(
                    outpoint: OutPoint(txid: txid, vout: UInt32(index)), address: payment.address, amount: value,
                    confirmations: confirmations, date: date, instantLocked: instantLocked, isChange: false))
            }
        }
        let paid = outputs.reduce(Int64(0)) { $0 + $1.amount.duffs }
        let change = input - paid - fee
        if change > 0 {
            let address = changeAddress ?? takeChangeAddress()
            let mine = isOwn(address)
            let vout = UInt32(outputs.count)
            outputs.append(TxOutput(
                vout: vout, address: address, amount: Amount(duffs: change), isMine: mine, isChange: mine, dataHex: nil))
            if mine {
                coins.append(DemoCoin(
                    outpoint: OutPoint(txid: txid, vout: vout), address: address, amount: change,
                    confirmations: confirmations, date: date, instantLocked: instantLocked, isChange: true))
            }
        }
        let ownReceived = outputs.filter(\.isMine).reduce(Int64(0)) { $0 + $1.amount.duffs }
        let inputs = selected.map {
            TxInput(previousOutput: $0.outpoint, address: $0.address, amount: Amount(duffs: $0.amount), isMine: true)
        }
        transactions.append(DemoTransaction(
            txid: txid, type: type, date: date, confirmations: confirmations, instantLocked: instantLocked,
            net: ownReceived - input, fee: fee, address: payments.first?.address, inputs: inputs,
            outputs: outputs, message: message))
        return txid
    }

    // MARK: History

    /// dash-qt records, one per transaction (the demo pays one recipient per
    /// scripted send; a demo payment to several recipients lists the first).
    var records: [TxRecord] {
        transactions.map(record(of:)).sorted { ($0.date ?? .distantPast) > ($1.date ?? .distantPast) }
    }

    private func record(of tx: DemoTransaction) -> TxRecord {
        let kind: TxStatusKind = tx.confirmations == 0 ? .unconfirmed : (tx.confirmations < 6 ? .confirming : .confirmed)
        let status = TxStatus(
            kind: kind, confirmations: tx.confirmations, instantLocked: tx.instantLocked,
            chainLocked: kind == .confirmed, maturesIn: nil)
        let category: TxCategory
        switch tx.type {
        case .recvWithAddress, .recvFromOther, .dustReceive: category = .received
        case .sendToSelf: category = .internalTransfer
        default: category = .sent
        }
        let label = txLabels[tx.txid] ?? tx.address.flatMap { address in
            addressBook.first { $0.address == address }?.label
        }
        return TxRecord(
            id: TxRecord.ID(txid: tx.txid, recordIndex: 0), type: tx.type, category: category, status: status,
            date: tx.date, blockHeight: Self.height(confirmations: tx.confirmations), amount: Amount(duffs: tx.net),
            fee: tx.fee.map { Amount(duffs: $0) }, address: tx.address, label: label?.isEmpty == true ? nil : label,
            countsTowardBalance: true, involvesWatchOnly: false)
    }

    static func height(confirmations: UInt32) -> UInt32? {
        confirmations == 0 ? nil : tipHeight - confirmations + 1
    }

    func detail(txid: String) -> TransactionDetail? {
        guard let tx = transactions.first(where: { $0.txid == txid }) else { return nil }
        let record = record(of: tx)
        let height = record.blockHeight
        var rng = DemoRandom(text: txid)
        let size = UInt32(10 + 148 * tx.inputs.count + 34 * tx.outputs.count)
        return TransactionDetail(
            txid: txid, records: [record], status: record.status, date: tx.date, blockHeight: height,
            blockHash: height == nil ? nil : rng.hex(bytes: 32), fee: record.fee, sizeBytes: size, inputs: tx.inputs,
            outputs: tx.outputs, message: tx.message, label: txLabels[txid], rawHex: "02000000" + rng.hex(bytes: Int(size) - 4))
    }

    /// The engine's `history_page` over the records: filter, sort, offset cursor.
    func page(_ query: HistoryQuery) throws(ServiceError) -> HistoryPage {
        let filter = query.filter
        var matching = records.filter { record in
            if !filter.types.isEmpty && !filter.types.contains(record.type) { return false }
            if !filter.categories.isEmpty && !filter.categories.contains(record.category) { return false }
            if !filter.statuses.isEmpty && !filter.statuses.contains(record.status.kind) { return false }
            if let from = filter.from, (record.date ?? .distantPast) < from { return false }
            if let until = filter.until, (record.date ?? .distantPast) >= until { return false }
            if let minimum = filter.minimumAmount, record.amount.duffs.magnitude < minimum.duffs.magnitude {
                return false
            }
            switch filter.watchOnly {
            case .all: break
            case .yes: if !record.involvesWatchOnly { return false }
            case .no: if record.involvesWatchOnly { return false }
            }
            if let text = filter.text?.lowercased(), !text.isEmpty {
                let haystack = [record.address ?? "", record.label ?? "", record.id.txid].joined(separator: " ")
                if !haystack.lowercased().contains(text) { return false }
            }
            return true
        }
        switch query.sort {
        case .newestFirst: matching.sort { ($0.date ?? .distantPast) > ($1.date ?? .distantPast) }
        case .oldestFirst: matching.sort { ($0.date ?? .distantPast) < ($1.date ?? .distantPast) }
        case .amountDescending: matching.sort { $0.amount > $1.amount }
        case .amountAscending: matching.sort { $0.amount < $1.amount }
        }
        guard (1...500).contains(query.limit), (filter.text?.count ?? 0) <= 256 else {
            throw .demo(.historyInvalidQuery)
        }
        let start = query.cursor.flatMap(Int.init) ?? 0
        guard start <= matching.count else { throw .demo(.historyStaleCursor) }
        let end = min(matching.count, start + query.limit)
        return HistoryPage(
            records: Array(matching[start..<end]), nextCursor: end < matching.count ? String(end) : nil,
            totalMatching: matching.count)
    }

    // MARK: Addresses

    func addressInfo(receiving index: Int) -> AddressInfo {
        let address = receiveAddresses[index]
        let txCount = UInt32(transactions.filter { tx in tx.outputs.contains { $0.address == address } }.count)
        let balance = coins.filter { $0.address == address }.reduce(Int64(0)) { $0 + $1.amount }
        return AddressInfo(
            address: address, chain: .receiving, index: UInt32(index), derivationPath: derivationPath(0, index),
            used: txCount > 0, label: addressBook.first { $0.address == address }?.label,
            balance: Amount(duffs: balance), txCount: txCount)
    }

    func addressInfo(change index: Int) -> AddressInfo {
        let address = changeAddresses[index]
        let txCount = UInt32(transactions.filter { tx in tx.outputs.contains { $0.address == address } }.count)
        let balance = coins.filter { $0.address == address }.reduce(Int64(0)) { $0 + $1.amount }
        return AddressInfo(
            address: address, chain: .change, index: UInt32(index), derivationPath: derivationPath(1, index),
            used: txCount > 0, label: nil, balance: Amount(duffs: balance), txCount: txCount)
    }

    private func derivationPath(_ chain: Int, _ index: Int) -> String {
        "m/44'/\(network == .mainnet ? 5 : 1)'/0'/\(chain)/\(index)"
    }

    /// First receiving address with no transaction that was not handed out.
    func currentReceiveIndex() throws(ServiceError) -> Int {
        guard let index = (0..<Self.receiveCount).first(where: { !issued.contains($0) && !addressInfo(receiving: $0).used })
        else { throw .demo(.receiveGapLimit) }
        return index
    }

    /// Hands out the current receiving address; the next call gets another.
    mutating func issueReceiveAddress() throws(ServiceError) -> Int {
        let index = try currentReceiveIndex()
        issued.insert(index)
        return index
    }

    mutating func addRequest(address: String, amount: Amount?, label: String?, message: String?, uri: String, now: Date)
        -> ReceiveRequest
    {
        let request = ReceiveRequest(
            id: nextRequestID, createdAt: now, address: address, amount: amount, label: label, message: message, uri: uri)
        nextRequestID += 1
        requests.append(request)
        return request
    }
}
