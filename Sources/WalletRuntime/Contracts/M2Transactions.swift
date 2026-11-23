// M2 service contracts: transaction actions, CSV export, notifications, fee
// policy and the coin-control summary (engine tx_actions.rs, fees.rs;
// m2-swift.md §2.2–2.3).
import Foundation

/// dash-qt details fields and action enablement `TransactionDetail` lacks
/// (QT-091/092, IOS-031).
public struct TransactionDetailExtras: Sendable, Hashable {
    public let txid: String
    public let isCoinbase: Bool
    public let totalCredit: Amount
    /// `nil` when an input value is unknown.
    public let totalDebit: Amount?
    public let net: Amount
    public let maturesIn: UInt32?
    /// `nil` = unknown (SPV sees no mempool); never shown as "not in memory pool".
    public let inMempool: Bool?
    public let abandoned: Bool
    public let canAbandon: Bool
    public let canResend: Bool
    /// "Unlock dust UTXO" targets (QT-075).
    public let dustLockedOutputs: [OutPoint]
    public let lastAnnouncedAt: Date?

    public init(
        txid: String, isCoinbase: Bool, totalCredit: Amount, totalDebit: Amount?, net: Amount, maturesIn: UInt32?,
        inMempool: Bool?, abandoned: Bool, canAbandon: Bool, canResend: Bool, dustLockedOutputs: [OutPoint],
        lastAnnouncedAt: Date?
    ) {
        self.txid = txid
        self.isCoinbase = isCoinbase
        self.totalCredit = totalCredit
        self.totalDebit = totalDebit
        self.net = net
        self.maturesIn = maturesIn
        self.inMempool = inMempool
        self.abandoned = abandoned
        self.canAbandon = canAbandon
        self.canResend = canResend
        self.dustLockedOutputs = dustLockedOutputs
        self.lastAnnouncedAt = lastAnnouncedAt
    }
}

/// Why abandon or resend was refused; `ServiceError.code` is
/// `tx_action.refused` and `parameters["refusal"]` is `rawValue`.
public enum TransactionActionRefusal: Int64, Sendable, Hashable, CaseIterable {
    case confirmed, instantLocked, alreadyAbandoned, coinbase, inMempool, notSentByWallet
}

/// Options for `exportCSV` (QT-093).
public struct HistoryCSVOptions: Sendable, Hashable {
    public var unit: DisplayUnit
    /// 19 localized `TxType` names in `TxType.allCases` order; empty = English.
    public var typeNames: [String]
    /// Dates are written in this zone (dash-qt: local time).
    public var timeZone: TimeZone

    public init(unit: DisplayUnit, typeNames: [String] = [], timeZone: TimeZone = .current) {
        self.unit = unit
        self.typeNames = typeNames
        self.timeZone = timeZone
    }
}

/// Abandon, resend, drop and export (owner S1 adapter over R1).
public protocol TransactionActing: AnyObject, Sendable {
    func extras(wallet: WalletID, txid: String) async throws(ServiceError) -> TransactionDetailExtras
    func abandon(wallet: WalletID, txid: String) async throws(ServiceError)
    func resend(wallet: WalletID, txid: String) async throws(ServiceError)
    /// Abandons every eligible unconfirmed transaction (all wallets when
    /// `nil`) and schedules a rescan; returns how many (IOS-034).
    func dropUnconfirmed(wallet: WalletID?) async throws(ServiceError) -> Int
    /// The exact dash-qt CSV bytes of the filtered view (UTF-8).
    func exportCSV(wallet: WalletID, filter: HistoryFilter, sort: HistorySort, options: HistoryCSVOptions)
        async throws(ServiceError) -> Data
}

/// One notification row (QT-031): one per dash-qt record.
public struct TransactionNotice: Sendable, Hashable {
    public let txid: String
    public let recordIndex: UInt32
    /// Positive = "Incoming transaction", otherwise "Sent transaction".
    public let amount: Amount
    public let date: Date?
    public let type: TxType
    public let address: String?
    public let label: String?
    /// Hidden unless "show CoinJoin popups" is on (QT-033).
    public let coinJoinInternal: Bool

    public init(
        txid: String, recordIndex: UInt32, amount: Amount, date: Date?, type: TxType, address: String?,
        label: String?, coinJoinInternal: Bool
    ) {
        self.txid = txid
        self.recordIndex = recordIndex
        self.amount = amount
        self.date = date
        self.type = type
        self.address = address
        self.label = label
        self.coinJoinInternal = coinJoinInternal
    }
}

/// The rows of one engine `NewTransactions` batch (100 ms, dash-qt).
public struct TransactionNoticeBatch: Sendable, Hashable {
    public let network: DashNetwork
    public let wallet: WalletID
    public let notices: [TransactionNotice]
    /// SPV was not caught up: dash-qt shows nothing during initial sync.
    public let catchUp: Bool

    public init(network: DashNetwork, wallet: WalletID, notices: [TransactionNotice], catchUp: Bool) {
        self.network = network
        self.wallet = wallet
        self.notices = notices
        self.catchUp = catchUp
    }
}

/// New-transaction batches of the active network (owner S1). The notifier
/// (S1) applies dash-qt's rules: nothing while `catchUp`, CoinJoin rows only
/// with the option on, one summary for 100 or more rows.
public protocol TransactionNotifying: AnyObject, Sendable {
    func batches() -> AsyncStream<TransactionNoticeBatch>
}

// MARK: Fees and coin control

public enum FeeSource: Sendable, Hashable {
    /// SPV: all targets pay the minimum relay fee (DESIGN-opus §1.14).
    case minimumRelay
    /// dashd RPC data source (post-M2).
    case nodeEstimate
}

public struct FeeTarget: Sendable, Hashable {
    public let targetBlocks: UInt32
    public let duffsPerKB: UInt64

    public init(targetBlocks: UInt32, duffsPerKB: UInt64) {
        self.targetBlocks = targetBlocks
        self.duffsPerKB = duffsPerKB
    }
}

/// QT-057/058 fee rules.
public struct FeePolicy: Sendable, Hashable {
    public let source: FeeSource
    public let minimumRelayPerKB: UInt64
    public let maximumCustomPerKB: UInt64
    public let maximumTransactionFee: Amount
    public let maximumBroadcastRatePerKB: UInt64
    public let targets: [FeeTarget]

    public init(
        source: FeeSource, minimumRelayPerKB: UInt64, maximumCustomPerKB: UInt64, maximumTransactionFee: Amount,
        maximumBroadcastRatePerKB: UInt64, targets: [FeeTarget]
    ) {
        self.source = source
        self.minimumRelayPerKB = minimumRelayPerKB
        self.maximumCustomPerKB = maximumCustomPerKB
        self.maximumTransactionFee = maximumTransactionFee
        self.maximumBroadcastRatePerKB = maximumBroadcastRatePerKB
        self.targets = targets
    }
}

/// dash-qt coin-control panel values (QT-072/074).
public struct CoinSelectionSummary: Sendable, Hashable {
    public let quantity: Int
    public let amount: Amount
    public let bytes: Int
    public let fee: Amount
    public let afterFee: Amount
    public let change: Amount
    public let changeToFee: Bool
    public let insufficientFunds: Bool
    public let feeTolerancePerInput: Amount
    /// Spent or vanished selections to unselect, with dash-qt's notice.
    public let unavailable: [OutPoint]

    public init(
        quantity: Int, amount: Amount, bytes: Int, fee: Amount, afterFee: Amount, change: Amount, changeToFee: Bool,
        insufficientFunds: Bool, feeTolerancePerInput: Amount, unavailable: [OutPoint]
    ) {
        self.quantity = quantity
        self.amount = amount
        self.bytes = bytes
        self.fee = fee
        self.afterFee = afterFee
        self.change = change
        self.changeToFee = changeToFee
        self.insufficientFunds = insufficientFunds
        self.feeTolerancePerInput = feeTolerancePerInput
        self.unavailable = unavailable
    }
}

/// Fee policy and coin-control summary (owner S1 adapter over R1).
public protocol FeeAndCoinSelectionProviding: AnyObject, Sendable {
    func feePolicy() async throws(ServiceError) -> FeePolicy
    /// `allChangeToFee` = the CoinJoin page rule.
    func summary(
        wallet: WalletID, outpoints: [OutPoint], payAmounts: [Amount], fee: FeeChoice, allChangeToFee: Bool
    ) async throws(ServiceError) -> CoinSelectionSummary
}
