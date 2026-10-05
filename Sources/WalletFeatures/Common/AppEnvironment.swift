// Composition root inputs for the view models (DESIGN-opus §1.11).
import Foundation
import PlatformServices
import WalletRuntime

/// Waits for a duration; injected so countdowns and debounces are testable.
public typealias Sleeper = @Sendable (Duration) async throws -> Void

/// Time sources the view models use for countdowns, lockouts and date filters.
public struct Timing: Sendable {
    public var now: @Sendable () -> Date
    public var sleep: Sleeper
    public var calendar: Calendar
    public var timeZone: TimeZone

    public init(
        now: @escaping @Sendable () -> Date = { Date() },
        sleep: @escaping Sleeper = { try await Task.sleep(for: $0) },
        calendar: Calendar = .current,
        timeZone: TimeZone = .current
    ) {
        self.now = now
        self.sleep = sleep
        self.calendar = calendar
        self.timeZone = timeZone
    }
}

/// The service instances one app run uses, built once in each app's `@main`
/// and passed to view models. There are no singletons.
@MainActor
public struct AppEnvironment {
    public var host: any WalletHosting
    public var lifecycle: any LifecycleQueueing
    public var walletState: any WalletStateProviding
    public var sync: any SyncStatusProviding
    public var vault: any VaultProviding
    public var auth: any AuthenticationGating
    public var sender: any TransactionSending
    public var history: any HistoryProviding
    public var receive: any ReceiveProviding
    public var coinControl: any CoinControlProviding
    public var addressBook: any AddressBookProviding
    public var messages: any MessageSigning
    public var uri: any URIHandling
    public var amounts: any AmountFormatting
    public var settings: any SettingsProviding
    public var preferences: any UIPreferencesStoring
    /// `nil` where the OS offers no capture protection.
    public var screenCapture: (any ScreenCaptureGuard)?
    public var timing: Timing
    /// Shows regtest and devnet in network choices (Developer toggle).
    public var developerMode: Bool

    public init(
        host: any WalletHosting, lifecycle: any LifecycleQueueing, walletState: any WalletStateProviding,
        sync: any SyncStatusProviding, vault: any VaultProviding, auth: any AuthenticationGating,
        sender: any TransactionSending, history: any HistoryProviding, receive: any ReceiveProviding,
        coinControl: any CoinControlProviding, addressBook: any AddressBookProviding, messages: any MessageSigning,
        uri: any URIHandling, amounts: any AmountFormatting, settings: any SettingsProviding,
        preferences: any UIPreferencesStoring, screenCapture: (any ScreenCaptureGuard)? = nil,
        timing: Timing = Timing(), developerMode: Bool = false
    ) {
        self.host = host
        self.lifecycle = lifecycle
        self.walletState = walletState
        self.sync = sync
        self.vault = vault
        self.auth = auth
        self.sender = sender
        self.history = history
        self.receive = receive
        self.coinControl = coinControl
        self.addressBook = addressBook
        self.messages = messages
        self.uri = uri
        self.amounts = amounts
        self.settings = settings
        self.preferences = preferences
        self.screenCapture = screenCapture
        self.timing = timing
        self.developerMode = developerMode
    }
}
