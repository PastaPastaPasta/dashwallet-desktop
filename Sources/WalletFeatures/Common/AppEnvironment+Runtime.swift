// The live composition: an `AppEnvironment` over WalletRuntime's adapters.
import Foundation
import Observation
import PlatformServices
import WalletRuntime

/// `UIPreferencesStoring` kept as the `"ui"` section of the runtime's
/// `settings.json`, next to the display settings.
@MainActor
@Observable
public final class SettingsUIPreferencesStore: UIPreferencesStoring {
    public static let sectionKey = "ui"

    public private(set) var preferences: UIPreferences
    @ObservationIgnored private let settings: SettingsStore

    public init(settings: SettingsStore) {
        self.settings = settings
        preferences = settings.section(Self.sectionKey, as: UIPreferences.self) ?? UIPreferences()
    }

    public func update(_ preferences: UIPreferences) throws(ServiceError) {
        try settings.setSection(Self.sectionKey, preferences)
        self.preferences = preferences
    }
}

extension AppEnvironment {
    /// Every service from `runtime`; UI preferences live in its settings file.
    ///
    /// An app's `@main` builds it like this:
    /// ```swift
    /// let runtime = try WalletRuntimeServices.live(dataRoot: root)
    /// let env = AppEnvironment(runtime: runtime)
    /// Task { try await runtime.launch() }        // opens the last network
    /// // …on quit: try await runtime.shutdown()
    /// ```
    public init(
        runtime: WalletRuntimeServices,
        screenCapture: (any ScreenCaptureGuard)? = nil,
        timing: Timing = Timing(),
        developerMode: Bool = false
    ) {
        self.init(
            host: runtime.host, lifecycle: runtime.lifecycle, walletState: runtime.walletState, sync: runtime.sync,
            vault: runtime.vault, auth: runtime.auth, sender: runtime.sender, history: runtime.history,
            receive: runtime.receive, coinControl: runtime.coinControl, addressBook: runtime.addressBook,
            messages: runtime.messages, uri: runtime.uri, amounts: runtime.amounts, settings: runtime.settings,
            preferences: SettingsUIPreferencesStore(settings: runtime.settings), screenCapture: screenCapture,
            timing: timing, developerMode: developerMode)
    }
}
