// The M2 view models of one macOS app run (docs/contracts/m2-swift.md §3):
// built once over the environment and the M2 services by the composition
// root, then shared by the menus, the Dock menu, the menu bar companion and
// every M2 window. No singletons: the app's `MacAppModel` owns one.
#if os(macOS)
import Foundation
import WalletFeatures
import WalletRuntime

@MainActor
public final class MacFeatureModels {
    public let m2: M2Services
    public let shell: ShellModel
    public let startup: StartupViewModel
    public let shutdown: ShutdownViewModel
    public let options: OptionsViewModel
    public let psbt: PSBTViewModel
    public let information: InformationViewModel
    public let console: ConsoleViewModel
    public let peers: PeersViewModel
    public let repair: RepairViewModel
    public let wallets: WalletManagementViewModel
    public let security: SecurityViewModel
    public let about: AboutViewModel
    public let backupReminder: BackupReminderViewModel
    public let companion: MenuBarCompanionViewModel

    private let env: AppEnvironment
    /// One shortcut bar per network (its defaults depend on it, IOS-025).
    private var shortcutBars: [DashNetwork: ShortcutBarViewModel] = [:]

    public init(env: AppEnvironment, m2: M2Services) {
        self.env = env
        self.m2 = m2
        shell = ShellModel(env: env, m2: m2)
        startup = StartupViewModel(m2: m2)
        shutdown = ShutdownViewModel(coordinator: m2.shutdown)
        options = OptionsViewModel(env: env, m2: m2)
        psbt = PSBTViewModel(env: env, m2: m2)
        information = InformationViewModel(env: env, m2: m2)
        console = ConsoleViewModel(env: env, m2: m2)
        peers = PeersViewModel(sync: env.sync, moderation: m2.peerModeration)
        repair = RepairViewModel(env: env, m2: m2)
        wallets = WalletManagementViewModel(env: env, m2: m2)
        security = SecurityViewModel(env: env, m2: m2)
        about = AboutViewModel(env: env, m2: m2)
        backupReminder = BackupReminderViewModel(env: env, m2: m2)
        companion = MenuBarCompanionViewModel(env: env, m2: m2)
    }

    /// The shortcut bar of `network`, built on first use.
    public func shortcutBar(for network: DashNetwork) -> ShortcutBarViewModel {
        if let bar = shortcutBars[network] { return bar }
        let bar = ShortcutBarViewModel(env: env, m2: m2, network: network)
        shortcutBars[network] = bar
        return bar
    }

    /// Follows the shell state, startup phases and the backup reminder.
    func start() async {
        startup.start()
        await shell.start()
        backupReminder.start()
    }

    func stop() {
        shell.stop()
        startup.stop()
        backupReminder.stop()
        information.stop()
        repair.stop()
        wallets.stop()
        for bar in shortcutBars.values { bar.stop() }
    }
}
#endif
