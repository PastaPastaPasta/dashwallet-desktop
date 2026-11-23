// Sending in demo mode, with the rules of the engine's `TxDraft`
// (m1-engine.md §2.7) as WalletRuntime's `TransactionSender` exposes them:
// recipients are validated offline (network, dust, duplicates); `prepare`
// checks the grant, plans, redeems the single-use spend grant, caps the
// payment and reserves its coins but never broadcasts; a first broadcast
// without peers fails with `send.no_peers` and releases the coins (sending
// again needs a new review); a broadcast whose outcome is unknown keeps them
// reserved and may be repeated, and a repeat is never `no_peers`.
import Foundation
import WalletRuntime

final class DemoSender: TransactionSending {
    let world: DemoWorld
    let addresses: any URIHandling

    init(world: DemoWorld, addresses: any URIHandling) {
        self.world = world
        self.addresses = addresses
    }

    func makeDraft(wallet: WalletID) async throws(ServiceError) -> any TransactionDrafting {
        _ = try await world.ledger(wallet)
        return await DemoDraft(world: world, wallet: wallet, addresses: addresses)
    }

    /// The sum of the coins `source` may spend; `fee` is only validated.
    func maxSpendable(wallet: WalletID, source: CoinSourceChoice, fee: FeeChoice) async throws(ServiceError) -> Amount {
        _ = try DemoDraft.rate(fee)
        let ledger = try await world.ledger(wallet)
        let coins = try DemoDraft.coins(for: source, in: ledger)
        return Amount(duffs: coins.reduce(Int64(0)) { $0 + $1.amount })
    }
}

@MainActor
final class DemoDraft: TransactionDrafting {
    /// A prepared transaction's state (the engine's `Phase` as the runtime tracks it).
    private enum Held {
        case ready
        case broadcasting(wasUnknown: Bool)
        case outcomeUnknown
    }

    private struct Prepared {
        let plan: Plan
        let txid: String
        var held: Held
    }

    /// One planned payment: the coins it spends and what it pays.
    struct Plan {
        let inputs: [DemoCoin]
        let payments: [(address: String, duffs: Int64)]
        let labels: [String?]
        let messages: [String]
        let fee: Int64
        let change: Int64
        let changeAddress: String?
        let sizeBytes: UInt32
        let externalSent: Int64
        var totalSent: Int64 { payments.reduce(Int64(0)) { $0 + $1.duffs } }
    }

    nonisolated static let maxMoney: Int64 = 21_000_000 * 100_000_000
    /// dash-qt `-maxtxfee` default (QT-058).
    nonisolated static let maxFee: Int64 = 10_000_000
    nonisolated static let p2pkhDust: Int64 = 546

    let world: DemoWorld
    let wallet: WalletID
    let addresses: any URIHandling
    private var recipients: [PaymentRecipient] = []
    private var source: CoinSourceChoice = .any
    private var rate: Int64 = 1000
    private var changeAddress: String?
    private var prepared: [UUID: Prepared] = [:]

    init(world: DemoWorld, wallet: WalletID, addresses: any URIHandling) {
        self.world = world
        self.wallet = wallet
        self.addresses = addresses
    }

    // MARK: Draft settings

    func setRecipients(_ recipients: [PaymentRecipient]) async throws(ServiceError) {
        releaseUnsent()
        guard !recipients.isEmpty else { throw .demo(.sendNoRecipients) }
        var seen = Set<String>()
        var total: Int64 = 0
        for (index, recipient) in recipients.enumerated() {
            let scriptHash: Bool
            switch addresses.classifyAddress(recipient.address) {
            case .core(let p2sh): scriptHash = p2sh
            case .platform: throw .demo(.sendPlatformAddress, recipient: index)
            case .shielded, .invalid: throw .demo(.sendInvalidAddress, recipient: index)
            }
            let amount = recipient.amount.duffs
            guard amount > 0, amount <= Self.maxMoney else { throw .demo(.sendInvalidAmount, recipient: index) }
            guard amount >= (scriptHash ? 540 : Self.p2pkhDust) else { throw .demo(.sendDustAmount, recipient: index) }
            guard seen.insert(recipient.address).inserted else { throw .demo(.sendDuplicateAddress, recipient: index) }
            total += amount
        }
        guard total <= Self.maxMoney else { throw .demo(.sendInvalidAmount, recipient: recipients.count - 1) }
        self.recipients = recipients
    }

    func setSource(_ source: CoinSourceChoice) async throws(ServiceError) {
        releaseUnsent()
        switch source {
        case .fullyMixed:
            throw .demo(.notImplemented, "CoinJoin rounds are not tracked yet")
        case .outpoints(let list):
            guard !list.isEmpty, Set(list).count == list.count else { throw .demo(.invalidArgument, "outpoints") }
        case .any:
            break
        }
        self.source = source
    }

    func setFee(_ fee: FeeChoice) async throws(ServiceError) {
        releaseUnsent()
        rate = try Self.rate(fee)
    }

    func setChange(_ change: ChangeChoice) async throws(ServiceError) {
        releaseUnsent()
        switch change {
        case .automatic:
            changeAddress = nil
        case .address(let text):
            guard case .core = addresses.classifyAddress(text) else { throw .demo(.sendInvalidChangeAddress) }
            changeAddress = text
        }
    }

    /// Duffs per kB: every recommended target pays the minimum relay fee on
    /// SPV (DESIGN-opus §1.14); a custom rate is 1000…10,000,000.
    nonisolated static func rate(_ fee: FeeChoice) throws(ServiceError) -> Int64 {
        switch fee {
        case .recommended(let blocks):
            guard (1...1008).contains(blocks) else { throw .demo(.invalidArgument, "target blocks") }
            return 1000
        case .perKilobyte(let amount):
            guard (1000...10_000_000).contains(amount.duffs) else { throw .demo(.invalidArgument, "fee rate") }
            return amount.duffs
        }
    }

    /// The coins `source` offers, largest first.
    nonisolated static func coins(for source: CoinSourceChoice, in ledger: DemoLedger) throws(ServiceError) -> [DemoCoin] {
        switch source {
        case .any:
            return ledger.spendableCoins
        case .fullyMixed:
            throw .demo(.notImplemented, "CoinJoin rounds are not tracked yet")
        case .outpoints(let list):
            let spendable = Dictionary(uniqueKeysWithValues: ledger.spendableCoins.map { ($0.outpoint, $0) })
            return try list.map { (outpoint) throws(ServiceError) -> DemoCoin in
                guard let coin = spendable[outpoint] ?? ledger.coins.first(where: {
                    $0.outpoint == outpoint && !ledger.reserved.contains(outpoint)
                        && !ledger.lockedOutpoints.contains(outpoint)
                }) else {
                    throw .demo(.sendOutpointUnavailable, "\(outpoint.txid):\(outpoint.vout)")
                }
                return coin
            }
        }
    }

    // MARK: Planning

    nonisolated static func size(inputs: Int, outputs: Int) -> Int64 { Int64(10 + 148 * inputs + 34 * outputs) }

    /// Plans the payment against the wallet's coins now: largest coins first
    /// (all chosen coins under coin control), change back to the wallet
    /// unless it would be dust, fee subtracted from the flagged recipients in
    /// equal shares when asked.
    private func plan() throws(ServiceError) -> Plan {
        guard !recipients.isEmpty else { throw .demo(.sendNoRecipients) }
        let ledger = try world.ledger(wallet)
        let offered = try Self.coins(for: source, in: ledger)
        let available = offered.reduce(Int64(0)) { $0 + $1.amount }
        let sent = recipients.reduce(Int64(0)) { $0 + $1.amount.duffs }
        let subtractors = recipients.indices.filter { recipients[$0].subtractFeeFromAmount }
        guard sent <= available else { throw .demo(.sendAmountExceedsBalance, parameters: ["available": available]) }

        var inputs: [DemoCoin] = []
        var inputSum: Int64 = 0
        var fee: Int64 = 0
        func feeFor(inputs: Int, change: Bool) -> Int64 {
            Self.size(inputs: inputs, outputs: recipients.count + (change ? 1 : 0)) * rate / 1000
        }
        let coinControl: Bool
        if case .outpoints = source { coinControl = true } else { coinControl = false }
        if coinControl {
            inputs = offered
            inputSum = available
            let inputFee = 148 * rate / 1000
            if let small = offered.first(where: { $0.amount <= inputFee }) {
                throw .demo(.sendOutpointUnavailable, "\(small.outpoint.txid):\(small.outpoint.vout)")
            }
            fee = feeFor(inputs: inputs.count, change: true)
        } else {
            for coin in offered {
                fee = feeFor(inputs: inputs.count, change: true)
                if inputSum >= sent + (subtractors.isEmpty ? fee : 0) { break }
                inputs.append(coin)
                inputSum += coin.amount
            }
            fee = feeFor(inputs: inputs.count, change: true)
        }
        var change: Int64
        // The part of the fee taken out of the flagged recipients' amounts.
        var subtracted: Int64 = 0
        if subtractors.isEmpty {
            guard sent + fee <= inputSum else {
                throw .demo(.sendAmountWithFeeExceedsBalance, parameters: ["fee": fee, "available": available])
            }
            change = inputSum - sent - fee
            if change < Self.p2pkhDust {
                // Too small for an output: it goes to the fee.
                fee = inputSum - sent
                change = 0
            }
        } else {
            change = inputSum - sent
            subtracted = fee
            if change < Self.p2pkhDust {
                subtracted = feeFor(inputs: inputs.count, change: false)
                fee = subtracted + change
                change = 0
            }
        }
        guard fee <= Self.maxFee else { throw .demo(.sendAbsurdFee, parameters: ["fee": fee]) }
        var amounts = recipients.map(\.amount.duffs)
        if !subtractors.isEmpty {
            let count = Int64(subtractors.count)
            for (position, index) in subtractors.enumerated() {
                amounts[index] -= subtracted / count + (position == 0 ? subtracted % count : 0)
                guard amounts[index] >= Self.p2pkhDust else { throw .demo(.sendAmountTooSmallAfterFee, recipient: index) }
            }
        }
        let payments = zip(recipients, amounts).map { (address: $0.address, duffs: $1) }
        let foreignChange = changeAddress.map { !ledger.isOwn($0) } ?? false
        let external = payments.filter { !ledger.isOwn($0.address) }.reduce(Int64(0)) { $0 + $1.duffs }
            + (foreignChange ? change : 0)
        return Plan(
            inputs: inputs, payments: payments, labels: recipients.map { $0.label?.isEmpty == false ? $0.label : nil },
            messages: recipients.compactMap { $0.message?.isEmpty == false ? $0.message : nil }, fee: fee,
            change: change, changeAddress: changeAddress,
            sizeBytes: UInt32(Self.size(inputs: inputs.count, outputs: payments.count + (change > 0 ? 1 : 0))),
            externalSent: external)
    }

    func estimate() async throws(ServiceError) -> TxEstimate {
        let plan = try plan()
        return TxEstimate(
            fee: Amount(duffs: plan.fee), sizeBytes: plan.sizeBytes, inputCount: UInt32(plan.inputs.count),
            change: plan.change > 0 ? Amount(duffs: plan.change) : nil, totalSent: Amount(duffs: plan.totalSent))
    }

    // MARK: Prepare, broadcast, abandon

    func prepare(grant: AuthGrant) async throws(ServiceError) -> PreparedTransaction {
        guard case .spend = grant.purpose else { throw .demo(.vaultGrantPurposeMismatch, "prepare needs a spend grant") }
        try world.check(grant, .spend, wallet: wallet, refuse: .send, locked: .sendVaultLocked)
        let plan = try plan()
        let issued = try world.redeem(grant, .spend, wallet: wallet, refuse: .send)
        // The cap covers what leaves the wallet, fee included, as in the engine.
        guard case .spend(let cap) = issued.grant.purpose, plan.externalSent + plan.fee <= cap.duffs else {
            throw .demo(.sendGrantExceeded)
        }
        try world.update(wallet) { (ledger: inout DemoLedger) throws(ServiceError) in
            ledger.reserved.formUnion(plan.inputs.map(\.outpoint))
        }
        var rng = DemoRandom(text: UUID().uuidString)
        let txid = rng.hex(bytes: 32)
        let ledger = try world.ledger(wallet)
        var outputs = zip(plan.payments, plan.labels).map { payment, label in
            PreparedOutput(
                address: payment.address, amount: Amount(duffs: payment.duffs), isChange: false, label: label,
                isMine: ledger.isOwn(payment.address))
        }
        if plan.change > 0 {
            outputs.append(PreparedOutput(
                address: plan.changeAddress ?? ledger.changeAddresses.first, amount: Amount(duffs: plan.change),
                isChange: true, label: nil, isMine: plan.changeAddress.map(ledger.isOwn) ?? true))
        }
        let summary = PreparedTxSummary(
            txid: txid, fee: Amount(duffs: plan.fee),
            feeRatePerKilobyte: Amount(duffs: plan.fee * 1000 / Int64(max(1, plan.sizeBytes))),
            sizeBytes: plan.sizeBytes, inputCount: plan.inputs.count, outputs: outputs,
            totalSent: Amount(duffs: plan.totalSent), totalDebit: Amount(duffs: plan.totalSent + plan.fee),
            externalSent: Amount(duffs: plan.externalSent))
        let id = UUID()
        prepared[id] = Prepared(plan: plan, txid: txid, held: .ready)
        return PreparedTransaction(id: id, summary: summary)
    }

    func broadcast(_ transaction: PreparedTransaction) async throws(ServiceError) -> BroadcastResult {
        guard let entry = prepared[transaction.id] else { throw .demo(.sendPreparedTxUnknown, "not held by this draft") }
        let first: Bool
        switch entry.held {
        case .ready: first = true
        case .outcomeUnknown: first = false
        case .broadcasting:
            throw .demo(.sendBroadcastOutcomeUnknown, "a broadcast of this transaction is in flight")
        }
        prepared[transaction.id]?.held = .broadcasting(wasUnknown: !first)
        // Announcing takes a moment on the network too.
        try? await Task.sleep(for: .milliseconds(300))
        guard world.sync.connectedPeers > 0 else {
            if first {
                // Never sent: the coins are released and this transaction is spent.
                prepared[transaction.id] = nil
                try? world.update(wallet) { (ledger: inout DemoLedger) throws(ServiceError) in
                    ledger.reserved.subtract(entry.plan.inputs.map(\.outpoint))
                }
                throw .demo(.sendNoPeers)
            }
            // A repeat: the first attempt's outcome stays unknown.
            prepared[transaction.id]?.held = .outcomeUnknown
            throw .demo(.sendBroadcastUnknown, "not sent this time: no connected peers")
        }
        prepared[transaction.id] = nil
        try record(entry)
        return BroadcastResult(txid: entry.txid, peersAnnounced: nil)
    }

    func abandon(_ transaction: PreparedTransaction) async throws(ServiceError) {
        guard let entry = prepared[transaction.id] else { return }
        guard case .ready = entry.held else {
            throw .demo(.sendBroadcastOutcomeUnknown, "the broadcast may have reached a peer; inputs stay reserved")
        }
        release(transaction.id, entry)
    }

    // MARK: Helpers

    private func releaseUnsent() {
        for (id, entry) in prepared {
            if case .ready = entry.held { release(id, entry) }
        }
    }

    private func release(_ id: UUID, _ entry: Prepared) {
        prepared[id] = nil
        try? world.update(wallet) { (ledger: inout DemoLedger) throws(ServiceError) in
            ledger.reserved.subtract(entry.plan.inputs.map(\.outpoint))
        }
    }

    /// The accepted payment: coins spent, change added, the transaction in
    /// the history with its message, and the recipients in the address book
    /// the way the engine adds them (new entries, empty labels filled, no
    /// relabelling).
    private func record(_ entry: Prepared) throws(ServiceError) {
        let plan = entry.plan
        let date = world.now()
        let wallet = wallet
        try world.update(wallet) { (ledger: inout DemoLedger) throws(ServiceError) in
            let type: TxType = plan.payments.allSatisfy { ledger.isOwn($0.address) } ? .sendToSelf : .sendToAddress
            _ = ledger.spend(
                paying: plan.payments, fee: plan.fee, type: type, date: date, confirmations: 0, instantLocked: true,
                inputs: plan.inputs, txid: entry.txid, changeAddress: plan.changeAddress,
                message: plan.messages.isEmpty ? nil : plan.messages.joined(separator: "\n"))
            for (payment, label) in zip(plan.payments, plan.labels) {
                if let index = ledger.addressBook.firstIndex(where: { $0.address == payment.address }) {
                    let old = ledger.addressBook[index]
                    if old.label.isEmpty, let label {
                        ledger.addressBook[index] = AddressBookEntry(
                            address: old.address, label: label, purpose: old.purpose, createdAt: old.createdAt)
                    }
                } else {
                    ledger.addressBook.append(AddressBookEntry(
                        address: payment.address, label: label ?? "",
                        purpose: ledger.isOwn(payment.address) ? .receive : .send, createdAt: date))
                }
            }
        }
        world.notifyHistory(wallet, txids: [entry.txid])
    }
}
