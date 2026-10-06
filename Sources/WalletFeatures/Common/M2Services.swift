// The M2 service instances the V1 view models use (docs/contracts/m2-swift.md
// §2), built once next to `AppEnvironment` by each app's composition root
// (live adapters: S1; demo: `WalletDemo.DemoEnvironment.makeWithM2`). There
// are no singletons.
import Foundation
import PlatformServices
import WalletRuntime

@MainActor
public struct M2Services {
    public var walletLifecycle: any WalletLifecycleManaging
    public var transactionActions: any TransactionActing
    public var fees: any FeeAndCoinSelectionProviding
    public var fileImporter: any WalletFileImporting
    public var coreExporter: any CoreExporting
    public var backups: any BackupProviding
    public var psbt: any PSBTHandling
    public var nodeInformation: any NodeInformationProviding
    public var peerModeration: any PeerModerating
    public var repair: any RepairProviding
    public var console: any ConsoleExecuting
    public var logs: any LogExporting
    public var quickUnlock: any QuickUnlockManaging
    public var autoLock: any AutoLockControlling
    public var vaultRecovery: any VaultRecovering
    public var startup: any StartupProgressing
    public var shutdown: any ShutdownCoordinating
    public var shellSettings: any ShellSettingsProviding
    public var launchArguments: any LaunchArgumentsParsing
    public var launchOptions: LaunchOptions
    public var desktopPreferences: any DesktopPreferencesStoring
    public var dustProtection: any DustProtectionControlling
    public var optionsReset: any OptionsResetting
    public var paymentAuthentication: any PaymentAuthenticationSetting
    public var launchAtLogin: any LaunchAtLoginManaging
    public var notifications: any SystemNotifying
    public var clipboard: any ClipboardProviding
    public var fileRevealer: any FileRevealing
    public var dataDirectories: any DataDirectoryInspecting
    public var platform: DesktopPlatform

    public init(
        walletLifecycle: any WalletLifecycleManaging, transactionActions: any TransactionActing,
        fees: any FeeAndCoinSelectionProviding, fileImporter: any WalletFileImporting,
        coreExporter: any CoreExporting, backups: any BackupProviding, psbt: any PSBTHandling,
        nodeInformation: any NodeInformationProviding, peerModeration: any PeerModerating,
        repair: any RepairProviding, console: any ConsoleExecuting, logs: any LogExporting,
        quickUnlock: any QuickUnlockManaging, autoLock: any AutoLockControlling, vaultRecovery: any VaultRecovering,
        startup: any StartupProgressing, shutdown: any ShutdownCoordinating,
        shellSettings: any ShellSettingsProviding, launchArguments: any LaunchArgumentsParsing,
        launchOptions: LaunchOptions, desktopPreferences: any DesktopPreferencesStoring,
        dustProtection: any DustProtectionControlling, optionsReset: any OptionsResetting,
        paymentAuthentication: any PaymentAuthenticationSetting, launchAtLogin: any LaunchAtLoginManaging,
        notifications: any SystemNotifying, clipboard: any ClipboardProviding, fileRevealer: any FileRevealing,
        dataDirectories: any DataDirectoryInspecting, platform: DesktopPlatform = .current
    ) {
        self.walletLifecycle = walletLifecycle
        self.transactionActions = transactionActions
        self.fees = fees
        self.fileImporter = fileImporter
        self.coreExporter = coreExporter
        self.backups = backups
        self.psbt = psbt
        self.nodeInformation = nodeInformation
        self.peerModeration = peerModeration
        self.repair = repair
        self.console = console
        self.logs = logs
        self.quickUnlock = quickUnlock
        self.autoLock = autoLock
        self.vaultRecovery = vaultRecovery
        self.startup = startup
        self.shutdown = shutdown
        self.shellSettings = shellSettings
        self.launchArguments = launchArguments
        self.launchOptions = launchOptions
        self.desktopPreferences = desktopPreferences
        self.dustProtection = dustProtection
        self.optionsReset = optionsReset
        self.paymentAuthentication = paymentAuthentication
        self.launchAtLogin = launchAtLogin
        self.notifications = notifications
        self.clipboard = clipboard
        self.fileRevealer = fileRevealer
        self.dataDirectories = dataDirectories
        self.platform = platform
    }
}

extension ServiceError {
    /// A `PlatformServiceError` with the same code (m2-swift.md §2.7).
    init(_ error: PlatformServiceError) {
        self.init(code: ServiceErrorCode(rawValue: error.code), detail: error.detail)
    }
}
