// Deterministic demo wallet content: transaction history, addresses,
// address book, payment requests and coins.
#if os(macOS)
import Foundation
import WalletRuntime

/// The demo wallet's chain data. Built once per wallet from a seed so
/// screenshots and UI tests see the same rows on every run.
struct DemoLedger {
    static let tipHeight: UInt32 = 1_234_567
    /// Testnet target spacing, 2.5 minutes.
    static let blockSeconds: TimeInterval = 150

    var records: [TxRecord] = []
    var details: [String: TransactionDetail] = [:]
    var receiveAddresses: [String] = []
    var changeAddresses: [String] = []
    /// Index of the next receiving address `nextAddress` hands out.
    var nextReceiveIndex = 0
    var requests: [ReceiveRequest] = []
    var addressBook: [AddressBookEntry] = []
    var lockedOutpoints: Set<OutPoint> = []
    var nextRequestID: UInt64 = 1
    var rng: DemoRandom

    init(seed: String) {
        rng = DemoRandom(text: seed)
        receiveAddresses = (0..<40).map { _ in rng.testnetAddress() }
        changeAddresses = (0..<20).map { _ in rng.testnetAddress() }
    }

    /// One row of the scripted history: age, dash-qt type, signed DASH amount, counterparty label.
    private struct Script {
        let hoursAgo: Double
        let type: TxType
        let dash: Double
        let label: String?
        var instantLocked = true
        var confirmations: UInt32?
    }

    /// A funded wallet with about four months of history.
    static func funded(seed: String, now: Date) -> DemoLedger {
        var ledger = DemoLedger(seed: seed)
        let scripts: [Script] = [
            Script(hoursAgo: 0.1, type: .recvWithAddress, dash: 0.2, label: nil, instantLocked: false, confirmations: 0),
            Script(hoursAgo: 0.6, type: .recvWithAddress, dash: 1.25, label: "Coffee refund", confirmations: 1),
            Script(hoursAgo: 20, type: .sendToAddress, dash: -0.5, label: "Alice"),
            Script(hoursAgo: 44, type: .recvWithAddress, dash: 3.0, label: "Bob"),
            Script(hoursAgo: 90, type: .sendToAddress, dash: -0.12, label: "Hosting"),
            Script(hoursAgo: 140, type: .sendToSelf, dash: -0.0000226, label: nil),
            Script(hoursAgo: 210, type: .recvWithAddress, dash: 0.75, label: "Bob"),
            Script(hoursAgo: 290, type: .sendToAddress, dash: -1.0, label: "Exchange"),
            Script(hoursAgo: 360, type: .recvWithAddress, dash: 5.0, label: "Salary"),
            Script(hoursAgo: 430, type: .sendToAddress, dash: -0.333, label: "Alice"),
            Script(hoursAgo: 500, type: .recvFromOther, dash: 0.05, label: nil),
            Script(hoursAgo: 610, type: .sendToAddress, dash: -2.1, label: "Exchange"),
            Script(hoursAgo: 700, type: .recvWithAddress, dash: 5.0, label: "Salary"),
            Script(hoursAgo: 760, type: .dustReceive, dash: 0.00000547, label: nil),
            Script(hoursAgo: 820, type: .sendToAddress, dash: -0.08, label: "Hosting"),
            Script(hoursAgo: 950, type: .recvWithAddress, dash: 0.4, label: "Carol"),
            Script(hoursAgo: 1_060, type: .sendToAddress, dash: -0.9, label: "Rent share"),
            Script(hoursAgo: 1_200, type: .recvWithAddress, dash: 5.0, label: "Salary"),
            Script(hoursAgo: 1_330, type: .sendToAddress, dash: -0.25, label: "Carol"),
            Script(hoursAgo: 1_500, type: .sendToAddress, dash: -0.12, label: "Hosting"),
            Script(hoursAgo: 1_700, type: .recvWithAddress, dash: 2.5, label: "Bob"),
            Script(hoursAgo: 1_900, type: .sendToAddress, dash: -1.75, label: "Exchange"),
            Script(hoursAgo: 2_150, type: .recvWithAddress, dash: 5.0, label: "Salary"),
            Script(hoursAgo: 2_400, type: .sendToAddress, dash: -0.6, label: "Alice"),
            Script(hoursAgo: 2_700, type: .recvWithAddress, dash: 10.0, label: "Opening balance"),
        ]
        let contacts = ["Alice", "Bob", "Carol", "Exchange", "Hosting", "Rent share", "Salary", "Coffee refund"]
        var contactAddress: [String: String] = [:]
        for name in contacts { contactAddress[name] = ledger.rng.testnetAddress() }

        var receiveCursor = 0
        for (index, script) in scripts.enumerated().reversed() {
            let date = now.addingTimeInterval(-script.hoursAgo * 3600)
            let confirmations = script.confirmations
                ?? UInt32(min(Double(tipHeight), script.hoursAgo * 3600 / blockSeconds))
            let incoming = script.dash > 0
            let ownAddress = ledger.receiveAddresses[receiveCursor % ledger.receiveAddresses.count]
            if incoming { receiveCursor += 1 }
            let counterparty = script.label.flatMap { contactAddress[$0] } ?? ledger.rng.testnetAddress()
            let address: String
            switch script.type {
            case .sendToSelf: address = ledger.receiveAddresses[receiveCursor % ledger.receiveAddresses.count]
            case .recvFromOther: address = ""
            default: address = incoming ? ownAddress : counterparty
            }
            ledger.addRecord(
                txid: ledger.rng.hex(bytes: 32), type: script.type, date: date, confirmations: confirmations,
                instantLocked: script.instantLocked, duffs: Int64((script.dash * 1e8).rounded()),
                address: address.isEmpty ? nil : address, label: script.label, counterparty: counterparty,
                index: index)
        }
        ledger.records.sort { ($0.date ?? .distantPast) > ($1.date ?? .distantPast) }
        ledger.nextReceiveIndex = receiveCursor

        let saved: [(String, String)] = [("Alice", "Alice"), ("Bob", "Bob"), ("Carol", "Carol"),
                                         ("Exchange", "Exchange deposit"), ("Hosting", "Hosting")]
        ledger.addressBook = saved.map { key, label in
            AddressBookEntry(address: contactAddress[key]!, label: label, purpose: .send, createdAt: now)
        } + [
            AddressBookEntry(address: ledger.receiveAddresses[0], label: "Opening balance", purpose: .receive,
                             createdAt: now),
            AddressBookEntry(address: ledger.receiveAddresses[2], label: "Salary", purpose: .receive, createdAt: now),
        ]
        ledger.requests = [
            ReceiveRequest(
                id: 1, createdAt: now.addingTimeInterval(-86_400 * 3), address: ledger.receiveAddresses[receiveCursor],
                amount: Amount(duffs: 25_000_000), label: "Invoice 1042", message: "Design work",
                uri: "dash:\(ledger.receiveAddresses[receiveCursor])?amount=0.25&label=Invoice%201042&message=Design%20work"),
        ]
        ledger.nextRequestID = 2
        return ledger
    }

    var balances: WalletBalances {
        var confirmed: Int64 = 0
        var unconfirmed: Int64 = 0
        for record in records where record.countsTowardBalance {
            if record.status.kind == .unconfirmed && !record.status.instantLocked {
                unconfirmed += record.amount.duffs
            } else {
                confirmed += record.amount.duffs
            }
        }
        return WalletBalances(
            confirmed: Amount(duffs: confirmed), unconfirmed: Amount(duffs: unconfirmed), immature: .zero,
            locked: .zero, total: Amount(duffs: confirmed + unconfirmed))
    }

    /// Adds one single-record transaction with plausible inputs and outputs.
    mutating func addRecord(
        txid: String, type: TxType, date: Date, confirmations: UInt32, instantLocked: Bool, duffs: Int64,
        address: String?, label: String?, counterparty: String, index: Int
    ) {
        let fee: Int64 = duffs < 0 ? 226 : 0
        let kind: TxStatusKind = confirmations == 0 ? .unconfirmed : (confirmations < 6 ? .confirming : .confirmed)
        let status = TxStatus(
            kind: kind, confirmations: confirmations, instantLocked: instantLocked,
            chainLocked: confirmations >= 1 && kind == .confirmed, maturesIn: nil)
        let height: UInt32? = confirmations == 0 ? nil : Self.tipHeight - confirmations + 1
        let category: TxCategory
        switch type {
        case .recvWithAddress, .recvFromOther, .dustReceive: category = .received
        case .sendToSelf: category = .internalTransfer
        default: category = .sent
        }
        let record = TxRecord(
            id: TxRecord.ID(txid: txid, recordIndex: 0), type: type, category: category, status: status, date: date,
            blockHeight: height, amount: Amount(duffs: duffs), fee: fee > 0 ? Amount(duffs: fee) : nil,
            address: address, label: label, countsTowardBalance: true, involvesWatchOnly: false)
        records.append(record)

        let magnitude = abs(duffs)
        let change = changeAddresses[index % changeAddresses.count]
        let inputValue = magnitude + 50_000_000
        let inputs = [
            TxInput(
                previousOutput: OutPoint(txid: rng.hex(bytes: 32), vout: UInt32(rng.next() % 3)),
                address: duffs < 0 ? receiveAddresses[index % receiveAddresses.count] : counterparty,
                amount: Amount(duffs: inputValue), isMine: duffs < 0),
        ]
        var outputs = [
            TxOutput(
                vout: 0, address: duffs < 0 ? counterparty : address, amount: Amount(duffs: magnitude - (duffs < 0 ? fee : 0)),
                isMine: duffs >= 0, isChange: false, dataHex: nil),
        ]
        if duffs < 0 {
            outputs.append(TxOutput(
                vout: 1, address: change, amount: Amount(duffs: inputValue - magnitude), isMine: true, isChange: true,
                dataHex: nil))
        }
        details[txid] = TransactionDetail(
            txid: txid, records: [record], status: status, date: date, blockHeight: height,
            blockHash: height == nil ? nil : rng.hex(bytes: 32), fee: record.fee, sizeBytes: duffs < 0 ? 226 : 191,
            inputs: inputs, outputs: outputs, message: nil, label: label, rawHex: "02000000" + rng.hex(bytes: 90))
    }

    /// Applies a `HistoryQuery` (filter, sort, offset cursor).
    func page(_ query: HistoryQuery) -> HistoryPage {
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
        let start = query.cursor.flatMap(Int.init) ?? 0
        let end = min(matching.count, start + max(1, query.limit))
        let slice = start < end ? Array(matching[start..<end]) : []
        return HistoryPage(
            records: slice, nextCursor: end < matching.count ? String(end) : nil, totalMatching: matching.count)
    }

    func addressInfo(receivingIndex index: Int) -> AddressInfo {
        let address = receiveAddresses[index % receiveAddresses.count]
        let txCount = UInt32(records.filter { $0.address == address }.count)
        return AddressInfo(
            address: address, chain: .receiving, index: UInt32(index), derivationPath: "m/44'/1'/0'/0/\(index)",
            used: txCount > 0, label: addressBook.first { $0.address == address }?.label, balance: nil,
            txCount: txCount)
    }

    /// Unspent outputs: one per incoming record, minus the amounts sent.
    func utxos(now: Date) -> [Utxo] {
        records.filter { $0.amount.duffs > 0 }.prefix(8).enumerated().map { offset, record in
            let outpoint = OutPoint(txid: record.id.txid, vout: 0)
            return Utxo(
                outpoint: outpoint, address: record.address ?? receiveAddresses[offset], amount: record.amount,
                confirmations: record.status.confirmations, date: record.date,
                instantLocked: record.status.instantLocked, chainLocked: record.status.chainLocked,
                userLocked: lockedOutpoints.contains(outpoint), reserved: false, label: record.label, isChange: false,
                coinJoinDenominated: false, coinJoinRounds: nil, spendable: !lockedOutpoints.contains(outpoint))
        }
    }
}
#endif
