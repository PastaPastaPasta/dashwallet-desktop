// M2 additions to the Transactions page: dash-qt's context-menu actions and
// details fields (QT-075, QT-090…093) and iOS history presentation (day
// groups, filter chips, CoinJoin day rows, explorer links; IOS-027…034).
import Foundation
import WalletRuntime

/// Abandon / resend / unlock-dust flow.
public enum TransactionActionState: Sendable, Hashable {
    case idle
    /// Abandon asks first; `mayStillConfirm` adds the SPV warning (`in_mempool` unknown).
    case confirmingAbandon(txid: String, mayStillConfirm: Bool)
    case working
    case done(String)
    case failed(String)
}

/// iOS history filter chips (IOS-028).
public enum HistoryChip: String, Sendable, Hashable, CaseIterable {
    case sent, received, rewards, masternode

    public var title: String {
        switch self {
        case .sent: L10n.TransactionsM2.chipSent
        case .received: L10n.TransactionsM2.chipReceived
        case .rewards: L10n.TransactionsM2.chipRewards
        case .masternode: L10n.TransactionsM2.chipMasternode
        }
    }

    var category: TxCategory {
        switch self {
        case .sent: .sent
        case .received: .received
        case .rewards: .reward
        case .masternode: .masternode
        }
    }
}

/// CoinJoin internal records of one day as a single row (IOS-030).
public struct CoinJoinDayRow: Sendable, Hashable {
    public let records: [TxRecord]
    /// Sum of the records: the mixing fees paid (negative or zero).
    public let total: Amount
    public var title: String { L10n.TransactionsM2.mixingTransactions }
}

/// The app's own CoinJoin sweeps as one combined row (IOS-030). Not per
/// day: the sweep is a one-off, sorted under its latest transaction's day.
public struct CoinJoinWithdrawalGroup: Sendable, Hashable {
    public let records: [TxRecord]
    /// What the sweeps changed the balance by (their fees, negative).
    public let total: Amount
    public let day: Date?
    public var title: String { L10n.CoinJoin.withdrawalsTitle }
    public var info: String { L10n.CoinJoin.withdrawalsInfo }
}

public enum HistoryItem: Sendable, Hashable, Identifiable {
    case record(TxRecord)
    case coinJoinMixing(CoinJoinDayRow)

    public var id: String {
        switch self {
        case .record(let record): "\(record.id.txid):\(record.id.recordIndex)"
        case .coinJoinMixing(let row): "coinjoin-\(row.records.first?.id.txid ?? "")"
        }
    }
}

/// One day of history (IOS-027); `day == nil` collects records without a date.
public struct HistoryDayGroup: Sendable, Hashable, Identifiable {
    public var id: String { day.map { String($0.timeIntervalSince1970) } ?? "unknown" }
    public let day: Date?
    public let title: String
    public let items: [HistoryItem]
}

/// A context-menu or details link to a block explorer.
public struct TransactionLink: Sendable, Hashable, Identifiable {
    public var id: URL { url }
    public let title: String
    public let url: URL
}

/// One line of dash-qt's details dialog.
public struct TransactionDetailField: Sendable, Hashable, Identifiable {
    public var id: String { title }
    public let title: String
    public let value: String
}

extension TransactionsViewModel {
    /// The CoinJoin types iOS folds into one row per day.
    public nonisolated static let coinJoinInternalTypes: Set<TxType> = [
        .coinJoinMixing, .coinJoinCollateralPayment, .coinJoinMakeCollaterals, .coinJoinCreateDenominations,
    ]

    // MARK: Chips (IOS-028)

    var chipCategories: Set<TxCategory> {
        let offered = Set(offeredChips)
        guard !offered.isSubset(of: selectedChips) else { return [] }
        return Set(selectedChips.intersection(offered).map(\.category))
    }

    /// Offers Rewards / Masternode only when such records exist (iOS).
    public func refreshChips() async {
        guard let wallet = walletState.selectedWalletID else { return }
        var offered: [HistoryChip] = [.sent, .received]
        for chip in [HistoryChip.rewards, .masternode] {
            let query = HistoryQuery(filter: HistoryFilter(categories: [chip.category]), limit: 1)
            if let page = try? await history.page(wallet: wallet, query: query), !page.records.isEmpty {
                offered.append(chip)
            }
        }
        offeredChips = offered
    }

    public func toggleChip(_ chip: HistoryChip) async {
        if selectedChips.contains(chip) { selectedChips.remove(chip) } else { selectedChips.insert(chip) }
        await reload()
    }

    /// "Only": this chip alone.
    public func selectOnlyChip(_ chip: HistoryChip) async {
        selectedChips = [chip]
        await reload()
    }

    /// "All".
    public func selectAllChips() async {
        selectedChips = Set(HistoryChip.allCases)
        await reload()
    }

    // MARK: Day groups (IOS-027, IOS-030)

    /// `rows` by calendar day, newest first; CoinJoin internal records of a
    /// day become one "Mixing Transactions" row; undated records go last.
    public var dayGroups: [HistoryDayGroup] {
        let calendar = timing.calendar
        var order: [Date?] = []
        var byDay: [Date?: [TxRecord]] = [:]
        let grouped = groupsCoinJoinWithdrawals ? Set(coinJoinWithdrawals?.records.map(\.id) ?? []) : []
        for record in rows where !grouped.contains(record.id) {
            let day = record.date.map { calendar.startOfDay(for: $0) }
            if byDay[day] == nil { order.append(day) }
            byDay[day, default: []].append(record)
        }
        let sortedDays = order.sorted { lhs, rhs in
            switch (lhs, rhs) {
            case (let l?, let r?): l > r
            case (.some, nil): true
            case (nil, _): false
            }
        }
        return sortedDays.map { day in
            let records = byDay[day] ?? []
            var items: [HistoryItem] = []
            var mixing: [TxRecord] = []
            for record in records {
                if Self.coinJoinInternalTypes.contains(record.type) {
                    if mixing.isEmpty { items.append(.coinJoinMixing(CoinJoinDayRow(records: [], total: .zero))) }
                    mixing.append(record)
                } else {
                    items.append(.record(record))
                }
            }
            if !mixing.isEmpty, let index = items.firstIndex(where: {
                if case .coinJoinMixing = $0 { return true }
                return false
            }) {
                items[index] = .coinJoinMixing(CoinJoinDayRow(
                    records: mixing, total: Amount(duffs: mixing.reduce(0) { $0 + $1.amount.duffs })))
            }
            return HistoryDayGroup(day: day, title: dayTitle(day), items: items)
        }
    }

    // MARK: CoinJoin withdrawals (IOS-030)

    /// The app's own "move mixed coins" sweeps among `rows`, as one combined
    /// "CoinJoin Withdrawals" row (iOS `CoinJoinWithdrawalTxSet`). Membership
    /// is by the txids the sweep recorded, not by transaction type.
    public var coinJoinWithdrawals: CoinJoinWithdrawalGroup? {
        guard let wallet = walletState.selectedWalletID,
            let tagged = desktopPreferences?.desktop.m3.coinJoinWithdrawals[wallet.hex], !tagged.isEmpty
        else { return nil }
        let txids = Set(tagged)
        let records = rows.filter { txids.contains($0.id.txid) }
        guard !records.isEmpty else { return nil }
        return CoinJoinWithdrawalGroup(
            records: records, total: Amount(duffs: records.reduce(0) { $0 + $1.amount.duffs }),
            day: records.compactMap(\.date).max())
    }

    private func dayTitle(_ day: Date?) -> String {
        guard let day else { return L10n.TransactionsM2.dateUnknown }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US")
        formatter.timeZone = timing.timeZone
        formatter.calendar = timing.calendar
        formatter.dateStyle = .long
        formatter.timeStyle = .none
        formatter.doesRelativeDateFormatting = true
        return formatter.string(from: day)
    }

    // MARK: Extras and actions (QT-075, QT-091)

    func loadExtras(wallet: WalletID, txid: String) async {
        guard let actions else {
            extras = nil
            return
        }
        do {
            extras = try await actions.extras(wallet: wallet, txid: txid)
        } catch {
            extras = nil
            if error.code != .notImplemented { errorMessage = ErrorText.m2(error.code) }
        }
    }

    /// "Abandon transaction" is enabled (dash-qt rules, checked by the engine).
    public var canAbandon: Bool { extras?.canAbandon ?? false }
    /// "Resend transaction" is enabled for a single selection.
    public var canResend: Bool { selection.count == 1 && (extras?.canResend ?? false) }
    /// "Unlock dust UTXO": DustReceive rows with locked dust.
    public var canUnlockDust: Bool { !(extras?.dustLockedOutputs.isEmpty ?? true) && coinControl != nil }

    public func requestAbandon() {
        guard let extras, extras.canAbandon else { return }
        actionState = .confirmingAbandon(txid: extras.txid, mayStillConfirm: extras.inMempool == nil)
    }

    public func confirmAbandon() async {
        guard case .confirmingAbandon(let txid, _) = actionState else { return }
        await act(txid: txid, done: L10n.TransactionsM2.abandoned) { (actions, wallet) throws(ServiceError) in
            try await actions.abandon(wallet: wallet, txid: txid)
        }
    }

    public func resend() async {
        guard canResend, let txid = extras?.txid else { return }
        await act(txid: txid, done: L10n.TransactionsM2.resent) { (actions, wallet) throws(ServiceError) in
            try await actions.resend(wallet: wallet, txid: txid)
        }
    }

    public func unlockDust() async {
        guard let extras, let coinControl, let wallet = walletState.selectedWalletID,
            !extras.dustLockedOutputs.isEmpty
        else { return }
        actionState = .working
        do {
            try await coinControl.unlock(wallet: wallet, outpoints: extras.dustLockedOutputs)
            actionState = .done(L10n.TransactionsM2.dustUnlocked)
            await loadDetail(txid: extras.txid)
        } catch {
            actionState = .failed(ErrorText.m2(error.code))
        }
    }

    public func dismissAction() {
        actionState = .idle
    }

    private func act(
        txid: String, done: String, _ body: (any TransactionActing, WalletID) async throws(ServiceError) -> Void
    ) async {
        guard let actions, let wallet = walletState.selectedWalletID else { return }
        actionState = .working
        do {
            try await body(actions, wallet)
            actionState = .done(done)
            await reload()
            await loadDetail(txid: txid)
        } catch {
            if error.code == .txActionRefused, let raw = error.parameters["refusal"],
                let refusal = TransactionActionRefusal(rawValue: raw)
            {
                actionState = .failed(L10n.TransactionsM2.refusal(refusal))
            } else {
                actionState = .failed(ErrorText.m2(error.code))
            }
        }
    }

    // MARK: Copy (QT-090)

    public func copyAddress(_ record: TxRecord) -> String { record.address ?? "" }
    public func copyLabel(_ record: TxRecord) -> String { record.label ?? "" }

    /// Signed, no separators, no unit (dash-qt "Copy amount").
    public func copyAmount(_ record: TxRecord) -> String {
        amounts.format(record.amount, unit: settings.display.unit, style: .plain(plusSign: false, separators: .never))
    }

    public func copyTransactionID(_ record: TxRecord) -> String { record.id.txid }

    /// The raw transaction hex of the loaded details.
    public func copyRawTransaction() -> String? { detail?.rawHex }

    /// dash-qt `TxPlainTextRole`: `M/d/yy HH:mm <status>. <type> (<label>) <address> <amount>`.
    public func copyFullDetails(_ record: TxRecord) -> String {
        var text = ""
        if let date = record.date {
            let formatter = DateFormatter()
            formatter.locale = Locale(identifier: "en_US_POSIX")
            formatter.timeZone = timing.timeZone
            formatter.dateFormat = "M/d/yy HH:mm"
            text += formatter.string(from: date)
        }
        text += " " + statusText(for: record) + ". "
        let type = typeText(for: record)
        if !type.isEmpty { text += type + " " }
        if let address = record.address, !address.isEmpty {
            if let label = record.label, !label.isEmpty {
                text += "(\(label)) "
            } else {
                text += L10n.TransactionsM2.noLabel + " "
            }
            text += address + " "
        }
        text += amounts.format(record.amount, unit: settings.display.unit, style: .plain(plusSign: false, separators: .never))
        return text
    }

    // MARK: Links (QT-094, IOS-032)

    /// Options ▸ Display third-party URLs: `|`-separated, `%s` = txid.
    public func thirdPartyLinks(for txid: String) -> [TransactionLink] {
        let setting = desktopPreferences?.desktop.options.thirdPartyTxURLs ?? ""
        return setting.split(separator: "|").compactMap { entry in
            let template = entry.trimmingCharacters(in: .whitespaces)
            guard template.contains("%s"), let url = URL(string: template.replacingOccurrences(of: "%s", with: txid)),
                let host = url.host, !host.isEmpty
            else { return nil }
            return TransactionLink(title: L10n.TransactionsM2.showIn(host), url: url)
        }
    }

    /// iOS block explorers: Insight (mainnet, testnet) and Blockchair (mainnet).
    public func explorerLinks(for txid: String) -> [TransactionLink] {
        switch network {
        case .mainnet:
            return [
                TransactionLink(
                    title: L10n.TransactionsM2.insight, url: URL(string: "https://insight.dash.org/insight/tx/\(txid)")!),
                TransactionLink(
                    title: L10n.TransactionsM2.blockchair,
                    url: URL(string: "https://blockchair.com/dash/transaction/\(txid)?from=dash")!),
            ]
        case .testnet:
            return [
                TransactionLink(
                    title: L10n.TransactionsM2.insight,
                    url: URL(string: "https://insight.testnet.networks.dash.org:3002/insight/tx/\(txid)")!),
            ]
        case .regtest, .devnet, nil:
            return []
        }
    }

    // MARK: Details (QT-092)

    /// dash-qt `FormatTxStatus`: never claims "not in memory pool" when the
    /// SPV wallet cannot know.
    public func detailStatusText() -> String? {
        guard let detail else { return nil }
        let status = detail.status
        typealias T = L10n.TransactionsM2
        var text: String
        switch status.kind {
        case .conflicted:
            text = T.conflicted()
        case .unconfirmed, .abandoned, .notAccepted:
            switch extras?.inMempool {
            case true?: text = T.inMempool
            case false?: text = T.notInMempool
            case nil: text = T.unconfirmed
            }
            if status.kind == .abandoned || extras?.abandoned == true { text += T.abandonedSuffix }
        case .confirming, .confirmed, .immature:
            text = !status.chainLocked && status.confirmations < 6
                ? T.depthUnconfirmed(status.confirmations) : T.confirmations(status.confirmations)
        }
        if status.chainLocked { text += T.chainLockedSuffix }
        if status.instantLocked { text += T.instantSendSuffix }
        return text
    }

    /// The details dialog fields in dash-qt's order (§4.6).
    public var detailFields: [TransactionDetailField] {
        guard let detail else { return [] }
        typealias T = L10n.TransactionsM2
        let unit = settings.display.unit
        func money(_ amount: Amount) -> String {
            amounts.format(amount, unit: unit, style: .withUnit(plusSign: true, separators: .standard))
        }
        var fields: [TransactionDetailField] = []
        if let status = detailStatusText() { fields.append(.init(title: T.status, value: status)) }
        if let date = detail.date {
            let formatter = DateFormatter()
            formatter.locale = Locale(identifier: "en_US")
            formatter.timeZone = timing.timeZone
            formatter.dateStyle = .medium
            formatter.timeStyle = .short
            fields.append(.init(title: T.date, value: formatter.string(from: date)))
        }
        let types = Set(detail.records.map(\.type))
        if let special = types.first(where: { [.masternodeRegistration, .masternodeUpdate, .assetLock].contains($0) }) {
            fields.append(.init(title: T.type, value: L10n.Transactions.typeName(special)))
        }
        if extras?.isCoinbase == true || types.contains(.generated) {
            fields.append(.init(title: T.source, value: T.generated))
        } else if types.contains(.platformTransfer) {
            fields.append(.init(title: T.source, value: T.platformTransfer))
        }
        let incoming = (extras?.net.duffs ?? detail.records.reduce(0) { $0 + $1.amount.duffs }) > 0
        if incoming {
            let from = detail.inputs.first { !$0.isMine }?.address
            if extras?.isCoinbase != true { fields.append(.init(title: T.from, value: from ?? T.unknown)) }
            for output in detail.outputs where output.isMine {
                fields.append(.init(title: T.to, value: describe(output.address, own: true)))
            }
        } else {
            for output in detail.outputs where !output.isMine {
                fields.append(.init(title: T.to, value: describe(output.address, own: false)))
            }
        }
        if let extras {
            if extras.totalCredit.duffs > 0 {
                var credit = money(extras.totalCredit)
                if let blocks = extras.maturesIn { credit += " (\(T.maturesIn(blocks)))" }
                fields.append(.init(title: T.credit, value: credit))
            }
            if let debit = extras.totalDebit, debit.duffs != 0 {
                fields.append(.init(title: T.totalDebit, value: money(Amount(duffs: -abs(debit.duffs)))))
                fields.append(.init(title: T.totalCredit, value: money(extras.totalCredit)))
            }
        }
        if let fee = detail.fee, fee.duffs != 0 {
            fields.append(.init(title: T.fee, value: money(Amount(duffs: -abs(fee.duffs)))))
        }
        let net = extras?.net ?? Amount(duffs: detail.records.reduce(0) { $0 + $1.amount.duffs })
        fields.append(.init(title: T.net, value: money(net)))
        if let message = detail.message, !message.isEmpty { fields.append(.init(title: T.message, value: message)) }
        if let label = detail.label, !label.isEmpty { fields.append(.init(title: T.comment, value: label)) }
        fields.append(.init(title: T.transactionID, value: detail.txid))
        fields.append(.init(title: T.totalSize, value: T.bytes(detail.sizeBytes)))
        if types.contains(.dataTransaction), let data = detail.outputs.compactMap(\.dataHex).first {
            fields.append(.init(title: T.payload, value: data))
        }
        return fields
    }

    private func describe(_ address: String?, own: Bool) -> String {
        guard let address else { return L10n.TransactionsM2.unknown }
        var notes: [String] = []
        if own { notes.append(L10n.TransactionsM2.ownAddress) }
        if let label = rows.first(where: { $0.address == address })?.label, !label.isEmpty {
            notes.append("\(L10n.TransactionsM2.label): \(label)")
        }
        return notes.isEmpty ? address : "\(address) (\(notes.joined(separator: ", ")))"
    }

    // MARK: CSV (QT-093)

    /// The engine's exact dash-qt bytes; `nil` falls back to the M1 writer
    /// while the engine answers `not_implemented`.
    func engineCSV(wallet: WalletID) async throws(ServiceError) -> String? {
        guard let actions else { return nil }
        do {
            let data = try await actions.exportCSV(
                wallet: wallet, filter: filter, sort: .newestFirst,
                options: HistoryCSVOptions(
                    unit: settings.display.unit, typeNames: TxType.allCases.map(L10n.Transactions.typeName),
                    timeZone: timing.timeZone))
            return String(decoding: data, as: UTF8.self)
        } catch {
            if error.code == .notImplemented { return nil }
            throw error
        }
    }
}
