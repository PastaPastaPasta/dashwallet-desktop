// The M2 service protocols (docs/contracts/m2-swift.md §2) over the demo
// world, with the engine's rules and error codes (m2-engine.md §2–4). Demo
// mode stores nothing and reads no user files: calls that write or parse
// files (backups, imports, exports for dash-qt, PSBTs, log export) answer
// `not_implemented`, as the engine stubs do, after the same argument checks.
import Foundation
import PlatformServices
import WalletFeatures
import WalletRuntime

/// M2 state the demo services share: unloaded wallets, load-on-startup,
/// bans, birth heights, dust protection, backups policy and shell settings.
@MainActor
final class DemoM2World {
    let world: DemoWorld
    let startedAt: Date
    var unloaded: [DashNetwork: [WalletInfo]] = [:]
    var notOnStartup: [DashNetwork: Set<WalletID>] = [:]
    var banned: [BannedPeer] = []
    var birthHeights: [WalletID: UInt32] = [:]
    var dustThreshold: Amount?
    var backupKeep = 10
    var shell = ShellSettings()
    var desktop = DesktopPreferences()
    var requireAuthenticationForEveryPayment = true
    var autoLockInterval: AutoLockInterval = .never
    nonisolated let loadChanges = DemoBroadcaster<Void>()
    nonisolated let shellChanges = DemoBroadcaster<ShellSettings>()

    init(world: DemoWorld) {
        self.world = world
        startedAt = world.now()
    }

    var network: DashNetwork { world.network }
    var dataDirectory: URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("dashwallet-demo/\(network.description)")
    }

    func loadStates() -> [WalletLoadState] {
        let startup = notOnStartup[network] ?? []
        let loaded = world.current.wallets.map {
            WalletLoadState(
                walletID: $0.id, name: $0.name, loaded: true, loadOnStartup: !startup.contains($0.id),
                watchOnly: $0.watchOnly)
        }
        let closed = (unloaded[network] ?? []).map {
            WalletLoadState(
                walletID: $0.id, name: $0.name, loaded: false, loadOnStartup: !startup.contains($0.id),
                watchOnly: $0.watchOnly)
        }
        return loaded + closed
    }

    /// Open Wallet: idempotent; `wallet_not_found` for an unknown id.
    func load(_ id: WalletID) throws(ServiceError) {
        if world.current.wallets.contains(where: { $0.id == id }) { return }
        guard let info = unloaded[network]?.first(where: { $0.id == id }) else { throw .demo(.walletNotFound) }
        unloaded[network]?.removeAll { $0.id == id }
        world.current.wallets.append(info)
        if world.current.selected == nil { world.current.selected = id }
        world.walletChanges.send(())
        loadChanges.send(())
    }

    /// Close Wallet: keeps the data; idempotent.
    func unload(_ id: WalletID) throws(ServiceError) {
        if unloaded[network]?.contains(where: { $0.id == id }) == true { return }
        guard let info = world.current.wallets.first(where: { $0.id == id }) else { throw .demo(.walletNotFound) }
        var state = world.current
        state.wallets.removeAll { $0.id == id }
        if state.selected == id { state.selected = state.wallets.first?.id }
        world.current = state
        unloaded[network, default: []].append(info)
        world.walletChanges.send(())
        loadChanges.send(())
    }

    func setLoadOnStartup(_ id: WalletID, _ value: Bool) throws(ServiceError) {
        guard loadStates().contains(where: { $0.walletID == id }) else { throw .demo(.walletNotFound) }
        if value { notOnStartup[network, default: []].remove(id) } else { notOnStartup[network, default: []].insert(id) }
        loadChanges.send(())
    }

    var registeredWalletCount: Int { world.current.wallets.count + (unloaded[network]?.count ?? 0) }
}

// MARK: Wallet lifecycle (§2.1)

final class DemoWalletLifecycle: WalletLifecycleManaging {
    let m2: DemoM2World

    init(m2: DemoM2World) {
        self.m2 = m2
    }

    @MainActor
    private func networks() -> [NetworkDataInfo] {
        m2.world.states.keys.sorted { $0.description < $1.description }.compactMap { network in
            guard let state = m2.world.states[network] else { return nil }
            let hasWallets = !state.wallets.isEmpty || !(m2.unloaded[network] ?? []).isEmpty
            guard hasWallets || state.vault.state != .noVault else { return nil }
            return NetworkDataInfo(
                network: network,
                directory: FileManager.default.temporaryDirectory.appendingPathComponent(
                    "dashwallet-demo/\(network.description)"),
                hasWalletState: hasWallets, hasVault: state.vault.state != .noVault,
                hasOSStoreKey: state.vault.state == .unencrypted)
        }
    }

    func existingNetworks() async throws(ServiceError) -> [NetworkDataInfo] { await networks() }
    func loadStates() async throws(ServiceError) -> [WalletLoadState] { await m2.loadStates() }
    func loadStateChanges() -> AsyncStream<Void> { m2.loadChanges.stream() }
    func load(_ wallet: WalletID) async throws(ServiceError) { try await m2.load(wallet) }
    func unload(_ wallet: WalletID) async throws(ServiceError) { try await m2.unload(wallet) }

    func setLoadOnStartup(_ wallet: WalletID, _ loadOnStartup: Bool) async throws(ServiceError) {
        try await m2.setLoadOnStartup(wallet, loadOnStartup)
    }

    /// The demo has no xpub codec: the engine's interim answer (U7) as well.
    func importWatchOnly(xpub: String, options: WatchOnlyImportOptions) async throws(ServiceError) -> WalletID {
        if let lookahead = options.lookahead, !(1...1000).contains(lookahead) {
            throw .demo(.invalidArgument, "lookahead")
        }
        throw .demo(.notImplemented, "import_watch_only")
    }

    func accountXpub(wallet: WalletID, account: UInt32) async throws(ServiceError) -> AccountXpub {
        _ = try await m2.world.ledger(wallet)
        throw .demo(.notImplemented, "account_xpub")
    }
}

// MARK: Transactions (§2.2)

final class DemoTransactionActions: TransactionActing {
    let m2: DemoM2World

    init(m2: DemoM2World) {
        self.m2 = m2
    }

    func extras(wallet: WalletID, txid: String) async throws(ServiceError) -> TransactionDetailExtras {
        try DemoHistory.checkTxid(txid)
        guard let extras = try await m2.world.ledger(wallet).extras(txid: txid) else {
            throw .demo(.historyTxNotFound)
        }
        return extras
    }

    func abandon(wallet: WalletID, txid: String) async throws(ServiceError) {
        try DemoHistory.checkTxid(txid)
        try await abandon(wallet, txid)
    }

    @MainActor
    private func abandon(_ wallet: WalletID, _ txid: String) throws(ServiceError) {
        let ledger = try m2.world.ledger(wallet)
        // As the engine: the spent coins return only through a rescan.
        guard m2.world.sync.running else { throw .demo(.txActionSpvNotRunning) }
        guard ledger.transaction(txid) != nil else { throw .demo(.txActionTxNotFound) }
        if let refusal = ledger.abandonRefusal(txid) {
            throw .demo(.txActionRefused, parameters: ["refusal": refusal.rawValue])
        }
        try m2.world.update(wallet) { (ledger: inout DemoLedger) throws(ServiceError) in ledger.abandon(txid) }
        m2.world.notifyHistory(wallet, txids: [txid])
    }

    func resend(wallet: WalletID, txid: String) async throws(ServiceError) {
        try DemoHistory.checkTxid(txid)
        try await resend(wallet, txid)
    }

    @MainActor
    private func resend(_ wallet: WalletID, _ txid: String) throws(ServiceError) {
        let ledger = try m2.world.ledger(wallet)
        guard ledger.transaction(txid) != nil else { throw .demo(.txActionTxNotFound) }
        if let refusal = ledger.resendRefusal(txid) {
            throw .demo(.txActionRefused, parameters: ["refusal": refusal.rawValue])
        }
        guard m2.world.sync.running else { throw .demo(.txActionSpvNotRunning) }
        guard m2.world.sync.connectedPeers > 0 else { throw .demo(.txActionNoPeers) }
    }

    func dropUnconfirmed(wallet: WalletID?) async throws(ServiceError) -> Int {
        try await drop(wallet)
    }

    @MainActor
    private func drop(_ wallet: WalletID?) throws(ServiceError) -> Int {
        guard m2.world.sync.running else { throw .demo(.txActionSpvNotRunning) }
        var ids = m2.world.current.wallets.map(\.id)
        if let wallet {
            _ = try m2.world.ledger(wallet)
            ids = [wallet]
        }
        var count = 0
        for id in ids {
            let ledger = try m2.world.ledger(id)
            let eligible = ledger.records.map(\.id.txid).filter { ledger.abandonRefusal($0) == nil }
            guard !eligible.isEmpty else { continue }
            try m2.world.update(id) { (ledger: inout DemoLedger) throws(ServiceError) in
                for txid in eligible { ledger.abandon(txid) }
            }
            m2.world.notifyHistory(id, txids: eligible)
            count += eligible.count
        }
        return count
    }

    /// dash-qt's CSV of the filtered history (QT-093).
    func exportCSV(wallet: WalletID, filter: HistoryFilter, sort: HistorySort, options: HistoryCSVOptions)
        async throws(ServiceError) -> Data
    {
        guard options.typeNames.isEmpty || options.typeNames.count == TxType.allCases.count else {
            throw .demo(.historyInvalidQuery, "type names")
        }
        let ledger = try await m2.world.ledger(wallet)
        var records: [TxRecord] = []
        var cursor: String?
        repeat {
            let page = try ledger.page(HistoryQuery(filter: filter, sort: sort, cursor: cursor, limit: 500))
            records += page.records
            cursor = page.nextCursor
        } while cursor != nil
        let network = await m2.network
        let csv = TransactionCSV.make(
            records: records, unit: options.unit, amounts: EngineFunctions.amountFormatter(network: { network }),
            watchOnlyColumn: false, timeZone: options.timeZone)
        return Data(csv.utf8)
    }
}

/// `fee_policy` and `coin_selection_summary` at the minimum relay fee.
final class DemoFees: FeeAndCoinSelectionProviding {
    static let minimumRelay: Int64 = 1000
    let world: DemoWorld

    init(world: DemoWorld) {
        self.world = world
    }

    func feePolicy() async throws(ServiceError) -> FeePolicy {
        FeePolicy(
            source: .minimumRelay, minimumRelayPerKB: 1000, maximumCustomPerKB: 10_000_000,
            maximumTransactionFee: Amount(duffs: 10_000_000), maximumBroadcastRatePerKB: 10_000_000,
            targets: [2, 4, 6, 12, 24, 48, 144, 504, 1008].map {
                FeeTarget(targetBlocks: $0, duffsPerKB: UInt64(Self.minimumRelay))
            })
    }

    /// dash-qt's panel formula: bytes = 148·in + 34·(out+1) + 10, −34 without
    /// change; dust change goes to the fee; spent coins are `unavailable`.
    func summary(
        wallet: WalletID, outpoints: [OutPoint], payAmounts: [Amount], fee: FeeChoice, allChangeToFee: Bool
    ) async throws(ServiceError) -> CoinSelectionSummary {
        for outpoint in outpoints { try DemoHistory.checkTxid(outpoint.txid) }
        let ledger = try await world.ledger(wallet)
        let rate: Int64
        switch fee {
        case .recommended: rate = Self.minimumRelay
        case .perKilobyte(let amount): rate = max(Self.minimumRelay, amount.duffs)
        }
        let coins = Dictionary(ledger.coins.map { ($0.outpoint, $0.amount) }, uniquingKeysWith: { a, _ in a })
        let chosen = outpoints.filter { coins[$0] != nil }
        let unavailable = outpoints.filter { coins[$0] == nil }
        let amount = chosen.reduce(Int64(0)) { $0 + (coins[$1] ?? 0) }
        let pay = payAmounts.reduce(Int64(0)) { $0 + $1.duffs }
        var bytes = 148 * chosen.count + 34 * (payAmounts.count + 1) + 10
        var feeDuffs = Int64(bytes) * rate / 1000
        var change = amount - pay - feeDuffs
        var changeToFee = false
        if allChangeToFee || (change > 0 && change < 546) {
            bytes -= 34
            feeDuffs = Int64(bytes) * rate / 1000 + max(0, amount - pay - Int64(bytes) * rate / 1000)
            changeToFee = change > 0
            change = 0
        } else if change <= 0 {
            bytes -= 34
            feeDuffs = Int64(bytes) * rate / 1000
            change = 0
        }
        return CoinSelectionSummary(
            quantity: chosen.count, amount: Amount(duffs: amount), bytes: bytes, fee: Amount(duffs: feeDuffs),
            afterFee: Amount(duffs: max(0, amount - feeDuffs)), change: Amount(duffs: change), changeToFee: changeToFee,
            insufficientFunds: amount < pay + feeDuffs, feeTolerancePerInput: Amount(duffs: max(1, rate / 1000)),
            unavailable: unavailable)
    }
}

// MARK: Compatibility, backups, PSBT (§2.3) — demo stores and parses no files

final class DemoFileImporter: WalletFileImporting {
    func inspect(_ file: URL) async throws(ServiceError) -> WalletFileKind {
        throw .demo(.notImplemented, "inspect_wallet_file (demo mode reads no files)")
    }

    func importDumpWallet(_ file: URL, options: WalletImportOptions) async throws(ServiceError) -> WalletImportReport {
        throw .demo(.notImplemented, "import_dump_wallet")
    }

    func importWalletDat(_ file: URL, passphrase: (any SecretBuffer)?, options: WalletImportOptions)
        async throws(ServiceError) -> WalletImportReport
    {
        throw .demo(.notImplemented, "import_wallet_dat")
    }

    /// The seed length is checked first, as the engine does.
    func importKeyMaterial(_ material: KeyMaterial, options: WalletImportOptions) async throws(ServiceError)
        -> WalletImportReport
    {
        if case .hdSeed(let seed) = material, !(16...64).contains(seed.count) {
            throw .demo(.compatInvalidKeyMaterial)
        }
        throw .demo(.notImplemented, "import_key_material")
    }
}

final class DemoCoreExporter: CoreExporting {
    let world: DemoWorld

    init(world: DemoWorld) {
        self.world = world
    }

    func export(wallet: WalletID, format: CoreExportFormat, to file: URL, grant: AuthGrant)
        async throws(ServiceError) -> CoreExportReport
    {
        _ = try await world.ledger(wallet)
        throw .demo(.notImplemented, "export_for_core (demo mode writes no files)")
    }

    func mnemonicCompatibility(wallet: WalletID) async throws(ServiceError) -> (
        coreCompatible: Bool, warnings: [CoreExportWarning]
    ) {
        _ = try await world.ledger(wallet)
        throw .demo(.notImplemented, "core_mnemonic_compatibility")
    }
}

final class DemoBackups: BackupProviding {
    let m2: DemoM2World

    init(m2: DemoM2World) {
        self.m2 = m2
    }

    func backup(wallet: WalletID, to file: URL, passphrase: (any SecretBuffer)?) async throws(ServiceError)
        -> WalletBackup
    {
        _ = try await m2.world.ledger(wallet)
        throw .demo(.notImplemented, "backup_wallet (demo mode writes no files)")
    }

    func restore(from file: URL, passphrase: (any SecretBuffer)?) async throws(ServiceError) -> [WalletID] {
        throw .demo(.notImplemented, "restore_backup")
    }

    /// Demo mode writes none.
    func automaticBackups(wallet: WalletID?) async throws(ServiceError) -> [WalletBackup] { [] }

    func policy() async throws(ServiceError) -> BackupPolicy {
        await BackupPolicy(keep: m2.backupKeep, directory: m2.dataDirectory.appendingPathComponent("backups"))
    }

    func setKeep(_ keep: Int) async throws(ServiceError) -> BackupPolicy {
        guard (0...10).contains(keep) else { throw .demo(.invalidArgument, "keep") }
        await setKeep(value: keep)
        return try await policy()
    }

    @MainActor
    private func setKeep(value: Int) {
        m2.backupKeep = value
    }
}

final class DemoPSBT: PSBTHandling {
    static let maxSize = 100 * 1024 * 1024

    func createUnsigned(from draft: any TransactionDrafting) async throws(ServiceError) -> PSBTReference {
        guard draft is DemoDraft else { throw .demo(.invalidArgument, "draft of another runtime") }
        throw .demo(.notImplemented, "TxDraft.create_unsigned")
    }

    /// `parse_psbt` already refuses input over 100 MiB.
    func load(_ data: Data) throws(ServiceError) -> PSBTReference {
        guard data.count <= Self.maxSize else { throw .demo(.psbtTooLarge, parameters: ["size_bytes": Int64(data.count)]) }
        throw .demo(.notImplemented, "parse_psbt")
    }

    func base64(_ psbt: PSBTReference) throws(ServiceError) -> String { throw .demo(.invalidArgument, "unknown psbt") }
    func bytes(_ psbt: PSBTReference) throws(ServiceError) -> Data { throw .demo(.invalidArgument, "unknown psbt") }

    func analyze(_ psbt: PSBTReference, wallet: WalletID?) async throws(ServiceError) -> PSBTAnalysis {
        throw .demo(.invalidArgument, "unknown psbt")
    }

    func sign(_ psbt: PSBTReference, wallet: WalletID, grant: AuthGrant) async throws(ServiceError) -> PSBTReference {
        throw .demo(.invalidArgument, "unknown psbt")
    }

    func broadcast(_ psbt: PSBTReference) async throws(ServiceError) -> String {
        throw .demo(.invalidArgument, "unknown psbt")
    }

    func release(_ psbt: PSBTReference) {}
}

// MARK: Tools (§2.4)

final class DemoNodeInformation: NodeInformationProviding {
    let m2: DemoM2World

    init(m2: DemoM2World) {
        self.m2 = m2
    }

    /// From the demo's sync status; masternode counts and mempool figures
    /// stay unknown (`nil`), as on SPV before the masternode phase.
    @MainActor
    private func read() -> NodeInformation {
        let sync = m2.world.sync
        var rng = DemoRandom(text: "tip|\(sync.tipHeight ?? 0)")
        let tipHash = sync.tipHeight.map { _ in rng.hex(bytes: 32) }
        return NodeInformation(
            clientVersion: "demo", userAgent: "/DashWalletDesktop:demo/", dataDirectory: m2.dataDirectory,
            startupDate: m2.startedAt, network: m2.network, connectionsIn: 0, connectionsOut: Int(sync.connectedPeers),
            localAddresses: [], tipHeight: sync.tipHeight, tipDate: sync.tipDate, tipHash: tipHash,
            bestChainLock: sync.chainLockHeight.map {
                ChainLockInfo(height: $0, blockHash: tipHash ?? "", blockDate: sync.tipDate)
            },
            masternodes: nil, evonodes: nil, mempoolTransactionCount: nil, mempoolUsageBytes: nil)
    }

    func information() async throws(ServiceError) -> NodeInformation { await read() }

    func warnings() async throws(ServiceError) -> [NodeWarning] {
        await m2.world.sync.isStalled ? [.syncStalled] : []
    }
}

final class DemoPeerModeration: PeerModerating {
    let m2: DemoM2World
    let sync: DemoSync

    init(m2: DemoM2World, sync: DemoSync) {
        self.m2 = m2
        self.sync = sync
    }

    @MainActor
    private func requireConnected(_ address: String) async throws(ServiceError) {
        guard m2.world.sync.running else { throw .demo(.syncSpvNotRunning) }
        let peers = try await sync.peers()
        guard peers.contains(where: { $0.address == address }) else { throw .demo(.syncPeerNotFound) }
    }

    func disconnect(address: String) async throws(ServiceError) {
        try await requireConnected(address)
    }

    func ban(address: String, for duration: Duration) async throws(ServiceError) {
        guard duration > .zero else { throw .demo(.invalidArgument, "duration") }
        try await requireConnected(address)
        await addBan(address, duration)
    }

    @MainActor
    private func addBan(_ address: String, _ duration: Duration) {
        let host = address.split(separator: ":").first.map(String.init) ?? address
        let subnet = host + "/32"
        m2.banned.removeAll { $0.subnet == subnet }
        m2.banned.append(BannedPeer(
            subnet: subnet, bannedUntil: m2.world.now().addingTimeInterval(TimeInterval(duration.components.seconds))))
    }

    func unban(subnet: String) async throws(ServiceError) {
        try await removeBan(subnet)
    }

    @MainActor
    private func removeBan(_ subnet: String) throws(ServiceError) {
        guard m2.banned.contains(where: { $0.subnet == subnet }) else { throw .demo(.syncPeerNotFound) }
        m2.banned.removeAll { $0.subnet == subnet }
    }

    func bannedPeers() async throws(ServiceError) -> [BannedPeer] { await m2.banned }
}

/// Demo rescans finish at once, so no progress is ever reported.
final class DemoRepair: RepairProviding {
    let m2: DemoM2World

    init(m2: DemoM2World) {
        self.m2 = m2
    }

    func rescanProgress() async throws(ServiceError) -> RescanProgress? { nil }
    func cancelRescan() async throws(ServiceError) -> Bool { false }

    /// The adapter's stop → reset → start; the demo keeps no chain data.
    func resetChainData() async throws(ServiceError) {}

    func setBirthHeight(wallet: WalletID, height: UInt32) async throws(ServiceError) {
        try await setBirth(wallet, height)
    }

    @MainActor
    private func setBirth(_ wallet: WalletID, _ height: UInt32) throws(ServiceError) {
        _ = try m2.world.ledger(wallet)
        guard height <= DemoLedger.tipHeight else {
            throw .demo(.syncHeightOutOfRange, parameters: ["height": Int64(height)])
        }
        m2.birthHeights[wallet] = height
    }
}

/// A few read-only Core commands over the demo world; the rest answer like
/// the engine's console does for commands it lacks.
final class DemoConsole: ConsoleExecuting {
    static let sensitive: Set<String> = [
        "importprivkey", "importmulti", "sethdseed", "signmessagewithprivkey", "signrawtransactionwithkey",
        "upgradetohd", "walletpassphrase", "walletpassphrasechange", "encryptwallet",
    ]
    static let supported = ["getbalance", "getbestblockhash", "getblockcount", "getconnectioncount", "help"]
    static let unavailable = ["getmempoolinfo", "getblock", "getrawmempool", "sendtoaddress", "dumpprivkey"]

    let m2: DemoM2World

    init(m2: DemoM2World) {
        self.m2 = m2
    }

    func commands() async throws(ServiceError) -> [ConsoleCommandInfo] {
        let names = (Self.supported + Self.unavailable + Array(Self.sensitive) + ["help-console"]).sorted()
        return names.map {
            ConsoleCommandInfo(
                name: $0, category: "demo", sensitive: Self.sensitive.contains($0),
                available: Self.supported.contains($0) || $0 == "help-console")
        }
    }

    func redact(_ line: any SecretBuffer) throws(ServiceError) -> String {
        let text = DemoSecret.text(line)
        let (command, _) = try Self.parse(text)
        return Self.sensitive.contains(command) ? "\(command)(…)" : text
    }

    func execute(_ line: any SecretBuffer, wallet: WalletID?, grant: AuthGrant?) async throws(ServiceError)
        -> ConsoleResult
    {
        let (command, _) = try Self.parse(DemoSecret.text(line))
        return try await run(command, wallet: wallet)
    }

    @MainActor
    private func run(_ command: String, wallet: WalletID?) throws(ServiceError) -> ConsoleResult {
        let sync = m2.world.sync
        switch command {
        case "help", "help-console":
            return .output(text: Self.supported.joined(separator: "\n"), isJSON: false)
        case "getblockcount":
            return .output(text: String(sync.tipHeight ?? 0), isJSON: false)
        case "getconnectioncount":
            return .output(text: String(sync.connectedPeers), isJSON: false)
        case "getbestblockhash":
            var rng = DemoRandom(text: "tip|\(sync.tipHeight ?? 0)")
            return .output(text: rng.hex(bytes: 32), isJSON: false)
        case "getbalance":
            guard let wallet else { throw .demo(.consoleWalletRequired) }
            let balance = try m2.world.ledger(wallet).balances.confirmed
            let formatter = EngineFunctions.amountFormatter(network: { [network = m2.network] in network })
            return .output(
                text: formatter.format(balance, unit: .dash, style: .plain(plusSign: false, separators: .never)),
                isJSON: false)
        case _ where Self.unavailable.contains(command) || Self.sensitive.contains(command):
            throw .demo(.consoleNotAvailable, command)
        default:
            throw .demo(.consoleRPCError, "Method not found (code -32601)", parameters: ["code": -32601])
        }
    }

    /// The command name; unbalanced quotes are `console.parse_error`.
    static func parse(_ line: String) throws(ServiceError) -> (String, [String]) {
        var quote: Character?
        for character in line {
            if let open = quote {
                if character == open { quote = nil }
            } else if character == "\"" || character == "'" {
                quote = character
            }
        }
        guard quote == nil else { throw .demo(.consoleParseError) }
        let parts = line.split(whereSeparator: { $0 == " " || $0 == "," || $0 == "(" }).map(String.init)
        guard let command = parts.first else { throw .demo(.consoleParseError) }
        return (command, Array(parts.dropFirst()))
    }
}

final class DemoLogs: LogExporting {
    func exportLogs(to file: URL) async throws(ServiceError) -> URL {
        throw .demo(.notImplemented, "export_logs (demo mode writes no files)")
    }
}

// MARK: Security (§2.5)

/// The demo vault has no slot B, like an engine vault before enrolment and
/// every Linux host: quick unlock is unavailable.
final class DemoQuickUnlock: QuickUnlockManaging {
    var provider: QuickUnlockProvider { .unavailable }

    func policy() async throws(ServiceError) -> QuickUnlockPolicy {
        QuickUnlockPolicy(
            enrolled: false, spendLimit: QuickUnlockPolicy.defaultSpendLimit, passphraseMaxAge: .seconds(604_800),
            lastPassphraseEntry: nil)
    }

    func enroll(grant: AuthGrant) async throws(ServiceError) -> QuickUnlockPolicy {
        throw .demo(.vaultQuickUnlockUnavailable)
    }

    func remove() async throws(ServiceError) -> QuickUnlockPolicy { try await policy() }

    func setSpendLimit(_ limit: Amount, grant: AuthGrant) async throws(ServiceError) -> QuickUnlockPolicy {
        guard QuickUnlockPolicy.spendLimitOptions.contains(limit) else { throw .demo(.invalidArgument, "limit") }
        throw .demo(.vaultQuickUnlockUnavailable)
    }

    func credential(reason: String) async throws(ServiceError) -> Credential {
        throw .demo(.vaultQuickUnlockUnavailable)
    }
}

/// Locks the demo vault after the chosen idle time; `immediately` locks on
/// sleep and screen lock only, which demo mode does not watch.
@MainActor
final class DemoAutoLock: AutoLockControlling {
    let m2: DemoM2World
    private var timer: Task<Void, Never>?

    init(m2: DemoM2World) {
        self.m2 = m2
    }

    var interval: AutoLockInterval { m2.autoLockInterval }

    func setInterval(_ interval: AutoLockInterval) throws(ServiceError) {
        m2.autoLockInterval = interval
        noteActivity()
    }

    func noteActivity() {
        timer?.cancel()
        guard let duration = interval.duration, duration > .zero else { return }
        let world = m2.world
        timer = Task { @MainActor in
            do { try await Task.sleep(for: duration) } catch { return }
            world.lock()
        }
    }
}

final class DemoVaultRecovery: VaultRecovering {
    let m2: DemoM2World

    init(m2: DemoM2World) {
        self.m2 = m2
    }

    func recover(
        wallet: WalletID, mnemonic: any SecretBuffer, bip39Passphrase: any SecretBuffer,
        newPassphrase: any SecretBuffer
    ) async throws(ServiceError) -> VaultRecoveryResult {
        let phrase = DemoSecret.text(mnemonic)
        let passphrase = DemoSecret.text(newPassphrase)
        return try await recover(wallet, phrase, passphrase)
    }

    @MainActor
    private func recover(_ wallet: WalletID, _ phrase: String, _ passphrase: String) throws(ServiceError)
        -> VaultRecoveryResult
    {
        try m2.world.recoverVault(wallet: wallet, phrase: phrase, newPassphrase: passphrase)
    }

    func destroy(credential: Credential) async throws(ServiceError) -> VaultStatus {
        let passphrase: String?
        switch credential {
        case .passphrase(let secret): passphrase = DemoSecret.text(secret)
        case .unencrypted: passphrase = nil
        case .quickUnlock: throw .demo(.vaultCredentialRequired)
        }
        return try await destroy(passphrase)
    }

    @MainActor
    private func destroy(_ passphrase: String?) throws(ServiceError) -> VaultStatus {
        try m2.world.destroyVault(passphrase: passphrase, remainingWallets: m2.registeredWalletCount)
    }
}

// MARK: Shell (§2.6)

/// Demo mode starts at once.
@MainActor
final class DemoStartup: StartupProgressing {
    private(set) var phase: StartupPhase = .ready
    private(set) var progress: Double = 1
    private(set) var quitRequested = false

    func changes() -> AsyncStream<StartupPhase> {
        let phase = phase
        return AsyncStream { continuation in
            continuation.yield(phase)
            continuation.finish()
        }
    }

    func requestEmergencyQuit() {
        quitRequested = true
    }
}

@MainActor
final class DemoShutdown: ShutdownCoordinating {
    private(set) var isShuttingDown = false

    /// Nothing to stop: demo mode has no engine session.
    func shutdown() async {
        isShuttingDown = true
        isShuttingDown = false
    }
}

@MainActor
final class DemoShellSettings: ShellSettingsProviding {
    let m2: DemoM2World

    init(m2: DemoM2World) {
        self.m2 = m2
    }

    var shell: ShellSettings { m2.shell }

    func update(_ shell: ShellSettings) throws(ServiceError) {
        m2.shell = shell
        m2.shellChanges.send(shell)
    }

    func changes() -> AsyncStream<ShellSettings> { m2.shellChanges.stream() }
}

/// dash-qt's GUI flags as the desktop wallet keeps them (QT-006).
struct DemoLaunchArguments: LaunchArgumentsParsing {
    var optionNames: [String] {
        [
            "--choosedatadir", "--datadir=<dir>", "--lang=<lang>", "--min", "--resetguisettings", "--splash",
            "--testnet", "--regtest", "--devnet=<name>", "--chain=<chain>", "--windowtitle=<name>",
        ]
    }

    func parse(_ arguments: [String]) throws(ServiceError) -> LaunchOptions {
        var options = LaunchOptions()
        for argument in arguments {
            if argument.lowercased().hasPrefix("dash:") {
                options.uris.append(argument)
                continue
            }
            guard options.uris.isEmpty else { throw .demo(.launchOptionAfterURI, argument) }
            guard argument.hasPrefix("-") else { throw .demo(.launchUnknownOption, argument) }
            let body = argument.drop { $0 == "-" }
            let parts = body.split(separator: "=", maxSplits: 1).map(String.init)
            let name = parts[0]
            let value = parts.count > 1 ? parts[1] : nil
            func flag() throws(ServiceError) -> Bool {
                switch value {
                case nil, "1": true
                case "0": false
                default: throw .demo(.launchInvalidValue, argument)
                }
            }
            func text() throws(ServiceError) -> String {
                guard let value, !value.isEmpty else { throw .demo(.launchInvalidValue, argument) }
                return value
            }
            switch name {
            case "min": options.startMinimized = try flag()
            case "splash": options.showSplash = try flag()
            case "resetguisettings": options.resetGUISettings = try flag()
            case "choosedatadir": options.chooseDataDirectory = try flag()
            case "datadir": options.dataDirectory = URL(fileURLWithPath: try text())
            case "testnet": if try flag() { options.network = .testnet }
            case "regtest": if try flag() { options.network = .regtest }
            case "devnet": options.network = .devnet(name: try text())
            case "chain":
                switch try text() {
                case "main": options.network = .mainnet
                case "test": options.network = .testnet
                case "regtest": options.network = .regtest
                default: throw .demo(.launchInvalidValue, argument)
                }
            case "lang": options.language = try text()
            case "windowtitle": options.windowTitleSuffix = try text()
            default: throw .demo(.launchUnknownOption, argument)
            }
        }
        return options
    }
}

@MainActor
final class DemoDesktopPreferences: DesktopPreferencesStoring {
    let m2: DemoM2World

    init(m2: DemoM2World) {
        self.m2 = m2
    }

    var desktop: DesktopPreferences { m2.desktop }
    func update(_ desktop: DesktopPreferences) throws(ServiceError) { m2.desktop = desktop }
}

final class DemoDustProtection: DustProtectionControlling {
    let m2: DemoM2World

    init(m2: DemoM2World) {
        self.m2 = m2
    }

    func threshold() async throws(ServiceError) -> Amount? { await m2.dustThreshold }

    func setThreshold(_ threshold: Amount?) async throws(ServiceError) {
        if let threshold, !(1...1_000_000).contains(threshold.duffs) {
            throw .demo(.invalidArgument, "dust threshold")
        }
        await set(threshold)
    }

    @MainActor
    private func set(_ threshold: Amount?) {
        m2.dustThreshold = threshold
    }
}

/// Reset Options in demo mode: every setting back to its default. Nothing is
/// stored, so there are no backup files to report.
@MainActor
final class DemoOptionsReset: OptionsResetting {
    let m2: DemoM2World

    init(m2: DemoM2World) {
        self.m2 = m2
    }

    var recoveredFromCorruption: [URL] { [] }

    func resetOptions() throws(ServiceError) -> [URL] {
        m2.world.updateDisplay(DisplaySettings())
        m2.world.preferences = UIPreferences()
        m2.desktop = DesktopPreferences()
        m2.shell = ShellSettings()
        m2.requireAuthenticationForEveryPayment = true
        return []
    }
}

@MainActor
final class DemoPaymentAuthentication: PaymentAuthenticationSetting {
    let m2: DemoM2World

    init(m2: DemoM2World) {
        self.m2 = m2
    }

    var requireAuthenticationForEveryPayment: Bool { m2.requireAuthenticationForEveryPayment }

    func setRequireAuthenticationForEveryPayment(_ value: Bool) throws(ServiceError) {
        m2.requireAuthenticationForEveryPayment = value
    }
}

// MARK: OS services for tests and headless runs (the apps pass their own)

/// Demo mode never registers an autostart entry.
struct DemoLaunchAtLogin: LaunchAtLoginManaging {
    var isSupported: Bool { false }
    func isEnabled() throws(PlatformServiceError) -> Bool { throw .unsupported("autostart in demo mode") }
    func setEnabled(_ enabled: Bool, arguments: [String]) throws(PlatformServiceError) {
        throw .unsupported("autostart in demo mode")
    }
}

/// Demo mode posts no system notifications.
final class DemoNotifier: SystemNotifying {
    func authorization() async -> NotificationAuthorization { .unavailable }
    func requestAuthorization() async -> NotificationAuthorization { .unavailable }
    func post(_ notification: SystemNotification) async throws(PlatformServiceError) {
        throw .unsupported("notifications in demo mode")
    }
    func activations() -> AsyncStream<String?> { AsyncStream { $0.finish() } }
}

/// An in-process clipboard for headless runs.
final class DemoClipboard: ClipboardProviding, @unchecked Sendable {
    private let text = DemoLocked<String?>(nil)
    func string() -> String? { text.current }
    func setString(_ string: String) { text.current = string }
    func imageData() -> Data? { nil }
}

/// Demo mode creates no files to reveal.
struct DemoFileRevealer: FileRevealing {
    func reveal(_ url: URL) throws(PlatformServiceError) { throw .unsupported("demo mode stores no files") }
}

/// Inspects for real (read-only); creates nothing.
struct DemoDataDirectories: DataDirectoryInspecting {
    func inspect(_ url: URL) async -> DataDirectoryStatus {
        var isDirectory: ObjCBool = false
        let exists = FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory)
        let state: DataDirectoryState = exists ? (isDirectory.boolValue ? .exists : .notADirectory) : .willCreate
        return DataDirectoryStatus(state: state, availableBytes: nil)
    }

    func create(_ url: URL) throws(PlatformServiceError) { throw .unsupported("demo mode creates no directories") }
}

/// OS services the demo composition uses unless the app passes real ones.
public struct DemoPlatformServices {
    public var launchAtLogin: any LaunchAtLoginManaging
    public var notifications: any SystemNotifying
    public var clipboard: any ClipboardProviding
    public var fileRevealer: any FileRevealing
    public var dataDirectories: any DataDirectoryInspecting

    public init(
        launchAtLogin: (any LaunchAtLoginManaging)? = nil, notifications: (any SystemNotifying)? = nil,
        clipboard: (any ClipboardProviding)? = nil, fileRevealer: (any FileRevealing)? = nil,
        dataDirectories: (any DataDirectoryInspecting)? = nil
    ) {
        self.launchAtLogin = launchAtLogin ?? DemoLaunchAtLogin()
        self.notifications = notifications ?? DemoNotifier()
        self.clipboard = clipboard ?? DemoClipboard()
        self.fileRevealer = fileRevealer ?? DemoFileRevealer()
        self.dataDirectories = dataDirectories ?? DemoDataDirectories()
    }
}
