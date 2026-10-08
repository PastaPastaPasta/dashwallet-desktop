// The live `M3Services`: the WalletRuntime adapters over the engine
// (`WalletRuntime/M3/<Domain>Adapters.swift`, built by `M3RuntimeServices`).
// A runtime without them (one over a test engine) is served by
// `UnavailableM3Service`, which answers every call with `not_implemented`, so
// its screens say "Not available yet" instead of showing an empty success
// state.
import Foundation
import WalletRuntime

extension M3Services {
    /// The M3 services of a live app run: the engine adapters of
    /// `runtime.m3`, or `not_implemented` everywhere when the runtime's
    /// engine has none.
    public static func live(runtime: WalletRuntimeServices) -> M3Services {
        guard let m3 = runtime.m3 else { return unavailable() }
        return M3Services(
            coinJoin: m3.coinJoin, mixedCoins: m3.coinJoin, networkStatistics: m3.networkStatistics,
            keychain: m3.keychain)
    }

    /// Every M3 domain answering `not_implemented`.
    public static func unavailable() -> M3Services {
        let missing = UnavailableM3Service()
        return M3Services(coinJoin: missing, mixedCoins: missing, networkStatistics: missing, keychain: missing)
    }
}

/// Stands in for the M3 adapters when the engine has none. The constant calls
/// return Core's values (`M3Defaults`, as the engine's working calls do);
/// every other call throws `not_implemented` naming the engine call; streams
/// end at once.
public final class UnavailableM3Service: Sendable {
    public init() {}

    private func missing(_ call: String) -> ServiceError {
        ServiceError(code: .notImplemented, detail: call)
    }

    private func ended<T>() -> AsyncStream<T> {
        AsyncStream { $0.finish() }
    }
}

extension UnavailableM3Service: CoinJoinControlling, MixedCoinsMoving, NetworkStatisticsProviding {
    public func limits() -> CoinJoinLimits { M3Defaults.coinJoinLimits }
    public func settings() async throws(ServiceError) -> CoinJoinSettings { throw missing("coinjoin_settings") }
    public func setSettings(_ settings: CoinJoinSettings) async throws(ServiceError) {
        if M3Defaults.invalidCoinJoinField(settings) != nil {
            throw ServiceError(code: .invalidArgument, detail: "coinjoin settings")
        }
        throw missing("set_coinjoin_settings")
    }
    public func status(wallet: WalletID) async throws(ServiceError) -> CoinJoinStatus { throw missing("coinjoin_status") }
    public func statusChanges() -> AsyncStream<WalletID> { ended() }
    public func start(wallet: WalletID) async throws(ServiceError) { throw missing("start_mixing") }
    public func stop(wallet: WalletID) async throws(ServiceError) { throw missing("stop_mixing") }
    public func salt(wallet: WalletID) async throws(ServiceError) -> String { throw missing("coinjoin_salt") }
    public func setSalt(_ salt: String, wallet: WalletID) async throws(ServiceError) {
        guard M3Defaults.isValidSalt(salt) else { throw ServiceError(code: .invalidArgument, detail: "salt") }
        throw missing("set_coinjoin_salt")
    }
    public func generateSalt(wallet: WalletID) async throws(ServiceError) -> String {
        throw missing("generate_coinjoin_salt")
    }
    public func recoveryScan(wallet: WalletID) async throws(ServiceError) -> CoinJoinRecoveryReport {
        throw missing("coinjoin_recovery_scan")
    }
    public func plan(wallet: WalletID, destination: MixedCoinsDestination) async throws(ServiceError)
        -> MixedCoinsSweepPlan
    {
        throw missing("mixed_coins_sweep_plan")
    }
    public func move(wallet: WalletID, destination: MixedCoinsDestination, grant: AuthGrant)
        async throws(ServiceError) -> MixedCoinsSweepResult
    {
        throw missing("move_mixed_coins")
    }
    public func statistics() async throws(ServiceError) -> NetworkStatistics { throw missing("network_stats") }
}

extension UnavailableM3Service: MasternodeKeychainProviding {
    public func keys(wallet: WalletID, role: MasternodeKeyRole, range: Range<UInt32>) async throws(ServiceError)
        -> [MasternodeKeyInfo]
    {
        guard range.count <= 100 else { throw ServiceError(code: .invalidArgument, detail: "count > 100") }
        throw missing("masternode_keys")
    }
    public func reveal(wallet: WalletID, role: MasternodeKeyRole, index: UInt32, grant: AuthGrant)
        async throws(ServiceError) -> RevealedMasternodeKey
    {
        throw missing("Vault.reveal_masternode_key")
    }
}
