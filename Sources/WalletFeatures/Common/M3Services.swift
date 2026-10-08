// The M3 service instances the V1 view models use (docs/contracts/m3-swift.md
// §2), built once next to `AppEnvironment` and `M2Services` by each app's
// composition root (live: the WalletRuntime adapters, `M3Services+Live.swift`;
// demo: `WalletDemo.DemoEnvironment.makeWithM3`). There are no singletons.
import Foundation
import WalletRuntime

@MainActor
public struct M3Services {
    // CoinJoin (R1)
    public var coinJoin: any CoinJoinControlling
    public var mixedCoins: any MixedCoinsMoving
    public var networkStatistics: any NetworkStatisticsProviding
    // Masternode Keys (R3, IOS-083)
    public var keychain: any MasternodeKeychainProviding

    public init(
        coinJoin: any CoinJoinControlling, mixedCoins: any MixedCoinsMoving,
        networkStatistics: any NetworkStatisticsProviding, keychain: any MasternodeKeychainProviding
    ) {
        self.coinJoin = coinJoin
        self.mixedCoins = mixedCoins
        self.networkStatistics = networkStatistics
        self.keychain = keychain
    }
}
