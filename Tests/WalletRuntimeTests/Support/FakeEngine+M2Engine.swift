// The R1/R2 engine calls of M2 on `FakeEngine`. Each makes the engine's
// session check first; the configurable ones answer what the test set in
// `State`, every other one fails with `not_implemented` (never a made-up
// success), as engine stubs do.
import DashKit
import Foundation

extension FakeEngine {
    private func unconfigured(_ network: DashNetwork, _ name: String) throws(DashKitError) -> Never {
        try requireOpen(network, name)
        throw .notImplemented(detail: "FakeEngine.\(name)")
    }

    // MARK: Wallet lifecycle

    func existingNetworks() async throws(DashKitError) -> [NetworkDataInfo] {
        throw .notImplemented(detail: "FakeEngine.existingNetworks")
    }

    func walletLoadStates(on network: DashNetwork) async throws(DashKitError) -> [WalletLoadState] {
        try requireOpen(network, "walletLoadStates")
        return try with { $0.loadStates }.get()
    }

    /// Flips `loaded` in the configured load states; unknown ids are
    /// `wallet_not_found`, as in the engine.
    private func setLoaded(_ loaded: Bool, _ network: DashNetwork, _ wallet: WalletID, _ name: String)
        throws(DashKitError)
    {
        try requireOpen(network, "\(name) \(wallet)")
        let result: Result<Void, DashKitError> = with { state in
            guard case .success(var rows) = state.loadStates else {
                return .failure(.notImplemented(detail: "FakeEngine.\(name)"))
            }
            guard let index = rows.firstIndex(where: { $0.walletID == wallet }) else {
                return .failure(.walletNotFound(detail: "\(wallet)"))
            }
            let row = rows[index]
            rows[index] = WalletLoadState(
                walletID: row.walletID, name: row.name, loaded: loaded, loadOnStartup: row.loadOnStartup,
                watchOnly: row.watchOnly)
            state.loadStates = .success(rows)
            return .success(())
        }
        try result.get()
    }

    func loadWallet(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) {
        try setLoaded(true, network, wallet, "loadWallet")
    }

    func unloadWallet(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) {
        try setLoaded(false, network, wallet, "unloadWallet")
    }

    func setLoadOnStartup(on network: DashNetwork, wallet: WalletID, loadOnStartup: Bool) async throws(DashKitError) {
        try unconfigured(network, "setLoadOnStartup")
    }

    func importWatchOnly(on network: DashNetwork, xpub: String, options: WatchOnlyOptions) async throws(DashKitError)
        -> WalletID
    {
        try unconfigured(network, "importWatchOnly")
    }

    func accountXpub(on network: DashNetwork, wallet: WalletID, account: UInt32) async throws(DashKitError)
        -> AccountXpub
    {
        try unconfigured(network, "accountXpub")
    }

    // MARK: Transactions, fees, dust

    func txDetailExtras(on network: DashNetwork, wallet: WalletID, txid: String) async throws(DashKitError)
        -> TxDetailExtras
    {
        try unconfigured(network, "txDetailExtras")
    }

    func abandonTransaction(on network: DashNetwork, wallet: WalletID, txid: String) async throws(DashKitError) {
        try unconfigured(network, "abandonTransaction")
    }

    func resendTransaction(on network: DashNetwork, wallet: WalletID, txid: String) async throws(DashKitError) {
        try unconfigured(network, "resendTransaction")
    }

    func dropUnconfirmed(on network: DashNetwork, wallet: WalletID?) async throws(DashKitError) -> UInt32 {
        try unconfigured(network, "dropUnconfirmed")
    }

    func exportHistoryCSV(
        on network: DashNetwork, wallet: WalletID, filter: HistoryFilter, sort: HistorySort, unit: DisplayUnit,
        typeNames: [String], utcOffsetSeconds: Int32
    ) async throws(DashKitError) -> String {
        try unconfigured(network, "exportHistoryCSV")
    }

    func feePolicy(on network: DashNetwork) async throws(DashKitError) -> FeePolicy {
        try requireOpen(network, "feePolicy")
        return try with { $0.feePolicy }.get()
    }

    func coinSelectionSummary(
        on network: DashNetwork, wallet: WalletID, outpoints: [OutPoint], payAmounts: [Amount], fee: FeeMode,
        allChangeToFee: Bool
    ) async throws(DashKitError) -> CoinSelectionSummary {
        try unconfigured(network, "coinSelectionSummary")
    }

    func dustProtection(on network: DashNetwork) async throws(DashKitError) -> Amount? {
        try requireOpen(network, "dustProtection")
        return try with { $0.dustThreshold }.get()
    }

    /// dw-engine's range check: 1...1,000,000 duffs or off.
    func setDustProtection(on network: DashNetwork, threshold: Amount?) async throws(DashKitError) {
        try requireOpen(network, "setDustProtection \(threshold.map { "\($0.duffs)" } ?? "off")")
        if let threshold, !(1...1_000_000).contains(threshold.duffs) {
            throw .invalidArgument(detail: "dust threshold \(threshold.duffs)")
        }
        with { $0.dustThreshold = .success(threshold) }
    }

    // MARK: Tools window

    func nodeInfo(on network: DashNetwork) async throws(DashKitError) -> NodeInfo {
        try unconfigured(network, "nodeInfo")
    }

    func warnings(on network: DashNetwork) async throws(DashKitError) -> [EngineWarning] {
        try unconfigured(network, "warnings")
    }

    func rescanProgress(on network: DashNetwork) async throws(DashKitError) -> RescanProgress? {
        try unconfigured(network, "rescanProgress")
    }

    func cancelRescan(on network: DashNetwork) async throws(DashKitError) -> Bool {
        try unconfigured(network, "cancelRescan")
    }

    /// The engine refuses while SPV runs (`sync.spv_running`).
    func resetChainData(on network: DashNetwork) async throws(DashKitError) {
        try requireOpen(network, "resetChainData")
        if with({ $0.spvRunning.contains(network) }) {
            throw .domain(code: "sync.spv_running", detail: "")
        }
    }

    func setBirthHeight(on network: DashNetwork, wallet: WalletID, height: UInt32) async throws(DashKitError) {
        try unconfigured(network, "setBirthHeight")
    }

    func disconnectPeer(on network: DashNetwork, address: String) async throws(DashKitError) {
        try unconfigured(network, "disconnectPeer")
    }

    func banPeer(on network: DashNetwork, address: String, durationSeconds: UInt64) async throws(DashKitError) {
        try unconfigured(network, "banPeer")
    }

    func unbanPeer(on network: DashNetwork, subnet: String) async throws(DashKitError) {
        try unconfigured(network, "unbanPeer")
    }

    func bannedPeers(on network: DashNetwork) async throws(DashKitError) -> [BannedPeer] {
        try unconfigured(network, "bannedPeers")
    }

    func consoleExecute(on network: DashNetwork, wallet: WalletID?, line: SecretBytes, grantID: String?)
        async throws(DashKitError) -> ConsoleExecution
    {
        try requireOpen(network, "consoleExecute \(line.utf8String() ?? "")")
        return try with {
            $0.consoleGrants.append(grantID)
            return $0.console
        }.get()
    }

    // MARK: Dash Core compatibility, backups, PSBT

    func inspectWalletFile(_ file: URL) async throws(DashKitError) -> WalletFileKind {
        throw .notImplemented(detail: "FakeEngine.inspectWalletFile")
    }

    func importDumpWallet(on network: DashNetwork, file: URL, options: ImportOptions) async throws(DashKitError)
        -> ImportReport
    {
        try unconfigured(network, "importDumpWallet")
    }

    func importWalletDat(on network: DashNetwork, file: URL, walletPassphrase: SecretBytes?, options: ImportOptions)
        async throws(DashKitError) -> ImportReport
    {
        try unconfigured(network, "importWalletDat")
    }

    func importKeyMaterial(on network: DashNetwork, material: KeyMaterial, options: ImportOptions)
        async throws(DashKitError) -> ImportReport
    {
        try unconfigured(network, "importKeyMaterial")
    }

    func exportForCore(on network: DashNetwork, wallet: WalletID, format: CoreExportFormat, to file: URL, grantID: String)
        async throws(DashKitError) -> ExportReport
    {
        try unconfigured(network, "exportForCore")
    }

    func coreMnemonicCompatibility(on network: DashNetwork, wallet: WalletID) async throws(DashKitError)
        -> CoreMnemonicCompatibility
    {
        try unconfigured(network, "coreMnemonicCompatibility")
    }

    func backupWallet(on network: DashNetwork, wallet: WalletID, to file: URL, passphrase: SecretBytes?)
        async throws(DashKitError) -> BackupInfo
    {
        try unconfigured(network, "backupWallet")
    }

    func restoreBackup(on network: DashNetwork, file: URL, passphrase: SecretBytes?) async throws(DashKitError)
        -> [WalletID]
    {
        try unconfigured(network, "restoreBackup")
    }

    func automaticBackups(on network: DashNetwork, wallet: WalletID?) async throws(DashKitError) -> [BackupInfo] {
        try unconfigured(network, "automaticBackups")
    }

    func backupPolicy(on network: DashNetwork) async throws(DashKitError) -> BackupPolicy {
        try unconfigured(network, "backupPolicy")
    }

    func setBackupPolicy(on network: DashNetwork, keep: UInt32) async throws(DashKitError) -> BackupPolicy {
        try unconfigured(network, "setBackupPolicy")
    }

    func parsePSBT(_ data: Data) throws(DashKitError) -> PSBTHandle {
        throw .notImplemented(detail: "FakeEngine.parsePSBT")
    }

    func analyzePSBT(on network: DashNetwork, wallet: WalletID?, psbt: PSBTHandle) async throws(DashKitError)
        -> PSBTAnalysis
    {
        try unconfigured(network, "analyzePSBT")
    }

    func signPSBT(on network: DashNetwork, wallet: WalletID, psbt: PSBTHandle, grantID: String)
        async throws(DashKitError) -> PSBTHandle
    {
        try unconfigured(network, "signPSBT")
    }

    func broadcastPSBT(on network: DashNetwork, psbt: PSBTHandle) async throws(DashKitError) -> String {
        try unconfigured(network, "broadcastPSBT")
    }
}

extension FakeTxDraft {
    func createUnsigned() async throws(DashKitError) -> PSBTHandle {
        throw .notImplemented(detail: "FakeTxDraft.createUnsigned")
    }
}
