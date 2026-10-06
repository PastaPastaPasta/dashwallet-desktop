// Adapters for the M3 CoinJoin contracts (m3-swift.md §2.1, §2.4; owner R1):
// `CoinJoinControlling`, `MixedCoinsMoving` and `NetworkStatisticsProviding`
// over the `EngineClient` wrappers of `EngineClient+M3CoinJoin.swift`. Each
// call reads the open network; `CoinJoin` engine events become
// `statusChanges()`.
import DashKit
import Foundation

/// Mixing control, options, salt and "move mixed coins" over the engine.
public final class CoinJoinService: CoinJoinControlling, MixedCoinsMoving {
    private let engine: EngineClient
    private let active: ActiveNetwork

    public init(engine: EngineClient, active: ActiveNetwork) {
        self.engine = engine
        self.active = active
    }

    private func call<T>(
        _ body: (EngineClient, DashKit.DashNetwork) async throws(DashKitError) -> T
    ) async throws(ServiceError) -> T {
        let network = try active.require()
        let engine = engine
        return try await serviceCall { () async throws(DashKitError) in try await body(engine, network) }
    }

    // MARK: CoinJoinControlling

    public func limits() -> CoinJoinLimits {
        let l = engine.coinJoinLimits()
        let range = { (r: ClosedRange<UInt32>) in Int(r.lowerBound)...Int(r.upperBound) }
        return CoinJoinLimits(
            denominations: l.denominations.map(Amount.init), minimumMixingBalance: Amount(l.minimumMixingBalance),
            rounds: range(l.rounds), sessions: range(l.sessions), targetAmountDash: range(l.targetAmountDash),
            denoms: range(l.denoms), defaults: CoinJoinSettings(l.defaults))
    }

    public func settings() async throws(ServiceError) -> CoinJoinSettings {
        CoinJoinSettings(try await call { engine, network throws(DashKitError) in
            try await engine.coinJoinOptions(on: network)
        })
    }

    public func setSettings(_ settings: CoinJoinSettings) async throws(ServiceError) {
        let options = try settings.kit
        try await call { engine, network throws(DashKitError) in
            try await engine.setCoinJoinOptions(on: network, options)
        }
    }

    public func status(wallet: WalletID) async throws(ServiceError) -> CoinJoinStatus {
        let id = try wallet.kit
        let s = try await call { engine, network throws(DashKitError) in
            try await engine.coinJoinStatus(on: network, wallet: id)
        }
        return CoinJoinStatus(s)
    }

    /// Engine `CoinJoin` events of the open network; `resynchronize` counts
    /// for every wallet, so it yields nothing here and hosts re-query all.
    public func statusChanges() -> AsyncStream<WalletID> {
        let subscription = engine.events.subscribe()
        let active = active
        let (stream, continuation) = AsyncStream<WalletID>.makeStream(bufferingPolicy: .bufferingNewest(16))
        let pump = Task {
            for await event in subscription {
                if Task.isCancelled { break }
                if case .coinJoinChanged(let n, let id) = event, active.network == n {
                    continuation.yield(WalletID(id))
                }
            }
            continuation.finish()
        }
        continuation.onTermination = { _ in pump.cancel() }
        return stream
    }

    public func start(wallet: WalletID) async throws(ServiceError) {
        let id = try wallet.kit
        try await call { engine, network throws(DashKitError) in try await engine.startMixing(on: network, wallet: id) }
    }

    public func stop(wallet: WalletID) async throws(ServiceError) {
        let id = try wallet.kit
        try await call { engine, network throws(DashKitError) in try await engine.stopMixing(on: network, wallet: id) }
    }

    public func salt(wallet: WalletID) async throws(ServiceError) -> String {
        let id = try wallet.kit
        return try await call { engine, network throws(DashKitError) in
            try await engine.coinJoinSalt(on: network, wallet: id)
        }
    }

    public func setSalt(_ salt: String, wallet: WalletID) async throws(ServiceError) {
        let id = try wallet.kit
        try await call { engine, network throws(DashKitError) in
            try await engine.setCoinJoinSalt(on: network, wallet: id, salt: salt)
        }
    }

    public func generateSalt(wallet: WalletID) async throws(ServiceError) -> String {
        let id = try wallet.kit
        return try await call { engine, network throws(DashKitError) in
            try await engine.generateCoinJoinSalt(on: network, wallet: id)
        }
    }

    // MARK: MixedCoinsMoving

    public func recoveryScan(wallet: WalletID) async throws(ServiceError) -> CoinJoinRecoveryReport {
        let id = try wallet.kit
        let r = try await call { engine, network throws(DashKitError) in
            try await engine.coinJoinRecoveryScan(on: network, wallet: id)
        }
        return CoinJoinRecoveryReport(
            coinJoinAddressesScanned: Int(r.coinJoinAddressesScanned),
            bip44AddressesScanned: Int(r.bip44AddressesScanned), coinJoinBalance: Amount(r.coinJoinBalance),
            newTransactions: Int(r.newTransactions))
    }

    public func plan(wallet: WalletID, destination: MixedCoinsDestination) async throws(ServiceError)
        -> MixedCoinsSweepPlan
    {
        let id = try wallet.kit
        let target = destination.kit
        let p = try await call { engine, network throws(DashKitError) in
            try await engine.mixedCoinsPlan(on: network, wallet: id, target: target)
        }
        return MixedCoinsSweepPlan(
            destination: destination, total: Amount(p.total),
            chunks: p.chunks.map {
                MixedCoinsChunk(inputs: Int($0.inputs), amount: Amount($0.amount), fee: Amount($0.fee))
            })
    }

    public func move(wallet: WalletID, destination: MixedCoinsDestination, grant: AuthGrant)
        async throws(ServiceError) -> MixedCoinsSweepResult
    {
        let id = try wallet.kit
        let target = destination.kit
        let grantID = grant.id
        let r = try await call { engine, network throws(DashKitError) in
            try await engine.moveMixedCoins(on: network, wallet: id, target: target, grantID: grantID)
        }
        return MixedCoinsSweepResult(
            txids: r.txids, moved: Amount(r.moved), remaining: Amount(r.remaining),
            failureCode: r.failureCode.map(ServiceErrorCode.init(rawValue:)))
    }
}

/// Tools → Information → Network (QT-144) over `network_stats`.
public final class NetworkStatisticsService: NetworkStatisticsProviding {
    private let engine: EngineClient
    private let active: ActiveNetwork

    public init(engine: EngineClient, active: ActiveNetwork) {
        self.engine = engine
        self.active = active
    }

    public func statistics() async throws(ServiceError) -> NetworkStatistics {
        let network = try active.require()
        let engine = engine
        let s = try await serviceCall { () async throws(DashKitError) in try await engine.networkStats(on: network) }
        // Credit pool and InstantSend counters are full-node data; the
        // engine reports them absent on SPV.
        return NetworkStatistics(
            creditPool: nil, instantSend: nil,
            masternodes: s.masternodes.map { MasternodeCount(total: Int($0.total), enabled: Int($0.enabled)) },
            evonodes: s.evonodes.map { MasternodeCount(total: Int($0.total), enabled: Int($0.enabled)) },
            bestChainLock: s.bestChainLock.map {
                ChainLockInfo(height: $0.height, blockHash: $0.blockHash, blockDate: $0.blockTime)
            },
            quorums: s.quorums.map {
                QuorumSummary(
                    name: $0.name, type: $0.type, active: Int($0.active), healthPercent: $0.healthPercent,
                    rotated: $0.rotated)
            })
    }
}

// MARK: Conversions

extension CoinJoinSettings {
    init(_ kit: CoinJoinOptions) {
        self.init(
            enabled: kit.enabled, multiSession: kit.multiSession, maxSessions: Int(kit.maxSessions),
            rounds: Int(kit.rounds), targetAmountDash: Int(kit.targetAmountDash), denomsGoal: Int(kit.denomsGoal),
            denomsHardCap: Int(kit.denomsHardCap))
    }

    /// `invalid_argument` for a negative or oversized value; the engine
    /// checks the ranges.
    var kit: CoinJoinOptions {
        get throws(ServiceError) {
            func u32(_ v: Int, _ field: String) throws(ServiceError) -> UInt32 {
                guard let x = UInt32(exactly: v) else {
                    throw ServiceError(code: .invalidArgument, detail: "\(field) = \(v) is out of range")
                }
                return x
            }
            return CoinJoinOptions(
                enabled: enabled, multiSession: multiSession, maxSessions: try u32(maxSessions, "max_sessions"),
                rounds: try u32(rounds, "rounds"), targetAmountDash: try u32(targetAmountDash, "target_amount_dash"),
                denomsGoal: try u32(denomsGoal, "denoms_goal"), denomsHardCap: try u32(denomsHardCap, "denoms_hard_cap"))
        }
    }
}

extension MixedCoinsDestination {
    var kit: MixedCoinsTarget {
        switch self {
        case .wallet: .wallet
        case .shielded: .shielded
        }
    }
}

extension CoinJoinPoolMessage {
    init(_ kit: CoinJoinMessage) {
        // Both enums list Core's `PoolMessage` in wire order.
        self = Self.allCases[CoinJoinMessage.allCases.firstIndex(of: kit)!]
    }
}

extension CoinJoinStatusCode {
    init(_ kit: CoinJoinCode) {
        self = switch kit {
        case .idle: .idle
        case .syncInProgress: .syncInProgress
        case .walletLocked: .walletLocked
        case .mixingInProgress: .mixingInProgress
        case .noMasternodes: .noMasternodes
        case .notEnoughFunds: .notEnoughFunds
        case .unconfirmedDenominated: .unconfirmedDenominated
        case .noCompatibleMasternode: .noCompatibleMasternode
        case .noCompatibleInputs: .noCompatibleInputs
        case .tryingToConnect: .tryingToConnect
        case .noQueueToJoin: .noQueueToJoin
        case .noRandomMasternode: .noRandomMasternode
        case .failedToStartQueue: .failedToStartQueue
        case .waitingInQueue: .waitingInQueue
        case .signing: .signing
        case .masternode(let m): .masternode(CoinJoinPoolMessage(m))
        }
    }
}

extension CoinJoinState {
    init(_ kit: CoinJoinMixingState) {
        switch kit {
        case .idle: self = .idle
        case .mixing: self = .mixing
        case .stopping: self = .stopping
        }
    }
}

extension CoinJoinStopReason {
    init(_ kit: CoinJoinStop) {
        switch kit {
        case .userRequested: self = .userRequested
        case .vaultLocked: self = .vaultLocked
        case .walletUnloaded: self = .walletUnloaded
        case .sessionClosed: self = .sessionClosed
        case .disabled: self = .disabled
        }
    }
}

extension CoinJoinUnavailable {
    init(_ kit: CoinJoinUnavailability) {
        switch kit {
        case .disabled: self = .disabled
        case .watchOnly: self = .watchOnly
        case .insufficientFunds(let min): self = .insufficientFunds(minimum: Amount(min))
        }
    }
}

extension CoinJoinPoolState {
    init(_ kit: CoinJoinPool) {
        switch kit {
        case .idle: self = .idle
        case .queue: self = .queue
        case .acceptingEntries: self = .acceptingEntries
        case .signing: self = .signing
        case .error: self = .error
        }
    }
}

extension CoinJoinStatus {
    init(_ s: CoinJoinWalletStatus) {
        self.init(
            wallet: WalletID(s.wallet),
            state: CoinJoinState(s.state),
            stopReason: s.stopReason.map(CoinJoinStopReason.init),
            unavailable: s.unavailable.map(CoinJoinUnavailable.init),
            balances: CoinJoinBalances(
                anonymizable: Amount(s.anonymizable), denominated: Amount(s.denominated),
                normalizedAnonymized: Amount(s.normalizedAnonymized), fullyMixed: Amount(s.fullyMixed)),
            progress: CoinJoinProgress(
                overall: s.overallPercent, denominated: s.denominatedPercent, partiallyMixed: s.partiallyMixedPercent,
                mixed: s.mixedPercent, averageRounds: s.averageRounds),
            amountAndRounds: CoinJoinAmountAndRounds(
                amount: Amount(s.amount), rounds: Int(s.rounds), insufficientInputs: s.insufficientInputs),
            submittedDenominations: s.submittedDenominations.map(Amount.init),
            sessions: s.sessions.map {
                CoinJoinSessionInfo(
                    proTxHash: $0.proTxHash, service: $0.service, denomination: $0.denomination.map(Amount.init),
                    state: CoinJoinPoolState($0.state), entries: Int($0.entries),
                    lastMessage: $0.lastMessage.map(CoinJoinPoolMessage.init))
            },
            status: CoinJoinStatusCode(s.status), queueSize: Int(s.queueSize), keysLeft: s.keysLeft.map(Int.init))
    }
}
