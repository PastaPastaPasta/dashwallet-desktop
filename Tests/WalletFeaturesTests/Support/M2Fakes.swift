// In-memory fakes of the M2 WalletRuntime and PlatformServices contracts
// (docs/contracts/m2-swift.md §2). Like the M1 fakes they record calls and
// answer from configured state; an unconfigured call throws not_implemented,
// as the engine stubs do. Argument rules the engine checks first (ranges,
// sizes, spend-limit options) are checked here too, with the engine's codes.
import Foundation
import PlatformServices
import WalletFeatures
import WalletRuntime

/// A per-call error table shared by the fakes below.
final class FakeErrors: @unchecked Sendable {
    private let errors = Locked<[String: ServiceError]>([:])

    func set(_ error: ServiceError?, for call: String) {
        errors.withLock { $0[call] = error }
    }

    func check(_ call: String) throws(ServiceError) {
        if let error = errors.current[call] { throw error }
    }
}

// MARK: Wallet lifecycle (§2.1)

final class FakeWalletLifecycleManager: WalletLifecycleManaging, @unchecked Sendable {
    let states = Locked<[WalletLoadState]?>(nil)
    let networks = Locked<[NetworkDataInfo]?>(nil)
    let xpubs = Locked<[WalletID: AccountXpub]>([:])
    let calls = Locked<[String]>([])
    let errors = FakeErrors()
    let changes = Broadcast<Void>()
    var nextWatchOnlyID = WalletID(hex: String(repeating: "cd", count: 32))!

    func existingNetworks() async throws(ServiceError) -> [NetworkDataInfo] {
        try errors.check("existingNetworks")
        guard let networks = networks.current else { throw notConfigured("existingNetworks") }
        return networks
    }

    func loadStates() async throws(ServiceError) -> [WalletLoadState] {
        guard let states = states.current else { throw notConfigured("loadStates") }
        return states
    }

    func loadStateChanges() -> AsyncStream<Void> { changes.stream() }

    func load(_ wallet: WalletID) async throws(ServiceError) {
        try setLoaded(wallet, true, call: "load")
    }

    func unload(_ wallet: WalletID) async throws(ServiceError) {
        try setLoaded(wallet, false, call: "unload")
    }

    func setLoadOnStartup(_ wallet: WalletID, _ loadOnStartup: Bool) async throws(ServiceError) {
        try update(wallet, call: "setLoadOnStartup(\(loadOnStartup))") { state in
            WalletLoadState(
                walletID: state.walletID, name: state.name, loaded: state.loaded, loadOnStartup: loadOnStartup,
                watchOnly: state.watchOnly)
        }
    }

    /// The engine checks the xpub prefix and network: `xpub`/`tpub` only.
    func importWatchOnly(xpub: String, options: WatchOnlyImportOptions) async throws(ServiceError) -> WalletID {
        calls.withLock { $0.append("importWatchOnly") }
        try errors.check("importWatchOnly")
        guard states.current != nil else { throw notConfigured("importWatchOnly") }
        guard xpub.hasPrefix("tpub") || xpub.hasPrefix("xpub"), xpub.count > 100 else {
            throw ServiceError(code: .walletInvalidXpub)
        }
        if let lookahead = options.lookahead, !(1...1000).contains(lookahead) {
            throw ServiceError(code: .invalidArgument, detail: "lookahead")
        }
        let id = nextWatchOnlyID
        states.withLock {
            $0?.append(WalletLoadState(
                walletID: id, name: options.name ?? "Watch-only", loaded: true, loadOnStartup: true, watchOnly: true))
        }
        changes.send(())
        return id
    }

    func accountXpub(wallet: WalletID, account: UInt32) async throws(ServiceError) -> AccountXpub {
        guard let key = xpubs.current[wallet] else { throw notConfigured("accountXpub") }
        guard key.account == account else { throw ServiceError(code: .invalidArgument, detail: "account") }
        return key
    }

    private func setLoaded(_ wallet: WalletID, _ loaded: Bool, call: String) throws(ServiceError) {
        try update(wallet, call: call) { state in
            WalletLoadState(
                walletID: state.walletID, name: state.name, loaded: loaded, loadOnStartup: state.loadOnStartup,
                watchOnly: state.watchOnly)
        }
    }

    private func update(
        _ wallet: WalletID, call: String, _ change: (WalletLoadState) -> WalletLoadState
    ) throws(ServiceError) {
        calls.withLock { $0.append("\(call):\(wallet.hex.prefix(4))") }
        try errors.check(String(call.prefix { $0 != "(" }))
        guard var list = states.current else { throw notConfigured(call) }
        guard let index = list.firstIndex(where: { $0.walletID == wallet }) else { throw ServiceError(code: .walletNotFound) }
        list[index] = change(list[index])
        states.withLock { $0 = list }
        changes.send(())
    }
}

// MARK: Transactions (§2.2)

final class FakeTransactionActions: TransactionActing, @unchecked Sendable {
    let extras = Locked<[String: TransactionDetailExtras]>([:])
    /// Refusals the engine would answer, by txid.
    let refusals = Locked<[String: TransactionActionRefusal]>([:])
    let abandoned = Locked<[String]>([])
    let resent = Locked<[String]>([])
    let dropCount = Locked<Int?>(nil)
    let dropCalls = Locked<[WalletID?]>([])
    let csv = Locked<Data?>(nil)
    let csvCalls = Locked<[(HistoryFilter, HistoryCSVOptions)]>([])
    let errors = FakeErrors()

    func extras(wallet: WalletID, txid: String) async throws(ServiceError) -> TransactionDetailExtras {
        try errors.check("extras")
        guard let extras = extras.current[txid] else { throw notConfigured("extras") }
        return extras
    }

    func abandon(wallet: WalletID, txid: String) async throws(ServiceError) {
        try errors.check("abandon")
        guard extras.current[txid] != nil else { throw ServiceError(code: .txActionTxNotFound) }
        if let refusal = refusals.current[txid] {
            throw ServiceError(code: .txActionRefused, parameters: ["refusal": refusal.rawValue])
        }
        abandoned.withLock { $0.append(txid) }
    }

    func resend(wallet: WalletID, txid: String) async throws(ServiceError) {
        try errors.check("resend")
        guard extras.current[txid] != nil else { throw ServiceError(code: .txActionTxNotFound) }
        if let refusal = refusals.current[txid] {
            throw ServiceError(code: .txActionRefused, parameters: ["refusal": refusal.rawValue])
        }
        resent.withLock { $0.append(txid) }
    }

    func dropUnconfirmed(wallet: WalletID?) async throws(ServiceError) -> Int {
        try errors.check("dropUnconfirmed")
        guard let count = dropCount.current else { throw notConfigured("dropUnconfirmed") }
        dropCalls.withLock { $0.append(wallet) }
        return count
    }

    /// `history.invalid_query` unless there are 0 or 19 type names (engine rule).
    func exportCSV(wallet: WalletID, filter: HistoryFilter, sort: HistorySort, options: HistoryCSVOptions)
        async throws(ServiceError) -> Data
    {
        guard options.typeNames.isEmpty || options.typeNames.count == TxType.allCases.count else {
            throw ServiceError(code: .historyInvalidQuery, detail: "type names")
        }
        guard let csv = csv.current else { throw notConfigured("exportCSV") }
        csvCalls.withLock { $0.append((filter, options)) }
        return csv
    }
}

final class FakeFees: FeeAndCoinSelectionProviding, @unchecked Sendable {
    /// dash-qt `fee_policy` on SPV (m2-engine.md §2.3).
    static let spvPolicy = FeePolicy(
        source: .minimumRelay, minimumRelayPerKB: 1000, maximumCustomPerKB: 10_000_000,
        maximumTransactionFee: Amount(duffs: 10_000_000), maximumBroadcastRatePerKB: 10_000_000,
        targets: [2, 4, 6, 12, 24, 48, 144, 504, 1008].map { FeeTarget(targetBlocks: $0, duffsPerKB: 1000) })

    let coins = Locked<[OutPoint: Amount]?>(nil)
    let summaries = Locked<[[OutPoint]]>([])
    let errors = FakeErrors()

    func feePolicy() async throws(ServiceError) -> FeePolicy {
        try errors.check("feePolicy")
        return Self.spvPolicy
    }

    /// dash-qt's panel formula at 1000 duff/kB: bytes = 148·in + 34·(out+1) + 10,
    /// −34 without change; coins not in `coins` are `unavailable`.
    func summary(
        wallet: WalletID, outpoints: [OutPoint], payAmounts: [Amount], fee: FeeChoice, allChangeToFee: Bool
    ) async throws(ServiceError) -> CoinSelectionSummary {
        try errors.check("summary")
        guard let known = coins.current else { throw notConfigured("summary") }
        summaries.withLock { $0.append(outpoints) }
        let unavailable = outpoints.filter { known[$0] == nil }
        let chosen = outpoints.filter { known[$0] != nil }
        let amount = chosen.reduce(Int64(0)) { $0 + (known[$1]?.duffs ?? 0) }
        let pay = payAmounts.reduce(Int64(0)) { $0 + $1.duffs }
        var bytes = 148 * chosen.count + 34 * (payAmounts.count + 1) + 10
        var feeDuffs = Int64(bytes)
        var change = amount - pay - feeDuffs
        var changeToFee = false
        if allChangeToFee || (change > 0 && change < 546) {
            // No change output: dust change (or all change on the CoinJoin page) goes to the fee.
            bytes -= 34
            feeDuffs = Int64(bytes) + max(0, amount - pay - Int64(bytes))
            changeToFee = true
            change = 0
        } else if change <= 0 {
            bytes -= 34
            feeDuffs = Int64(bytes)
            change = 0
        }
        return CoinSelectionSummary(
            quantity: chosen.count, amount: Amount(duffs: amount), bytes: bytes, fee: Amount(duffs: feeDuffs),
            afterFee: Amount(duffs: max(0, amount - feeDuffs)), change: Amount(duffs: change),
            changeToFee: changeToFee, insufficientFunds: amount < pay + feeDuffs,
            feeTolerancePerInput: Amount(duffs: 2), unavailable: unavailable)
    }
}

// MARK: Compatibility, backups, PSBT (§2.3)

final class FakeFileImporter: WalletFileImporting, @unchecked Sendable {
    let kinds = Locked<[URL: WalletFileKind]>([:])
    let report = Locked<WalletImportReport?>(nil)
    /// Passphrase an encrypted wallet.dat accepts.
    let walletDatPassphrase = Locked<String?>(nil)
    let calls = Locked<[String]>([])
    let materials = Locked<[String]>([])
    let errors = FakeErrors()

    func inspect(_ file: URL) async throws(ServiceError) -> WalletFileKind {
        try errors.check("inspect")
        guard let kind = kinds.current[file] else { throw ServiceError(code: .compatFileUnreadable) }
        return kind
    }

    func importDumpWallet(_ file: URL, options: WalletImportOptions) async throws(ServiceError) -> WalletImportReport {
        calls.withLock { $0.append("dump") }
        try errors.check("importDumpWallet")
        return try configuredReport("importDumpWallet")
    }

    /// BDB is not_implemented "import_wallet_dat.bdb" until M6 (engine rule).
    func importWalletDat(_ file: URL, passphrase: (any SecretBuffer)?, options: WalletImportOptions)
        async throws(ServiceError) -> WalletImportReport
    {
        calls.withLock { $0.append("walletDat") }
        try errors.check("importWalletDat")
        switch kinds.current[file] {
        case .walletDatBerkeleyDB?:
            throw ServiceError(code: .notImplemented, detail: "import_wallet_dat.bdb")
        case .walletDatSQLite(let encrypted, _)?:
            if encrypted {
                guard let passphrase else { throw ServiceError(code: .compatPassphraseRequired) }
                guard passphrase.testString == walletDatPassphrase.current else {
                    throw ServiceError(code: .compatWrongPassphrase)
                }
            }
        default:
            throw ServiceError(code: .compatUnsupportedFormat)
        }
        return try configuredReport("importWalletDat")
    }

    func importKeyMaterial(_ material: KeyMaterial, options: WalletImportOptions) async throws(ServiceError)
        -> WalletImportReport
    {
        switch material {
        case .hdSeed(let seed):
            guard (16...64).contains(seed.count) else { throw ServiceError(code: .compatInvalidKeyMaterial) }
            materials.withLock { $0.append("seed:\(seed.count)") }
        case .xprv(let text): materials.withLock { $0.append("xprv:\(text.testString)") }
        case .descriptors(let json): materials.withLock { $0.append("descriptors:\(json.count)") }
        }
        try errors.check("importKeyMaterial")
        return try configuredReport("importKeyMaterial")
    }

    private func configuredReport(_ call: String) throws(ServiceError) -> WalletImportReport {
        guard let report = report.current else { throw notConfigured(call) }
        return report
    }
}

final class FakeCoreExporter: CoreExporting, @unchecked Sendable {
    let report = Locked<CoreExportReport?>(nil)
    let compatible = Locked<(Bool, [CoreExportWarning])?>(nil)
    let grants = Locked<[AuthGrant]>([])

    func export(wallet: WalletID, format: CoreExportFormat, to file: URL, grant: AuthGrant)
        async throws(ServiceError) -> CoreExportReport
    {
        guard case .revealSecret = grant.purpose else { throw ServiceError(code: .compatGrantInvalid) }
        grants.withLock { $0.append(grant) }
        guard let report = report.current else { throw notConfigured("export") }
        return report
    }

    func mnemonicCompatibility(wallet: WalletID) async throws(ServiceError) -> (
        coreCompatible: Bool, warnings: [CoreExportWarning]
    ) {
        guard let compatible = compatible.current else { throw notConfigured("mnemonicCompatibility") }
        return (compatible.0, compatible.1)
    }
}

final class FakeBackups: BackupProviding, @unchecked Sendable {
    let policyValue = Locked<BackupPolicy?>(nil)
    let automatic = Locked<[WalletBackup]?>(nil)
    let backupResult = Locked<WalletBackup?>(nil)
    let backupPassphrases = Locked<[String?]>([])
    /// The passphrase a restore accepts; `nil` = the backup needs none.
    let restorePassphrase = Locked<String?>(nil)
    let restoreIDs = Locked<[WalletID]?>(nil)
    let keeps = Locked<[Int]>([])

    func backup(wallet: WalletID, to file: URL, passphrase: (any SecretBuffer)?) async throws(ServiceError)
        -> WalletBackup
    {
        backupPassphrases.withLock { $0.append(passphrase?.testString) }
        guard let result = backupResult.current else { throw notConfigured("backup") }
        return result
    }

    func restore(from file: URL, passphrase: (any SecretBuffer)?) async throws(ServiceError) -> [WalletID] {
        guard let ids = restoreIDs.current else { throw notConfigured("restore") }
        if let expected = restorePassphrase.current {
            guard let passphrase else { throw ServiceError(code: .backupPassphraseRequired) }
            guard passphrase.testString == expected else { throw ServiceError(code: .backupWrongPassphrase) }
        }
        return ids
    }

    func automaticBackups(wallet: WalletID?) async throws(ServiceError) -> [WalletBackup] {
        guard let automatic = automatic.current else { throw notConfigured("automaticBackups") }
        return automatic.filter { wallet == nil || $0.walletID == wallet }
    }

    func policy() async throws(ServiceError) -> BackupPolicy {
        guard let policy = policyValue.current else { throw notConfigured("policy") }
        return policy
    }

    /// 0...10 (`invalid_argument` otherwise), as `set_backup_policy`.
    func setKeep(_ keep: Int) async throws(ServiceError) -> BackupPolicy {
        guard let policy = policyValue.current else { throw notConfigured("setKeep") }
        guard (0...10).contains(keep) else { throw ServiceError(code: .invalidArgument, detail: "keep") }
        keeps.withLock { $0.append(keep) }
        let next = BackupPolicy(keep: keep, directory: policy.directory)
        policyValue.withLock { $0 = next }
        return next
    }
}

final class FakePSBT: PSBTHandling, @unchecked Sendable {
    static let maxSize = 100 * 1024 * 1024

    /// Analysis per reference id; `load` hands out `loadAnalysis`.
    let analyses = Locked<[UUID: PSBTAnalysis]>([:])
    let loadAnalysis = Locked<PSBTAnalysis?>(nil)
    let signedAnalysis = Locked<PSBTAnalysis?>(nil)
    let broadcastResult = Locked<Result<String, ServiceError>?>(nil)
    let released = Locked<[UUID]>([])
    let signGrants = Locked<[AuthGrant]>([])
    let loaded = Locked<[Data]>([])
    let createdFrom = Locked<Int>(0)

    func createUnsigned(from draft: any TransactionDrafting) async throws(ServiceError) -> PSBTReference {
        guard let analysis = loadAnalysis.current else { throw notConfigured("createUnsigned") }
        createdFrom.withLock { $0 += 1 }
        return make(analysis)
    }

    /// `psbt.too_large` above 100 MiB, as `parse_psbt` already does.
    func load(_ data: Data) throws(ServiceError) -> PSBTReference {
        guard data.count <= Self.maxSize else { throw ServiceError(code: .psbtTooLarge) }
        loaded.withLock { $0.append(data) }
        guard let analysis = loadAnalysis.current else { throw notConfigured("load") }
        guard String(decoding: data, as: UTF8.self) != "garbage" else { throw ServiceError(code: .psbtInvalid) }
        return make(analysis)
    }

    func base64(_ psbt: PSBTReference) throws(ServiceError) -> String {
        try known(psbt)
        return "cHNidP8B\(psbt.unsignedTxid.prefix(8))"
    }

    func bytes(_ psbt: PSBTReference) throws(ServiceError) -> Data {
        try known(psbt)
        return Data([0x70, 0x73, 0x62, 0x74, 0xff])
    }

    func analyze(_ psbt: PSBTReference, wallet: WalletID?) async throws(ServiceError) -> PSBTAnalysis {
        guard let analysis = analyses.current[psbt.id] else { throw ServiceError(code: .invalidArgument) }
        return analysis
    }

    /// Like the engine: refuses an unknown fee, then needs
    /// `.spend(max: ≥ total)` (`psbt.grant_exceeded{max_duffs}`).
    func sign(_ psbt: PSBTReference, wallet: WalletID, grant: AuthGrant) async throws(ServiceError) -> PSBTReference {
        guard let analysis = analyses.current[psbt.id] else { throw ServiceError(code: .invalidArgument) }
        guard case .spend(let max) = grant.purpose else { throw ServiceError(code: .psbtGrantInvalid) }
        guard let total = analysis.total else { throw ServiceError(code: .psbtFeeUnknown) }
        if max < total {
            throw ServiceError(code: .psbtGrantExceeded, parameters: ["max_duffs": max.duffs])
        }
        signGrants.withLock { $0.append(grant) }
        guard let signed = signedAnalysis.current else { throw notConfigured("sign") }
        return make(signed)
    }

    func broadcast(_ psbt: PSBTReference) async throws(ServiceError) -> String {
        guard let analysis = analyses.current[psbt.id] else { throw ServiceError(code: .invalidArgument) }
        guard analysis.status == .complete else { throw ServiceError(code: .psbtNotComplete) }
        guard let result = broadcastResult.current else { throw notConfigured("broadcast") }
        return try result.get()
    }

    func release(_ psbt: PSBTReference) {
        released.withLock { $0.append(psbt.id) }
        analyses.withLock { $0[psbt.id] = nil }
    }

    private func make(_ analysis: PSBTAnalysis) -> PSBTReference {
        let reference = PSBTReference(id: UUID(), unsignedTxid: txid(7))
        analyses.withLock { $0[reference.id] = analysis }
        return reference
    }

    private func known(_ psbt: PSBTReference) throws(ServiceError) {
        guard analyses.current[psbt.id] != nil else { throw ServiceError(code: .invalidArgument) }
    }
}

// MARK: Tools (§2.4)

final class FakeNodeInformation: NodeInformationProviding, @unchecked Sendable {
    let info = Locked<NodeInformation?>(nil)
    let warningList = Locked<[NodeWarning]>([])

    func information() async throws(ServiceError) -> NodeInformation {
        guard let info = info.current else { throw notConfigured("information") }
        return info
    }

    func warnings() async throws(ServiceError) -> [NodeWarning] { warningList.current }

    static func sample(network: DashNetwork = .testnet) -> NodeInformation {
        NodeInformation(
            clientVersion: "v0.2.0", userAgent: "/DashWalletDesktop:0.2.0/", dataDirectory: URL(fileURLWithPath: "/data"),
            startupDate: Date(timeIntervalSince1970: 1_760_000_000), network: network, connectionsIn: 0,
            connectionsOut: 8, localAddresses: [], tipHeight: 1_000_000, tipDate: Date(timeIntervalSince1970: 1_760_000_000),
            tipHash: String(repeating: "0a", count: 32), bestChainLock: nil,
            masternodes: MasternodeCount(total: 3000, enabled: 2900), evonodes: nil, mempoolTransactionCount: nil,
            mempoolUsageBytes: nil)
    }
}

final class FakePeerModeration: PeerModerating, @unchecked Sendable {
    /// Connected addresses; `nil` = not configured.
    let connected = Locked<[String]?>(nil)
    let banned = Locked<[BannedPeer]>([])
    let calls = Locked<[String]>([])

    func disconnect(address: String) async throws(ServiceError) {
        try requirePeer(address, "disconnect")
        calls.withLock { $0.append("disconnect \(address)") }
    }

    func ban(address: String, for duration: Duration) async throws(ServiceError) {
        try requirePeer(address, "ban")
        calls.withLock { $0.append("ban \(address) \(duration.components.seconds)") }
        banned.withLock {
            $0.append(BannedPeer(subnet: address + "/32", bannedUntil: Date(timeIntervalSince1970: 1_760_000_000)))
        }
    }

    func unban(subnet: String) async throws(ServiceError) {
        guard connected.current != nil else { throw notConfigured("unban") }
        calls.withLock { $0.append("unban \(subnet)") }
        banned.withLock { $0.removeAll { $0.subnet == subnet } }
    }

    func bannedPeers() async throws(ServiceError) -> [BannedPeer] {
        guard connected.current != nil else { throw notConfigured("bannedPeers") }
        return banned.current
    }

    private func requirePeer(_ address: String, _ call: String) throws(ServiceError) {
        guard let connected = connected.current else { throw notConfigured(call) }
        guard connected.contains(address) else { throw ServiceError(code: .syncPeerNotFound) }
    }
}

final class FakeRepair: RepairProviding, @unchecked Sendable {
    let progress = Locked<RescanProgress?>(nil)
    let configured = Locked(false)
    let resets = Locked(0)
    let cancels = Locked(0)
    let birthHeights = Locked<[(WalletID, UInt32)]>([])
    let errors = FakeErrors()
    var tip: UInt32 = 1_000_000

    func rescanProgress() async throws(ServiceError) -> RescanProgress? {
        guard configured.current else { throw notConfigured("rescanProgress") }
        return progress.current
    }

    func cancelRescan() async throws(ServiceError) -> Bool {
        guard configured.current else { throw notConfigured("cancelRescan") }
        cancels.withLock { $0 += 1 }
        let ran = progress.current != nil
        progress.withLock { $0 = nil }
        return ran
    }

    func resetChainData() async throws(ServiceError) {
        try errors.check("resetChainData")
        guard configured.current else { throw notConfigured("resetChainData") }
        resets.withLock { $0 += 1 }
    }

    /// `sync.height_out_of_range` above the tip.
    func setBirthHeight(wallet: WalletID, height: UInt32) async throws(ServiceError) {
        guard configured.current else { throw notConfigured("setBirthHeight") }
        guard height <= tip else { throw ServiceError(code: .syncHeightOutOfRange, parameters: ["height": Int64(height)]) }
        birthHeights.withLock { $0.append((wallet, height)) }
    }
}

/// A console that knows a few commands, redacts the dash-qt sensitive ones
/// and asks for a grant for `sendtoaddress` / `dumpprivkey`.
final class FakeConsole: ConsoleExecuting, @unchecked Sendable {
    static let sensitive: Set<String> = [
        "importprivkey", "importmulti", "sethdseed", "signmessagewithprivkey", "signrawtransactionwithkey",
        "upgradetohd", "walletpassphrase", "walletpassphrasechange", "encryptwallet",
    ]

    let executed = Locked<[(String, WalletID?, AuthGrant?)]>([])

    func commands() async throws(ServiceError) -> [ConsoleCommandInfo] {
        ["getblockcount", "getbalance", "getnetworkinfo", "sendtoaddress", "walletpassphrase"].map {
            ConsoleCommandInfo(name: $0, category: "", sensitive: Self.sensitive.contains($0), available: true)
        }
    }

    func redact(_ line: any SecretBuffer) throws(ServiceError) -> String {
        let text = line.testString
        guard text.filter({ $0 == "\"" }).count % 2 == 0 else { throw ServiceError(code: .consoleParseError) }
        let command = String(text.split(separator: " ").first ?? "")
        return Self.sensitive.contains(command) ? "\(command)(…)" : text
    }

    func execute(_ line: any SecretBuffer, wallet: WalletID?, grant: AuthGrant?) async throws(ServiceError)
        -> ConsoleResult
    {
        let text = line.testString
        executed.withLock { $0.append((text, wallet, grant)) }
        let command = String(text.split(separator: " ").first ?? "")
        switch command {
        case "getblockcount": return .output(text: "1000000", isJSON: false)
        case "getbalance":
            guard wallet != nil else { throw ServiceError(code: .consoleWalletRequired) }
            return .output(text: "1.00000000", isJSON: false)
        case "sendtoaddress":
            guard let wallet else { throw ServiceError(code: .consoleWalletRequired) }
            guard let grant else { return .authorizationRequired(.spend(max: Amount(duffs: 100_000_000)), wallet: wallet) }
            guard case .spend = grant.purpose else { throw ServiceError(code: .sendGrantInvalid) }
            return .output(text: txid(9), isJSON: false)
        case "getblock":
            throw ServiceError(code: .consoleRPCError, detail: "Block not found (code -5)", parameters: ["code": -5])
        case "getmempoolinfo":
            throw ServiceError(code: .consoleNotAvailable, detail: command)
        default:
            throw ServiceError(code: .consoleRPCError, detail: "Method not found (code -32601)", parameters: ["code": -32601])
        }
    }
}

final class FakeLogs: LogExporting, @unchecked Sendable {
    let exported = Locked<[URL]>([])
    let configured = Locked(false)

    func exportLogs(to file: URL) async throws(ServiceError) -> URL {
        guard configured.current else { throw notConfigured("exportLogs") }
        exported.withLock { $0.append(file) }
        return file
    }
}

// MARK: Security (§2.5)

final class FakeQuickUnlock: QuickUnlockManaging, @unchecked Sendable {
    let providerValue: QuickUnlockProvider
    let current = Locked<QuickUnlockPolicy?>(nil)
    let grants = Locked<[AuthGrant]>([])

    init(provider: QuickUnlockProvider = .touchID) {
        providerValue = provider
    }

    var provider: QuickUnlockProvider { providerValue }

    func policy() async throws(ServiceError) -> QuickUnlockPolicy {
        guard let policy = current.current else { throw notConfigured("policy") }
        return policy
    }

    func enroll(grant: AuthGrant) async throws(ServiceError) -> QuickUnlockPolicy {
        try requireChangeCredential(grant)
        guard providerValue != .unavailable else { throw ServiceError(code: .vaultQuickUnlockUnavailable) }
        return set { QuickUnlockPolicy(enrolled: true, spendLimit: QuickUnlockPolicy.defaultSpendLimit, passphraseMaxAge: $0.passphraseMaxAge, lastPassphraseEntry: $0.lastPassphraseEntry) }
    }

    func remove() async throws(ServiceError) -> QuickUnlockPolicy {
        guard current.current != nil else { throw notConfigured("remove") }
        return set { QuickUnlockPolicy(enrolled: false, spendLimit: $0.spendLimit, passphraseMaxAge: $0.passphraseMaxAge, lastPassphraseEntry: $0.lastPassphraseEntry) }
    }

    /// One of the five iOS options (`invalid_argument` otherwise).
    func setSpendLimit(_ limit: Amount, grant: AuthGrant) async throws(ServiceError) -> QuickUnlockPolicy {
        try requireChangeCredential(grant)
        guard QuickUnlockPolicy.spendLimitOptions.contains(limit) else {
            throw ServiceError(code: .invalidArgument, detail: "spend limit")
        }
        return set { QuickUnlockPolicy(enrolled: $0.enrolled, spendLimit: limit, passphraseMaxAge: $0.passphraseMaxAge, lastPassphraseEntry: $0.lastPassphraseEntry) }
    }

    func credential(reason: String) async throws(ServiceError) -> Credential {
        throw ServiceError(code: .platformCancelled)
    }

    private func requireChangeCredential(_ grant: AuthGrant) throws(ServiceError) {
        guard current.current != nil else { throw notConfigured("quickUnlock") }
        guard grant.purpose == .changeCredential else { throw ServiceError(code: .vaultGrantPurposeMismatch) }
        grants.withLock { $0.append(grant) }
    }

    private func set(_ change: (QuickUnlockPolicy) -> QuickUnlockPolicy) -> QuickUnlockPolicy {
        current.withLock { policy in
            policy = change(policy!)
            return policy!
        }
    }
}

@MainActor
final class FakeAutoLock: AutoLockControlling {
    var interval: AutoLockInterval = .never
    var activity = 0

    func setInterval(_ interval: AutoLockInterval) throws(ServiceError) {
        self.interval = interval
    }

    func noteActivity() {
        activity += 1
    }
}

/// `recover` checks the phrase against the wallet's phrase
/// (`vault.recovery_mismatch`); `destroy` refuses while wallets remain
/// (`vault.not_empty`).
final class FakeVaultRecovery: VaultRecovering, @unchecked Sendable {
    let phrases = Locked<[WalletID: String]>([:])
    let remainingWallets = Locked<@Sendable () -> Int>({ 0 })
    let recoveries = Locked<[(WalletID, String)]>([])
    let destroyed = Locked(0)

    func recover(
        wallet: WalletID, mnemonic: any SecretBuffer, bip39Passphrase: any SecretBuffer,
        newPassphrase: any SecretBuffer
    ) async throws(ServiceError) -> VaultRecoveryResult {
        let phrases = phrases.current
        guard phrases[wallet] != nil else { throw ServiceError(code: .walletNotFound) }
        guard phrases[wallet] == mnemonic.testString else { throw ServiceError(code: .vaultRecoveryMismatch) }
        recoveries.withLock { $0.append((wallet, newPassphrase.testString)) }
        let status = VaultStatus(
            state: .unlocked, encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSeconds: nil,
            walletsWithSecrets: [wallet])
        return VaultRecoveryResult(status: status, walletsWithoutSecrets: phrases.keys.filter { $0 != wallet }.sorted { $0.hex < $1.hex })
    }

    func destroy(credential: Credential) async throws(ServiceError) -> VaultStatus {
        guard remainingWallets.current() == 0 else { throw ServiceError(code: .vaultNotEmpty) }
        destroyed.withLock { $0 += 1 }
        return VaultStatus(
            state: .noVault, encrypted: false, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSeconds: nil,
            walletsWithSecrets: [])
    }
}

// MARK: Shell (§2.6)

@MainActor
final class FakeStartup: StartupProgressing {
    var phase: StartupPhase = .loadingSettings
    var progress: Double = 0
    var quitRequests = 0
    let broadcast = Broadcast<StartupPhase>()

    func changes() -> AsyncStream<StartupPhase> { broadcast.stream() }

    func publish(_ phase: StartupPhase, progress: Double) {
        self.phase = phase
        self.progress = max(self.progress, progress)
        broadcast.send(phase)
    }

    func requestEmergencyQuit() {
        quitRequests += 1
    }
}

@MainActor
final class FakeShutdown: ShutdownCoordinating {
    var isShuttingDown = false
    var shutdowns = 0
    var gate: Gate?

    func shutdown() async {
        isShuttingDown = true
        shutdowns += 1
        if let gate { await gate.wait() }
        isShuttingDown = false
    }
}

@MainActor
final class FakeShellSettings: ShellSettingsProviding {
    var shell = ShellSettings()
    var updates: [ShellSettings] = []
    let broadcast = Broadcast<ShellSettings>()

    func update(_ shell: ShellSettings) throws(ServiceError) {
        updates.append(shell)
        self.shell = shell
        broadcast.send(shell)
    }

    func changes() -> AsyncStream<ShellSettings> { broadcast.stream() }
}

struct FakeLaunchArguments: LaunchArgumentsParsing {
    var optionNames: [String] {
        ["--min", "--splash=<0|1>", "--resetguisettings", "--datadir=<dir>", "--testnet", "--windowtitle=<name>"]
    }

    func parse(_ arguments: [String]) throws(ServiceError) -> LaunchOptions {
        throw notConfigured("parse")
    }
}

@MainActor
final class FakeDesktopPreferences: DesktopPreferencesStoring {
    var desktop = DesktopPreferences()
    var updates = 0
    var updateError: ServiceError?

    func update(_ desktop: DesktopPreferences) throws(ServiceError) {
        if let updateError { throw updateError }
        updates += 1
        self.desktop = desktop
    }
}

/// `set_dust_protection`: `nil` or 1...1,000,000 duffs (`invalid_argument`).
final class FakeDustProtection: DustProtectionControlling, @unchecked Sendable {
    let value = Locked<Amount?>(nil)
    let configured = Locked(false)
    let sets = Locked<[Amount?]>([])

    func threshold() async throws(ServiceError) -> Amount? {
        guard configured.current else { throw notConfigured("dustProtection") }
        return value.current
    }

    func setThreshold(_ threshold: Amount?) async throws(ServiceError) {
        guard configured.current else { throw notConfigured("setDustProtection") }
        if let threshold, !(1...1_000_000).contains(threshold.duffs) {
            throw ServiceError(code: .invalidArgument, detail: "dust threshold")
        }
        sets.withLock { $0.append(threshold) }
        value.withLock { $0 = threshold }
    }
}

@MainActor
final class FakeOptionsReset: OptionsResetting {
    var recoveredFromCorruption: [URL] = []
    var resets = 0
    var resetError: ServiceError?

    func resetOptions() throws(ServiceError) -> [URL] {
        if let resetError { throw resetError }
        resets += 1
        return [URL(fileURLWithPath: "/data/settings.json.bak"), URL(fileURLWithPath: "/data/global.json.bak")]
    }
}

@MainActor
final class FakePaymentAuthentication: PaymentAuthenticationSetting {
    var requireAuthenticationForEveryPayment = true

    func setRequireAuthenticationForEveryPayment(_ value: Bool) throws(ServiceError) {
        requireAuthenticationForEveryPayment = value
    }
}

// MARK: OS services (§2.7)

final class FakeLaunchAtLogin: LaunchAtLoginManaging, @unchecked Sendable {
    let supported: Bool
    let enabled = Locked(false)
    let calls = Locked<[(Bool, [String])]>([])

    init(supported: Bool) {
        self.supported = supported
    }

    var isSupported: Bool { supported }

    func isEnabled() throws(PlatformServiceError) -> Bool {
        guard supported else { throw .unsupported("autostart") }
        return enabled.current
    }

    func setEnabled(_ enabled: Bool, arguments: [String]) throws(PlatformServiceError) {
        guard supported else { throw .unsupported("autostart") }
        calls.withLock { $0.append((enabled, arguments)) }
        self.enabled.withLock { $0 = enabled }
    }
}

final class FakeNotifier: SystemNotifying, @unchecked Sendable {
    let state = Locked(NotificationAuthorization.notDetermined)
    let answer = Locked(NotificationAuthorization.authorized)
    let requests = Locked(0)

    func authorization() async -> NotificationAuthorization { state.current }

    func requestAuthorization() async -> NotificationAuthorization {
        requests.withLock { $0 += 1 }
        state.withLock { $0 = answer.current }
        return state.current
    }

    func post(_ notification: SystemNotification) async throws(PlatformServiceError) {}

    func activations() -> AsyncStream<String?> { AsyncStream { $0.finish() } }
}

final class FakeClipboard: ClipboardProviding, @unchecked Sendable {
    let text = Locked<String?>(nil)

    func string() -> String? { text.current }
    func setString(_ string: String) { text.withLock { $0 = string } }
    func imageData() -> Data? { nil }
}

final class FakeFileRevealer: FileRevealing, @unchecked Sendable {
    let revealed = Locked<[URL]>([])

    func reveal(_ url: URL) throws(PlatformServiceError) {
        revealed.withLock { $0.append(url) }
    }
}

final class FakeDataDirectories: DataDirectoryInspecting, @unchecked Sendable {
    let states = Locked<[URL: DataDirectoryState]>([:])
    let created = Locked<[URL]>([])
    let available = Locked<Int64?>(123_000_000_000)

    func inspect(_ url: URL) async -> DataDirectoryStatus {
        DataDirectoryStatus(state: states.current[url] ?? .willCreate, availableBytes: available.current)
    }

    func create(_ url: URL) throws(PlatformServiceError) {
        guard states.current[url] != .cannotCreate else { throw PlatformServiceError(code: "desktop.os_error") }
        created.withLock { $0.append(url) }
        states.withLock { $0[url] = .exists }
    }
}

// MARK: World

/// Every M2 fake, next to the M1 `FakeWorld`.
@MainActor
final class FakeM2World {
    let walletLifecycle = FakeWalletLifecycleManager()
    let actions = FakeTransactionActions()
    let fees = FakeFees()
    let importer = FakeFileImporter()
    let exporter = FakeCoreExporter()
    let backups = FakeBackups()
    let psbt = FakePSBT()
    let nodeInformation = FakeNodeInformation()
    let peers = FakePeerModeration()
    let repair = FakeRepair()
    let console = FakeConsole()
    let logs = FakeLogs()
    let quickUnlock: FakeQuickUnlock
    let autoLock = FakeAutoLock()
    let recovery = FakeVaultRecovery()
    let startup = FakeStartup()
    let shutdown = FakeShutdown()
    let shellSettings = FakeShellSettings()
    let desktopPreferences = FakeDesktopPreferences()
    let dustProtection = FakeDustProtection()
    let optionsReset = FakeOptionsReset()
    let paymentAuthentication = FakePaymentAuthentication()
    let launchAtLogin: FakeLaunchAtLogin
    let notifier = FakeNotifier()
    let clipboard = FakeClipboard()
    let fileRevealer = FakeFileRevealer()
    let dataDirectories = FakeDataDirectories()
    var launchOptions = LaunchOptions()
    let platform: DesktopPlatform

    init(platform: DesktopPlatform = .linux, quickUnlockProvider: QuickUnlockProvider = .touchID) {
        self.platform = platform
        launchAtLogin = FakeLaunchAtLogin(supported: platform != .macOS)
        quickUnlock = FakeQuickUnlock(provider: quickUnlockProvider)
    }

    var services: M2Services {
        M2Services(
            walletLifecycle: walletLifecycle, transactionActions: actions, fees: fees, fileImporter: importer,
            coreExporter: exporter, backups: backups, psbt: psbt, nodeInformation: nodeInformation,
            peerModeration: peers, repair: repair, console: console, logs: logs, quickUnlock: quickUnlock,
            autoLock: autoLock, vaultRecovery: recovery, startup: startup, shutdown: shutdown,
            shellSettings: shellSettings, launchArguments: FakeLaunchArguments(), launchOptions: launchOptions,
            desktopPreferences: desktopPreferences, dustProtection: dustProtection, optionsReset: optionsReset,
            paymentAuthentication: paymentAuthentication, launchAtLogin: launchAtLogin, notifications: notifier,
            clipboard: clipboard, fileRevealer: fileRevealer, dataDirectories: dataDirectories, platform: platform)
    }
}

func utxo(
    _ n: Int, amount: Int64, address: String = testnetAddress1, label: String? = nil, confirmations: UInt32 = 10,
    locked: Bool = false, change: Bool = false, denominated: Bool = false, rounds: UInt32? = nil, date: Date? = nil
) -> Utxo {
    Utxo(
        outpoint: OutPoint(txid: txid(n), vout: 0), address: address, amount: Amount(duffs: amount),
        confirmations: confirmations, date: date, instantLocked: false, chainLocked: true, userLocked: locked,
        reserved: false, label: label, isChange: change, coinJoinDenominated: denominated, coinJoinRounds: rounds,
        spendable: !locked)
}
