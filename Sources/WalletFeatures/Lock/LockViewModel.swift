// Lock screen (QT-111, QT-112, IOS-012, IOS-013).
import Foundation
import Observation
import WalletRuntime

/// iOS passphrase lockout policy (IOS-012), applied as UX throttling only:
/// the real protection is the vault's Argon2id cost (DESIGN-opus §1.8).
/// Three failures are free; after `n ≥ 3` failures the next attempt waits
/// `6^(n−3) · 60 s` (1 min, 6 min, 36 min, 3.6 h, 21.6 h); at 8 failures the
/// lock screen stops accepting passphrases and offers a restore from the
/// recovery phrase instead of wiping.
public enum LockoutPolicy {
    public static let freeAttempts = 3
    public static let disabledAfter = 8

    public enum Verdict: Sendable, Hashable {
        case allowed
        case wait(seconds: UInt64)
        case disabled
    }

    public static func verdict(failures: Int) -> Verdict {
        if failures >= disabledAfter { return .disabled }
        if failures < freeAttempts { return .allowed }
        var seconds: UInt64 = 60
        for _ in 0..<(failures - freeAttempts) { seconds *= 6 }
        return .wait(seconds: seconds)
    }

    /// Attempts left before the policy disables passphrase entry.
    public static func attemptsRemaining(failures: Int) -> Int {
        max(0, disabledAfter - failures)
    }
}

@MainActor
@Observable
public final class LockViewModel {
    public private(set) var lockState: VaultLockState?
    public private(set) var failedAttempts = 0
    /// When the next attempt is accepted; `nil` when not throttled.
    public private(set) var retryDeadline: Date?
    /// Time left until `retryDeadline`, refreshed by `refreshCountdown()`.
    public private(set) var retryAfter: Duration?
    /// Eight failures: only "Restore with recovery phrase" is offered.
    public private(set) var isDisabled = false
    public private(set) var error: ServiceErrorCode?
    /// User-facing text for `error` or the throttle state.
    public private(set) var message: String?
    public private(set) var isWorking = false

    public var isLocked: Bool { lockState == .locked || lockState == .unlockedMixingOnly }

    private let auth: any AuthenticationGating
    private let vault: any VaultProviding
    private let timing: Timing
    private var observation: Task<Void, Never>?

    public init(auth: any AuthenticationGating, vault: any VaultProviding, timing: Timing) {
        self.auth = auth
        self.vault = vault
        self.timing = timing
        self.lockState = auth.lockState
    }

    /// Reads the vault's persisted failure count so a restart does not reset
    /// the throttle.
    public func load() async {
        lockState = auth.lockState
        do {
            let status = try await vault.status()
            failedAttempts = Int(status.failedAttempts)
            applyPolicy(engineRetryAfter: status.retryAfterSeconds)
        } catch {
            self.error = error.code
            message = ErrorText.common(error.code)
        }
    }

    /// Follows lock-state changes until `stop()`.
    public func start() {
        observation?.cancel()
        let stream = auth.lockStateChanges()
        observation = Task { [weak self] in
            for await state in stream {
                guard let self else { return }
                self.lockState = state
            }
        }
    }

    public func stop() {
        observation?.cancel()
        observation = nil
    }

    public func refreshCountdown() {
        guard let deadline = retryDeadline else {
            retryAfter = nil
            return
        }
        let left = deadline.timeIntervalSince(timing.now())
        if left <= 0 {
            retryDeadline = nil
            retryAfter = nil
            if message?.hasPrefix(L10n.Lock.tryAgainIn("")) == true { message = nil }
        } else {
            retryAfter = .seconds(left.rounded(.up))
        }
    }

    /// Unlocks fully, or for mixing only (QT-112). Refused while throttled
    /// or disabled.
    public func unlock(passphrase: String, mixingOnly: Bool) async {
        guard !isWorking else { return }
        refreshCountdown()
        if isDisabled {
            message = L10n.Lock.disabled
            return
        }
        if let retryAfter {
            message = L10n.Lock.tryAgainIn(Self.describe(retryAfter))
            return
        }
        guard !passphrase.isEmpty else {
            message = L10n.Common.passphraseRequired
            return
        }
        isWorking = true
        defer { isWorking = false }
        let secret = vault.makeSecret(utf8: passphrase)
        do {
            try await auth.unlock(passphrase: secret, scope: mixingOnly ? .mixingOnly : .full)
            failedAttempts = 0
            retryDeadline = nil
            retryAfter = nil
            error = nil
            message = nil
            lockState = auth.lockState
        } catch {
            self.error = error.code
            switch error.code {
            case .vaultWrongPassphrase:
                failedAttempts += 1
                applyPolicy(engineRetryAfter: error.retryAfterSeconds)
                if !isDisabled && retryDeadline == nil {
                    message = L10n.Common.wrongPassphrase + " "
                        + L10n.Lock.attemptsRemaining(LockoutPolicy.attemptsRemaining(failures: failedAttempts))
                }
            case .vaultThrottled:
                applyPolicy(engineRetryAfter: error.retryAfterSeconds)
            default:
                message = ErrorText.common(error.code)
            }
        }
    }

    public func lock() async {
        do {
            try await auth.lock()
            lockState = auth.lockState
        } catch {
            self.error = error.code
            message = ErrorText.common(error.code)
        }
    }

    /// Sets the deadline from the local policy, or the engine's wait when longer.
    private func applyPolicy(engineRetryAfter: UInt64?) {
        let now = timing.now()
        var waitSeconds: UInt64?
        switch LockoutPolicy.verdict(failures: failedAttempts) {
        case .allowed:
            isDisabled = false
        case .wait(let seconds):
            isDisabled = false
            waitSeconds = seconds
        case .disabled:
            isDisabled = true
            retryDeadline = nil
            retryAfter = nil
            message = L10n.Lock.disabled
            return
        }
        if let engineRetryAfter, engineRetryAfter > (waitSeconds ?? 0) {
            waitSeconds = engineRetryAfter
        }
        if let waitSeconds, waitSeconds > 0 {
            retryDeadline = now.addingTimeInterval(TimeInterval(waitSeconds))
            refreshCountdown()
            if let retryAfter { message = L10n.Lock.tryAgainIn(Self.describe(retryAfter)) }
        } else {
            retryDeadline = nil
            retryAfter = nil
        }
    }

    /// "6 minutes", "4 hours": minutes below two hours, rounded up.
    static func describe(_ duration: Duration) -> String {
        let seconds = duration.components.seconds
        let minutes = Int((seconds + 59) / 60)
        if minutes < 120 { return L10n.Lock.minutes(max(1, minutes)) }
        return L10n.Lock.hours(Int((seconds + 3599) / 3600))
    }
}
