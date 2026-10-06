// The single authorization primitive (iOS `AuthenticationGate`, DESIGN-opus §1.8).
import DashKit
import Foundation
import Observation

/// Owns the open network's vault lock state and issues engine grants.
///
/// - `lockState` is `nil` until the first `Vault.status()` of a session; it
///   follows the engine's `LockState` events and every status a vault call
///   returns (`apply(_:)`).
/// - `authorize` runs under a watchdog: if the vault has not answered after
///   `authorizeTimeout`, it fails with `auth.timed_out`, and a grant that
///   arrives later is revoked so nobody can use it.
/// - `requirement(for:)` decides what the UI asks for (see its doc).
@MainActor
@Observable
public final class AuthenticationGate: AuthenticationGating, SessionObserving {
    public static let defaultAuthorizeTimeout: Duration = .seconds(60)

    public private(set) var lockState: VaultLockState?
    /// The last vault status read; `nil` while unknown.
    public private(set) var vaultStatus: VaultStatus?
    /// The last failure to read the vault status; cleared by the next success.
    public private(set) var lastError: ServiceError?

    @ObservationIgnored private let engine: any EngineProtocol
    @ObservationIgnored private let clock: any RuntimeClock
    @ObservationIgnored private let authorizeTimeout: Duration
    @ObservationIgnored private let requireAuthenticationForEveryPayment: @MainActor () -> Bool
    @ObservationIgnored private var network: DashKit.DashNetwork?
    @ObservationIgnored private let broadcaster = StateBroadcaster<VaultLockState>()
    @ObservationIgnored private let tasks = TaskBag()
    @ObservationIgnored private var coalescer: Coalescer!

    /// - Parameter requireAuthenticationForEveryPayment: the iOS setting
    ///   (default on). Off gives dash-qt behaviour: an unlocked vault spends
    ///   and signs without asking again.
    public init(
        engine: any EngineProtocol,
        clock: any RuntimeClock = SystemClock(),
        authorizeTimeout: Duration = AuthenticationGate.defaultAuthorizeTimeout,
        requireAuthenticationForEveryPayment: @escaping @MainActor () -> Bool = { true }
    ) {
        self.engine = engine
        self.clock = clock
        self.authorizeTimeout = authorizeTimeout
        self.requireAuthenticationForEveryPayment = requireAuthenticationForEveryPayment
        coalescer = Coalescer { [weak self] in await self?.refresh() }
    }

    public func lockStateChanges() -> AsyncStream<VaultLockState> {
        broadcaster.stream()
    }

    /// What the UI must collect before `authorize(purpose, …)`:
    /// - no vault, no keys, or an unencrypted vault: nothing (`.unencrypted`
    ///   credential); an unencrypted vault has no passphrase to ask for;
    /// - locked or unlocked for mixing only: the passphrase;
    /// - unlocked: the passphrase for reveal, credential change and wipe
    ///   (dw-vault `Vault::authorize` refuses these without it whenever the
    ///   vault has a passphrase slot, unlocked or not); for spending, signing
    ///   and the M3/M4 operations only while "require authentication for every
    ///   payment" is on;
    /// - unknown state: the passphrase.
    /// `.quickUnlockOrPassphrase` replaces `.passphrase` once quick unlock is
    /// enrolled (M2).
    public func requirement(for purpose: GrantPurpose) -> CredentialRequirement {
        let ask: CredentialRequirement = vaultStatus?.quickUnlockEnrolled == true ? .quickUnlockOrPassphrase : .passphrase
        switch lockState {
        case .noVault, .noKeys, .unencrypted:
            return .none
        case .locked, .unlockedMixingOnly, nil:
            return ask
        case .unlocked:
            switch purpose {
            case .revealSecret, .changeCredential, .wipe:
                return ask
            case .spend, .signMessage, .masternodeOperation, .governance, .platformOperation:
                return requireAuthenticationForEveryPayment() ? ask : .none
            }
        }
    }

    public func authorize(_ purpose: GrantPurpose, credential: Credential) async throws(ServiceError) -> AuthGrant {
        let network = try requireNetwork()
        let engine = engine
        let kitPurpose = purpose.kit
        let kitCredential = credential.kit
        let result = await withWatchdog(
            timeout: authorizeTimeout, clock: clock,
            operation: { () async -> Result<DashKit.AuthGrant, DashKitError> in
                do throws(DashKitError) {
                    return .success(try await engine.authorize(on: network, purpose: kitPurpose, credential: kitCredential))
                } catch {
                    return .failure(error)
                }
            },
            onLateResult: { late in
                if case .success(let grant) = late {
                    try? await engine.revokeGrant(on: network, grantID: grant.id)
                }
            })
        // A passphrase may have unlocked the vault, or a failed attempt
        // changed the throttle; re-read either way.
        coalescer.request()
        switch result {
        case nil:
            throw ServiceError(code: .authTimedOut, detail: "the vault did not answer within \(authorizeTimeout)")
        case .success(let grant)?:
            return AuthGrant(grant)
        case .failure(let error)?:
            throw ServiceError(error)
        }
    }

    /// Withdraws an unused grant on the open network (engine
    /// `Vault.revoke_grant`). Runs in the background; without an open network,
    /// or for an id the engine no longer knows, there is nothing to withdraw.
    public func revoke(_ grant: AuthGrant) {
        guard let network else { return }
        let engine = engine
        let grantID = grant.id
        Task { try? await engine.revokeGrant(on: network, grantID: grantID) }
    }

    public func unlock(passphrase: any SecretBuffer, scope: UnlockScope) async throws(ServiceError) {
        let network = try requireNetwork()
        let engine = engine
        let secret = secretBytes(passphrase)
        let kitScope = scope.kit
        do {
            let status = try await serviceCall { () async throws(DashKitError) in
                try await engine.unlockVault(on: network, passphrase: secret, scope: kitScope)
            }
            apply(VaultStatus(status))
        } catch {
            // Failed attempts change `failedAttempts` / `retryAfterSeconds`.
            coalescer.request()
            throw error
        }
    }

    public func lock() async throws(ServiceError) {
        let network = try requireNetwork()
        let engine = engine
        let status = try await serviceCall { () async throws(DashKitError) in try await engine.lockVault(on: network) }
        apply(VaultStatus(status))
    }

    /// Takes a status a vault call returned (create, encrypt, change
    /// passphrase) so the lock state is current without waiting for the event.
    public func apply(_ status: VaultStatus) {
        guard network != nil else { return }
        lastError = nil
        vaultStatus = status
        if lockState != status.state {
            lockState = status.state
            broadcaster.send(status.state)
        }
    }

    /// Returns once every requested status refresh has run (tests).
    public func settle() async {
        await coalescer.idle()
    }

    // MARK: SessionObserving

    public func sessionDidStart(_ network: DashKit.DashNetwork) async {
        self.network = network
        clearState()
        startEventPump()
        coalescer.request()
        await coalescer.idle()
    }

    public func sessionWillStop(_ network: DashKit.DashNetwork) async {
        self.network = nil
        tasks.cancelAll()
        coalescer.cancel()
        clearState()
    }

    public func walletsDidChange(_ network: DashKit.DashNetwork) async {
        // Importing or removing a wallet changes `walletsWithSecrets` and
        // can move the vault between `noKeys` and `unencrypted`.
        coalescer.request()
        await coalescer.idle()
    }

    // MARK: Private

    private func clearState() {
        lockState = nil
        vaultStatus = nil
        lastError = nil
        broadcaster.clear()
    }

    private func requireNetwork() throws(ServiceError) -> DashKit.DashNetwork {
        guard let network else { throw ServiceError(code: .networkNotOpen, detail: "no network is open") }
        return network
    }

    private func startEventPump() {
        let subscription = engine.events.subscribe()
        tasks.set("events", Task { [weak self] in
            for await event in subscription {
                guard let self else { return }
                self.handle(event)
            }
        })
    }

    private func handle(_ event: EngineEvent) {
        guard let network else { return }
        switch event {
        case .lockStateChanged(let n) where n == network:
            coalescer.request()
        case .walletCreated(let n, _) where n == network, .walletRemoved(let n, _) where n == network:
            coalescer.request()
        case .resynchronize:
            coalescer.request()
        default:
            break
        }
    }

    private func refresh() async {
        guard let network else { return }
        let status: DashKit.VaultStatus
        do {
            status = try await engine.vaultStatus(on: network)
        } catch {
            if self.network == network { lastError = ServiceError(error) }
            return
        }
        guard self.network == network else { return }
        apply(VaultStatus(status))
    }
}
