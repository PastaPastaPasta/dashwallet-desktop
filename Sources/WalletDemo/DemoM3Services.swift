// The M3 service protocols (docs/contracts/m3-swift.md §2) over
// `DemoM3World`. "Move mixed coins" builds transactions the demo does not
// make and answers `not_implemented` after the engine's argument checks.
import Foundation
import WalletFeatures
import WalletRuntime

private func notInDemo(_ call: String) -> ServiceError {
    .demo(.notImplemented, "demo: \(call) builds no transactions")
}

// MARK: CoinJoin (§2.1, §2.4)

final class DemoCoinJoin: CoinJoinControlling, MixedCoinsMoving, NetworkStatisticsProviding {
    let m3: DemoM3World

    init(m3: DemoM3World) {
        self.m3 = m3
    }

    func limits() -> CoinJoinLimits { M3Defaults.coinJoinLimits }
    func settings() async throws(ServiceError) -> CoinJoinSettings { await m3.settings }
    func setSettings(_ settings: CoinJoinSettings) async throws(ServiceError) { try await m3.setSettings(settings) }
    func status(wallet: WalletID) async throws(ServiceError) -> CoinJoinStatus { try await m3.status(wallet) }
    func statusChanges() -> AsyncStream<WalletID> { m3.coinJoinChanges.stream() }
    func start(wallet: WalletID) async throws(ServiceError) { try await m3.start(wallet) }
    func stop(wallet: WalletID) async throws(ServiceError) { try await m3.stopMixing(wallet) }
    func salt(wallet: WalletID) async throws(ServiceError) -> String { try await m3.salt(wallet) }
    func setSalt(_ salt: String, wallet: WalletID) async throws(ServiceError) { try await m3.setSalt(salt, wallet) }
    func generateSalt(wallet: WalletID) async throws(ServiceError) -> String { try await m3.generateSalt(wallet) }

    func recoveryScan(wallet: WalletID) async throws(ServiceError) -> CoinJoinRecoveryReport {
        let status = try await m3.status(wallet)
        return CoinJoinRecoveryReport(
            coinJoinAddressesScanned: 1_000, bip44AddressesScanned: 2_000, coinJoinBalance: status.balances.fullyMixed,
            newTransactions: 0)
    }

    func plan(wallet: WalletID, destination: MixedCoinsDestination) async throws(ServiceError) -> MixedCoinsSweepPlan {
        try await m3.sweepPlan(wallet, destination: destination)
    }

    func move(wallet: WalletID, destination: MixedCoinsDestination, grant: AuthGrant) async throws(ServiceError)
        -> MixedCoinsSweepResult
    {
        _ = try await m3.sweepPlan(wallet, destination: destination)
        throw notInDemo("move_mixed_coins")
    }

    func statistics() async throws(ServiceError) -> NetworkStatistics { await m3.statistics() }
}

// MARK: Masternode keychain (§2.3)

final class DemoMasternodeKeychain: MasternodeKeychainProviding {
    let m3: DemoM3World

    init(m3: DemoM3World) {
        self.m3 = m3
    }

    func keys(wallet: WalletID, role: MasternodeKeyRole, range: Range<UInt32>) async throws(ServiceError)
        -> [MasternodeKeyInfo]
    {
        try await m3.keys(wallet, role: role, range: range)
    }

    func reveal(wallet: WalletID, role: MasternodeKeyRole, index: UInt32, grant: AuthGrant) async throws(ServiceError)
        -> RevealedMasternodeKey
    {
        try await m3.revealKey(wallet, role: role, index: index, grant: grant)
    }
}
