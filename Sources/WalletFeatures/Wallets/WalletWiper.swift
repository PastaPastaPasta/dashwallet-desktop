// Shared pieces of the wallet-management and security flows: a zeroing byte
// buffer for raw key material, and the full wipe (IOS-009 "Delete All",
// IOS-109): every registered wallet removed with a `.wipe` grant, then the
// vault destroyed.
import Foundation
import WalletRuntime

/// Raw secret bytes (an HD seed decoded from hex) in a buffer that is zeroed
/// on deinit, for `KeyMaterial.hdSeed`.
final class ZeroizingBytes: SecretBuffer, @unchecked Sendable {
    private var bytes: [UInt8]

    init(_ bytes: [UInt8]) {
        self.bytes = bytes
    }

    deinit {
        bytes.withUnsafeMutableBufferPointer { buffer in
            for index in buffer.indices { buffer[index] = 0 }
        }
    }

    var count: Int { bytes.count }

    func withUnsafeBytes<R>(_ body: (UnsafeRawBufferPointer) throws -> R) rethrows -> R {
        try bytes.withUnsafeBytes(body)
    }

    /// Decodes 32...128 hex characters (16...64 bytes); `nil` otherwise.
    static func hex(_ text: String) -> ZeroizingBytes? {
        let digits = Array(text.utf8.filter { !($0 == 0x20 || $0 == 0x0A || $0 == 0x0D || $0 == 0x09) })
        guard digits.count % 2 == 0, (32...128).contains(digits.count) else { return nil }
        func value(_ c: UInt8) -> UInt8? {
            switch c {
            case 0x30...0x39: c - 0x30
            case 0x61...0x66: c - 0x61 + 10
            case 0x41...0x46: c - 0x41 + 10
            default: nil
            }
        }
        var out = [UInt8](repeating: 0, count: digits.count / 2)
        for index in out.indices {
            guard let high = value(digits[2 * index]), let low = value(digits[2 * index + 1]) else {
                for i in out.indices { out[i] = 0 }
                return nil
            }
            out[index] = high << 4 | low
        }
        return ZeroizingBytes(out)
    }
}

/// The credential a purpose needs, from an optional passphrase. `nil` when
/// the gate needs a passphrase that was not given.
@MainActor
func makeCredential(
    for purpose: GrantPurpose, passphrase: String?, auth: any AuthenticationGating, vault: any VaultProviding
) -> Credential? {
    switch auth.requirement(for: purpose) {
    case .none:
        return .unencrypted
    case .passphrase, .quickUnlockOrPassphrase:
        guard let passphrase, !passphrase.isEmpty else { return nil }
        return .passphrase(vault.makeSecret(utf8: passphrase))
    }
}

/// "Delete All" for the open network.
@MainActor
struct WalletWiper {
    let walletLifecycle: any WalletLifecycleManaging
    let lifecycle: any LifecycleQueueing
    let walletState: any WalletStateProviding
    let auth: any AuthenticationGating
    let vault: any VaultProviding
    let recovery: any VaultRecovering

    /// Removes every registered wallet (loading unloaded ones first, since the
    /// engine only removes loaded wallets), then destroys the vault.
    func wipeAll(passphrase: String?) async throws(ServiceError) -> VaultStatus {
        var ids: [WalletID]
        do {
            let states = try await walletLifecycle.loadStates()
            for state in states where !state.loaded { try await walletLifecycle.load(state.walletID) }
            ids = states.map(\.walletID)
        } catch {
            guard error.code == .notImplemented else { throw error }
            ids = (walletState.wallets ?? []).map(\.id)
        }
        for id in ids {
            guard let credential = makeCredential(for: .wipe, passphrase: passphrase, auth: auth, vault: vault) else {
                throw ServiceError(code: .vaultCredentialRequired)
            }
            let grant = try await auth.authorize(.wipe, wallet: id, credential: credential)
            try await lifecycle.removeWallet(id, grant: grant)
        }
        guard let credential = makeCredential(for: .wipe, passphrase: passphrase, auth: auth, vault: vault) else {
            throw ServiceError(code: .vaultCredentialRequired)
        }
        return try await recovery.destroy(credential: credential)
    }
}
