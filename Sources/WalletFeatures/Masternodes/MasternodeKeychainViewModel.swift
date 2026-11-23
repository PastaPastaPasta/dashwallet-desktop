// iOS's Masternode Keys tool (IOS-083): owner, voting, operator BLS and
// Platform Ed25519 keys with paths, addresses, public keys and node ids;
// private keys behind a `.revealSecret` grant. Masternode list, tracking and
// evonode tools stay in Dash Core (repo CLAUDE.md "Product scope").
import Foundation
import Observation
import PlatformServices
import WalletRuntime

/// The roles the keychain viewer pages through (payout keys are BIP44
/// addresses and live in Receive).
public let keychainRoles: [MasternodeKeyRole] = [.owner, .voting, .operator, .platformNode]

@MainActor
@Observable
public final class MasternodeKeychainViewModel {
    public static let pageSize: UInt32 = 20

    public private(set) var role: MasternodeKeyRole = .owner
    public private(set) var keys: [MasternodeKeyInfo] = []
    /// Revealed private keys by derivation path; dropped by `hide`.
    public private(set) var revealed: [String: RevealedMasternodeKey] = [:]
    /// The key waiting for the passphrase.
    public private(set) var pendingReveal: MasternodeKeyInfo?
    public private(set) var available = true
    public private(set) var errorMessage: String?

    public var roles: [MasternodeKeyRole] { keychainRoles }
    public var title: String { L10n.Masternodes.keychainTitle }

    private let keychain: any MasternodeKeychainProviding
    private let walletState: any WalletStateProviding
    private let grants: GrantRequester
    private let clipboard: any ClipboardProviding

    public init(
        keychain: any MasternodeKeychainProviding, walletState: any WalletStateProviding,
        auth: any AuthenticationGating, vault: any VaultProviding, clipboard: any ClipboardProviding
    ) {
        self.keychain = keychain
        self.walletState = walletState
        grants = GrantRequester(auth: auth, vault: vault)
        self.clipboard = clipboard
    }

    public convenience init(env: AppEnvironment, m2: M2Services, m3: M3Services) {
        self.init(
            keychain: m3.keychain, walletState: env.walletState, auth: env.auth, vault: env.vault,
            clipboard: m2.clipboard)
    }

    /// The first page of `role`'s keys.
    public func select(_ role: MasternodeKeyRole) async {
        self.role = role
        hideAll()
        keys = []
        await loadMore()
    }

    /// The next `pageSize` keys (the engine allows ≤ 100 per call).
    public func loadMore() async {
        guard let wallet = walletState.selectedWalletID else { return }
        let start = UInt32(keys.count)
        do {
            keys += try await keychain.keys(wallet: wallet, role: role, range: start..<(start + Self.pageSize))
            available = true
        } catch {
            if error.isNotImplemented { available = false } else { errorMessage = ErrorText.m3(error, amount: { "\($0.duffs)" }) }
        }
    }

    /// Reveals one private key behind a `.revealSecret` grant (always the
    /// passphrase on an encrypted vault).
    public func reveal(_ key: MasternodeKeyInfo, passphrase: String? = nil) async {
        guard let wallet = walletState.selectedWalletID else { return }
        errorMessage = nil
        do {
            guard let secret = try await grants.with(.revealSecret, wallet: wallet, passphrase: passphrase, {
                grant async throws(ServiceError) in
                try await self.keychain.reveal(wallet: wallet, role: key.role, index: key.index, grant: grant)
            }) else {
                pendingReveal = key
                return
            }
            pendingReveal = nil
            revealed[key.derivationPath] = secret
        } catch {
            errorMessage = ErrorText.m3(error, amount: { "\($0.duffs)" })
            if error.code != .vaultWrongPassphrase { pendingReveal = nil }
        }
    }

    public func cancelReveal() {
        pendingReveal = nil
    }

    public func hide(_ key: MasternodeKeyInfo) {
        revealed[key.derivationPath] = nil
    }

    /// Drops every revealed key (role switch, window close).
    public func hideAll() {
        revealed = [:]
    }

    public func copy(_ text: String) {
        clipboard.setString(text)
    }

    /// Copies a revealed secret (private key, WIF or Tenderdash key).
    public func copy(_ secret: any SecretBuffer) {
        clipboard.setString(secret.withUnsafeBytes { String(decoding: $0, as: UTF8.self) })
    }
}
