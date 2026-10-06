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
        let world = DemoWorld(network: network, scenario: scenario, now: timing.now)
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
