// Auto-lock (IOS-015): locks the vault after inactivity, on system sleep
// and on screen lock. dash-qt never auto-locks, so the default is `.never`.
import Foundation
import Observation
import PlatformServices

/// `AutoLockControlling` over a lock action, an inactivity timer and the
/// OS idle events.
///
/// - `.never`: nothing locks the vault.
/// - `.immediately`: locks on every OS idle event (sleep, screen lock, user
///   idle) and when the app goes to the background (`noteBackgrounded()`);
///   there is no in-app timer.
/// - other intervals: locks when no `noteActivity()` arrived for the
///   interval, when the OS reports the user idle that long, and on sleep or
///   screen lock.
///
/// The interval is kept in `settings.json` (section `autoLock`). Locking an
/// already locked or unencrypted vault is harmless (`Vault.lock` is
/// idempotent); a failure is kept in `lastError`. CoinJoin mixing (M3)
/// will lock to "mixing only" instead; until then it locks fully.
@MainActor
@Observable
public final class AutoLockController: AutoLockControlling {
    public static let settingsSection = "autoLock"

    public private(set) var interval: AutoLockInterval
    /// The last failure of the lock action.
    public private(set) var lastError: ServiceError?
    /// How many times the controller locked the vault (tests, diagnostics).
    public private(set) var lockCount = 0
    /// How many OS idle events it has handled (tests, diagnostics).
    public private(set) var handledIdleEvents = 0

    @ObservationIgnored private let settings: SettingsStore
    @ObservationIgnored private let clock: any RuntimeClock
    @ObservationIgnored private let lockAction: @MainActor () async throws(ServiceError) -> Void
    @ObservationIgnored private let tasks = TaskBag()
    @ObservationIgnored private var lastActivity: Duration
    /// The timer locked since the last activity; it waits for activity
    /// before it locks again.
    @ObservationIgnored private var timerLocked = false

    private struct Stored: Codable {
        var interval: AutoLockInterval
    }

    /// - Parameters:
    ///   - lock: locks the vault of the open network (`AuthenticationGate.lock`).
    ///   - idle: OS sleep / screen-lock / idle events; `nil` where none exist.
    public init(
        settings: SettingsStore, idle: (any IdleMonitoring)?, clock: any RuntimeClock = SystemClock(),
        lock: @escaping @MainActor () async throws(ServiceError) -> Void
    ) {
        self.settings = settings
        self.clock = clock
        lockAction = lock
        interval = settings.section(Self.settingsSection, as: Stored.self)?.interval ?? .never
        lastActivity = clock.now()
        if let idle {
            let events = idle.events()
            tasks.set("idle", Task { [weak self] in
                for await event in events {
                    guard let self else { return }
                    await self.handle(event)
                }
            })
        }
        restartTimer()
    }

    public func setInterval(_ interval: AutoLockInterval) throws(ServiceError) {
        guard interval != self.interval else { return }
        try settings.setSection(Self.settingsSection, Stored(interval: interval))
        self.interval = interval
        lastActivity = clock.now()
        timerLocked = false
        restartTimer()
    }

    public func noteActivity() {
        lastActivity = clock.now()
        timerLocked = false
    }

    /// The app moved to the background (all windows closed or hidden).
    public func noteBackgrounded() async {
        if interval == .immediately { await lockNow() }
    }

    /// Stops the timer and the idle subscription.
    public func stop() {
        tasks.cancelAll()
    }

    private func handle(_ event: IdleEvent) async {
        defer { handledIdleEvents += 1 }
        switch (interval, event) {
        case (.never, _):
            return
        case (_, .systemWillSleep), (_, .screenLocked):
            await lockNow()
        case (.immediately, .userIdle):
            await lockNow()
        case (let interval, .userIdle(let seconds)):
            if let limit = interval.duration, Duration.seconds(seconds) >= limit {
                await lockNow()
            }
        }
    }

    /// Wakes when the interval since the last activity has passed; activity
    /// in between moves the deadline.
    private func restartTimer() {
        guard let limit = interval.duration, interval != .immediately else {
            tasks.set("timer", nil)
            return
        }
        let clock = clock
        tasks.set("timer", Task { [weak self] in
            // `self` is held only inside `timerStep`, never across the sleep,
            // so the controller can be released while the timer waits.
            while !Task.isCancelled {
                guard let wait = await self?.timerStep(limit: limit) else { return }
                do { try await clock.sleep(for: wait) } catch { return }
            }
        })
    }

    /// Locks when the interval since the last activity has passed (once per
    /// idle period) and returns how long to wait before checking again.
    private func timerStep(limit: Duration) async -> Duration {
        let remaining = limit - (clock.now() - lastActivity)
        guard remaining <= .zero else { return remaining }
        if !timerLocked {
            timerLocked = true
            await lockNow()
        }
        // Locked: check again one interval later for activity.
        return limit
    }

    /// Locks the open network's vault. Without an open network there is
    /// nothing to lock, which is not an error.
    private func lockNow() async {
        do {
            try await lockAction()
            lockCount += 1
            lastError = nil
        } catch where error.code == .networkNotOpen {
            lastError = nil
        } catch {
            lastError = error
        }
    }
}
