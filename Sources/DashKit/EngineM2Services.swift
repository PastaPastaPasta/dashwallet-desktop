import DashWalletCore
import Foundation

// DashKit side of the S1 engine calls of M2 (docs/contracts/m2-engine.md
// §2.2 `tx_notices`, §2.9 vault, §2.10 `Engine.export_logs`): value types
// and the `EngineClient` conformance. The free desktop functions are in
// `Desktop.swift`.

/// Quick-unlock rules the vault enforces (IOS-011/016).
public struct QuickUnlockPolicy: Sendable, Hashable {
    public let enrolled: Bool
    public let spendLimit: Amount
    public let passphraseMaxAgeSeconds: UInt64
    public let lastPassphraseAt: Date?

    public init(enrolled: Bool, spendLimit: Amount, passphraseMaxAgeSeconds: UInt64, lastPassphraseAt: Date?) {
        self.enrolled = enrolled
        self.spendLimit = spendLimit
        self.passphraseMaxAgeSeconds = passphraseMaxAgeSeconds
        self.lastPassphraseAt = lastPassphraseAt
    }

    init(_ ffi: DashWalletCore.QuickUnlockPolicy) throws(DashKitError) {
        self.init(
            enrolled: ffi.enrolled, spendLimit: try Amount(engine: ffi.spendLimitDuffs),
            passphraseMaxAgeSeconds: ffi.passphraseMaxAgeSecs, lastPassphraseAt: ffi.lastPassphraseAt.engineDate)
    }
}

/// Outcome of `recoverVault` (IOS-014).
public struct VaultRecovery: Sendable, Hashable {
    public let status: VaultStatus
    /// Wallets whose seeds were only in the replaced vault.
    public let walletsWithoutSecrets: [WalletID]

    public init(status: VaultStatus, walletsWithoutSecrets: [WalletID]) {
        self.status = status
        self.walletsWithoutSecrets = walletsWithoutSecrets
    }

    init(_ ffi: DashWalletCore.VaultRecovery) throws(DashKitError) {
        var ids: [WalletID] = []
        for hex in ffi.walletsWithoutSecrets {
            ids.append(try .engine(hex))
        }
        self.init(status: try VaultStatus(ffi.status), walletsWithoutSecrets: ids)
    }
}

/// One notification row of a `newTransactions` event (QT-031).
public struct TxNotice: Sendable, Hashable {
    public let txid: String
    public let recordIndex: UInt32
    /// Positive = incoming.
    public let amount: Amount
    public let timestamp: Date?
    public let type: TxType
    public let address: String?
    public let label: String?
    public let coinJoinInternal: Bool

    public init(
        txid: String, recordIndex: UInt32, amount: Amount, timestamp: Date?, type: TxType, address: String?,
        label: String?, coinJoinInternal: Bool
    ) {
        self.txid = txid
        self.recordIndex = recordIndex
        self.amount = amount
        self.timestamp = timestamp
        self.type = type
        self.address = address
        self.label = label
        self.coinJoinInternal = coinJoinInternal
    }

    init(_ ffi: DashWalletCore.TxNotice) {
        self.init(
            txid: ffi.txid, recordIndex: ffi.recordIndex, amount: Amount(duffs: ffi.amount),
            timestamp: ffi.timestamp.engineDate, type: TxType(ffi.txType), address: ffi.address, label: ffi.label,
            coinJoinInternal: ffi.coinjoinInternal)
    }
}

/// What `exportLogs` wrote (IOS-112).
public struct LogExport: Sendable, Hashable {
    public let file: URL
    /// Log files in the zip, without its `manifest.txt`.
    public let fileCount: Int
    public let sizeBytes: UInt64

    public init(file: URL, fileCount: Int, sizeBytes: UInt64) {
        self.file = file
        self.fileCount = fileCount
        self.sizeBytes = sizeBytes
    }
}

extension EngineClient {
    // MARK: Vault (M2, slot B and recovery)

    public func quickUnlockPolicy(on network: DashNetwork) throws(DashKitError) -> QuickUnlockPolicy {
        let vault = try session(network).vault()
        return try QuickUnlockPolicy(try mapped { try vault.quickUnlockPolicy() })
    }

    /// The slot B wrap key, in a zeroing buffer; the binding's copy is wiped.
    public func enrollQuickUnlock(on network: DashNetwork, grantID: String) async throws(DashKitError) -> SecretBytes {
        let vault = try session(network).vault()
        var key = try await mapped { try await vault.enrollQuickUnlock(grantId: grantID) }
        return SecretBytes(consuming: &key)
    }

    public func removeQuickUnlock(on network: DashNetwork) async throws(DashKitError) -> VaultStatus {
        let vault = try session(network).vault()
        return try VaultStatus(try await mapped { try await vault.removeQuickUnlock() })
    }

    public func setQuickUnlockSpendLimit(on network: DashNetwork, grantID: String, limit: Amount)
        async throws(DashKitError) -> QuickUnlockPolicy
    {
        let vault = try session(network).vault()
        let duffs = try limit.engineDuffs()
        return try QuickUnlockPolicy(try await mapped {
            try await vault.setQuickUnlockSpendLimit(grantId: grantID, spendLimitDuffs: duffs)
        })
    }

    public func recoverVault(
        on network: DashNetwork, wallet: WalletID, mnemonic: SecretBytes, bip39Passphrase: SecretBytes,
        newPassphrase: SecretBytes
    ) async throws(DashKitError) -> VaultRecovery {
        let vault = try session(network).vault()
        let recovery = try await mapped {
            try await mnemonic.withTemporaryData { phrase in
                try await bip39Passphrase.withTemporaryData { passphrase in
                    try await newPassphrase.withTemporaryData { new in
                        try await vault.recoverWithMnemonic(
                            walletId: wallet.hex, mnemonic: phrase, bip39Passphrase: passphrase, newPassphrase: new)
                    }
                }
            }
        }
        return try VaultRecovery(recovery)
    }

    public func destroyVault(on network: DashNetwork, credential: VaultCredential) async throws(DashKitError)
        -> VaultStatus
    {
        let vault = try session(network).vault()
        let status = try await mapped {
            switch credential {
            case .passphrase(let secret):
                try await secret.withTemporaryData { try await vault.destroy(credential: .passphrase(passphrase: $0)) }
            case .quickUnlock(let key):
                try await key.withTemporaryData { try await vault.destroy(credential: .quickUnlock(wrapKey: $0)) }
            case .unencrypted:
                try await vault.destroy(credential: .unencrypted)
            }
        }
        return try VaultStatus(status)
    }

    // MARK: Notifications (QT-031)

    public func txNotices(on network: DashNetwork, wallet: WalletID, txids: [String]) async throws(DashKitError)
        -> [TxNotice]
    {
        let session = try session(network)
        return try await mapped { try await session.txNotices(walletId: wallet.hex, txids: txids) }.map(TxNotice.init)
    }

    // MARK: Logs (IOS-112)

    public func exportLogs(to file: URL, extraFiles: [URL]) async throws(DashKitError) -> LogExport {
        let engine = try core.get()
        let export = try await mapped {
            try await engine.exportLogs(destPath: file.path, extraFiles: extraFiles.map(\.path))
        }
        return LogExport(
            file: URL(fileURLWithPath: export.path), fileCount: Int(export.fileCount), sizeBytes: export.sizeBytes)
    }
}
