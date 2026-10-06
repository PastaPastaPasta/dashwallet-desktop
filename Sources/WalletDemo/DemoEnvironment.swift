// Demo mode for both apps (`--demo`): every WalletRuntime service protocol
// over one in-memory `DemoWorld`. Amounts, `dash:` URIs, address checks, QR
// codes, signature verification and mnemonic checks are the engine's pure
// functions (`EngineFunctions`); vault, grant, send and address-book rules
// follow docs/contracts/m1-engine.md, so the screens meet the same error
// codes they meet on the engine. Nothing is sent and nothing is stored.
import Foundation
import PlatformServices
import WalletFeatures
import WalletRuntime

/// How demo mode starts (`--demo <scenario>` / `--demo-scenario`).
public enum DemoScenario: String, Sendable, CaseIterable {
    /// One funded wallet, unencrypted vault, sync done.
    case funded
    /// The funded wallet with an encrypted, locked vault (passphrase `demo`).
    case locked
    /// No vault and no wallet: the app opens onboarding.
    case fresh
    /// The funded wallet (unencrypted) while SPV has no peers and is three
    /// days behind: the sync overlay shows and a payment ends in `send.no_peers`.
    case offline

    /// Parses a scenario name; `onboarding` is accepted for `fresh`.
    public init?(name: String) {
        if name == "onboarding" {
            self = .fresh
        } else if let scenario = DemoScenario(rawValue: name) {
            self = scenario
        } else {
            return nil
        }
    }
}

public enum DemoEnvironment {
    /// The passphrase of the `locked` vault.
    public static let passphrase = "demo"
    /// The phrase `generateMnemonic` returns (12 or 24 of these words). Both
    /// lengths pass the engine's BIP39 checksum, so the create and restore
    /// flows accept them.
    public static let phrase = [
        "galaxy", "rocket", "velvet", "harbor", "tiny", "maple", "oyster", "crane", "sunset", "ribbon", "empty", "above",
        "tunnel", "author", "lizard", "bamboo", "ripple", "wisdom", "canyon", "frost", "pulse", "gentle", "vivid", "animal",
    ]
    /// Restoring this phrase brings back the scripted history; any other
    /// valid phrase makes an empty wallet.
    public static let sampleWalletPhrase = phrase.prefix(12).joined(separator: " ")

    /// A complete environment over one fresh world. Developer mode is on so
    /// regtest and devnets appear in the network lists.
    @MainActor
    public static func make(
        scenario: DemoScenario, network: DashNetwork = .testnet, screenCapture: (any ScreenCaptureGuard)? = nil,
        timing: Timing = Timing()
    ) -> AppEnvironment {
        environment(world: DemoWorld(network: network, scenario: scenario, now: timing.now), screenCapture: screenCapture, timing: timing)
    }

    /// The M1 environment and the M2 services over one shared world. OS
    /// services default to inert demo ones (no autostart, notifications,
    /// files); the apps pass their real clipboard and the rest as needed.
    @MainActor
    public static func makeWithM2(
        scenario: DemoScenario, network: DashNetwork = .testnet, screenCapture: (any ScreenCaptureGuard)? = nil,
        timing: Timing = Timing(), launchOptions: LaunchOptions = LaunchOptions(),
        platform: DemoPlatformServices = DemoPlatformServices(), desktopPlatform: DesktopPlatform = .current
    ) -> (env: AppEnvironment, m2: M2Services) {
        let built = build(
            scenario: scenario, network: network, screenCapture: screenCapture, timing: timing,
            launchOptions: launchOptions, platform: platform, desktopPlatform: desktopPlatform)
        return (built.env, built.m2)
    }

    /// The M1 environment and the M2 and M3 services over one shared world:
    /// sample masternodes, proposals and CoinJoin progress for the sample
    /// wallet (DemoM3World).
    @MainActor
    public static func makeWithM3(
        scenario: DemoScenario, network: DashNetwork = .testnet, screenCapture: (any ScreenCaptureGuard)? = nil,
        timing: Timing = Timing(), launchOptions: LaunchOptions = LaunchOptions(),
        platform: DemoPlatformServices = DemoPlatformServices(), desktopPlatform: DesktopPlatform = .current
    ) -> (env: AppEnvironment, m2: M2Services, m3: M3Services) {
        let built = build(
            scenario: scenario, network: network, screenCapture: screenCapture, timing: timing,
            launchOptions: launchOptions, platform: platform, desktopPlatform: desktopPlatform)
        let world = DemoM3World(world: built.world, m2: built.m2World)
        let coinJoin = DemoCoinJoin(m3: world)
        let governance = DemoGovernance(m3: world)
        let masternodes = DemoMasternodes(m3: world)
        let m3 = M3Services(
            coinJoin: coinJoin, mixedCoins: coinJoin, networkStatistics: coinJoin, governance: governance,
            voting: governance, proposals: governance, masternodes: masternodes, registration: masternodes,
            maintenance: masternodes, shared: masternodes, keychain: masternodes, tracked: masternodes,
            evonodes: masternodes)
        return (built.env, built.m2, m3)
    }

    @MainActor
    private static func build(
        scenario: DemoScenario, network: DashNetwork, screenCapture: (any ScreenCaptureGuard)?, timing: Timing,
        launchOptions: LaunchOptions, platform: DemoPlatformServices, desktopPlatform: DesktopPlatform
    ) -> (world: DemoWorld, m2World: DemoM2World, env: AppEnvironment, m2: M2Services) {
        let world = DemoWorld(network: network, scenario: scenario, now: timing.now)
        let env = environment(world: world, screenCapture: screenCapture, timing: timing)
        let m2 = DemoM2World(world: world)
        let services = M2Services(
            walletLifecycle: DemoWalletLifecycle(m2: m2), transactionActions: DemoTransactionActions(m2: m2),
            fees: DemoFees(world: world), fileImporter: DemoFileImporter(), coreExporter: DemoCoreExporter(world: world),
            backups: DemoBackups(m2: m2), psbt: DemoPSBT(), nodeInformation: DemoNodeInformation(m2: m2),
            peerModeration: DemoPeerModeration(m2: m2, sync: DemoSync(world: world)), repair: DemoRepair(m2: m2),
            console: DemoConsole(m2: m2), logs: DemoLogs(), quickUnlock: DemoQuickUnlock(),
            autoLock: DemoAutoLock(m2: m2), vaultRecovery: DemoVaultRecovery(m2: m2), startup: DemoStartup(),
            shutdown: DemoShutdown(), shellSettings: DemoShellSettings(m2: m2), launchArguments: DemoLaunchArguments(),
            launchOptions: launchOptions, desktopPreferences: DemoDesktopPreferences(m2: m2),
            dustProtection: DemoDustProtection(m2: m2), optionsReset: DemoOptionsReset(m2: m2),
            paymentAuthentication: DemoPaymentAuthentication(m2: m2), launchAtLogin: platform.launchAtLogin,
            notifications: platform.notifications, clipboard: platform.clipboard, fileRevealer: platform.fileRevealer,
            dataDirectories: platform.dataDirectories, platform: desktopPlatform)
        return (world, m2, env, services)
    }

    @MainActor
    private static func environment(
        world: DemoWorld, screenCapture: (any ScreenCaptureGuard)?, timing: Timing
    ) -> AppEnvironment {
        let active = world.activeNetwork
        let uri = EngineFunctions.uriHandler(network: { active.current })
        let settings = DemoSettings(world: world)
        return AppEnvironment(
            host: DemoHost(world: world), lifecycle: DemoLifecycle(world: world),
            walletState: DemoWalletState(world: world), sync: DemoSync(world: world), vault: DemoVault(world: world),
            auth: DemoAuth(world: world), sender: DemoSender(world: world, addresses: uri),
            history: DemoHistory(world: world), receive: DemoReceive(world: world, uri: uri),
            coinControl: DemoCoinControl(world: world), addressBook: DemoAddressBook(world: world, addresses: uri),
            messages: DemoMessages(world: world), uri: uri,
            amounts: EngineFunctions.amountFormatter(network: { active.current }), settings: settings,
            preferences: settings, screenCapture: screenCapture, timing: timing, developerMode: true)
    }
}
