// The S1 engine calls of M2 on `FakeEngine`. Each answers what the test
// configured in `State`, after the same session check the engine makes;
// unconfigured calls fail with `not_implemented`, as engine stubs do.
import DashKit
import Foundation

extension FakeEngine {
    func quickUnlockPolicy(on network: DashNetwork) async throws(DashKitError) -> QuickUnlockPolicy {
        try requireOpen(network, "quickUnlockPolicy")
        return try with { $0.quickUnlockPolicy }.get()
    }

    func enrollQuickUnlock(on network: DashNetwork, grantID: String) async throws(DashKitError) -> SecretBytes {
        try requireOpen(network, "enrollQuickUnlock \(grantID)")
        return SecretBytes(try with { $0.enrollKey }.get())
    }

    func removeQuickUnlock(on network: DashNetwork) async throws(DashKitError) -> VaultStatus {
        try requireOpen(network, "removeQuickUnlock")
        return try with { $0.removeQuickUnlock }.get()
    }

    func setQuickUnlockSpendLimit(on network: DashNetwork, grantID: String, limit: Amount)
        async throws(DashKitError) -> QuickUnlockPolicy
    {
        try requireOpen(network, "setQuickUnlockSpendLimit \(grantID)")
        let policy = try with { $0.quickUnlockPolicy }.get()
        let updated = QuickUnlockPolicy(
            enrolled: policy.enrolled, spendLimit: limit, passphraseMaxAgeSeconds: policy.passphraseMaxAgeSeconds,
            lastPassphraseAt: policy.lastPassphraseAt)
        with {
            $0.spendLimits.append(limit)
            $0.quickUnlockPolicy = .success(updated)
        }
        return updated
    }

    func recoverVault(
        on network: DashNetwork, wallet: WalletID, mnemonic: SecretBytes, bip39Passphrase: SecretBytes,
        newPassphrase: SecretBytes
    ) async throws(DashKitError) -> VaultRecovery {
        try requireOpen(network, "recoverVault \(wallet.hex)")
        return try with { $0.recovery }.get()
    }

    func destroyVault(on network: DashNetwork, credential: VaultCredential) async throws(DashKitError) -> VaultStatus {
        try requireOpen(network, "destroyVault")
        let kind: String
        switch credential {
        case .passphrase: kind = "passphrase"
        case .quickUnlock: kind = "quickUnlock"
        case .unencrypted: kind = "unencrypted"
        }
        with { $0.destroyCredentials.append(kind) }
        return try with { $0.destroy }.get()
    }

    func txNotices(on network: DashNetwork, wallet: WalletID, txids: [String]) async throws(DashKitError)
        -> [TxNotice]
    {
        try requireOpen(network, "txNotices \(txids.joined(separator: ","))")
        return try with { $0.txNotices }.get().filter { txids.contains($0.txid) }
    }

    func exportLogs(to file: URL, extraFiles: [URL]) async throws(DashKitError) -> LogExport {
        with {
            $0.calls.append("exportLogs \(file.lastPathComponent)")
            $0.exportedExtraFiles = extraFiles
        }
        return try with { $0.logExport }.get()
    }
}
