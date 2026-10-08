// The M3 adapters of one app run (docs/contracts/m3-swift.md §1): CoinJoin,
// the Network sub-tab and the masternode keychain over the app's
// `EngineClient`. WalletFeatures bundles them as `M3Services.live(runtime:)`.
import DashKit
import Foundation

/// The M3 engine adapters for one engine; each call reads the host's open
/// network.
public final class M3RuntimeServices: Sendable {
    public let coinJoin: CoinJoinService
    public let networkStatistics: NetworkStatisticsService
    public let keychain: MasternodeKeychainService

    public init(engine: EngineClient, active: ActiveNetwork) {
        coinJoin = CoinJoinService(engine: engine, active: active)
        networkStatistics = NetworkStatisticsService(engine: engine, active: active)
        keychain = MasternodeKeychainService(engine: engine, active: active)
    }
}
