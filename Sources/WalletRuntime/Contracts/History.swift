// M1 service contracts: transaction history (engine history.rs).
import Foundation

/// dash-qt record types, in dash-qt's order (QT-086).
public enum TxType: Sendable, Hashable, CaseIterable {
    case other, generated, sendToAddress, sendToOther, recvWithAddress, recvFromOther, sendToSelf,
        recvWithCoinJoin, coinJoinMixing, coinJoinCollateralPayment, coinJoinMakeCollaterals,
        coinJoinCreateDenominations, coinJoinSend, platformTransfer, dustReceive, dataTransaction,
        masternodeRegistration, masternodeUpdate, assetLock
}

/// iOS history categories (IOS-028).
public enum TxCategory: Sendable, Hashable, CaseIterable {
    case sent, received, reward, masternode, internalTransfer, coinJoin, platform, other
}

public enum TxStatusKind: Sendable, Hashable, CaseIterable {
    case unconfirmed, confirming, confirmed, conflicted, abandoned, immature, notAccepted
}

public struct TxStatus: Sendable, Hashable {
    public let kind: TxStatusKind
    public let confirmations: UInt32
    public let instantLocked: Bool
    public let chainLocked: Bool
    public let maturesIn: UInt32?

    public init(kind: TxStatusKind, confirmations: UInt32, instantLocked: Bool, chainLocked: Bool, maturesIn: UInt32?) {
        self.kind = kind
        self.confirmations = confirmations
        self.instantLocked = instantLocked
        self.chainLocked = chainLocked
        self.maturesIn = maturesIn
    }
}

/// One dash-qt transaction record; a transaction can have several.
public struct TxRecord: Sendable, Hashable, Identifiable {
    public struct ID: Sendable, Hashable {
        public let txid: String
        public let recordIndex: UInt32

        public init(txid: String, recordIndex: UInt32) {
            self.txid = txid
            self.recordIndex = recordIndex
        }
    }

    public let id: ID
    public let type: TxType
    public let category: TxCategory
    public let status: TxStatus
    public let date: Date?
    public let blockHeight: UInt32?
    /// Signed net amount; sends are negative and include the fee on the first record.
    public let amount: Amount
    public let fee: Amount?
    public let address: String?
    public let label: String?
    public let countsTowardBalance: Bool
    public let involvesWatchOnly: Bool

    public init(
        id: ID, type: TxType, category: TxCategory, status: TxStatus, date: Date?, blockHeight: UInt32?,
        amount: Amount, fee: Amount?, address: String?, label: String?, countsTowardBalance: Bool,
        involvesWatchOnly: Bool
    ) {
        self.id = id
        self.type = type
        self.category = category
        self.status = status
        self.date = date
        self.blockHeight = blockHeight
        self.amount = amount
        self.fee = fee
        self.address = address
        self.label = label
        self.countsTowardBalance = countsTowardBalance
        self.involvesWatchOnly = involvesWatchOnly
    }
}

public enum WatchOnlyFilter: Sendable, Hashable {
    case all, yes, no
}

/// Empty sets and `nil` mean "any" (QT-089, IOS-028).
public struct HistoryFilter: Sendable, Hashable {
    public var types: Set<TxType>
    public var categories: Set<TxCategory>
    public var statuses: Set<TxStatusKind>
    /// Inclusive start.
    public var from: Date?
    /// Exclusive end (dash-qt range semantics).
    public var until: Date?
    public var text: String?
    public var minimumAmount: Amount?
    public var watchOnly: WatchOnlyFilter

    public init(
        types: Set<TxType> = [], categories: Set<TxCategory> = [], statuses: Set<TxStatusKind> = [],
        from: Date? = nil, until: Date? = nil, text: String? = nil, minimumAmount: Amount? = nil,
        watchOnly: WatchOnlyFilter = .all
    ) {
        self.types = types
        self.categories = categories
        self.statuses = statuses
        self.from = from
        self.until = until
        self.text = text
        self.minimumAmount = minimumAmount
        self.watchOnly = watchOnly
    }
}

public enum HistorySort: Sendable, Hashable {
    case newestFirst, oldestFirst, amountDescending, amountAscending
}

public struct HistoryQuery: Sendable, Hashable {
    public var filter: HistoryFilter
    public var sort: HistorySort
    public var cursor: String?
    public var limit: Int

    public init(filter: HistoryFilter = HistoryFilter(), sort: HistorySort = .newestFirst, cursor: String? = nil, limit: Int = 100) {
        self.filter = filter
        self.sort = sort
        self.cursor = cursor
        self.limit = limit
    }
}

public struct HistoryPage: Sendable, Hashable {
    public let records: [TxRecord]
    public let nextCursor: String?
    public let totalMatching: Int?

    public init(records: [TxRecord], nextCursor: String?, totalMatching: Int?) {
        self.records = records
        self.nextCursor = nextCursor
        self.totalMatching = totalMatching
    }
}

public struct TxInput: Sendable, Hashable {
    public let previousOutput: OutPoint
    public let address: String?
    public let amount: Amount?
    public let isMine: Bool

    public init(previousOutput: OutPoint, address: String?, amount: Amount?, isMine: Bool) {
        self.previousOutput = previousOutput
        self.address = address
        self.amount = amount
        self.isMine = isMine
    }
}

public struct TxOutput: Sendable, Hashable {
    public let vout: UInt32
    public let address: String?
    public let amount: Amount
    public let isMine: Bool
    public let isChange: Bool
    public let dataHex: String?

    public init(vout: UInt32, address: String?, amount: Amount, isMine: Bool, isChange: Bool, dataHex: String?) {
        self.vout = vout
        self.address = address
        self.amount = amount
        self.isMine = isMine
        self.isChange = isChange
        self.dataHex = dataHex
    }
}

/// Transaction details (QT-092, IOS-031).
public struct TransactionDetail: Sendable, Hashable {
    public let txid: String
    public let records: [TxRecord]
    public let status: TxStatus
    public let date: Date?
    public let blockHeight: UInt32?
    public let blockHash: String?
    public let fee: Amount?
    public let sizeBytes: UInt32
    public let inputs: [TxInput]
    public let outputs: [TxOutput]
    public let message: String?
    public let label: String?
    public let rawHex: String

    public init(
        txid: String, records: [TxRecord], status: TxStatus, date: Date?, blockHeight: UInt32?, blockHash: String?,
        fee: Amount?, sizeBytes: UInt32, inputs: [TxInput], outputs: [TxOutput], message: String?, label: String?,
        rawHex: String
    ) {
        self.txid = txid
        self.records = records
        self.status = status
        self.date = date
        self.blockHeight = blockHeight
        self.blockHash = blockHash
        self.fee = fee
        self.sizeBytes = sizeBytes
        self.inputs = inputs
        self.outputs = outputs
        self.message = message
        self.label = label
        self.rawHex = rawHex
    }
}

public protocol HistoryProviding: AnyObject, Sendable {
    func page(wallet: WalletID, query: HistoryQuery) async throws(ServiceError) -> HistoryPage
    func detail(wallet: WalletID, txid: String) async throws(ServiceError) -> TransactionDetail
    func setLabel(wallet: WalletID, txid: String, label: String?) async throws(ServiceError)
    /// Yields the affected txids (empty = reload all) after each engine
    /// `HistoryChanged` for `wallet`, coalesced.
    func changes(wallet: WalletID) -> AsyncStream<[String]>
}
