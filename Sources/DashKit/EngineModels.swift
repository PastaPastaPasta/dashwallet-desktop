// DashKit mirrors of the M1 engine records (docs/contracts/m1-engine.md §2).
// Amounts are `Amount` (Int64 duffs), times are `Date`, ids are `WalletID`.
// Each type converts from (and, where the host sends it, to) its generated
// DashWalletCore counterpart.
import DashWalletCore
import Foundation

// MARK: Conversion helpers

extension Amount {
    /// An engine `u64` duff count. Throws `internal` above `Int64.max`, which
    /// no real amount reaches (total supply < 2.2e15 duffs).
    init(engine duffs: UInt64) throws(DashKitError) {
        guard let value = Amount(exactly: duffs) else {
            throw .internal(detail: "amount \(duffs) exceeds Int64 range")
        }
        self = value
    }

    /// The amount as an engine `u64`. Negative amounts are `invalid_argument`.
    func engineDuffs() throws(DashKitError) -> UInt64 {
        guard duffs >= 0 else { throw .invalidArgument(detail: "negative amount \(duffs)") }
        return UInt64(duffs)
    }
}

extension Optional where Wrapped == UInt64 {
    func engineAmount() throws(DashKitError) -> Amount? {
        guard let self else { return nil }
        return try Amount(engine: self)
    }

    var engineDate: Date? {
        map { Date(timeIntervalSince1970: TimeInterval($0)) }
    }
}

extension WalletID {
    /// Parses an engine wallet id; malformed ids from Rust are `internal`.
    static func engine(_ hex: String) throws(DashKitError) -> WalletID {
        guard let id = WalletID(hex: hex) else { throw .internal(detail: "malformed wallet id from engine") }
        return id
    }
}

// MARK: Common

public struct OutPoint: Sendable, Hashable {
    public let txid: String
    public let vout: UInt32

    public init(txid: String, vout: UInt32) {
        self.txid = txid
        self.vout = vout
    }

    init(_ ffi: DashWalletCore.OutPoint) {
        self.init(txid: ffi.txid, vout: ffi.vout)
    }

    var ffi: DashWalletCore.OutPoint { .init(txid: txid, vout: vout) }
}

// MARK: Wallets

/// Options for `importWallet` (engine `ImportOptions`).
public struct ImportOptions: Sendable, Hashable {
    public var name: String?
    /// `0` scans from genesis; `nil` lets the engine choose.
    public var birthHeight: UInt32?
    public var coreCompatible: Bool
    public var lookahead: UInt32?

    public init(name: String? = nil, birthHeight: UInt32? = nil, coreCompatible: Bool = false, lookahead: UInt32? = nil) {
        self.name = name
        self.birthHeight = birthHeight
        self.coreCompatible = coreCompatible
        self.lookahead = lookahead
    }

    var ffi: DashWalletCore.ImportOptions {
        .init(name: name, birthHeight: birthHeight, coreCompat: coreCompatible, lookahead: lookahead)
    }
}

/// A registered wallet (engine `WalletInfo`).
public struct WalletInfo: Sendable, Hashable {
    public let walletID: WalletID
    public let name: String
    public let watchOnly: Bool
    public let hasMnemonic: Bool
    public let hd: Bool
    public let birthHeight: UInt32?
    public let createdAt: Date?
    /// `nil` until the wallet's scan has passed its birth height.
    public let balances: WalletBalances?

    public init(
        walletID: WalletID, name: String, watchOnly: Bool, hasMnemonic: Bool, hd: Bool, birthHeight: UInt32?,
        createdAt: Date?, balances: WalletBalances?
    ) {
        self.walletID = walletID
        self.name = name
        self.watchOnly = watchOnly
        self.hasMnemonic = hasMnemonic
        self.hd = hd
        self.birthHeight = birthHeight
        self.createdAt = createdAt
        self.balances = balances
    }

    init(_ ffi: DashWalletCore.WalletInfo) throws(DashKitError) {
        self.init(
            walletID: try .engine(ffi.walletId), name: ffi.name, watchOnly: ffi.watchOnly,
            hasMnemonic: ffi.hasMnemonic, hd: ffi.hd, birthHeight: ffi.birthHeight,
            createdAt: ffi.createdAt.engineDate, balances: try ffi.balances.map { b throws(DashKitError) in
                try WalletBalances(b)
            })
    }
}

public struct MnemonicCheck: Sendable, Hashable {
    public let wordCount: Int
    public let unknownWordIndices: [Int]
    public let language: MnemonicLanguage?
    public let checksum: MnemonicChecksum

    public init(wordCount: Int, unknownWordIndices: [Int], language: MnemonicLanguage?, checksum: MnemonicChecksum) {
        self.wordCount = wordCount
        self.unknownWordIndices = unknownWordIndices
        self.language = language
        self.checksum = checksum
    }

    init(_ ffi: DashWalletCore.MnemonicCheck) {
        self.init(
            wordCount: Int(ffi.wordCount), unknownWordIndices: ffi.unknownWordIndices.map { Int($0) },
            language: ffi.language.map(MnemonicLanguage.init), checksum: MnemonicChecksum(ffi.checksum))
    }
}

// MARK: Vault

public struct VaultStatus: Sendable, Hashable {
    public let state: VaultLockState
    public let encrypted: Bool
    public let quickUnlockEnrolled: Bool
    public let failedAttempts: UInt32
    public let retryAfterSeconds: UInt64?
    public let walletsWithSecrets: [WalletID]

    public init(
        state: VaultLockState, encrypted: Bool, quickUnlockEnrolled: Bool, failedAttempts: UInt32,
        retryAfterSeconds: UInt64?, walletsWithSecrets: [WalletID]
    ) {
        self.state = state
        self.encrypted = encrypted
        self.quickUnlockEnrolled = quickUnlockEnrolled
        self.failedAttempts = failedAttempts
        self.retryAfterSeconds = retryAfterSeconds
        self.walletsWithSecrets = walletsWithSecrets
    }

    init(_ ffi: DashWalletCore.VaultStatus) throws(DashKitError) {
        var ids: [WalletID] = []
        for hex in ffi.walletsWithSecrets {
            ids.append(try .engine(hex))
        }
        self.init(
            state: VaultLockState(ffi.state), encrypted: ffi.encrypted, quickUnlockEnrolled: ffi.quickUnlockEnrolled,
            failedAttempts: ffi.failedAttempts, retryAfterSeconds: ffi.retryAfterSecs, walletsWithSecrets: ids)
    }
}

public enum GrantPurpose: Sendable, Hashable {
    case spend(max: Amount)
    case revealSecret
    case signMessage
    case changeCredential
    case wipe
    case masternodeOperation
    case governance
    case platformOperation

    init(_ ffi: DashWalletCore.GrantPurpose) throws(DashKitError) {
        switch ffi {
        case .spend(let max): self = .spend(max: try Amount(engine: max))
        case .revealSecret: self = .revealSecret
        case .signMessage: self = .signMessage
        case .changeCredential: self = .changeCredential
        case .wipe: self = .wipe
        case .masternodeOp: self = .masternodeOperation
        case .governance: self = .governance
        case .platformOp: self = .platformOperation
        }
    }

    func ffi() throws(DashKitError) -> DashWalletCore.GrantPurpose {
        switch self {
        case .spend(let max): .spend(maxDuffs: try max.engineDuffs())
        case .revealSecret: .revealSecret
        case .signMessage: .signMessage
        case .changeCredential: .changeCredential
        case .wipe: .wipe
        case .masternodeOperation: .masternodeOp
        case .governance: .governance
        case .platformOperation: .platformOp
        }
    }
}

public struct AuthGrant: Sendable, Hashable {
    public let id: String
    public let purpose: GrantPurpose
    public let expiresAt: Date
    public let singleUse: Bool

    public init(id: String, purpose: GrantPurpose, expiresAt: Date, singleUse: Bool) {
        self.id = id
        self.purpose = purpose
        self.expiresAt = expiresAt
        self.singleUse = singleUse
    }

    init(_ ffi: DashWalletCore.AuthGrant) throws(DashKitError) {
        self.init(
            id: ffi.id, purpose: try GrantPurpose(ffi.purpose),
            expiresAt: Date(timeIntervalSince1970: TimeInterval(ffi.expiresAt)), singleUse: ffi.singleUse)
    }
}

/// What the caller presents to `authorize`.
public enum VaultCredential: Sendable {
    case passphrase(SecretBytes)
    /// Key released by the OS biometric store (M2).
    case quickUnlock(SecretBytes)
    /// Unencrypted vault without "require authentication for every payment".
    case unencrypted
}

/// A revealed recovery phrase and BIP39 passphrase, as zeroing buffers.
public struct RevealedMnemonic: Sendable {
    public let phrase: SecretBytes
    public let bip39Passphrase: SecretBytes

    public init(phrase: SecretBytes, bip39Passphrase: SecretBytes) {
        self.phrase = phrase
        self.bip39Passphrase = bip39Passphrase
    }
}

// MARK: Sync

public struct SyncPhaseProgress: Sendable, Hashable {
    public let phase: SyncPhase
    public let currentHeight: UInt32?
    public let targetHeight: UInt32?
    public let done: Bool

    public init(phase: SyncPhase, currentHeight: UInt32?, targetHeight: UInt32?, done: Bool) {
        self.phase = phase
        self.currentHeight = currentHeight
        self.targetHeight = targetHeight
        self.done = done
    }

    init(_ ffi: DashWalletCore.SyncPhaseProgress) {
        self.init(
            phase: SyncPhase(ffi.phase), currentHeight: ffi.currentHeight, targetHeight: ffi.targetHeight,
            done: ffi.done)
    }
}

/// Raw sync state of a network (engine `SyncSnapshot`). Damping is the
/// runtime's `SPVCoordinator`'s job.
public struct SyncSnapshot: Sendable, Hashable {
    public let running: Bool
    public let phases: [SyncPhaseProgress]
    public let activePhase: SyncPhase?
    public let tipHeight: UInt32?
    public let tipDate: Date?
    public let chainLockHeight: UInt32?
    public let connectedPeers: UInt32
    /// dash-spv reached its steady state; the iOS `syncDone` gate.
    public let caughtUp: Bool
    public let secondsSinceProgress: UInt64?

    public init(
        running: Bool, phases: [SyncPhaseProgress], activePhase: SyncPhase?, tipHeight: UInt32?, tipDate: Date?,
        chainLockHeight: UInt32?, connectedPeers: UInt32, caughtUp: Bool, secondsSinceProgress: UInt64?
    ) {
        self.running = running
        self.phases = phases
        self.activePhase = activePhase
        self.tipHeight = tipHeight
        self.tipDate = tipDate
        self.chainLockHeight = chainLockHeight
        self.connectedPeers = connectedPeers
        self.caughtUp = caughtUp
        self.secondsSinceProgress = secondsSinceProgress
    }

    init(_ ffi: DashWalletCore.SyncSnapshot) {
        self.init(
            running: ffi.running, phases: ffi.phases.map(SyncPhaseProgress.init),
            activePhase: ffi.activePhase.map(SyncPhase.init), tipHeight: ffi.tipHeight,
            tipDate: ffi.tipTime.engineDate, chainLockHeight: ffi.chainlockHeight,
            connectedPeers: ffi.connectedPeers, caughtUp: ffi.caughtUp,
            secondsSinceProgress: ffi.secondsSinceProgress)
    }
}

public struct PeerInfo: Sendable, Hashable {
    public let address: String
    public let userAgent: String?
    public let protocolVersion: UInt32?
    public let bestHeight: UInt32?
    public let pingMilliseconds: UInt32?
    public let connectedSince: Date?
    public let inbound: Bool
    public let bytesSent: UInt64?
    public let bytesReceived: UInt64?

    public init(
        address: String, userAgent: String?, protocolVersion: UInt32?, bestHeight: UInt32?,
        pingMilliseconds: UInt32?, connectedSince: Date?, inbound: Bool, bytesSent: UInt64?, bytesReceived: UInt64?
    ) {
        self.address = address
        self.userAgent = userAgent
        self.protocolVersion = protocolVersion
        self.bestHeight = bestHeight
        self.pingMilliseconds = pingMilliseconds
        self.connectedSince = connectedSince
        self.inbound = inbound
        self.bytesSent = bytesSent
        self.bytesReceived = bytesReceived
    }

    init(_ ffi: DashWalletCore.PeerInfo) {
        self.init(
            address: ffi.address, userAgent: ffi.userAgent, protocolVersion: ffi.protocolVersion,
            bestHeight: ffi.bestHeight, pingMilliseconds: ffi.pingMs, connectedSince: ffi.connectedSince.engineDate,
            inbound: ffi.inbound, bytesSent: ffi.bytesSent, bytesReceived: ffi.bytesReceived)
    }
}

public enum RescanStart: Sendable, Hashable {
    case walletBirth
    case genesis
    case height(UInt32)

    var ffi: DashWalletCore.RescanFrom {
        switch self {
        case .walletBirth: .walletBirth
        case .genesis: .genesis
        case .height(let h): .height(height: h)
        }
    }
}

// MARK: History

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

    init(_ ffi: DashWalletCore.TxStatus) {
        self.init(
            kind: TxStatusKind(ffi.kind), confirmations: ffi.confirmations, instantLocked: ffi.instantLocked,
            chainLocked: ffi.chainLocked, maturesIn: ffi.maturesIn)
    }
}

public struct TxRecord: Sendable, Hashable {
    public let txid: String
    public let recordIndex: UInt32
    public let type: TxType
    public let category: TxCategory
    public let status: TxStatus
    public let date: Date?
    public let blockHeight: UInt32?
    /// Signed net amount.
    public let amount: Amount
    public let fee: Amount?
    public let address: String?
    public let label: String?
    public let countsTowardBalance: Bool
    public let involvesWatchOnly: Bool

    public init(
        txid: String, recordIndex: UInt32, type: TxType, category: TxCategory, status: TxStatus, date: Date?,
        blockHeight: UInt32?, amount: Amount, fee: Amount?, address: String?, label: String?,
        countsTowardBalance: Bool, involvesWatchOnly: Bool
    ) {
        self.txid = txid
        self.recordIndex = recordIndex
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

    init(_ ffi: DashWalletCore.TxRecord) throws(DashKitError) {
        self.init(
            txid: ffi.txid, recordIndex: ffi.recordIndex, type: TxType(ffi.txType), category: TxCategory(ffi.category),
            status: TxStatus(ffi.status), date: ffi.timestamp.engineDate, blockHeight: ffi.blockHeight,
            amount: Amount(duffs: ffi.amount), fee: try ffi.fee.engineAmount(), address: ffi.address,
            label: ffi.label, countsTowardBalance: ffi.countsTowardBalance, involvesWatchOnly: ffi.involvesWatchOnly)
    }
}

public struct HistoryFilter: Sendable, Hashable {
    public var types: Set<TxType>
    public var categories: Set<TxCategory>
    public var statuses: Set<TxStatusKind>
    /// Inclusive.
    public var from: Date?
    /// Exclusive.
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

    func ffi() throws(DashKitError) -> DashWalletCore.HistoryFilter {
        // Sets are sent in declaration order so equal filters send equal queries.
        func unixSeconds(_ date: Date?) -> UInt64? {
            date.map { UInt64(max(0, $0.timeIntervalSince1970.rounded(.down))) }
        }
        return .init(
            types: TxType.allCases.filter(types.contains).map(\.ffi),
            categories: TxCategory.allCases.filter(categories.contains).map(\.ffi),
            statuses: TxStatusKind.allCases.filter(statuses.contains).map(\.ffi),
            dateFrom: unixSeconds(from), dateTo: unixSeconds(until), text: text,
            minAmount: try minimumAmount.map { (a) throws(DashKitError) in try a.engineDuffs() },
            watchOnly: watchOnly.ffi)
    }
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

    func ffi() throws(DashKitError) -> DashWalletCore.HistoryQuery {
        guard (1...500).contains(limit) else { throw .invalidArgument(detail: "history limit \(limit) not in 1...500") }
        return .init(filter: try filter.ffi(), sort: sort.ffi, cursor: cursor, limit: UInt32(limit))
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

    init(_ ffi: DashWalletCore.HistoryPage) throws(DashKitError) {
        var records: [TxRecord] = []
        for record in ffi.records {
            records.append(try TxRecord(record))
        }
        self.init(records: records, nextCursor: ffi.nextCursor, totalMatching: ffi.totalMatching.map { Int($0) })
    }
}

public struct TxInputDetail: Sendable, Hashable {
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

    init(_ ffi: DashWalletCore.TxInputDetail) throws(DashKitError) {
        self.init(
            previousOutput: OutPoint(ffi.previousOutput), address: ffi.address, amount: try ffi.amount.engineAmount(),
            isMine: ffi.isMine)
    }
}

public struct TxOutputDetail: Sendable, Hashable {
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

    init(_ ffi: DashWalletCore.TxOutputDetail) throws(DashKitError) {
        self.init(
            vout: ffi.vout, address: ffi.address, amount: try Amount(engine: ffi.amount), isMine: ffi.isMine,
            isChange: ffi.isChange, dataHex: ffi.dataHex)
    }
}

public struct TxDetail: Sendable, Hashable {
    public let txid: String
    public let records: [TxRecord]
    public let status: TxStatus
    public let date: Date?
    public let blockHeight: UInt32?
    public let blockHash: String?
    public let fee: Amount?
    public let sizeBytes: UInt32
    public let inputs: [TxInputDetail]
    public let outputs: [TxOutputDetail]
    public let message: String?
    public let label: String?
    public let rawHex: String

    public init(
        txid: String, records: [TxRecord], status: TxStatus, date: Date?, blockHeight: UInt32?, blockHash: String?,
        fee: Amount?, sizeBytes: UInt32, inputs: [TxInputDetail], outputs: [TxOutputDetail], message: String?,
        label: String?, rawHex: String
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

    init(_ ffi: DashWalletCore.TxDetail) throws(DashKitError) {
        var records: [TxRecord] = []
        for r in ffi.records { records.append(try TxRecord(r)) }
        var inputs: [TxInputDetail] = []
        for i in ffi.inputs { inputs.append(try TxInputDetail(i)) }
        var outputs: [TxOutputDetail] = []
        for o in ffi.outputs { outputs.append(try TxOutputDetail(o)) }
        self.init(
            txid: ffi.txid, records: records, status: TxStatus(ffi.status), date: ffi.timestamp.engineDate,
            blockHeight: ffi.blockHeight, blockHash: ffi.blockHash, fee: try ffi.fee.engineAmount(),
            sizeBytes: ffi.sizeBytes, inputs: inputs, outputs: outputs, message: ffi.message, label: ffi.label,
            rawHex: ffi.rawHex)
    }
}

// MARK: Receive

public struct AddressInfo: Sendable, Hashable {
    public let address: String
    public let chain: AddressChain
    public let index: UInt32
    public let derivationPath: String
    public let used: Bool
    public let label: String?
    public let balance: Amount?
    public let txCount: UInt32

    public init(
        address: String, chain: AddressChain, index: UInt32, derivationPath: String, used: Bool, label: String?,
        balance: Amount?, txCount: UInt32
    ) {
        self.address = address
        self.chain = chain
        self.index = index
        self.derivationPath = derivationPath
        self.used = used
        self.label = label
        self.balance = balance
        self.txCount = txCount
    }

    init(_ ffi: DashWalletCore.AddressInfo) throws(DashKitError) {
        self.init(
            address: ffi.address, chain: AddressChain(ffi.chain), index: ffi.index, derivationPath: ffi.derivationPath,
            used: ffi.used, label: ffi.label, balance: try ffi.balance.engineAmount(), txCount: ffi.txCount)
    }
}

public struct AddressFilter: Sendable, Hashable {
    public var chain: AddressChain?
    public var used: Bool?

    public init(chain: AddressChain? = nil, used: Bool? = nil) {
        self.chain = chain
        self.used = used
    }

    var ffi: DashWalletCore.AddressFilter { .init(chain: chain?.ffi, used: used) }
}

public struct ReceiveRequest: Sendable, Hashable {
    public let id: UInt64
    public let createdAt: Date
    public let address: String
    public let amount: Amount?
    public let label: String?
    public let message: String?
    public let uri: String

    public init(id: UInt64, createdAt: Date, address: String, amount: Amount?, label: String?, message: String?, uri: String) {
        self.id = id
        self.createdAt = createdAt
        self.address = address
        self.amount = amount
        self.label = label
        self.message = message
        self.uri = uri
    }

    init(_ ffi: DashWalletCore.ReceiveRequest) throws(DashKitError) {
        self.init(
            id: ffi.id, createdAt: Date(timeIntervalSince1970: TimeInterval(ffi.createdAt)), address: ffi.address,
            amount: try ffi.amount.engineAmount(), label: ffi.label, message: ffi.message, uri: ffi.uri)
    }
}

// MARK: Send

public struct Recipient: Sendable, Hashable {
    public var address: String
    public var amount: Amount
    public var subtractFeeFromAmount: Bool
    public var label: String?
    public var message: String?

    public init(address: String, amount: Amount, subtractFeeFromAmount: Bool = false, label: String? = nil, message: String? = nil) {
        self.address = address
        self.amount = amount
        self.subtractFeeFromAmount = subtractFeeFromAmount
        self.label = label
        self.message = message
    }

    func ffi(index: Int) throws(DashKitError) -> DashWalletCore.Recipient {
        guard amount.duffs >= 0 else { throw .recipient(code: "send.invalid_amount", index: index) }
        return .init(
            address: address, amount: UInt64(amount.duffs), subtractFeeFromAmount: subtractFeeFromAmount,
            label: label, message: message)
    }
}

public enum CoinSource: Sendable, Hashable {
    case any
    case fullyMixedOnly
    case outpoints([OutPoint])

    var ffi: DashWalletCore.CoinSource {
        switch self {
        case .any: .any
        case .fullyMixedOnly: .fullyMixedOnly
        case .outpoints(let o): .outpoints(outpoints: o.map(\.ffi))
        }
    }
}

public enum FeeMode: Sendable, Hashable {
    case recommended(targetBlocks: UInt32)
    case perKilobyte(Amount)

    func ffi() throws(DashKitError) -> DashWalletCore.FeeMode {
        switch self {
        case .recommended(let blocks): .recommended(targetBlocks: blocks)
        case .perKilobyte(let rate): .perKb(duffsPerKb: try rate.engineDuffs())
        }
    }
}

public enum ChangePolicy: Sendable, Hashable {
    case automatic
    case address(String)

    var ffi: DashWalletCore.ChangePolicy {
        switch self {
        case .automatic: .auto
        case .address(let a): .address(address: a)
        }
    }
}

public struct TxEstimate: Sendable, Hashable {
    public let fee: Amount
    public let sizeBytes: UInt32
    public let inputCount: UInt32
    public let change: Amount?
    public let totalSent: Amount

    public init(fee: Amount, sizeBytes: UInt32, inputCount: UInt32, change: Amount?, totalSent: Amount) {
        self.fee = fee
        self.sizeBytes = sizeBytes
        self.inputCount = inputCount
        self.change = change
        self.totalSent = totalSent
    }

    init(_ ffi: DashWalletCore.TxEstimate) throws(DashKitError) {
        self.init(
            fee: try Amount(engine: ffi.fee), sizeBytes: ffi.sizeBytes, inputCount: ffi.inputCount,
            change: try ffi.change.engineAmount(), totalSent: try Amount(engine: ffi.totalSent))
    }
}

public struct PreparedInput: Sendable, Hashable {
    public let outpoint: OutPoint
    public let address: String?
    public let amount: Amount

    public init(outpoint: OutPoint, address: String?, amount: Amount) {
        self.outpoint = outpoint
        self.address = address
        self.amount = amount
    }
}

public struct PreparedOutput: Sendable, Hashable {
    public let address: String?
    public let amount: Amount
    public let isChange: Bool
    public let label: String?

    public init(address: String?, amount: Amount, isChange: Bool, label: String?) {
        self.address = address
        self.amount = amount
        self.isChange = isChange
        self.label = label
    }
}

/// What the confirm dialog shows (QT-059, IOS-046).
public struct PreparedTxSummary: Sendable, Hashable {
    public let txid: String
    public let fee: Amount
    public let feeRatePerKilobyte: Amount
    public let sizeBytes: UInt32
    public let inputs: [PreparedInput]
    public let outputs: [PreparedOutput]
    public let totalSent: Amount
    public let totalDebit: Amount

    public init(
        txid: String, fee: Amount, feeRatePerKilobyte: Amount, sizeBytes: UInt32, inputs: [PreparedInput],
        outputs: [PreparedOutput], totalSent: Amount, totalDebit: Amount
    ) {
        self.txid = txid
        self.fee = fee
        self.feeRatePerKilobyte = feeRatePerKilobyte
        self.sizeBytes = sizeBytes
        self.inputs = inputs
        self.outputs = outputs
        self.totalSent = totalSent
        self.totalDebit = totalDebit
    }

    init(_ ffi: DashWalletCore.PreparedTxSummary) throws(DashKitError) {
        var inputs: [PreparedInput] = []
        for i in ffi.inputs {
            inputs.append(PreparedInput(outpoint: OutPoint(i.outpoint), address: i.address, amount: try Amount(engine: i.amount)))
        }
        var outputs: [PreparedOutput] = []
        for o in ffi.outputs {
            outputs.append(PreparedOutput(address: o.address, amount: try Amount(engine: o.amount), isChange: o.isChange, label: o.label))
        }
        self.init(
            txid: ffi.txid, fee: try Amount(engine: ffi.fee), feeRatePerKilobyte: try Amount(engine: ffi.feeRatePerKb),
            sizeBytes: ffi.sizeBytes, inputs: inputs, outputs: outputs, totalSent: try Amount(engine: ffi.totalSent),
            totalDebit: try Amount(engine: ffi.totalDebit))
    }
}

public struct BroadcastOutcome: Sendable, Hashable {
    public let txid: String
    public let peersAnnounced: UInt32

    public init(txid: String, peersAnnounced: UInt32) {
        self.txid = txid
        self.peersAnnounced = peersAnnounced
    }
}

// MARK: Coins and labels

public struct Utxo: Sendable, Hashable {
    public let outpoint: OutPoint
    public let address: String
    public let amount: Amount
    public let confirmations: UInt32
    public let blockHeight: UInt32?
    public let date: Date?
    public let instantLocked: Bool
    public let chainLocked: Bool
    public let userLocked: Bool
    public let reserved: Bool
    public let label: String?
    public let isChange: Bool
    public let isCoinbase: Bool
    public let coinJoinDenominated: Bool
    public let coinJoinRounds: UInt32?
    public let spendable: Bool

    public init(
        outpoint: OutPoint, address: String, amount: Amount, confirmations: UInt32, blockHeight: UInt32?, date: Date?,
        instantLocked: Bool, chainLocked: Bool, userLocked: Bool, reserved: Bool, label: String?, isChange: Bool,
        isCoinbase: Bool, coinJoinDenominated: Bool, coinJoinRounds: UInt32?, spendable: Bool
    ) {
        self.outpoint = outpoint
        self.address = address
        self.amount = amount
        self.confirmations = confirmations
        self.blockHeight = blockHeight
        self.date = date
        self.instantLocked = instantLocked
        self.chainLocked = chainLocked
        self.userLocked = userLocked
        self.reserved = reserved
        self.label = label
        self.isChange = isChange
        self.isCoinbase = isCoinbase
        self.coinJoinDenominated = coinJoinDenominated
        self.coinJoinRounds = coinJoinRounds
        self.spendable = spendable
    }

    init(_ ffi: DashWalletCore.Utxo) throws(DashKitError) {
        self.init(
            outpoint: OutPoint(ffi.outpoint), address: ffi.address, amount: try Amount(engine: ffi.amount),
            confirmations: ffi.confirmations, blockHeight: ffi.blockHeight, date: ffi.timestamp.engineDate,
            instantLocked: ffi.instantLocked, chainLocked: ffi.chainLocked, userLocked: ffi.userLocked,
            reserved: ffi.reserved, label: ffi.label, isChange: ffi.isChange, isCoinbase: ffi.isCoinbase,
            coinJoinDenominated: ffi.coinjoinDenominated, coinJoinRounds: ffi.coinjoinRounds, spendable: ffi.spendable)
    }
}

public struct UtxoFilter: Sendable, Hashable {
    public var includeLocked: Bool
    public var fullyMixedOnly: Bool
    public var minimumConfirmations: UInt32?

    public init(includeLocked: Bool = true, fullyMixedOnly: Bool = false, minimumConfirmations: UInt32? = nil) {
        self.includeLocked = includeLocked
        self.fullyMixedOnly = fullyMixedOnly
        self.minimumConfirmations = minimumConfirmations
    }

    var ffi: DashWalletCore.UtxoFilter {
        .init(includeLocked: includeLocked, fullyMixedOnly: fullyMixedOnly, minConfirmations: minimumConfirmations)
    }
}

public struct AddressBookEntry: Sendable, Hashable {
    public let address: String
    public let label: String
    public let purpose: AddressPurpose
    public let createdAt: Date?

    public init(address: String, label: String, purpose: AddressPurpose, createdAt: Date?) {
        self.address = address
        self.label = label
        self.purpose = purpose
        self.createdAt = createdAt
    }

    init(_ ffi: DashWalletCore.AddressBookEntry) {
        self.init(
            address: ffi.address, label: ffi.label, purpose: AddressPurpose(ffi.purpose),
            createdAt: ffi.createdAt.engineDate)
    }
}

// MARK: URI, QR, units

public struct PaymentURI: Sendable, Hashable {
    public let address: String
    public let amount: Amount?
    public let label: String?
    public let message: String?

    public init(address: String, amount: Amount?, label: String?, message: String?) {
        self.address = address
        self.amount = amount
        self.label = label
        self.message = message
    }
}

public enum AddressProblem: Sendable, Hashable {
    case invalidBase58Length
    case invalidBase58Prefix
    case notBech32mOrBase58
    case invalidBase58ChecksumOrLength
    case platformAddress
    case bech32(detail: String)

    init(_ ffi: DashWalletCore.AddressProblem) {
        switch ffi {
        case .invalidBase58Length: self = .invalidBase58Length
        case .invalidBase58Prefix: self = .invalidBase58Prefix
        case .notBech32mOrBase58: self = .notBech32mOrBase58
        case .invalidBase58ChecksumOrLength: self = .invalidBase58ChecksumOrLength
        case .platformAddress: self = .platformAddress
        case .bech32(let detail): self = .bech32(detail: detail)
        }
    }
}

public enum AddressClass: Sendable, Hashable {
    case core(scriptHash: Bool)
    case platform
    case shielded
    case invalid(AddressProblem)

    init(_ ffi: DashWalletCore.AddressClass) {
        switch ffi {
        case .core(let p2sh): self = .core(scriptHash: p2sh)
        case .platform: self = .platform
        case .shielded: self = .shielded
        case .invalid(let problem): self = .invalid(AddressProblem(problem))
        }
    }
}

/// QR modules, row-major, `true` = dark, no quiet zone.
public struct QRMatrix: Sendable, Hashable {
    public let size: Int
    public let modules: [Bool]

    public init(size: Int, modules: [Bool]) {
        self.size = size
        self.modules = modules
    }
}

/// dash-qt amount formatter (engine `AmountStyle`).
public enum AmountStyle: Sendable, Hashable {
    case plain(plusSign: Bool, separators: Separators)
    case withUnit(plusSign: Bool, separators: Separators)
    /// `digits` 0...8.
    case floored(plusSign: Bool, separators: Separators, digits: UInt8)
    case privacy(separators: Separators, hidden: Bool)
    case gui(signed: Bool, truncate: UInt8?)

    var ffi: DashWalletCore.AmountStyle {
        switch self {
        case .plain(let plus, let sep): .plain(plusSign: plus, separators: sep.ffi)
        case .withUnit(let plus, let sep): .withUnit(plusSign: plus, separators: sep.ffi)
        case .floored(let plus, let sep, let digits): .floored(plusSign: plus, separators: sep.ffi, digits: digits)
        case .privacy(let sep, let hidden): .privacy(separators: sep.ffi, hidden: hidden)
        case .gui(let signed, let truncate): .gui(signed: signed, truncate: truncate)
        }
    }
}
