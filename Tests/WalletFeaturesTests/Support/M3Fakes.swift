// In-memory fakes of the M3 WalletRuntime contracts (docs/contracts/
// m3-swift.md §2). Like the M1/M2 fakes they record calls and answer from
// configured state; an unconfigured call throws not_implemented, as the
// engine does without an adapter. The checks the engine already makes
// (CoinJoin ranges, the salt, ≤ 100 keys, the reveal's grant) are made here
// too, with the engine's codes.
import Foundation
import WalletFeatures
import WalletRuntime

// MARK: CoinJoin (§2.1, §2.4)

final class FakeCoinJoin: CoinJoinControlling, MixedCoinsMoving, NetworkStatisticsProviding, @unchecked Sendable {
    let settingsValue = Locked<CoinJoinSettings?>(nil)
    let statuses = Locked<[WalletID: CoinJoinStatus]>([:])
    let starts = Locked<[WalletID]>([])
    let stops = Locked<[WalletID]>([])
    let settingsWrites = Locked<[CoinJoinSettings]>([])
    let report = Locked<CoinJoinRecoveryReport?>(nil)
    let sweepPlan = Locked<MixedCoinsSweepPlan?>(nil)
    let moveResult = Locked<MixedCoinsSweepResult?>(nil)
    let moveGrants = Locked<[AuthGrant]>([])
    let networkStatistics = Locked<NetworkStatistics?>(nil)
    let errors = FakeErrors()
    let changes = Broadcast<WalletID>()
    /// `start` refuses with `coinjoin.vault_locked` while this says the vault is locked.
    let vaultLocked = Locked<@Sendable () -> Bool>({ false })

    func limits() -> CoinJoinLimits { M3Defaults.coinJoinLimits }

    func settings() async throws(ServiceError) -> CoinJoinSettings {
        try errors.check("settings")
        guard let value = settingsValue.current else { throw notConfigured("settings") }
        return value
    }

    func setSettings(_ settings: CoinJoinSettings) async throws(ServiceError) {
        if let field = M3Defaults.invalidCoinJoinField(settings) {
            throw ServiceError(code: .invalidArgument, detail: field)
        }
        guard settingsValue.current != nil else { throw notConfigured("setSettings") }
        settingsWrites.withLock { $0.append(settings) }
        settingsValue.withLock { $0 = settings }
    }

    func status(wallet: WalletID) async throws(ServiceError) -> CoinJoinStatus {
        try errors.check("status")
        guard let status = statuses.current[wallet] else { throw notConfigured("status") }
        return status
    }

    func statusChanges() -> AsyncStream<WalletID> { changes.stream() }

    func start(wallet: WalletID) async throws(ServiceError) {
        try errors.check("start")
        if vaultLocked.current() { throw ServiceError(code: .coinjoinVaultLocked) }
        guard let status = statuses.current[wallet] else { throw notConfigured("start") }
        starts.withLock { $0.append(wallet) }
        statuses.withLock { $0[wallet] = status.with(state: .mixing) }
        changes.send(wallet)
    }

    func stop(wallet: WalletID) async throws(ServiceError) {
        guard let status = statuses.current[wallet] else { throw notConfigured("stop") }
        stops.withLock { $0.append(wallet) }
        statuses.withLock { $0[wallet] = status.with(state: .idle, stopReason: .userRequested) }
        changes.send(wallet)
    }

    func salt(wallet: WalletID) async throws(ServiceError) -> String { throw notConfigured("salt") }

    func setSalt(_ salt: String, wallet: WalletID) async throws(ServiceError) {
        guard M3Defaults.isValidSalt(salt) else { throw ServiceError(code: .invalidArgument, detail: "salt") }
        throw notConfigured("setSalt")
    }

    func generateSalt(wallet: WalletID) async throws(ServiceError) -> String { throw notConfigured("generateSalt") }

    func recoveryScan(wallet: WalletID) async throws(ServiceError) -> CoinJoinRecoveryReport {
        guard let report = report.current else { throw notConfigured("recoveryScan") }
        return report
    }

    func plan(wallet: WalletID, destination: MixedCoinsDestination) async throws(ServiceError) -> MixedCoinsSweepPlan {
        if destination == .shielded { throw ServiceError(code: .notImplemented, detail: "move_mixed_coins.shielded") }
        guard let plan = sweepPlan.current else { throw notConfigured("plan") }
        return plan
    }

    func move(wallet: WalletID, destination: MixedCoinsDestination, grant: AuthGrant) async throws(ServiceError)
        -> MixedCoinsSweepResult
    {
        guard let plan = sweepPlan.current, let result = moveResult.current else { throw notConfigured("move") }
        guard case .spend(let max) = grant.purpose, max >= plan.total else { throw ServiceError(code: .coinjoinGrantInvalid) }
        moveGrants.withLock { $0.append(grant) }
        return result
    }

    func statistics() async throws(ServiceError) -> NetworkStatistics {
        guard let value = networkStatistics.current else { throw notConfigured("statistics") }
        return value
    }
}

extension CoinJoinStatus {
    func with(state: CoinJoinState, stopReason: CoinJoinStopReason? = nil) -> CoinJoinStatus {
        CoinJoinStatus(
            wallet: wallet, state: state, stopReason: stopReason, unavailable: unavailable, balances: balances,
            progress: progress, amountAndRounds: amountAndRounds, submittedDenominations: submittedDenominations,
            sessions: sessions, status: state == .mixing ? .mixingInProgress : .idle, queueSize: queueSize,
            keysLeft: keysLeft)
    }
}

func coinJoinStatus(
    _ wallet: WalletID = walletA, state: CoinJoinState = .idle, unavailable: CoinJoinUnavailable? = nil,
    anonymizable: Int64 = 5_00000000, fullyMixed: Int64 = 1_50000000, denominated: Int64 = 3_00000000,
    amount: Int64 = 1000_00000000, insufficientInputs: Bool = false, keysLeft: Int? = nil,
    status: CoinJoinStatusCode = .idle, stopReason: CoinJoinStopReason? = nil
) -> CoinJoinStatus {
    CoinJoinStatus(
        wallet: wallet, state: state, stopReason: stopReason, unavailable: unavailable,
        balances: CoinJoinBalances(
            anonymizable: Amount(duffs: anonymizable), denominated: Amount(duffs: denominated),
            normalizedAnonymized: Amount(duffs: fullyMixed), fullyMixed: Amount(duffs: fullyMixed)),
        progress: CoinJoinProgress(overall: 42.5, denominated: 60, partiallyMixed: 30, mixed: 25, averageRounds: 1.75),
        amountAndRounds: CoinJoinAmountAndRounds(
            amount: Amount(duffs: amount), rounds: 4, insufficientInputs: insufficientInputs),
        submittedDenominations: [], sessions: [], status: status, queueSize: 0, keysLeft: keysLeft)
}

// MARK: Masternode keychain (§2.3)

final class FakeMasternodeKeychain: MasternodeKeychainProviding, @unchecked Sendable {
    let keyInfos = Locked<[MasternodeKeyInfo]?>(nil)

    func keys(wallet: WalletID, role: MasternodeKeyRole, range: Range<UInt32>) async throws(ServiceError)
        -> [MasternodeKeyInfo]
    {
        guard range.count <= 100 else { throw ServiceError(code: .invalidArgument, detail: "count > 100") }
        guard let infos = keyInfos.current else { throw notConfigured("keys") }
        return infos.filter { $0.role == role && range.contains($0.index) }
    }

    func reveal(wallet: WalletID, role: MasternodeKeyRole, index: UInt32, grant: AuthGrant) async throws(ServiceError)
        -> RevealedMasternodeKey
    {
        guard grant.purpose == .revealSecret else { throw ServiceError(code: .masternodeGrantInvalid) }
        return RevealedMasternodeKey(privateKeyHex: FakeSecret("priv-\(role)-\(index)"), wif: nil, tenderdashKey: nil)
    }
}

// MARK: World

@MainActor
final class FakeM3World {
    let coinJoin = FakeCoinJoin()
    let keychain = FakeMasternodeKeychain()

    var services: M3Services {
        M3Services(coinJoin: coinJoin, mixedCoins: coinJoin, networkStatistics: coinJoin, keychain: keychain)
    }
}
