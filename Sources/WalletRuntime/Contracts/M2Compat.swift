// M2 service contracts: dash-qt imports and exports, backups and PSBT
// (engine compat.rs, backup.rs, psbt.rs; m2-swift.md §2.4–2.6).
import Foundation

// MARK: Import and export

/// What a user-chosen file is (QT-106/107/110).
public enum WalletFileKind: Sendable, Hashable {
    case dumpWallet(
        network: DashNetwork?, hasMnemonic: Bool, hasHDSeed: Bool, hasXprv: Bool, looseKeyCount: Int,
        scriptCount: Int, labelCount: Int)
    case walletDatSQLite(encrypted: Bool, hasMnemonic: Bool)
    /// Detected; importing it lands in M6 (`not_implemented`).
    case walletDatBerkeleyDB(encrypted: Bool?)
    case dwBackup(network: DashNetwork, walletCount: Int, createdAt: Date, formatVersion: UInt32)
    case psbt
    case unknown
}

/// Raw key material for QT-108. Every payload is a zeroing buffer.
public enum KeyMaterial: Sendable {
    /// BIP32 seed bytes (16...64).
    case hdSeed(any SecretBuffer)
    /// Master xprv, base58 text.
    case xprv(any SecretBuffer)
    /// `listdescriptors true` JSON.
    case descriptors(any SecretBuffer)
}

public struct WalletImportReport: Sendable, Hashable {
    public let walletID: WalletID
    public let labelsImported: Int
    /// Loose WIF keys and scripts not imported (sweep is M5); tell the user.
    public let keysNotImported: Int
    public let scriptsNotImported: Int
    public let coreCompatibleSeed: Bool

    public init(
        walletID: WalletID, labelsImported: Int, keysNotImported: Int, scriptsNotImported: Int,
        coreCompatibleSeed: Bool
    ) {
        self.walletID = walletID
        self.labelsImported = labelsImported
        self.keysNotImported = keysNotImported
        self.scriptsNotImported = scriptsNotImported
        self.coreCompatibleSeed = coreCompatibleSeed
    }
}

public enum CoreExportFormat: Sendable, Hashable, CaseIterable {
    case dumpWallet
    case importDescriptorsJSON
}

public enum CoreExportWarning: Sendable, Hashable {
    case mnemonicNotCoreCompatible
    case coinJoinAccountNotScannedByLegacyCore
}

public struct CoreExportReport: Sendable, Hashable {
    public let file: URL
    public let format: CoreExportFormat
    public let keyCount: Int
    public let warnings: [CoreExportWarning]

    public init(file: URL, format: CoreExportFormat, keyCount: Int, warnings: [CoreExportWarning]) {
        self.file = file
        self.format = format
        self.keyCount = keyCount
        self.warnings = warnings
    }
}

/// dash-qt restore/import/export (owner S1 adapter over R2). Imports run on
/// the lifecycle queue like `LifecycleQueueing.importWallet` and need the
/// vault unlocked (`compat.vault_locked`).
public protocol WalletFileImporting: AnyObject, Sendable {
    func inspect(_ file: URL) async throws(ServiceError) -> WalletFileKind
    func importDumpWallet(_ file: URL, options: WalletImportOptions) async throws(ServiceError) -> WalletImportReport
    /// SQLite descriptor wallets; Berkeley DB is `not_implemented` until M6.
    func importWalletDat(_ file: URL, passphrase: (any SecretBuffer)?, options: WalletImportOptions)
        async throws(ServiceError) -> WalletImportReport
    func importKeyMaterial(_ material: KeyMaterial, options: WalletImportOptions) async throws(ServiceError)
        -> WalletImportReport
}

public protocol CoreExporting: AnyObject, Sendable {
    /// Writes the file (mode 0600, never replacing one). `.revealSecret` grant.
    func export(wallet: WalletID, format: CoreExportFormat, to file: URL, grant: AuthGrant)
        async throws(ServiceError) -> CoreExportReport
    /// Whether phrase + passphrase + `upgradetohd` rebuilds the wallet in
    /// dash-qt; `warnings` explain why not.
    func mnemonicCompatibility(wallet: WalletID) async throws(ServiceError) -> (
        coreCompatible: Bool, warnings: [CoreExportWarning]
    )
}

// MARK: Backups

public struct WalletBackup: Sendable, Hashable {
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
}

public struct BackupPolicy: Sendable, Hashable {
    /// 0...10; 0 turns automatic backups off.
    public let keep: Int
    public let directory: URL

    public init(keep: Int, directory: URL) {
        self.keep = keep
        self.directory = directory
    }
}

/// `.dwbackup` files and automatic backups (QT-110/116; owner S1 adapter
/// over R2).
public protocol BackupProviding: AnyObject, Sendable {
    /// Encrypted vault: `passphrase` must be `nil` (the vault slot wraps it;
    /// vault unlocked). Unencrypted vault: `passphrase` is required.
    func backup(wallet: WalletID, to file: URL, passphrase: (any SecretBuffer)?) async throws(ServiceError)
        -> WalletBackup
    func restore(from file: URL, passphrase: (any SecretBuffer)?) async throws(ServiceError) -> [WalletID]
    /// Newest first; all wallets when `wallet` is `nil`.
    func automaticBackups(wallet: WalletID?) async throws(ServiceError) -> [WalletBackup]
    func policy() async throws(ServiceError) -> BackupPolicy
    func setKeep(_ keep: Int) async throws(ServiceError) -> BackupPolicy
}

// MARK: PSBT

/// A PSBT the runtime holds (engine `Psbt` object), named by `id`. Values are
/// immutable: signing gives a new one. `release(_:)` drops the engine object.
public struct PSBTReference: Sendable, Hashable, Identifiable {
    public let id: UUID
    public let unsignedTxid: String

    public init(id: UUID, unsignedTxid: String) {
        self.id = id
        self.unsignedTxid = unsignedTxid
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

/// PSBT Operations dialog content (QT-079).
public struct PSBTAnalysis: Sendable, Hashable {
    public let outputs: [PSBTOutput]
    public let fee: Amount?
    public let total: Amount?
    public let unsignedInputs: Int
    public let status: PSBTStatus
    public let signability: PSBTSignability
    /// The `.spend(max:)` grant for `sign` must cover this.
    public let externalSent: Amount?

    public init(
        outputs: [PSBTOutput], fee: Amount?, total: Amount?, unsignedInputs: Int, status: PSBTStatus,
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
}

/// QT-076…079 (owner S1 adapter over R2).
public protocol PSBTHandling: AnyObject, Sendable {
    /// "Create Unsigned" from a draft made by this runtime's
    /// `TransactionSending` (another conformance: `invalid_argument`).
    func createUnsigned(from draft: any TransactionDrafting) async throws(ServiceError) -> PSBTReference
    /// File bytes (binary or base64) or clipboard text bytes, ≤ 100 MiB.
    func load(_ data: Data) throws(ServiceError) -> PSBTReference
    func base64(_ psbt: PSBTReference) throws(ServiceError) -> String
    /// BIP174 binary for "Save…".
    func bytes(_ psbt: PSBTReference) throws(ServiceError) -> Data
    func analyze(_ psbt: PSBTReference, wallet: WalletID?) async throws(ServiceError) -> PSBTAnalysis
    /// Needs `.spend(max: ≥ externalSent)` for `wallet`.
    func sign(_ psbt: PSBTReference, wallet: WalletID, grant: AuthGrant) async throws(ServiceError) -> PSBTReference
    /// Returns the txid; send's broadcast verdicts apply.
    func broadcast(_ psbt: PSBTReference) async throws(ServiceError) -> String
    func release(_ psbt: PSBTReference)
}
