// Adapters for the R1/R2 engine calls of M2 (m2-swift.md §2.1–2.4): wallet
// lifecycle, transaction actions, fees, Dash Core imports and exports,
// backups, PSBT, the Tools window and dust protection. Each reads the open
// network at every call; operations that add, open or close wallets run on
// the lifecycle queue so they never interleave with a start, stop or import.
import DashKit
import Foundation

// MARK: Wallet lifecycle (QT-101, QT-114, IOS-009, IOS-110, IOS-111)

/// `WalletLifecycleManaging` over the engine's load/unload, watch-only and
/// xpub calls. Load and unload run on the lifecycle queue without an
/// overlay; the wallet list reloads afterwards.
public final class WalletLifecycleService: WalletLifecycleManaging {
    private let context: EngineContext
    private let queue: LifecycleQueue

    init(context: EngineContext, queue: LifecycleQueue) {
        self.context = context
        self.queue = queue
    }

    public func existingNetworks() async throws(ServiceError) -> [NetworkDataInfo] {
        let engine = context.engine
        let rows = try await serviceCall { () async throws(DashKitError) in try await engine.existingNetworks() }
        return rows.map {
            NetworkDataInfo(
                network: DashNetwork($0.network), directory: $0.directory, hasWalletState: $0.hasWalletState,
                hasVault: $0.hasVault, hasOSStoreKey: $0.hasOSStoreKey)
        }
    }

    public func loadStates() async throws(ServiceError) -> [WalletLoadState] {
        let rows = try await context.call { engine, network throws(DashKitError) in
            try await engine.walletLoadStates(on: network)
        }
        return rows.map {
            WalletLoadState(
                walletID: WalletID($0.walletID), name: $0.name, loaded: $0.loaded, loadOnStartup: $0.loadOnStartup,
                watchOnly: $0.watchOnly)
        }
    }

    /// Engine `WalletLoadChanged`, `WalletCreated` and `WalletRemoved` of the
    /// open network, and `resynchronize`.
    public func loadStateChanges() -> AsyncStream<Void> {
        let subscription = context.engine.events.subscribe()
        let active = context.active
        let (stream, continuation) = AsyncStream<Void>.makeStream(bufferingPolicy: .bufferingNewest(1))
        let pump = Task {
            for await event in subscription {
                switch event {
                case .walletLoadChanged(let n, _, _), .walletCreated(let n, _), .walletRemoved(let n, _):
                    if active.network == n { continuation.yield() }
                case .resynchronize:
                    continuation.yield()
                default:
                    continue
                }
            }
            continuation.finish()
        }
        continuation.onTermination = { _ in pump.cancel() }
        return stream
    }

    public func load(_ wallet: WalletID) async throws(ServiceError) {
        let id = try wallet.kit
        try await queue.runWalletOperation(transition: nil, walletsChanged: true) { engine, network throws(ServiceError) in
            try await serviceCall { () async throws(DashKitError) in try await engine.loadWallet(on: network, wallet: id) }
        }
    }

    public func unload(_ wallet: WalletID) async throws(ServiceError) {
        let id = try wallet.kit
        try await queue.runWalletOperation(transition: nil, walletsChanged: true) { engine, network throws(ServiceError) in
            try await serviceCall { () async throws(DashKitError) in
                try await engine.unloadWallet(on: network, wallet: id)
            }
        }
    }

    public func setLoadOnStartup(_ wallet: WalletID, _ loadOnStartup: Bool) async throws(ServiceError) {
        let id = try wallet.kit
        try await context.call { engine, network throws(DashKitError) in
            try await engine.setLoadOnStartup(on: network, wallet: id, loadOnStartup: loadOnStartup)
        }
    }

    public func importWatchOnly(xpub: String, options: WatchOnlyImportOptions) async throws(ServiceError) -> WalletID {
        let kitOptions = DashKit.WatchOnlyOptions(
            name: options.name, birthHeight: options.birthHeight, lookahead: options.lookahead)
        return try await queue.runWalletOperation(transition: .addingWallet, walletsChanged: true) {
            engine, network throws(ServiceError) in
            WalletID(try await serviceCall { () async throws(DashKitError) in
                try await engine.importWatchOnly(on: network, xpub: xpub, options: kitOptions)
            })
        }
    }

    public func accountXpub(wallet: WalletID, account: UInt32) async throws(ServiceError) -> AccountXpub {
        let id = try wallet.kit
        let xpub = try await context.call { engine, network throws(DashKitError) in
            try await engine.accountXpub(on: network, wallet: id, account: account)
        }
        return AccountXpub(account: xpub.account, derivationPath: xpub.derivationPath, xpub: xpub.xpub)
    }
}

// MARK: Transactions (QT-075, QT-090…093, IOS-031/032, IOS-034)

/// `TransactionActing` over the engine's abandon/resend/drop and its CSV
/// writer (the exact dash-qt bytes).
public final class TransactionActionService: TransactionActing {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func extras(wallet: WalletID, txid: String) async throws(ServiceError) -> TransactionDetailExtras {
        let id = try wallet.kit
        let x = try await context.call { engine, network throws(DashKitError) in
            try await engine.txDetailExtras(on: network, wallet: id, txid: txid)
        }
        return TransactionDetailExtras(
            txid: x.txid, isCoinbase: x.isCoinbase, totalCredit: Amount(x.totalCredit), totalDebit: x.totalDebit.map(Amount.init),
            net: Amount(x.net), maturesIn: x.maturesIn, inMempool: x.inMempool, abandoned: x.abandoned,
            canAbandon: x.canAbandon, canResend: x.canResend, dustLockedOutputs: x.dustLockedOutputs.map(OutPoint.init),
            lastAnnouncedAt: x.lastAnnouncedAt)
    }

    public func abandon(wallet: WalletID, txid: String) async throws(ServiceError) {
        let id = try wallet.kit
        try await context.call { engine, network throws(DashKitError) in
            try await engine.abandonTransaction(on: network, wallet: id, txid: txid)
        }
    }

    public func resend(wallet: WalletID, txid: String) async throws(ServiceError) {
        let id = try wallet.kit
        try await context.call { engine, network throws(DashKitError) in
            try await engine.resendTransaction(on: network, wallet: id, txid: txid)
        }
    }

    public func dropUnconfirmed(wallet: WalletID?) async throws(ServiceError) -> Int {
        let id = try wallet?.kit
        return Int(try await context.call { engine, network throws(DashKitError) in
            try await engine.dropUnconfirmed(on: network, wallet: id)
        })
    }

    /// Dates are written at `options.timeZone`'s current UTC offset (the
    /// engine takes one offset for the whole file).
    public func exportCSV(wallet: WalletID, filter: HistoryFilter, sort: HistorySort, options: HistoryCSVOptions)
        async throws(ServiceError) -> Data
    {
        let id = try wallet.kit
        let kitFilter = filter.kit
        let kitSort = sort.kit
        let unit = options.unit.kit
        let typeNames = options.typeNames
        let offset = Int32(options.timeZone.secondsFromGMT())
        let text = try await context.call { engine, network throws(DashKitError) in
            try await engine.exportHistoryCSV(
                on: network, wallet: id, filter: kitFilter, sort: kitSort, unit: unit, typeNames: typeNames,
                utcOffsetSeconds: offset)
        }
        return Data(text.utf8)
    }
}

// MARK: Fees and coin control (QT-057/058, QT-072/074)

/// `FeeAndCoinSelectionProviding` over `fee_policy` and
/// `coin_selection_summary` (dash-qt's 148/34/+10 byte formula).
public final class FeeService: FeeAndCoinSelectionProviding {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func feePolicy() async throws(ServiceError) -> FeePolicy {
        let p = try await context.call { engine, network throws(DashKitError) in try await engine.feePolicy(on: network) }
        let source: FeeSource = switch p.source {
        case .minimumRelay: .minimumRelay
        case .nodeEstimate: .nodeEstimate
        }
        return FeePolicy(
            source: source, minimumRelayPerKB: p.minimumRelayPerKB, maximumCustomPerKB: p.maximumCustomPerKB,
            maximumTransactionFee: Amount(p.maximumTransactionFee), maximumBroadcastRatePerKB: p.maximumBroadcastRatePerKB,
            targets: p.targets.map { FeeTarget(targetBlocks: $0.targetBlocks, duffsPerKB: $0.duffsPerKB) })
    }

    public func summary(
        wallet: WalletID, outpoints: [OutPoint], payAmounts: [Amount], fee: FeeChoice, allChangeToFee: Bool
    ) async throws(ServiceError) -> CoinSelectionSummary {
        let id = try wallet.kit
        let kitOutpoints = outpoints.map(\.kit)
        let amounts = payAmounts.map(\.kit)
        let kitFee = fee.kit
        let s = try await context.call { engine, network throws(DashKitError) in
            try await engine.coinSelectionSummary(
                on: network, wallet: id, outpoints: kitOutpoints, payAmounts: amounts, fee: kitFee,
                allChangeToFee: allChangeToFee)
        }
        return CoinSelectionSummary(
            quantity: s.quantity, amount: Amount(s.amount), bytes: s.bytes, fee: Amount(s.fee),
            afterFee: Amount(s.afterFee), change: Amount(s.change), changeToFee: s.changeToFee,
            insufficientFunds: s.insufficientFunds, feeTolerancePerInput: Amount(s.feeTolerancePerInput),
            unavailable: s.unavailable.map(OutPoint.init))
    }
}

/// dash-qt dust attack protection (QT-075) over the M1 engine calls
/// `dust_protection` / `set_dust_protection`: `nil` = off, otherwise
/// 1...1,000,000 duffs (the engine answers `invalid_argument` outside).
public final class DustProtectionService: Sendable {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func threshold() async throws(ServiceError) -> Amount? {
        try await context.call { engine, network throws(DashKitError) in
            try await engine.dustProtection(on: network)
        }.map(Amount.init)
    }

    public func setThreshold(_ threshold: Amount?) async throws(ServiceError) {
        let kit = threshold?.kit
        try await context.call { engine, network throws(DashKitError) in
            try await engine.setDustProtection(on: network, threshold: kit)
        }
    }
}

// MARK: Dash Core imports and exports (QT-106…109)

extension WalletImportReport {
    init(_ kit: DashKit.ImportReport) {
        self.init(
            walletID: WalletID(kit.walletID), labelsImported: Int(kit.labelsImported),
            keysNotImported: Int(kit.keysNotImported), scriptsNotImported: Int(kit.scriptsNotImported),
            coreCompatibleSeed: kit.coreCompatibleSeed)
    }
}

extension CoreExportWarning {
    init(_ kit: DashKit.ExportWarning) {
        switch kit {
        case .mnemonicNotCoreCompatible: self = .mnemonicNotCoreCompatible
        case .coinJoinAccountNotScannedByLegacyCore: self = .coinJoinAccountNotScannedByLegacyCore
        }
    }
}

/// `WalletFileImporting` over the engine's Dash Core readers. Imports run
/// on the lifecycle queue with the `.addingWallet` overlay; secrets go to
/// the engine as zeroing buffers.
public final class WalletFileImportService: WalletFileImporting {
    private let context: EngineContext
    private let queue: LifecycleQueue

    init(context: EngineContext, queue: LifecycleQueue) {
        self.context = context
        self.queue = queue
    }

    public func inspect(_ file: URL) async throws(ServiceError) -> WalletFileKind {
        let engine = context.engine
        let kind = try await serviceCall { () async throws(DashKitError) in try await engine.inspectWalletFile(file) }
        return switch kind {
        case .dumpWallet(let network, let mnemonic, let seed, let xprv, let loose, let scripts, let labels):
            .dumpWallet(
                network: network.map(DashNetwork.init), hasMnemonic: mnemonic, hasHDSeed: seed, hasXprv: xprv,
                looseKeyCount: Int(loose), scriptCount: Int(scripts), labelCount: Int(labels))
        case .walletDatSQLite(let encrypted, let mnemonic): .walletDatSQLite(encrypted: encrypted, hasMnemonic: mnemonic)
        case .walletDatBerkeleyDB(let encrypted): .walletDatBerkeleyDB(encrypted: encrypted)
        case .dwBackup(let network, let count, let created, let version):
            .dwBackup(network: DashNetwork(network), walletCount: Int(count), createdAt: created, formatVersion: version)
        case .psbt: .psbt
        case .unknown: .unknown
        }
    }

    public func importDumpWallet(_ file: URL, options: WalletImportOptions) async throws(ServiceError)
        -> WalletImportReport
    {
        let kitOptions = options.kit
        return try await addWallet { engine, network throws(DashKitError) in
            try await engine.importDumpWallet(on: network, file: file, options: kitOptions)
        }
    }

    public func importWalletDat(_ file: URL, passphrase: (any SecretBuffer)?, options: WalletImportOptions)
        async throws(ServiceError) -> WalletImportReport
    {
        let secret = passphrase.map(secretBytes)
        let kitOptions = options.kit
        return try await addWallet { engine, network throws(DashKitError) in
            try await engine.importWalletDat(on: network, file: file, walletPassphrase: secret, options: kitOptions)
        }
    }

    public func importKeyMaterial(_ material: KeyMaterial, options: WalletImportOptions) async throws(ServiceError)
        -> WalletImportReport
    {
        let kitMaterial: DashKit.KeyMaterial = switch material {
        case .hdSeed(let buffer): .hdSeed(secretBytes(buffer))
        case .xprv(let buffer): .xprv(secretBytes(buffer))
        case .descriptors(let buffer): .descriptors(secretBytes(buffer))
        }
        let kitOptions = options.kit
        return try await addWallet { engine, network throws(DashKitError) in
            try await engine.importKeyMaterial(on: network, material: kitMaterial, options: kitOptions)
        }
    }

    private func addWallet(
        _ body: @escaping @Sendable (any EngineProtocol, DashKit.DashNetwork) async throws(DashKitError) -> DashKit.ImportReport
    ) async throws(ServiceError) -> WalletImportReport {
        try await queue.runWalletOperation(transition: .addingWallet, walletsChanged: true) {
            engine, network throws(ServiceError) in
            WalletImportReport(try await serviceCall { () async throws(DashKitError) in try await body(engine, network) })
        }
    }
}

/// `CoreExporting` over `export_for_core` (needs a `.revealSecret` grant)
/// and `core_mnemonic_compatibility`.
public final class CoreExportService: CoreExporting {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func export(wallet: WalletID, format: CoreExportFormat, to file: URL, grant: AuthGrant)
        async throws(ServiceError) -> CoreExportReport
    {
        guard case .revealSecret = grant.purpose else {
            throw ServiceError(code: .vaultGrantPurposeMismatch, detail: "exporting keys needs a revealSecret grant")
        }
        let id = try wallet.kit
        let kitFormat: DashKit.CoreExportFormat = switch format {
        case .dumpWallet: .dumpWallet
        case .importDescriptorsJSON: .importDescriptorsJSON
        }
        let grantID = grant.id
        let report = try await context.call { engine, network throws(DashKitError) in
            try await engine.exportForCore(on: network, wallet: id, format: kitFormat, to: file, grantID: grantID)
        }
        let reportFormat: CoreExportFormat = switch report.format {
        case .dumpWallet: .dumpWallet
        case .importDescriptorsJSON: .importDescriptorsJSON
        }
        return CoreExportReport(
            file: report.file, format: reportFormat, keyCount: Int(report.keyCount),
            warnings: report.warnings.map(CoreExportWarning.init))
    }

    public func mnemonicCompatibility(wallet: WalletID) async throws(ServiceError) -> (
        coreCompatible: Bool, warnings: [CoreExportWarning]
    ) {
        let id = try wallet.kit
        let result = try await context.call { engine, network throws(DashKitError) in
            try await engine.coreMnemonicCompatibility(on: network, wallet: id)
        }
        return (result.coreCompatible, result.warnings.map(CoreExportWarning.init))
    }
}

// MARK: Backups (QT-110, QT-116)

extension WalletBackup {
    init(_ kit: DashKit.BackupInfo) {
        self.init(
            file: kit.file, walletID: WalletID(kit.walletID), createdAt: kit.createdAt, sizeBytes: kit.sizeBytes,
            automatic: kit.automatic)
    }
}

/// `BackupProviding` over the engine's `.dwbackup` calls. A restore adds
/// wallets, so it runs on the lifecycle queue with the `.addingWallet`
/// overlay.
public final class BackupService: BackupProviding {
    private let context: EngineContext
    private let queue: LifecycleQueue

    init(context: EngineContext, queue: LifecycleQueue) {
        self.context = context
        self.queue = queue
    }

    public func backup(wallet: WalletID, to file: URL, passphrase: (any SecretBuffer)?) async throws(ServiceError)
        -> WalletBackup
    {
        let id = try wallet.kit
        let secret = passphrase.map(secretBytes)
        return WalletBackup(try await context.call { engine, network throws(DashKitError) in
            try await engine.backupWallet(on: network, wallet: id, to: file, passphrase: secret)
        })
    }

    public func restore(from file: URL, passphrase: (any SecretBuffer)?) async throws(ServiceError) -> [WalletID] {
        let secret = passphrase.map(secretBytes)
        return try await queue.runWalletOperation(transition: .addingWallet, walletsChanged: true) {
            engine, network throws(ServiceError) in
            try await serviceCall { () async throws(DashKitError) in
                try await engine.restoreBackup(on: network, file: file, passphrase: secret)
            }.map(WalletID.init)
        }
    }

    public func automaticBackups(wallet: WalletID?) async throws(ServiceError) -> [WalletBackup] {
        let id = try wallet?.kit
        return try await context.call { engine, network throws(DashKitError) in
            try await engine.automaticBackups(on: network, wallet: id)
        }.map(WalletBackup.init)
    }

    public func policy() async throws(ServiceError) -> BackupPolicy {
        let p = try await context.call { engine, network throws(DashKitError) in try await engine.backupPolicy(on: network) }
        return BackupPolicy(keep: Int(p.keep), directory: p.directory)
    }

    /// `keep` outside 0…10 is the engine's `invalid_argument`.
    public func setKeep(_ keep: Int) async throws(ServiceError) -> BackupPolicy {
        guard let value = UInt32(exactly: keep) else {
            throw ServiceError(code: .invalidArgument, detail: "keep \(keep)")
        }
        let p = try await context.call { engine, network throws(DashKitError) in
            try await engine.setBackupPolicy(on: network, keep: value)
        }
        return BackupPolicy(keep: Int(p.keep), directory: p.directory)
    }
}

// MARK: PSBT (QT-076…079)

/// `PSBTHandling`: holds the engine's PSBT objects behind `PSBTReference`s
/// until `release`, as `TransactionDraft` holds prepared transactions.
public final class PSBTService: PSBTHandling, @unchecked Sendable {
    private let context: EngineContext
    // `lock` guards `handles`.
    private let lock = NSLock()
    private var handles: [UUID: DashKit.PSBTHandle] = [:]

    init(context: EngineContext) {
        self.context = context
    }

    private func store(_ handle: DashKit.PSBTHandle) throws(ServiceError) -> PSBTReference {
        let txid = try serviceCall { () throws(DashKitError) in try handle.unsignedTxid() }
        let reference = PSBTReference(id: UUID(), unsignedTxid: txid)
        lock.withLock { handles[reference.id] = handle }
        return reference
    }

    private func handle(_ reference: PSBTReference) throws(ServiceError) -> DashKit.PSBTHandle {
        guard let handle = lock.withLock({ handles[reference.id] }) else {
            throw ServiceError(code: .invalidArgument, detail: "unknown or released PSBT \(reference.unsignedTxid)")
        }
        return handle
    }

    /// Only drafts from this runtime's `TransactionSender` can be exported.
    public func createUnsigned(from draft: any TransactionDrafting) async throws(ServiceError) -> PSBTReference {
        guard let draft = draft as? TransactionDraft else {
            throw ServiceError(code: .invalidArgument, detail: "not an engine transaction draft")
        }
        return try store(try await draft.createUnsignedPSBT())
    }

    public func load(_ data: Data) throws(ServiceError) -> PSBTReference {
        let engine = context.engine
        return try store(try serviceCall { () throws(DashKitError) in try engine.parsePSBT(data) })
    }

    public func base64(_ psbt: PSBTReference) throws(ServiceError) -> String {
        let handle = try handle(psbt)
        return try serviceCall { () throws(DashKitError) in try handle.base64() }
    }

    public func bytes(_ psbt: PSBTReference) throws(ServiceError) -> Data {
        let handle = try handle(psbt)
        return try serviceCall { () throws(DashKitError) in try handle.bytes() }
    }

    public func analyze(_ psbt: PSBTReference, wallet: WalletID?) async throws(ServiceError) -> PSBTAnalysis {
        let handle = try handle(psbt)
        let id = try wallet?.kit
        let a = try await context.call { engine, network throws(DashKitError) in
            try await engine.analyzePSBT(on: network, wallet: id, psbt: handle)
        }
        let status: PSBTStatus = switch a.status {
        case .missingInputInfo: .missingInputInfo
        case .needsSignatures: .needsSignatures
        case .complete: .complete
        }
        let signability: PSBTSignability = switch a.signability {
        case .noWallet: .noWallet
        case .watchOnly: .watchOnly
        case .noMatchingKeys: .noMatchingKeys
        case .canSign: .canSign
        }
        return PSBTAnalysis(
            outputs: a.outputs.map { PSBTOutput(address: $0.address, amount: Amount($0.amount), isMine: $0.isMine) },
            fee: a.fee.map(Amount.init), total: a.total.map(Amount.init), unsignedInputs: Int(a.unsignedInputs),
            status: status, signability: signability, externalSent: a.externalSent.map(Amount.init))
    }

    /// Needs a `.spend` grant; the engine checks it covers the external outputs.
    public func sign(_ psbt: PSBTReference, wallet: WalletID, grant: AuthGrant) async throws(ServiceError)
        -> PSBTReference
    {
        guard case .spend = grant.purpose else {
            throw ServiceError(code: .vaultGrantPurposeMismatch, detail: "signing a PSBT needs a spend grant")
        }
        let handle = try handle(psbt)
        let id = try wallet.kit
        let grantID = grant.id
        let signed = try await context.call { engine, network throws(DashKitError) in
            try await engine.signPSBT(on: network, wallet: id, psbt: handle, grantID: grantID)
        }
        return try store(signed)
    }

    public func broadcast(_ psbt: PSBTReference) async throws(ServiceError) -> String {
        let handle = try handle(psbt)
        return try await context.call { engine, network throws(DashKitError) in
            try await engine.broadcastPSBT(on: network, psbt: handle)
        }
    }

    public func release(_ psbt: PSBTReference) {
        _ = lock.withLock { handles.removeValue(forKey: psbt.id) }
    }
}

// MARK: Tools window (QT-040, QT-117, QT-143…148, IOS-107, IOS-113)

/// `NodeInformationProviding` over `node_info` and `warnings`. SPV-unknown
/// fields stay `nil`.
public final class NodeInformationService: NodeInformationProviding {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func information() async throws(ServiceError) -> NodeInformation {
        let i = try await context.call { engine, network throws(DashKitError) in try await engine.nodeInfo(on: network) }
        return NodeInformation(
            clientVersion: i.clientVersion, userAgent: i.userAgent, dataDirectory: i.dataDirectory,
            startupDate: i.startupTime, network: DashNetwork(i.network), connectionsIn: Int(i.connectionsIn),
            connectionsOut: Int(i.connectionsOut), localAddresses: i.localAddresses, tipHeight: i.tipHeight,
            tipDate: i.tipTime, tipHash: i.tipHash,
            bestChainLock: i.bestChainLock.map {
                ChainLockInfo(height: $0.height, blockHash: $0.blockHash, blockDate: $0.blockTime)
            },
            masternodes: i.masternodes.map { MasternodeCount(total: Int($0.total), enabled: Int($0.enabled)) },
            evonodes: i.evonodes.map { MasternodeCount(total: Int($0.total), enabled: Int($0.enabled)) },
            mempoolTransactionCount: i.mempoolTransactionCount.map(Int.init), mempoolUsageBytes: i.mempoolUsageBytes)
    }

    public func warnings() async throws(ServiceError) -> [NodeWarning] {
        try await context.call { engine, network throws(DashKitError) in try await engine.warnings(on: network) }.map {
            switch $0.code {
            case .prereleaseBuild: .prereleaseBuild
            case .uncleanShutdown: .uncleanShutdown
            case .syncStalled: .syncStalled
            case .clockSkew: .clockSkew
            case .platformContextUnavailable: .platformContextUnavailable
            }
        }
    }
}

/// `PeerModerating` over the engine's peer calls. The engine answers
/// `not_implemented` until dash-spv exposes peer moderation (upstream U2).
public final class PeerModerationService: PeerModerating {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func disconnect(address: String) async throws(ServiceError) {
        try await context.call { engine, network throws(DashKitError) in
            try await engine.disconnectPeer(on: network, address: address)
        }
    }

    public func ban(address: String, for duration: Duration) async throws(ServiceError) {
        let seconds = duration.components.seconds
        guard seconds > 0 else { throw ServiceError(code: .invalidArgument, detail: "ban duration \(duration)") }
        try await context.call { engine, network throws(DashKitError) in
            try await engine.banPeer(on: network, address: address, durationSeconds: UInt64(seconds))
        }
    }

    public func unban(subnet: String) async throws(ServiceError) {
        try await context.call { engine, network throws(DashKitError) in
            try await engine.unbanPeer(on: network, subnet: subnet)
        }
    }

    public func bannedPeers() async throws(ServiceError) -> [BannedPeer] {
        try await context.call { engine, network throws(DashKitError) in try await engine.bannedPeers(on: network) }
            .map { BannedPeer(subnet: $0.subnet, bannedUntil: $0.bannedUntil) }
    }
}

/// `RepairProviding`: rescan progress and cancel, birth height, and the
/// chain-data reset (SPV stop → reset → SPV start on the lifecycle queue;
/// SPV is started again even when the reset fails).
public final class RepairService: RepairProviding {
    private let context: EngineContext
    private let queue: LifecycleQueue

    init(context: EngineContext, queue: LifecycleQueue) {
        self.context = context
        self.queue = queue
    }

    public func rescanProgress() async throws(ServiceError) -> RescanProgress? {
        try await context.call { engine, network throws(DashKitError) in try await engine.rescanProgress(on: network) }
            .map {
                RescanProgress(
                    fromHeight: $0.fromHeight, currentHeight: $0.currentHeight, targetHeight: $0.targetHeight,
                    startedAt: $0.startedAt)
            }
    }

    public func cancelRescan() async throws(ServiceError) -> Bool {
        try await context.call { engine, network throws(DashKitError) in try await engine.cancelRescan(on: network) }
    }

    public func resetChainData() async throws(ServiceError) {
        try await queue.runWalletOperation(transition: nil, walletsChanged: false) { engine, network throws(ServiceError) in
            try await serviceCall { () async throws(DashKitError) in
                if try await engine.isSPVRunning(on: network) {
                    try await engine.stopSPV(on: network)
                }
                let reset: Result<Void, DashKitError>
                do throws(DashKitError) {
                    try await engine.resetChainData(on: network)
                    reset = .success(())
                } catch {
                    reset = .failure(error)
                }
                try await engine.startSPV(on: network)
                try reset.get()
            }
        }
    }

    public func setBirthHeight(wallet: WalletID, height: UInt32) async throws(ServiceError) {
        let id = try wallet.kit
        try await context.call { engine, network throws(DashKitError) in
            try await engine.setBirthHeight(on: network, wallet: id, height: height)
        }
    }
}

/// `ConsoleExecuting` over dw-console. The engine's
/// `console.authorization_required` arrives as `.authorizationRequired`.
public final class ConsoleService: ConsoleExecuting {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func commands() async throws(ServiceError) -> [ConsoleCommandInfo] {
        try serviceCall { () throws(DashKitError) in try CoreFunctions.consoleCommands() }.map {
            ConsoleCommandInfo(name: $0.name, category: $0.category, sensitive: $0.sensitive, available: $0.available)
        }
    }

    public func redact(_ line: any SecretBuffer) throws(ServiceError) -> String {
        let secret = secretBytes(line)
        return try serviceCall { () throws(DashKitError) in try CoreFunctions.consoleRedact(secret) }
    }

    public func execute(_ line: any SecretBuffer, wallet: WalletID?, grant: AuthGrant?) async throws(ServiceError)
        -> ConsoleResult
    {
        let secret = secretBytes(line)
        let id = try wallet?.kit
        let grantID = grant?.id
        let result = try await context.call { engine, network throws(DashKitError) in
            try await engine.consoleExecute(on: network, wallet: id, line: secret, grantID: grantID)
        }
        return switch result {
        case .output(let text, let isJSON): .output(text: text, isJSON: isJSON)
        case .authorizationRequired(let purpose, let wallet): .authorizationRequired(GrantPurpose(purpose), wallet: wallet.map(WalletID.init))
        }
    }
}
