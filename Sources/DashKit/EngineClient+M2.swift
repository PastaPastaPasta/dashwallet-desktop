import DashWalletCore
import Foundation

// `EngineClient` conformance for the R1/R2 engine calls of M2
// (docs/contracts/m2-engine.md §2.1–2.8). Each call goes to the open
// session of `network`; errors keep the engine's codes.

extension EngineClient {
    // MARK: Wallet lifecycle

    public func existingNetworks() async throws(DashKitError) -> [NetworkDataInfo] {
        let engine = try core.get()
        return try await mapped { try await engine.existingNetworks() }.map(NetworkDataInfo.init)
    }

    public func walletLoadStates(on network: DashNetwork) throws(DashKitError) -> [WalletLoadState] {
        let session = try session(network)
        var result: [WalletLoadState] = []
        for row in try mapped({ try session.walletLoadStates() }) {
            result.append(try WalletLoadState(row))
        }
        return result
    }

    public func loadWallet(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.loadWallet(walletId: wallet.hex) }
    }

    public func unloadWallet(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.unloadWallet(walletId: wallet.hex) }
    }

    public func setLoadOnStartup(on network: DashNetwork, wallet: WalletID, loadOnStartup: Bool)
        async throws(DashKitError)
    {
        let session = try session(network)
        try await mapped { try await session.setLoadOnStartup(walletId: wallet.hex, loadOnStartup: loadOnStartup) }
    }

    public func importWatchOnly(on network: DashNetwork, xpub: String, options: WatchOnlyOptions)
        async throws(DashKitError) -> WalletID
    {
        let session = try session(network)
        let ffi = options.ffi
        return try .engine(try await mapped { try await session.importWatchOnly(xpub: xpub, options: ffi) })
    }

    public func accountXpub(on network: DashNetwork, wallet: WalletID, account: UInt32) async throws(DashKitError)
        -> AccountXpub
    {
        let session = try session(network)
        return AccountXpub(try await mapped { try await session.accountXpub(walletId: wallet.hex, account: account) })
    }

    // MARK: Transactions, fees, dust

    public func txDetailExtras(on network: DashNetwork, wallet: WalletID, txid: String) async throws(DashKitError)
        -> TxDetailExtras
    {
        let session = try session(network)
        return try TxDetailExtras(try await mapped { try await session.txDetailExtras(walletId: wallet.hex, txid: txid) })
    }

    public func abandonTransaction(on network: DashNetwork, wallet: WalletID, txid: String) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.abandonTransaction(walletId: wallet.hex, txid: txid) }
    }

    public func resendTransaction(on network: DashNetwork, wallet: WalletID, txid: String) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.resendTransaction(walletId: wallet.hex, txid: txid) }
    }

    public func dropUnconfirmed(on network: DashNetwork, wallet: WalletID?) async throws(DashKitError) -> UInt32 {
        let session = try session(network)
        return try await mapped { try await session.dropUnconfirmed(walletId: wallet?.hex) }
    }

    public func exportHistoryCSV(
        on network: DashNetwork, wallet: WalletID, filter: HistoryFilter, sort: HistorySort, unit: DisplayUnit,
        typeNames: [String], utcOffsetSeconds: Int32
    ) async throws(DashKitError) -> String {
        let session = try session(network)
        let ffiFilter = try filter.ffi()
        return try await mapped {
            try await session.exportHistoryCsv(
                walletId: wallet.hex, filter: ffiFilter, sort: sort.ffi, unit: unit.ffi, typeNames: typeNames,
                utcOffsetSecs: utcOffsetSeconds)
        }
    }

    public func feePolicy(on network: DashNetwork) throws(DashKitError) -> FeePolicy {
        let session = try session(network)
        return try FeePolicy(try mapped { try session.feePolicy() })
    }

    public func coinSelectionSummary(
        on network: DashNetwork, wallet: WalletID, outpoints: [OutPoint], payAmounts: [Amount], fee: FeeMode,
        allChangeToFee: Bool
    ) async throws(DashKitError) -> CoinSelectionSummary {
        let session = try session(network)
        var amounts: [UInt64] = []
        for amount in payAmounts {
            amounts.append(try amount.engineDuffs())
        }
        let ffiFee = try fee.ffi()
        let ffiOutpoints = outpoints.map(\.ffi)
        let summary = try await mapped {
            try await session.coinSelectionSummary(
                walletId: wallet.hex, outpoints: ffiOutpoints, payAmounts: amounts, fee: ffiFee,
                allChangeToFee: allChangeToFee)
        }
        return try CoinSelectionSummary(summary)
    }

    public func dustProtection(on network: DashNetwork) async throws(DashKitError) -> Amount? {
        let session = try session(network)
        return try (try await mapped { try await session.dustProtection() }).engineAmount()
    }

    public func setDustProtection(on network: DashNetwork, threshold: Amount?) async throws(DashKitError) {
        let session = try session(network)
        let duffs = try threshold.map { (a) throws(DashKitError) in try a.engineDuffs() }
        try await mapped { try await session.setDustProtection(threshold: duffs) }
    }

    // MARK: Tools window

    public func nodeInfo(on network: DashNetwork) throws(DashKitError) -> NodeInfo {
        let session = try session(network)
        return NodeInfo(try mapped { try session.nodeInfo() })
    }

    public func warnings(on network: DashNetwork) throws(DashKitError) -> [EngineWarning] {
        let session = try session(network)
        return try mapped { try session.warnings() }.map {
            EngineWarning(code: EngineWarningCode($0.code), detail: $0.detail)
        }
    }

    public func rescanProgress(on network: DashNetwork) throws(DashKitError) -> RescanProgress? {
        let session = try session(network)
        return try mapped { try session.rescanProgress() }.map(RescanProgress.init)
    }

    public func cancelRescan(on network: DashNetwork) async throws(DashKitError) -> Bool {
        let session = try session(network)
        return try await mapped { try await session.cancelRescan() }
    }

    public func resetChainData(on network: DashNetwork) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.resetChainData() }
    }

    public func setBirthHeight(on network: DashNetwork, wallet: WalletID, height: UInt32) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.setBirthHeight(walletId: wallet.hex, height: height) }
    }

    public func disconnectPeer(on network: DashNetwork, address: String) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.disconnectPeer(address: address) }
    }

    public func banPeer(on network: DashNetwork, address: String, durationSeconds: UInt64) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.banPeer(address: address, durationSecs: durationSeconds) }
    }

    public func unbanPeer(on network: DashNetwork, subnet: String) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.unbanPeer(subnet: subnet) }
    }

    public func bannedPeers(on network: DashNetwork) async throws(DashKitError) -> [BannedPeer] {
        let session = try session(network)
        return try await mapped { try await session.bannedPeers() }.map {
            BannedPeer(subnet: $0.subnet, bannedUntil: Date(timeIntervalSince1970: TimeInterval($0.bannedUntil)))
        }
    }

    public func consoleExecute(on network: DashNetwork, wallet: WalletID?, line: SecretBytes, grantID: String?)
        async throws(DashKitError) -> ConsoleExecution
    {
        let session = try session(network)
        do {
            let output = try await line.withTemporaryData { data in
                try await session.consoleExecute(walletId: wallet?.hex, line: data, grantId: grantID)
            }
            return .output(text: output.text, isJSON: output.isJson)
        } catch DashWalletCore.ConsoleError.AuthorizationRequired(let purpose, let walletID) {
            return .authorizationRequired(try GrantPurpose(purpose), wallet: try walletID.map { (hex) throws(DashKitError) in try .engine(hex) })
        } catch {
            throw DashKitError.from(error)
        }
    }

    // MARK: Dash Core compatibility, backups, PSBT

    public func inspectWalletFile(_ file: URL) async throws(DashKitError) -> WalletFileKind {
        let engine = try core.get()
        return WalletFileKind(try await mapped { try await engine.inspectWalletFile(path: file.path) })
    }

    public func importDumpWallet(on network: DashNetwork, file: URL, options: ImportOptions) async throws(DashKitError)
        -> ImportReport
    {
        let session = try session(network)
        let ffi = options.ffi
        return try ImportReport(try await mapped { try await session.importDumpWallet(path: file.path, options: ffi) })
    }

    public func importWalletDat(on network: DashNetwork, file: URL, walletPassphrase: SecretBytes?, options: ImportOptions)
        async throws(DashKitError) -> ImportReport
    {
        let session = try session(network)
        let ffi = options.ffi
        let report = try await mapped {
            if let walletPassphrase {
                try await walletPassphrase.withTemporaryData {
                    try await session.importWalletDat(path: file.path, walletPassphrase: $0, options: ffi)
                }
            } else {
                try await session.importWalletDat(path: file.path, walletPassphrase: nil, options: ffi)
            }
        }
        return try ImportReport(report)
    }

    public func importKeyMaterial(on network: DashNetwork, material: KeyMaterial, options: ImportOptions)
        async throws(DashKitError) -> ImportReport
    {
        let session = try session(network)
        let ffi = options.ffi
        let report = try await mapped {
            switch material {
            case .hdSeed(let secret):
                try await secret.withTemporaryData {
                    try await session.importKeyMaterial(material: .hdSeed(seed: $0), options: ffi)
                }
            case .xprv(let secret):
                try await secret.withTemporaryData {
                    try await session.importKeyMaterial(material: .xprv(xprv: $0), options: ffi)
                }
            case .descriptors(let secret):
                try await secret.withTemporaryData {
                    try await session.importKeyMaterial(material: .descriptors(json: $0), options: ffi)
                }
            }
        }
        return try ImportReport(report)
    }

    public func exportForCore(
        on network: DashNetwork, wallet: WalletID, format: CoreExportFormat, to file: URL, grantID: String
    ) async throws(DashKitError) -> ExportReport {
        let session = try session(network)
        return ExportReport(try await mapped {
            try await session.exportForCore(walletId: wallet.hex, format: format.ffi, destPath: file.path, grantId: grantID)
        })
    }

    public func coreMnemonicCompatibility(on network: DashNetwork, wallet: WalletID) async throws(DashKitError)
        -> CoreMnemonicCompatibility
    {
        let session = try session(network)
        let result = try await mapped { try await session.coreMnemonicCompatibility(walletId: wallet.hex) }
        return CoreMnemonicCompatibility(
            coreCompatible: result.coreCompatible, warnings: result.warnings.map(ExportWarning.init))
    }

    public func backupWallet(on network: DashNetwork, wallet: WalletID, to file: URL, passphrase: SecretBytes?)
        async throws(DashKitError) -> BackupInfo
    {
        let session = try session(network)
        let info = try await mapped {
            if let passphrase {
                try await passphrase.withTemporaryData {
                    try await session.backupWallet(walletId: wallet.hex, destPath: file.path, backupPassphrase: $0)
                }
            } else {
                try await session.backupWallet(walletId: wallet.hex, destPath: file.path, backupPassphrase: nil)
            }
        }
        return try BackupInfo(info)
    }

    public func restoreBackup(on network: DashNetwork, file: URL, passphrase: SecretBytes?) async throws(DashKitError)
        -> [WalletID]
    {
        let session = try session(network)
        let ids = try await mapped {
            if let passphrase {
                try await passphrase.withTemporaryData { try await session.restoreBackup(path: file.path, passphrase: $0) }
            } else {
                try await session.restoreBackup(path: file.path, passphrase: nil)
            }
        }
        var result: [WalletID] = []
        for hex in ids {
            result.append(try .engine(hex))
        }
        return result
    }

    public func automaticBackups(on network: DashNetwork, wallet: WalletID?) async throws(DashKitError) -> [BackupInfo] {
        let session = try session(network)
        var result: [BackupInfo] = []
        for row in try await mapped({ try await session.automaticBackups(walletId: wallet?.hex) }) {
            result.append(try BackupInfo(row))
        }
        return result
    }

    public func backupPolicy(on network: DashNetwork) throws(DashKitError) -> BackupPolicy {
        let session = try session(network)
        return BackupPolicy(try mapped { try session.backupPolicy() })
    }

    public func setBackupPolicy(on network: DashNetwork, keep: UInt32) async throws(DashKitError) -> BackupPolicy {
        let session = try session(network)
        return BackupPolicy(try await mapped { try await session.setBackupPolicy(keep: keep) })
    }

    public nonisolated func parsePSBT(_ data: Data) throws(DashKitError) -> PSBTHandle {
        PSBTHandle(try mapped { try DashWalletCore.parsePsbt(data: data) })
    }

    public func analyzePSBT(on network: DashNetwork, wallet: WalletID?, psbt: PSBTHandle) async throws(DashKitError)
        -> PSBTAnalysis
    {
        let session = try session(network)
        let object = psbt.engineObject
        return try PSBTAnalysis(try await mapped { try await session.analyzePsbt(walletId: wallet?.hex, psbt: object) })
    }

    public func signPSBT(on network: DashNetwork, wallet: WalletID, psbt: PSBTHandle, grantID: String)
        async throws(DashKitError) -> PSBTHandle
    {
        let session = try session(network)
        let object = psbt.engineObject
        return PSBTHandle(try await mapped {
            try await session.signPsbt(walletId: wallet.hex, psbt: object, grantId: grantID)
        })
    }

    public func broadcastPSBT(on network: DashNetwork, psbt: PSBTHandle) async throws(DashKitError) -> String {
        let session = try session(network)
        let object = psbt.engineObject
        return try await mapped { try await session.broadcastPsbt(psbt: object) }
    }
}

extension CoreFunctions {
    // MARK: Console (QT-145)

    /// Every command name with its category and availability.
    public static func consoleCommands() throws(DashKitError) -> [ConsoleCommand] {
        try mapped { try DashWalletCore.consoleCommands() }.map {
            ConsoleCommand(name: $0.name, category: $0.category, sensitive: $0.sensitive, available: $0.available)
        }
    }

    /// dash-qt's history form of `line` (sensitive arguments replaced).
    public static func consoleRedact(_ line: SecretBytes) throws(DashKitError) -> String {
        try mapped { try line.withTemporaryData { try DashWalletCore.consoleRedact(line: $0) } }
    }
}
