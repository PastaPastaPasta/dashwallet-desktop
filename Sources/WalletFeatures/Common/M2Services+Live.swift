// The live `M2Services`: every M2 protocol backed by the engine adapters
// and OS services of one `DesktopRuntimeServices` (docs/contracts/m2-swift.md
// §1). The demo counterpart is `WalletDemo.DemoEnvironment.makeWithM2`.
import Foundation
import PlatformServices
import WalletRuntime

extension DustProtectionService: DustProtectionControlling {}

extension SettingsStore: OptionsResetting {
    /// Options ▸ Reset Options and the corrupt-settings Reset (QT-007, QT-141).
    public func resetOptions() throws(ServiceError) -> [URL] {
        try resetToDefaults()
    }
}

extension M2Services {
    /// The M2 services of a live app run.
    ///
    /// - Parameters:
    ///   - desktop: the runtime's M2 composition (engine adapters and OS services).
    ///   - launchOptions: the parsed command line of this run.
    ///   - clipboard: the app's clipboard when `desktop.platform` has none
    ///     (for example the toolkit's on Windows). One of the two is required:
    ///     `desktop.unsupported` otherwise.
    public static func live(
        desktop: DesktopRuntimeServices, launchOptions: LaunchOptions, clipboard: (any ClipboardProviding)? = nil
    ) throws(ServiceError) -> M2Services {
        guard let clipboard = clipboard ?? desktop.platform.clipboard else {
            throw ServiceError(code: .desktopUnsupported, detail: "no clipboard service")
        }
        let runtime = desktop.runtime
        let os = desktop.platform
        return M2Services(
            walletLifecycle: desktop.walletLifecycle, transactionActions: desktop.transactionActions,
            fees: desktop.fees, fileImporter: desktop.fileImporter, coreExporter: desktop.coreExporter,
            backups: desktop.backups, psbt: desktop.psbt, nodeInformation: desktop.nodeInformation,
            peerModeration: desktop.peerModeration, repair: desktop.repair, console: desktop.console,
            logs: desktop.logs, quickUnlock: desktop.quickUnlock, autoLock: desktop.autoLock,
            vaultRecovery: desktop.vaultRecovery, startup: desktop.startup, shutdown: desktop.shutdown,
            shellSettings: desktop.shellSettings, launchArguments: desktop.launchArguments,
            launchOptions: launchOptions,
            desktopPreferences: SettingsDesktopPreferencesStore(settings: runtime.settings),
            dustProtection: desktop.dustProtection, optionsReset: runtime.settings,
            paymentAuthentication: runtime.settings, launchAtLogin: os.launchAtLogin,
            notifications: os.notifications, clipboard: clipboard, fileRevealer: os.fileRevealer,
            dataDirectories: os.dataDirectories)
    }
}
