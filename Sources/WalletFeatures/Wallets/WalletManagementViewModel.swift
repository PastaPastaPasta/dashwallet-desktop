// Wallet management (QT-101, QT-106…110, QT-114, QT-116, IOS-009, IOS-110,
// IOS-111): open/close/load-on-startup, rename, remove, imports from dash-qt
// files and key material, watch-only wallets, backups, exports for dash-qt,
// the account xpub and first-run detection of existing data.
import Foundation
import Observation
import PlatformServices
import WalletRuntime

/// Raw key material kinds for QT-108.
public enum KeyMaterialKind: Sendable, Hashable, CaseIterable {
    /// `hdseed` hex (16...64 bytes).
    case hdSeed
    /// Master `xprv` (`tprv` off mainnet).
    case xprv
    /// `listdescriptors true` JSON.
    case descriptors
}

/// What a running operation is, for progress text.
public enum WalletOperation: Sendable, Hashable {
    case opening, closing, renaming, removing, importing, restoring, backingUp, exporting, deleting
}

/// The wallet-management flow (one operation at a time).
public enum WalletManagementFlow: Sendable, Hashable {
    case idle
    case working(WalletOperation)
    /// An encrypted wallet.dat or a `.dwbackup` needs its passphrase.
    case needsFilePassphrase(URL, WalletFileKind)
    /// The vault passphrase is needed for a grant (`.wipe` or `.revealSecret`).
    case needsVaultPassphrase(WalletOperation)
    case confirmingRemove(WalletID, name: String)
    case imported(WalletImportReport)
    case restored([WalletID])
    case backedUp(WalletBackup)
    case exported(CoreExportReport, coreCompatible: Bool)
    /// The chosen file is a PSBT: the UI opens the PSBT dialog with it.
    case openPSBT(URL)
    case deletedAll
    case failed(String)
}

@MainActor
@Observable
public final class WalletManagementViewModel {
    public private(set) var wallets: [WalletLoadState] = []
    public private(set) var flow: WalletManagementFlow = .idle
    public private(set) var automaticBackups: [WalletBackup] = []
    public private(set) var backupDirectory: URL?
    /// IOS-009: networks with data under the data root.
    public private(set) var existingData: [NetworkDataInfo] = []
    public private(set) var xpub: AccountXpub?
    public private(set) var xpubQR: QRMatrix?
    public private(set) var errorMessage: String?
    /// The engine does not list load states yet; the list shows loaded wallets.
    public private(set) var loadStatesUnavailable = false

    /// First-run "Wallets found on this device" (IOS-009).
    public var showsExistingDataPrompt: Bool { existingData.contains { $0.hasWalletState || $0.hasVault } }

    /// Lines for the import result (labels, keys not imported).
    public var importSummary: [String] {
        guard case .imported(let report) = flow else { return [] }
        var lines = [L10n.Wallets.imported(labels: report.labelsImported)]
        if report.keysNotImported + report.scriptsNotImported > 0 {
            lines.append(L10n.Wallets.keysNotImported(report.keysNotImported, scripts: report.scriptsNotImported))
        }
        return lines
    }

    /// Lines for the export result: key count and compatibility warnings.
    public var exportSummary: [String] {
        guard case .exported(let report, _) = flow else { return [] }
        return [L10n.Wallets.exported(keys: report.keyCount)] + report.warnings.map { warning in
            switch warning {
            case .mnemonicNotCoreCompatible: L10n.Wallets.mnemonicNotCoreCompatible
            case .coinJoinAccountNotScannedByLegacyCore: L10n.Wallets.coinJoinNotScanned
            }
        }
    }

    private let walletLifecycle: any WalletLifecycleManaging
    private let lifecycle: any LifecycleQueueing
    private let walletState: any WalletStateProviding
    private let auth: any AuthenticationGating
    private let vault: any VaultProviding
    private let importer: any WalletFileImporting
    private let exporter: any CoreExporting
    private let backups: any BackupProviding
    private let recovery: any VaultRecovering
    private let uri: any URIHandling
    private let fileRevealer: any FileRevealing
    private let desktopPreferences: any DesktopPreferencesStoring
    /// The export or removal that waits for the vault passphrase.
    private var pending: Pending?
    private var task: Task<Void, Never>?

    private enum Pending {
        case remove(WalletID)
        case export(WalletID, CoreExportFormat, URL)
        case deleteAll
    }

    public init(
        walletLifecycle: any WalletLifecycleManaging, lifecycle: any LifecycleQueueing,
        walletState: any WalletStateProviding, auth: any AuthenticationGating, vault: any VaultProviding,
        importer: any WalletFileImporting, exporter: any CoreExporting, backups: any BackupProviding,
        recovery: any VaultRecovering, uri: any URIHandling, fileRevealer: any FileRevealing,
        desktopPreferences: any DesktopPreferencesStoring
    ) {
        self.walletLifecycle = walletLifecycle
        self.lifecycle = lifecycle
        self.walletState = walletState
        self.auth = auth
        self.vault = vault
        self.importer = importer
        self.exporter = exporter
        self.backups = backups
        self.recovery = recovery
        self.uri = uri
        self.fileRevealer = fileRevealer
        self.desktopPreferences = desktopPreferences
    }

    public convenience init(env: AppEnvironment, m2: M2Services) {
        self.init(
            walletLifecycle: m2.walletLifecycle, lifecycle: env.lifecycle, walletState: env.walletState,
            auth: env.auth, vault: env.vault, importer: m2.fileImporter, exporter: m2.coreExporter,
            backups: m2.backups, recovery: m2.vaultRecovery, uri: env.uri, fileRevealer: m2.fileRevealer,
            desktopPreferences: m2.desktopPreferences)
    }

    // MARK: Loading

    public func load() async {
        await reloadWallets()
        await reloadBackups()
    }

    /// Follows load-state changes until `stop()`.
    public func start() {
        stop()
        let changes = walletLifecycle.loadStateChanges()
        task = Task { [weak self] in
            for await _ in changes {
                await self?.reloadWallets()
            }
        }
    }

    public func stop() {
        task?.cancel()
        task = nil
    }

    /// IOS-009: what the data root already holds (first run after a reinstall).
    public func loadExistingData() async {
        do {
            existingData = try await walletLifecycle.existingNetworks()
        } catch {
            existingData = []
            if error.code != .notImplemented { errorMessage = ErrorText.m2(error.code) }
        }
    }

    // MARK: Open, close, rename, remove (QT-101, IOS-110)

    public func open(_ id: WalletID) async {
        await run(.opening) { () async throws(ServiceError) in
            try await self.walletLifecycle.load(id)
            try await self.walletLifecycle.setLoadOnStartup(id, true)
        }
    }

    /// dash-qt Close: unloads and drops the wallet from load-on-startup.
    public func close(_ id: WalletID) async {
        await run(.closing) { () async throws(ServiceError) in
            try await self.walletLifecycle.unload(id)
            try await self.walletLifecycle.setLoadOnStartup(id, false)
        }
    }

    public func closeAll() async {
        let loaded = wallets.filter(\.loaded).map(\.walletID)
        await run(.closing) { () async throws(ServiceError) in
            for id in loaded {
                try await self.walletLifecycle.unload(id)
                try await self.walletLifecycle.setLoadOnStartup(id, false)
            }
        }
    }

    public func setLoadOnStartup(_ id: WalletID, _ value: Bool) async {
        await run(nil) { () async throws(ServiceError) in try await self.walletLifecycle.setLoadOnStartup(id, value) }
    }

    public func rename(_ id: WalletID, to name: String) async {
        await run(.renaming) { () async throws(ServiceError) in try await self.walletState.rename(id, to: name) }
    }

    public func requestRemove(_ id: WalletID) {
        let name = wallets.first { $0.walletID == id }?.name
            ?? walletState.wallets?.first { $0.id == id }?.name ?? ""
        flow = .confirmingRemove(id, name: name)
    }

    /// Removes the wallet with a `.wipe` grant; an encrypted vault needs `passphrase`.
    public func confirmRemove(passphrase: String? = nil) async {
        let id: WalletID
        switch flow {
        case .confirmingRemove(let wallet, _): id = wallet
        case .needsVaultPassphrase(.removing):
            guard case .remove(let wallet) = pending else { return }
            id = wallet
        default: return
        }
        guard let credential = makeCredential(for: .wipe, passphrase: passphrase, auth: auth, vault: vault) else {
            pending = .remove(id)
            flow = .needsVaultPassphrase(.removing)
            return
        }
        pending = nil
        await run(.removing) { () async throws(ServiceError) in
            let grant = try await self.auth.authorize(.wipe, wallet: id, credential: credential)
            try await self.lifecycle.removeWallet(id, grant: grant)
        }
    }

    // MARK: Imports (QT-106…108, QT-110, QT-114)

    /// Inspects the file, then imports it by kind: dumpwallet, wallet.dat
    /// (asking for its passphrase when encrypted) or a `.dwbackup`.
    public func importFile(_ url: URL) async {
        flow = .working(.importing)
        let kind: WalletFileKind
        do {
            kind = try await importer.inspect(url)
        } catch {
            flow = .failed(ErrorText.m2(error.code))
            return
        }
        switch kind {
        case .dumpWallet:
            await runImport(.importing) { () async throws(ServiceError) in
                try await self.importer.importDumpWallet(url, options: WalletImportOptions())
            }
        case .walletDatSQLite(let encrypted, _):
            if encrypted {
                flow = .needsFilePassphrase(url, kind)
            } else {
                await runImport(.importing) { () async throws(ServiceError) in
                    try await self.importer.importWalletDat(url, passphrase: nil, options: WalletImportOptions())
                }
            }
        case .walletDatBerkeleyDB:
            // M6: the engine answers not_implemented; say what to do instead.
            await runImport(.importing) { () async throws(ServiceError) in
                try await self.importer.importWalletDat(url, passphrase: nil, options: WalletImportOptions())
            }
            if case .failed = flow { flow = .failed(L10n.Wallets.berkeleyDBUnavailable) }
        case .dwBackup:
            await restore(url, passphrase: nil)
        case .psbt:
            flow = .openPSBT(url)
        case .unknown:
            flow = .failed(L10n.Wallets.unknownFile)
        }
    }

    /// The passphrase of an encrypted wallet.dat or of a backup.
    public func provideFilePassphrase(_ passphrase: String) async {
        guard case .needsFilePassphrase(let url, let kind) = flow else { return }
        let secret = vault.makeSecret(utf8: passphrase)
        // Only an encrypted wallet.dat goes to the importer; every other file
        // that asked for a passphrase is a backup being restored.
        if case .walletDatSQLite = kind {
            await runImport(.importing) { () async throws(ServiceError) in
                try await self.importer.importWalletDat(url, passphrase: secret, options: WalletImportOptions())
            }
        } else {
            await restore(url, passphrase: secret)
        }
    }

    /// QT-108: `text` is hex for `.hdSeed`, base58 for `.xprv`, JSON for
    /// `.descriptors`. It goes straight into a zeroing buffer.
    public func importKeyMaterial(_ kind: KeyMaterialKind, text: String) async {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let material: KeyMaterial
        switch kind {
        case .hdSeed:
            guard let bytes = ZeroizingBytes.hex(trimmed) else {
                flow = .failed(L10n.Wallets.invalidSeedHex)
                return
            }
            material = .hdSeed(bytes)
        case .xprv:
            material = .xprv(vault.makeSecret(utf8: trimmed))
        case .descriptors:
            material = .descriptors(vault.makeSecret(utf8: trimmed))
        }
        await runImport(.importing) { () async throws(ServiceError) in
            try await self.importer.importKeyMaterial(material, options: WalletImportOptions())
        }
    }

    /// QT-114: a watch-only wallet from an account xpub.
    public func addWatchOnly(xpub: String, name: String?, birthHeight: UInt32? = nil) async {
        let trimmedName = name?.trimmingCharacters(in: .whitespaces)
        await run(.importing) { () async throws(ServiceError) in
            let options = WatchOnlyImportOptions(
                name: trimmedName?.isEmpty == false ? trimmedName : nil, birthHeight: birthHeight)
            _ = try await self.walletLifecycle.importWatchOnly(
                xpub: xpub.trimmingCharacters(in: .whitespacesAndNewlines), options: options)
        }
    }

    // MARK: Backups (QT-110, QT-116)

    /// Backup Wallet. An unencrypted vault needs a backup passphrase; an
    /// encrypted one uses its own (the vault must be unlocked).
    public func backup(_ id: WalletID, to url: URL, passphrase: String?) async {
        let secret: (any SecretBuffer)?
        if auth.lockState == .unencrypted {
            guard let passphrase, !passphrase.isEmpty else {
                flow = .failed(L10n.M2Errors.backupPassphraseRequired)
                return
            }
            secret = vault.makeSecret(utf8: passphrase)
        } else {
            secret = nil
        }
        flow = .working(.backingUp)
        do {
            let backup = try await backups.backup(wallet: id, to: url, passphrase: secret)
            flow = .backedUp(backup)
            await reloadBackups()
        } catch {
            flow = .failed(ErrorText.m2(error.code))
        }
    }

    public func restore(_ url: URL, passphrase: (any SecretBuffer)?) async {
        flow = .working(.restoring)
        do {
            let ids = try await backups.restore(from: url, passphrase: passphrase)
            flow = .restored(ids)
            await reloadWallets()
        } catch {
            if error.code == .backupPassphraseRequired, passphrase == nil {
                flow = .needsFilePassphrase(url, (try? await importer.inspect(url)) ?? .unknown)
            } else {
                flow = .failed(ErrorText.m2(error.code))
            }
        }
    }

    /// File ▸ Show Automatic Backups.
    public func showBackupsFolder() {
        guard let backupDirectory else { return }
        do {
            try fileRevealer.reveal(backupDirectory)
        } catch {
            errorMessage = ErrorText.m2(ServiceError(error).code)
        }
    }

    // MARK: Export for dash-qt (QT-109)

    /// Writes a dumpwallet or importdescriptors file with a `.revealSecret` grant.
    public func exportForCore(_ id: WalletID, format: CoreExportFormat, to url: URL, passphrase: String? = nil) async {
        guard let credential = makeCredential(for: .revealSecret, passphrase: passphrase, auth: auth, vault: vault)
        else {
            pending = .export(id, format, url)
            flow = .needsVaultPassphrase(.exporting)
            return
        }
        pending = nil
        flow = .working(.exporting)
        do {
            let grant = try await auth.authorize(.revealSecret, wallet: id, credential: credential)
            let report = try await exporter.export(wallet: id, format: format, to: url, grant: grant)
            let compatible = (try? await exporter.mnemonicCompatibility(wallet: id).coreCompatible) ?? false
            BackupReminderViewModel.recordBackup(id, in: desktopPreferences)
            flow = .exported(report, coreCompatible: compatible)
        } catch {
            flow = .failed(ErrorText.m2(error.code))
        }
    }

    /// The vault passphrase for the operation in `.needsVaultPassphrase`.
    public func provideVaultPassphrase(_ passphrase: String) async {
        guard case .needsVaultPassphrase = flow, let pending else { return }
        switch pending {
        case .remove:
            await confirmRemove(passphrase: passphrase)
        case .export(let id, let format, let url):
            await exportForCore(id, format: format, to: url, passphrase: passphrase)
        case .deleteAll:
            await deleteAll(passphrase: passphrase)
        }
    }

    // MARK: xpub (IOS-111)

    public func loadXpub(_ id: WalletID, account: UInt32 = 0) async {
        do {
            let key = try await walletLifecycle.accountXpub(wallet: id, account: account)
            xpub = key
            xpubQR = try? uri.qrMatrix(for: key.xpub)
            errorMessage = nil
        } catch {
            xpub = nil
            xpubQR = nil
            errorMessage = ErrorText.m2(error.code)
        }
    }

    // MARK: Existing data (IOS-009)

    /// "Keep Wallets".
    public func keepExistingData() {
        existingData = []
    }

    /// "Delete All" after the typed acceptance sentence: removes every wallet
    /// of the open network and destroys its vault.
    public func deleteAll(acceptance: String, passphrase: String? = nil) async {
        guard acceptance.trimmingCharacters(in: .whitespacesAndNewlines) == L10n.Wallets.wipeAcceptPhrase else {
            flow = .failed(L10n.Wallets.acceptPhraseMismatch)
            return
        }
        await deleteAll(passphrase: passphrase)
    }

    private func deleteAll(passphrase: String?) async {
        if makeCredential(for: .wipe, passphrase: passphrase, auth: auth, vault: vault) == nil {
            pending = .deleteAll
            flow = .needsVaultPassphrase(.deleting)
            return
        }
        pending = nil
        flow = .working(.deleting)
        let wiper = WalletWiper(
            walletLifecycle: walletLifecycle, lifecycle: lifecycle, walletState: walletState, auth: auth, vault: vault,
            recovery: recovery)
        do {
            _ = try await wiper.wipeAll(passphrase: passphrase)
            existingData = []
            flow = .deletedAll
        } catch {
            flow = .failed(ErrorText.m2(error.code))
        }
        await reloadWallets()
    }

    public func dismiss() {
        pending = nil
        flow = .idle
    }

    // MARK: Private

    private func reloadWallets() async {
        do {
            wallets = try await walletLifecycle.loadStates()
            loadStatesUnavailable = false
        } catch {
            loadStatesUnavailable = error.code == .notImplemented
            // Until the engine lists load states, show the loaded wallets as loaded.
            wallets = (walletState.wallets ?? []).map {
                WalletLoadState(walletID: $0.id, name: $0.name, loaded: true, loadOnStartup: true, watchOnly: $0.watchOnly)
            }
            if !loadStatesUnavailable { errorMessage = ErrorText.m2(error.code) }
        }
    }

    private func reloadBackups() async {
        do {
            automaticBackups = try await backups.automaticBackups(wallet: nil)
            backupDirectory = try await backups.policy().directory
        } catch {
            automaticBackups = []
            if error.code != .notImplemented { errorMessage = ErrorText.m2(error.code) }
        }
    }

    private func run(_ operation: WalletOperation?, _ body: () async throws(ServiceError) -> Void) async {
        if let operation { flow = .working(operation) }
        do {
            try await body()
            flow = .idle
            errorMessage = nil
        } catch {
            flow = .failed(ErrorText.m2(error.code))
        }
        await reloadWallets()
    }

    private func runImport(
        _ operation: WalletOperation, _ body: () async throws(ServiceError) -> WalletImportReport
    ) async {
        flow = .working(operation)
        do {
            flow = .imported(try await body())
        } catch {
            flow = .failed(ErrorText.m2(error.code))
        }
        await reloadWallets()
    }
}
