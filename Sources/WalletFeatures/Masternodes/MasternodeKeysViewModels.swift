// iOS's masternode tools: the keychain viewer (IOS-083: owner, voting,
// operator BLS and Platform Ed25519 keys with paths, addresses, public keys,
// node ids and where they are used; private keys behind a `.revealSecret`
// grant) and tracked masternodes (IOS-082: find any masternode by IP, hash,
// address or key, track it, attach its keys to the vault, and act with them:
// unban, vote, evonode credit withdrawal, which is Platform work and says
// "Not available yet" until the engine offers it).
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

    public func usageText(_ key: MasternodeKeyInfo) -> String {
        guard !key.usedBy.isEmpty else { return L10n.Masternodes.unused }
        return key.usedBy.map { use in
            L10n.Masternodes.usedBy(use.proTxHash, service: use.service) + (use.revoked ? L10n.Masternodes.revokedSuffix : "")
        }.joined(separator: "\n")
    }

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

@MainActor
@Observable
public final class TrackedMasternodesViewModel {
    public var locateQuery = ""
    public private(set) var results: [MasternodeRow] = []
    public private(set) var tracked: [TrackedMasternode] = []
    public private(set) var available = true
    public private(set) var needsPassphrase = false
    public private(set) var errorMessage: String?
    public private(set) var message: String?
    /// A revealed attached key, by "proTxHash/role".
    public private(set) var revealed: [String: RevealedMasternodeKey] = [:]

    public var noMatchesText: String? {
        results.isEmpty && !locateQuery.isEmpty && searched ? L10n.Masternodes.noMatches : nil
    }

    public func capabilityLines(_ node: TrackedMasternode) -> [String] {
        typealias M = L10n.Masternodes
        var lines: [String] = []
        if node.capabilities.canUpdateService { lines.append(M.capabilityUpdateService) }
        if node.capabilities.canUpdateRegistrar { lines.append(M.capabilityUpdateRegistrar) }
        if node.capabilities.canVote { lines.append(M.capabilityVote) }
        if node.capabilities.canWithdraw { lines.append(M.capabilityWithdraw) }
        return lines
    }

    private let manager: any TrackedMasternodeManaging
    private let evonodes: any EvonodeServicing
    private let masternodes: any MasternodeListProviding
    private let walletState: any WalletStateProviding
    private let vault: any VaultProviding
    private let grants: GrantRequester
    private var searched = false
    private var task: Task<Void, Never>?

    public init(
        manager: any TrackedMasternodeManaging, evonodes: any EvonodeServicing, masternodes: any MasternodeListProviding,
        walletState: any WalletStateProviding, auth: any AuthenticationGating, vault: any VaultProviding
    ) {
        self.manager = manager
        self.evonodes = evonodes
        self.masternodes = masternodes
        self.walletState = walletState
        self.vault = vault
        grants = GrantRequester(auth: auth, vault: vault)
    }

    public convenience init(env: AppEnvironment, m3: M3Services) {
        self.init(
            manager: m3.tracked, evonodes: m3.evonodes, masternodes: m3.masternodes, walletState: env.walletState,
            auth: env.auth, vault: env.vault)
    }

    private func errorText(_ error: ServiceError) -> String { ErrorText.m3(error, amount: { "\($0.duffs)" }) }

    public func start() async {
        task?.cancel()
        await reload()
        let changes = masternodes.changes()
        task = Task { [weak self] in
            for await _ in changes { await self?.reload() }
        }
    }

    public func stop() {
        task?.cancel()
        task = nil
        revealed = [:]
    }

    public func reload() async {
        do {
            tracked = try await manager.tracked()
            available = true
        } catch {
            if error.isNotImplemented { available = false } else { errorMessage = errorText(error) }
        }
    }

    /// Finds masternodes by IP, `IP:port`, proTxHash, address or operator key.
    public func locate() async {
        let query = locateQuery.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return }
        errorMessage = nil
        do {
            results = try await manager.locate(query)
            searched = true
        } catch {
            if error.isNotImplemented { available = false } else { errorMessage = errorText(error) }
        }
    }

    public func track(_ proTxHash: String, label: String? = nil) async {
        await run { () async throws(ServiceError) in
            _ = try await self.manager.track(proTxHash: proTxHash, label: label?.isEmpty == true ? nil : label)
            await self.reload()
        }
    }

    /// Stops tracking; the engine deletes the attached keys from the vault.
    public func untrack(_ proTxHash: String) async {
        await run { () async throws(ServiceError) in
            _ = try await self.manager.untrack(proTxHash: proTxHash)
            self.revealed = self.revealed.filter { !$0.key.hasPrefix(proTxHash + "/") }
            await self.reload()
        }
    }

    public func setLabel(_ label: String, for proTxHash: String) async {
        await run { () async throws(ServiceError) in
            try await self.manager.setLabel(label.isEmpty ? nil : label, proTxHash: proTxHash)
            await self.reload()
        }
    }

    /// Stores a key for `role` in the vault (`.masternodeOperation` grant).
    /// The typed text becomes a zeroing buffer at once.
    public func attach(keyText: String, role: MasternodeKeyRole, proTxHash: String, passphrase: String? = nil) async {
        let trimmed = keyText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let wallet = walletState.selectedWalletID else { return }
        let key = vault.makeSecret(utf8: trimmed)
        await authorized(.masternodeOperation, wallet: wallet, passphrase: passphrase) { grant async throws(ServiceError) in
            try await self.manager.attach(key, role: role, proTxHash: proTxHash, grant: grant)
            await self.reload()
        }
    }

    public func detach(role: MasternodeKeyRole, proTxHash: String) async {
        await run { () async throws(ServiceError) in
            try await self.manager.detach(role: role, proTxHash: proTxHash)
            self.revealed["\(proTxHash)/\(role)"] = nil
            await self.reload()
        }
    }

    public func reveal(role: MasternodeKeyRole, proTxHash: String, passphrase: String? = nil) async {
        guard let wallet = walletState.selectedWalletID else { return }
        await authorized(.revealSecret, wallet: wallet, passphrase: passphrase) { grant async throws(ServiceError) in
            self.revealed["\(proTxHash)/\(role)"] = try await self.manager.reveal(
                role: role, proTxHash: proTxHash, grant: grant)
        }
    }

    public func hideRevealed() {
        revealed = [:]
    }

    /// Evonode credit withdrawal (IOS-081/082). Platform work: "Not
    /// available yet" while the engine answers `not_implemented`.
    public func withdraw(
        proTxHash: String, credits: UInt64, destination: CreditWithdrawalDestination, passphrase: String? = nil
    ) async {
        guard let wallet = walletState.selectedWalletID else { return }
        await authorized(.masternodeOperation, wallet: wallet, passphrase: passphrase) { grant async throws(ServiceError) in
            let id = try await self.evonodes.withdraw(
                proTxHash: proTxHash, credits: credits, destination: destination, grant: grant)
            self.message = L10n.Masternodes.withdrawn(id)
        }
    }

    public func cancelPassphrase() {
        needsPassphrase = false
    }

    private func run(_ body: () async throws(ServiceError) -> Void) async {
        errorMessage = nil
        message = nil
        do {
            try await body()
        } catch {
            errorMessage = error.isNotImplemented ? L10n.Masternodes.notAvailableYet : errorText(error)
        }
    }

    private func authorized(
        _ purpose: GrantPurpose, wallet: WalletID, passphrase: String?,
        _ body: (AuthGrant) async throws(ServiceError) -> Void
    ) async {
        await run { () async throws(ServiceError) in
            let done: Void? = try await self.grants.with(purpose, wallet: wallet, passphrase: passphrase, body)
            self.needsPassphrase = done == nil
        }
    }
}
