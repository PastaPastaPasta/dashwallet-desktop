import DashWalletCore
import Foundation

// DashKit mirrors of the R1/R2 engine records of M2 (docs/contracts/m2-engine.md
// §2.1–2.8). Amounts are `Amount`, times are `Date`, ids are `WalletID`,
// paths are `URL`. The calls are in `EngineClient+M2.swift`.

// MARK: Wallet lifecycle (§2.1)

/// Load state of one registered wallet (engine `WalletLoadState`).
public struct WalletLoadState: Sendable, Hashable {
    public let walletID: WalletID
    public let name: String
    public let loaded: Bool
    public let loadOnStartup: Bool
    public let watchOnly: Bool

    public init(walletID: WalletID, name: String, loaded: Bool, loadOnStartup: Bool, watchOnly: Bool) {
        self.walletID = walletID
        self.name = name
        self.loaded = loaded
        self.loadOnStartup = loadOnStartup
        self.watchOnly = watchOnly
    }

    init(_ ffi: DashWalletCore.WalletLoadState) throws(DashKitError) {
        self.init(
            walletID: try .engine(ffi.walletId), name: ffi.name, loaded: ffi.loaded, loadOnStartup: ffi.loadOnStartup,
            watchOnly: ffi.watchOnly)
    }
}

/// Options for a watch-only wallet (engine `WatchOnlyOptions`).
public struct WatchOnlyOptions: Sendable, Hashable {
    public var name: String?
    public var birthHeight: UInt32?
    public var lookahead: UInt32?

    public init(name: String? = nil, birthHeight: UInt32? = nil, lookahead: UInt32? = nil) {
        self.name = name
        self.birthHeight = birthHeight
        self.lookahead = lookahead
    }

    var ffi: DashWalletCore.WatchOnlyOptions {
        .init(name: name, birthHeight: birthHeight, lookahead: lookahead)
    }
}

/// A BIP44 account xpub (engine `AccountXpub`).
public struct AccountXpub: Sendable, Hashable {
    public let account: UInt32
    public let derivationPath: String
    public let xpub: String

    public init(account: UInt32, derivationPath: String, xpub: String) {
        self.account = account
        self.derivationPath = derivationPath
        self.xpub = xpub
    }

    init(_ ffi: DashWalletCore.AccountXpub) {
        self.init(account: ffi.account, derivationPath: ffi.derivationPath, xpub: ffi.xpub)
    }
}

/// What the data root holds for one network (engine `NetworkDataInfo`).
public struct NetworkDataInfo: Sendable, Hashable {
    public let network: DashNetwork
    public let directory: URL
    public let hasWalletState: Bool
    public let hasVault: Bool
    public let hasOSStoreKey: Bool

    public init(network: DashNetwork, directory: URL, hasWalletState: Bool, hasVault: Bool, hasOSStoreKey: Bool) {
        self.network = network
        self.directory = directory
        self.hasWalletState = hasWalletState
        self.hasVault = hasVault
        self.hasOSStoreKey = hasOSStoreKey
    }

    init(_ ffi: DashWalletCore.NetworkDataInfo) {
        self.init(
            network: DashNetwork(ffi.network), directory: URL(fileURLWithPath: ffi.directory, isDirectory: true),
            hasWalletState: ffi.hasWalletState, hasVault: ffi.hasVault, hasOSStoreKey: ffi.hasOsStoreKey)
    }
}

// MARK: Transactions and fees (§2.2–2.3)

/// dash-qt details fields and action enablement (engine `TxDetailExtras`).
public struct TxDetailExtras: Sendable, Hashable {
    public let txid: String
    public let isCoinbase: Bool
    public let totalCredit: Amount
    public let totalDebit: Amount?
    public let net: Amount
    public let maturesIn: UInt32?
    /// `nil` = unknown (SPV sees no mempool).
    public let inMempool: Bool?
    public let abandoned: Bool
    public let canAbandon: Bool
    public let canResend: Bool
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

    init(_ ffi: DashWalletCore.TxDetailExtras) throws(DashKitError) {
        self.init(
            txid: ffi.txid, isCoinbase: ffi.isCoinbase, totalCredit: try Amount(engine: ffi.totalCredit),
            totalDebit: try ffi.totalDebit.engineAmount(), net: Amount(duffs: ffi.net), maturesIn: ffi.maturesIn,
            inMempool: ffi.inMempool, abandoned: ffi.abandoned, canAbandon: ffi.canAbandon, canResend: ffi.canResend,
            dustLockedOutputs: ffi.dustLockedOutputs.map(OutPoint.init), lastAnnouncedAt: ffi.lastAnnouncedAt.engineDate)
    }
}

public enum FeeSource: Sendable, Hashable {
    case minimumRelay
    case nodeEstimate

    init(_ ffi: DashWalletCore.FeeSource) {
        switch ffi {
        case .minimumRelay: self = .minimumRelay
        case .nodeEstimate: self = .nodeEstimate
        }
    }
}

public struct FeeTarget: Sendable, Hashable {
    public let targetBlocks: UInt32
    public let duffsPerKB: UInt64

    public init(targetBlocks: UInt32, duffsPerKB: UInt64) {
        self.targetBlocks = targetBlocks
        self.duffsPerKB = duffsPerKB
    }
}

/// QT-057/058 fee rules (engine `FeePolicy`).
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

    init(_ ffi: DashWalletCore.FeePolicy) throws(DashKitError) {
        self.init(
            source: FeeSource(ffi.source), minimumRelayPerKB: ffi.minRelayPerKb, maximumCustomPerKB: ffi.maxCustomPerKb,
            maximumTransactionFee: try Amount(engine: ffi.maxTxFee), maximumBroadcastRatePerKB: ffi.maxBroadcastRatePerKb,
            targets: ffi.targets.map { FeeTarget(targetBlocks: $0.targetBlocks, duffsPerKB: $0.duffsPerKb) })
    }
}

/// dash-qt coin-control panel values (engine `CoinSelectionSummary`).
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

    init(_ ffi: DashWalletCore.CoinSelectionSummary) throws(DashKitError) {
        self.init(
            quantity: Int(ffi.quantity), amount: try Amount(engine: ffi.amount), bytes: Int(ffi.bytes),
            fee: try Amount(engine: ffi.fee), afterFee: try Amount(engine: ffi.afterFee),
            change: try Amount(engine: ffi.change), changeToFee: ffi.changeToFee,
            insufficientFunds: ffi.insufficientFunds, feeTolerancePerInput: try Amount(engine: ffi.feeTolerancePerInput),
            unavailable: ffi.unavailable.map(OutPoint.init))
    }
}

// MARK: Tools (§2.4–2.5)

public struct MasternodeCount: Sendable, Hashable {
    public let total: UInt32
    public let enabled: UInt32

    public init(total: UInt32, enabled: UInt32) {
        self.total = total
        self.enabled = enabled
    }
}

public struct ChainLockInfo: Sendable, Hashable {
    public let height: UInt32
    public let blockHash: String
    public let blockTime: Date?

    public init(height: UInt32, blockHash: String, blockTime: Date?) {
        self.height = height
        self.blockHash = blockHash
        self.blockTime = blockTime
    }
}

/// Tools ▸ Information (engine `NodeInfo`). `nil` = unknown to an SPV
/// client, never zero.
public struct NodeInfo: Sendable, Hashable {
    public let clientVersion: String
    public let userAgent: String
    public let dataDirectory: URL
    public let startupTime: Date
    public let network: DashNetwork
    public let connectionsIn: UInt32
    public let connectionsOut: UInt32
    public let localAddresses: [String]
    public let tipHeight: UInt32?
    public let tipTime: Date?
    public let tipHash: String?
    public let bestChainLock: ChainLockInfo?
    public let masternodes: MasternodeCount?
    public let evonodes: MasternodeCount?
    public let mempoolTransactionCount: UInt32?
    public let mempoolUsageBytes: UInt64?

    public init(
        clientVersion: String, userAgent: String, dataDirectory: URL, startupTime: Date, network: DashNetwork,
        connectionsIn: UInt32, connectionsOut: UInt32, localAddresses: [String], tipHeight: UInt32?, tipTime: Date?,
        tipHash: String?, bestChainLock: ChainLockInfo?, masternodes: MasternodeCount?, evonodes: MasternodeCount?,
        mempoolTransactionCount: UInt32?, mempoolUsageBytes: UInt64?
    ) {
        self.clientVersion = clientVersion
        self.userAgent = userAgent
        self.dataDirectory = dataDirectory
        self.startupTime = startupTime
        self.network = network
        self.connectionsIn = connectionsIn
        self.connectionsOut = connectionsOut
        self.localAddresses = localAddresses
        self.tipHeight = tipHeight
        self.tipTime = tipTime
        self.tipHash = tipHash
        self.bestChainLock = bestChainLock
        self.masternodes = masternodes
        self.evonodes = evonodes
        self.mempoolTransactionCount = mempoolTransactionCount
        self.mempoolUsageBytes = mempoolUsageBytes
    }

    init(_ ffi: DashWalletCore.NodeInfo) {
        self.init(
            clientVersion: ffi.clientVersion, userAgent: ffi.userAgent,
            dataDirectory: URL(fileURLWithPath: ffi.dataDir, isDirectory: true),
            startupTime: Date(timeIntervalSince1970: TimeInterval(ffi.startupTime)), network: DashNetwork(ffi.network),
            connectionsIn: ffi.connectionsIn, connectionsOut: ffi.connectionsOut, localAddresses: ffi.localAddresses,
            tipHeight: ffi.tipHeight, tipTime: ffi.tipTime.engineDate, tipHash: ffi.tipHash,
            bestChainLock: ffi.bestChainlock.map {
                ChainLockInfo(height: $0.height, blockHash: $0.blockHash, blockTime: $0.blockTime.engineDate)
            },
            masternodes: ffi.masternodes.map { MasternodeCount(total: $0.total, enabled: $0.enabled) },
            evonodes: ffi.evonodes.map { MasternodeCount(total: $0.total, enabled: $0.enabled) },
            mempoolTransactionCount: ffi.mempoolTxCount, mempoolUsageBytes: ffi.mempoolUsageBytes)
    }
}

/// Status-bar warnings (engine `EngineWarning`, QT-040).
public enum EngineWarningCode: Sendable, Hashable {
    case prereleaseBuild, uncleanShutdown, syncStalled, clockSkew, platformContextUnavailable

    init(_ ffi: DashWalletCore.WarningCode) {
        switch ffi {
        case .prereleaseBuild: self = .prereleaseBuild
        case .uncleanShutdown: self = .uncleanShutdown
        case .syncStalled: self = .syncStalled
        case .clockSkew: self = .clockSkew
        case .platformContextUnavailable: self = .platformContextUnavailable
        }
    }
}

public struct EngineWarning: Sendable, Hashable {
    public let code: EngineWarningCode
    public let detail: String

    public init(code: EngineWarningCode, detail: String) {
        self.code = code
        self.detail = detail
    }
}

/// A running rescan (engine `RescanProgress`, QT-117).
public struct RescanProgress: Sendable, Hashable {
    public let fromHeight: UInt32
    public let currentHeight: UInt32?
    public let targetHeight: UInt32?
    public let startedAt: Date

    public init(fromHeight: UInt32, currentHeight: UInt32?, targetHeight: UInt32?, startedAt: Date) {
        self.fromHeight = fromHeight
        self.currentHeight = currentHeight
        self.targetHeight = targetHeight
        self.startedAt = startedAt
    }

    init(_ ffi: DashWalletCore.RescanProgress) {
        self.init(
            fromHeight: ffi.fromHeight, currentHeight: ffi.currentHeight, targetHeight: ffi.targetHeight,
            startedAt: Date(timeIntervalSince1970: TimeInterval(ffi.startedAt)))
    }
}

public struct BannedPeer: Sendable, Hashable {
    public let subnet: String
    public let bannedUntil: Date

    public init(subnet: String, bannedUntil: Date) {
        self.subnet = subnet
        self.bannedUntil = bannedUntil
    }
}

/// One console command name (engine `ConsoleCommand`).
public struct ConsoleCommand: Sendable, Hashable {
    public let name: String
    public let category: String
    public let sensitive: Bool
    public let available: Bool

    public init(name: String, category: String, sensitive: Bool, available: Bool) {
        self.name = name
        self.category = category
        self.sensitive = sensitive
        self.available = available
    }
}

/// What one console line produced. The engine's
/// `console.authorization_required` error becomes `.authorizationRequired`
/// so the host can authorize `purpose` and run the line again.
public enum ConsoleExecution: Sendable, Hashable {
    case output(text: String, isJSON: Bool)
    case authorizationRequired(GrantPurpose, wallet: WalletID?)
}

// MARK: Compatibility, backups, PSBT (§2.6–2.8)

/// What a user-chosen file is (engine `WalletFileKind`).
public enum WalletFileKind: Sendable, Hashable {
    case dumpWallet(
        network: DashNetwork?, hasMnemonic: Bool, hasHDSeed: Bool, hasXprv: Bool, looseKeyCount: UInt32,
        scriptCount: UInt32, labelCount: UInt32)
    case walletDatSQLite(encrypted: Bool, hasMnemonic: Bool)
    case walletDatBerkeleyDB(encrypted: Bool?)
    case dwBackup(network: DashNetwork, walletCount: UInt32, createdAt: Date, formatVersion: UInt32)
    case psbt
    case unknown

    init(_ ffi: DashWalletCore.WalletFileKind) {
        switch ffi {
        case .dumpWallet(let network, let mnemonic, let seed, let xprv, let loose, let scripts, let labels):
            self = .dumpWallet(
                network: network.map(DashNetwork.init), hasMnemonic: mnemonic, hasHDSeed: seed, hasXprv: xprv,
                looseKeyCount: loose, scriptCount: scripts, labelCount: labels)
        case .walletDatSqlite(let encrypted, let mnemonic):
            self = .walletDatSQLite(encrypted: encrypted, hasMnemonic: mnemonic)
        case .walletDatBdb(let encrypted):
            self = .walletDatBerkeleyDB(encrypted: encrypted)
        case .dwBackup(let network, let count, let created, let version):
            self = .dwBackup(
                network: DashNetwork(network), walletCount: count,
                createdAt: Date(timeIntervalSince1970: TimeInterval(created)), formatVersion: version)
        case .psbt: self = .psbt
        case .unknown: self = .unknown
        }
    }
}

/// Key material from Dash Core's `dumphdinfo` / `listdescriptors true`
/// (engine `KeyMaterial`). The payload stays in a zeroing buffer.
public enum KeyMaterial: Sendable {
    case hdSeed(SecretBytes)
    case xprv(SecretBytes)
    case descriptors(SecretBytes)
}

/// Outcome of a Dash Core import (engine `ImportReport`).
public struct ImportReport: Sendable, Hashable {
    public let walletID: WalletID
    public let labelsImported: UInt32
    public let keysNotImported: UInt32
    public let scriptsNotImported: UInt32
    public let coreCompatibleSeed: Bool

    public init(
        walletID: WalletID, labelsImported: UInt32, keysNotImported: UInt32, scriptsNotImported: UInt32,
        coreCompatibleSeed: Bool
    ) {
        self.walletID = walletID
        self.labelsImported = labelsImported
        self.keysNotImported = keysNotImported
        self.scriptsNotImported = scriptsNotImported
        self.coreCompatibleSeed = coreCompatibleSeed
    }

    init(_ ffi: DashWalletCore.ImportReport) throws(DashKitError) {
        self.init(
            walletID: try .engine(ffi.walletId), labelsImported: ffi.labelsImported,
            keysNotImported: ffi.keysNotImported, scriptsNotImported: ffi.scriptsNotImported,
            coreCompatibleSeed: ffi.coreCompatSeed)
    }
}

public enum CoreExportFormat: Sendable, Hashable {
    case dumpWallet
    case importDescriptorsJSON

    init(_ ffi: DashWalletCore.CoreExportFormat) {
        switch ffi {
        case .dumpWallet: self = .dumpWallet
        case .importDescriptorsJson: self = .importDescriptorsJSON
        }
    }

    var ffi: DashWalletCore.CoreExportFormat {
        switch self {
        case .dumpWallet: .dumpWallet
        case .importDescriptorsJSON: .importDescriptorsJson
        }
    }
}

public enum ExportWarning: Sendable, Hashable {
    case mnemonicNotCoreCompatible
    case coinJoinAccountNotScannedByLegacyCore

    init(_ ffi: DashWalletCore.ExportWarning) {
        switch ffi {
        case .mnemonicNotCoreCompatible: self = .mnemonicNotCoreCompatible
        case .coinJoinAccountNotScannedByLegacyCore: self = .coinJoinAccountNotScannedByLegacyCore
        }
    }
}

public struct ExportReport: Sendable, Hashable {
    public let file: URL
    public let format: CoreExportFormat
    public let keyCount: UInt32
    public let warnings: [ExportWarning]

    public init(file: URL, format: CoreExportFormat, keyCount: UInt32, warnings: [ExportWarning]) {
        self.file = file
        self.format = format
        self.keyCount = keyCount
        self.warnings = warnings
    }

    init(_ ffi: DashWalletCore.ExportReport) {
        self.init(
            file: URL(fileURLWithPath: ffi.path), format: CoreExportFormat(ffi.format), keyCount: ffi.keyCount,
            warnings: ffi.warnings.map(ExportWarning.init))
    }
}

public struct CoreMnemonicCompatibility: Sendable, Hashable {
    public let coreCompatible: Bool
    public let warnings: [ExportWarning]

    public init(coreCompatible: Bool, warnings: [ExportWarning]) {
        self.coreCompatible = coreCompatible
        self.warnings = warnings
    }
}

/// One `.dwbackup` file (engine `BackupInfo`).
public struct BackupInfo: Sendable, Hashable {
    public let file: URL
    public let walletID: WalletID
    public let createdAt: Date
    public let sizeBytes: UInt64
    public let automatic: Bool

    public init(file: URL, walletID: WalletID, createdAt: Date, sizeBytes: UInt64, automatic: Bool) {
        self.file = file
        self.walletID = walletID
        self.createdAt = createdAt
        self.sizeBytes = sizeBytes
        self.automatic = automatic
    }

    init(_ ffi: DashWalletCore.BackupInfo) throws(DashKitError) {
        self.init(
            file: URL(fileURLWithPath: ffi.path), walletID: try .engine(ffi.walletId),
            createdAt: Date(timeIntervalSince1970: TimeInterval(ffi.createdAt)), sizeBytes: ffi.sizeBytes,
            automatic: ffi.automatic)
    }
}

public struct BackupPolicy: Sendable, Hashable {
    public let keep: UInt32
    public let directory: URL

    public init(keep: UInt32, directory: URL) {
        self.keep = keep
        self.directory = directory
    }

    init(_ ffi: DashWalletCore.BackupPolicy) {
        self.init(keep: ffi.keep, directory: URL(fileURLWithPath: ffi.directory, isDirectory: true))
    }
}

/// A parsed PSBT held by the engine (engine `Psbt`). Immutable: signing
/// returns a new handle.
public final class PSBTHandle: Sendable {
    let engineObject: DashWalletCore.Psbt

    init(_ engineObject: DashWalletCore.Psbt) {
        self.engineObject = engineObject
    }

    public func unsignedTxid() throws(DashKitError) -> String {
        try mapped { try engineObject.unsignedTxid() }
    }

    public func base64() throws(DashKitError) -> String {
        try mapped { try engineObject.toBase64() }
    }

    public func bytes() throws(DashKitError) -> Data {
        try mapped { try engineObject.toBytes() }
    }
}

public struct PSBTOutput: Sendable, Hashable {
    public let address: String?
    public let amount: Amount
    public let isMine: Bool

    public init(address: String?, amount: Amount, isMine: Bool) {
        self.address = address
        self.amount = amount
        self.isMine = isMine
    }
}

public enum PSBTStatus: Sendable, Hashable {
    case missingInputInfo, needsSignatures, complete
}

public enum PSBTSignability: Sendable, Hashable {
    case noWallet, watchOnly, noMatchingKeys, canSign
}

/// dash-qt's PSBT dialog values (engine `PsbtAnalysis`).
public struct PSBTAnalysis: Sendable, Hashable {
    public let outputs: [PSBTOutput]
    public let fee: Amount?
    public let total: Amount?
    public let unsignedInputs: UInt32
    public let status: PSBTStatus
    public let signability: PSBTSignability
    public let externalSent: Amount?

    public init(
        outputs: [PSBTOutput], fee: Amount?, total: Amount?, unsignedInputs: UInt32, status: PSBTStatus,
        signability: PSBTSignability, externalSent: Amount?
    ) {
        self.outputs = outputs
        self.fee = fee
        self.total = total
        self.unsignedInputs = unsignedInputs
        self.status = status
        self.signability = signability
        self.externalSent = externalSent
    }

    init(_ ffi: DashWalletCore.PsbtAnalysis) throws(DashKitError) {
        var outputs: [PSBTOutput] = []
        for output in ffi.outputs {
            outputs.append(PSBTOutput(address: output.address, amount: try Amount(engine: output.amount), isMine: output.isMine))
        }
        let status: PSBTStatus = switch ffi.status {
        case .missingInputInfo: .missingInputInfo
        case .needsSignatures: .needsSignatures
        case .complete: .complete
        }
        let signability: PSBTSignability = switch ffi.signability {
        case .noWallet: .noWallet
        case .watchOnly: .watchOnly
        case .noMatchingKeys: .noMatchingKeys
        case .canSign: .canSign
        }
        self.init(
            outputs: outputs, fee: try ffi.fee.engineAmount(), total: try ffi.total.engineAmount(),
            unsignedInputs: ffi.unsignedInputs, status: status, signability: signability,
            externalSent: try ffi.externalSent.engineAmount())
    }
}
