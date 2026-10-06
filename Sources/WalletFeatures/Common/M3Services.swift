// The M3 service instances the V1 view models use (docs/contracts/m3-swift.md
// §2), built once next to `AppEnvironment` and `M2Services` by each app's
// composition root (live: the R1–R3 adapters, `M3Services+Live.swift`; demo:
// `WalletDemo.DemoEnvironment.makeWithM3`). There are no singletons.
import Foundation
import WalletRuntime

@MainActor
public struct M3Services {
    // CoinJoin (R1)
    public var coinJoin: any CoinJoinControlling
    public var mixedCoins: any MixedCoinsMoving
    public var networkStatistics: any NetworkStatisticsProviding
    // Governance (R2)
    public var governance: any GovernanceProviding
    public var voting: any GovernanceVoting
    public var proposals: any ProposalCreating
    // Masternodes (R3)
    public var masternodes: any MasternodeListProviding
    public var registration: any MasternodeRegistering
    public var maintenance: any MasternodeMaintaining
    public var shared: any SharedMasternodeCoordinating
    public var keychain: any MasternodeKeychainProviding
    public var tracked: any TrackedMasternodeManaging
    public var evonodes: any EvonodeServicing

    public init(
        coinJoin: any CoinJoinControlling, mixedCoins: any MixedCoinsMoving,
        networkStatistics: any NetworkStatisticsProviding, governance: any GovernanceProviding,
        voting: any GovernanceVoting, proposals: any ProposalCreating, masternodes: any MasternodeListProviding,
        registration: any MasternodeRegistering, maintenance: any MasternodeMaintaining,
        shared: any SharedMasternodeCoordinating, keychain: any MasternodeKeychainProviding,
        tracked: any TrackedMasternodeManaging, evonodes: any EvonodeServicing
    ) {
        self.coinJoin = coinJoin
        self.mixedCoins = mixedCoins
        self.networkStatistics = networkStatistics
        self.governance = governance
        self.voting = voting
        self.proposals = proposals
        self.masternodes = masternodes
        self.registration = registration
        self.maintenance = maintenance
        self.shared = shared
        self.keychain = keychain
        self.tracked = tracked
        self.evonodes = evonodes
    }
}
