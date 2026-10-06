import DashWalletCore
import Foundation

// DashKit wrappers of the M3 CoinJoin calls and the Network sub-tab
// (docs/contracts/m3-engine.md §2.1, §2.6; owner R1). The records mirror the
// engine's; amounts are `Amount`, wallet ids `WalletID`.

// MARK: Models

/// Options → CoinJoin (engine `CoinJoinSettings`).
public struct CoinJoinOptions: Sendable, Hashable {
    public var enabled: Bool
    public var multiSession: Bool
    public var maxSessions: UInt32
    public var rounds: UInt32
    public var targetAmountDash: UInt32
    public var denomsGoal: UInt32
    public var denomsHardCap: UInt32

    public init(
        enabled: Bool, multiSession: Bool, maxSessions: UInt32, rounds: UInt32, targetAmountDash: UInt32,
        denomsGoal: UInt32, denomsHardCap: UInt32
    ) {
        self.enabled = enabled
        self.multiSession = multiSession
        self.maxSessions = maxSessions
        self.rounds = rounds
        self.targetAmountDash = targetAmountDash
        self.denomsGoal = denomsGoal
        self.denomsHardCap = denomsHardCap
    }

    init(_ ffi: DashWalletCore.CoinJoinSettings) {
        self.init(
            enabled: ffi.enabled, multiSession: ffi.multiSession, maxSessions: ffi.maxSessions, rounds: ffi.rounds,
            targetAmountDash: ffi.targetAmountDash, denomsGoal: ffi.denomsGoal, denomsHardCap: ffi.denomsHardCap)
    }

    var ffi: DashWalletCore.CoinJoinSettings {
        .init(
            enabled: enabled, multiSession: multiSession, maxSessions: maxSessions, rounds: rounds,
            targetAmountDash: targetAmountDash, denomsGoal: denomsGoal, denomsHardCap: denomsHardCap)
    }
}

/// Fixed CoinJoin values (engine `coinjoin_limits`).
public struct CoinJoinLimitValues: Sendable, Hashable {
    public let denominations: [Amount]
    public let minimumMixingBalance: Amount
    public let rounds: ClosedRange<UInt32>
    public let sessions: ClosedRange<UInt32>
    public let targetAmountDash: ClosedRange<UInt32>
    public let denoms: ClosedRange<UInt32>
    public let defaults: CoinJoinOptions
}

public enum CoinJoinMixingState: Sendable, Hashable { case idle, mixing, stopping }

public enum CoinJoinStop: Sendable, Hashable { case userRequested, vaultLocked, walletUnloaded, sessionClosed, disabled }

public enum CoinJoinUnavailability: Sendable, Hashable {
    case disabled, watchOnly
    case insufficientFunds(minimum: Amount)
}

public enum CoinJoinPool: Sendable, Hashable {
    case idle, queue, acceptingEntries, signing, error

    init(_ ffi: DashWalletCore.CoinJoinPoolState) {
        switch ffi {
        case .idle: self = .idle
        case .queue: self = .queue
        case .acceptingEntries: self = .acceptingEntries
        case .signing: self = .signing
        case .error: self = .error
        }
    }
}

/// Core `PoolMessage`, in wire order (the engine enum's order).
public enum CoinJoinMessage: Sendable, Hashable, CaseIterable {
    case alreadyHave, denom, entriesFull, existingTx, fees, invalidCollateral, invalidInput, invalidScript, invalidTx
    case maximum, mnList, mode, nonStandardPubkey, notAMasternode, queueFull, recent, session, missingTx, version
    case noError, success, entriesAdded, sizeMismatch

    init(_ ffi: DashWalletCore.CoinJoinPoolMessage) {
        switch ffi {
        case .alreadyHave: self = .alreadyHave
        case .denom: self = .denom
        case .entriesFull: self = .entriesFull
        case .existingTx: self = .existingTx
        case .fees: self = .fees
        case .invalidCollateral: self = .invalidCollateral
        case .invalidInput: self = .invalidInput
        case .invalidScript: self = .invalidScript
        case .invalidTx: self = .invalidTx
        case .maximum: self = .maximum
        case .mnList: self = .mnList
        case .mode: self = .mode
        case .nonStandardPubkey: self = .nonStandardPubkey
        case .notAMasternode: self = .notAMasternode
        case .queueFull: self = .queueFull
        case .recent: self = .recent
        case .session: self = .session
        case .missingTx: self = .missingTx
        case .version: self = .version
        case .noError: self = .noError
        case .success: self = .success
        case .entriesAdded: self = .entriesAdded
        case .sizeMismatch: self = .sizeMismatch
        }
    }
}

/// `strAutoDenomResult` (engine `CoinJoinStatusCode`).
public enum CoinJoinCode: Sendable, Hashable {
    case idle, syncInProgress, walletLocked, mixingInProgress, noMasternodes, notEnoughFunds, unconfirmedDenominated
    case noCompatibleMasternode, noCompatibleInputs, tryingToConnect, noQueueToJoin, noRandomMasternode
    case failedToStartQueue, waitingInQueue, signing
    case masternode(CoinJoinMessage)

    init(_ ffi: DashWalletCore.CoinJoinStatusCode) {
        switch ffi {
        case .idle: self = .idle
        case .syncInProgress: self = .syncInProgress
        case .walletLocked: self = .walletLocked
        case .mixingInProgress: self = .mixingInProgress
        case .noMasternodes: self = .noMasternodes
        case .notEnoughFunds: self = .notEnoughFunds
        case .unconfirmedDenominated: self = .unconfirmedDenominated
        case .noCompatibleMasternode: self = .noCompatibleMasternode
        case .noCompatibleInputs: self = .noCompatibleInputs
        case .tryingToConnect: self = .tryingToConnect
        case .noQueueToJoin: self = .noQueueToJoin
        case .noRandomMasternode: self = .noRandomMasternode
        case .failedToStartQueue: self = .failedToStartQueue
        case .waitingInQueue: self = .waitingInQueue
        case .signing: self = .signing
        case .masternode(let message): self = .masternode(CoinJoinMessage(message))
        }
    }
}

public struct CoinJoinSessionState: Sendable, Hashable {
    public let proTxHash: String?
    public let service: String?
    public let denomination: Amount?
    public let state: CoinJoinPool
    public let entries: UInt32
    public let lastMessage: CoinJoinMessage?
}

/// One wallet's mixing status (engine `CoinJoinStatus`).
public struct CoinJoinWalletStatus: Sendable, Hashable {
    public let wallet: WalletID
    public let state: CoinJoinMixingState
    public let stopReason: CoinJoinStop?
    public let unavailable: CoinJoinUnavailability?
    public let anonymizable: Amount
    public let denominated: Amount
    public let normalizedAnonymized: Amount
    public let fullyMixed: Amount
    public let overallPercent: Double
    public let denominatedPercent: Double
    public let partiallyMixedPercent: Double
    public let mixedPercent: Double
    public let averageRounds: Double
    public let amount: Amount
    public let rounds: UInt32
    public let insufficientInputs: Bool
    public let submittedDenominations: [Amount]
    public let sessions: [CoinJoinSessionState]
    public let status: CoinJoinCode
    public let queueSize: UInt32
    public let keysLeft: UInt32?

    init(_ ffi: DashWalletCore.CoinJoinStatus) throws(DashKitError) {
        wallet = try .engine(ffi.walletId)
        state = switch ffi.state {
        case .idle: .idle
        case .mixing: .mixing
        case .stopping: .stopping
        }
        stopReason = ffi.stopReason.map {
            switch $0 {
            case .userRequested: .userRequested
            case .vaultLocked: .vaultLocked
            case .walletUnloaded: .walletUnloaded
            case .sessionClosed: .sessionClosed
            case .disabled: .disabled
            }
        }
        unavailable = ffi.unavailable.map {
            switch $0 {
            case .disabled: .disabled
            case .watchOnly: .watchOnly
            case .insufficientFunds(let min): .insufficientFunds(minimum: Amount(duffs: Int64(clamping: min)))
            }
        }
        anonymizable = Amount(duffs: Int64(clamping: ffi.balances.anonymizable))
        denominated = Amount(duffs: Int64(clamping: ffi.balances.denominated))
        normalizedAnonymized = Amount(duffs: Int64(clamping: ffi.balances.normalizedAnonymized))
        fullyMixed = Amount(duffs: Int64(clamping: ffi.balances.fullyMixed))
        overallPercent = ffi.progress.overallPercent
        denominatedPercent = ffi.progress.denominatedPercent
        partiallyMixedPercent = ffi.progress.partiallyMixedPercent
        mixedPercent = ffi.progress.mixedPercent
        averageRounds = ffi.progress.averageRounds
        amount = Amount(duffs: Int64(clamping: ffi.amountAndRounds.amount))
        rounds = ffi.amountAndRounds.rounds
        insufficientInputs = ffi.amountAndRounds.insufficientInputs
        submittedDenominations = ffi.submittedDenominations.map { Amount(duffs: Int64(clamping: $0)) }
        sessions = ffi.sessions.map {
            CoinJoinSessionState(
                proTxHash: $0.proTxHash, service: $0.service,
                denomination: $0.denomination.map { Amount(duffs: Int64(clamping: $0)) },
                state: CoinJoinPool($0.state), entries: $0.entries,
                lastMessage: $0.lastMessage.map(CoinJoinMessage.init))
        }
        status = CoinJoinCode(ffi.status)
        queueSize = ffi.queueSize
        keysLeft = ffi.keysLeft
    }
}

public struct CoinJoinRecovery: Sendable, Hashable {
    public let coinJoinAddressesScanned: UInt32
    public let bip44AddressesScanned: UInt32
    public let coinJoinBalance: Amount
    public let newTransactions: UInt32
}

public enum MixedCoinsTarget: Sendable, Hashable {
    case wallet, shielded

    var ffi: DashWalletCore.MixedCoinsDestination {
        switch self {
        case .wallet: .wallet
        case .shielded: .shielded
        }
    }
}

public struct MixedCoinsPlanChunk: Sendable, Hashable {
    public let inputs: UInt32
    public let amount: Amount
    public let fee: Amount
}

public struct MixedCoinsPlan: Sendable, Hashable {
    public let total: Amount
    public let chunks: [MixedCoinsPlanChunk]
}

public struct MixedCoinsOutcome: Sendable, Hashable {
    public let txids: [String]
    public let moved: Amount
    public let remaining: Amount
    public let failureCode: String?
}

public struct NetworkStatsSnapshot: Sendable, Hashable {
    /// Full-node data: `nil` on SPV.
    public let creditPoolTotalLocked: Amount?
    public let instantSendVerified: UInt32?
    public let masternodes: MasternodeCount?
    public let evonodes: MasternodeCount?
    public let bestChainLock: ChainLockInfo?
    public let quorums: [(name: String, type: UInt8, active: UInt32, healthPercent: Double?, rotated: Bool)]

    public static func == (a: Self, b: Self) -> Bool {
        a.creditPoolTotalLocked == b.creditPoolTotalLocked && a.instantSendVerified == b.instantSendVerified
            && a.masternodes == b.masternodes && a.evonodes == b.evonodes && a.bestChainLock == b.bestChainLock
            && a.quorums.map(\.name) == b.quorums.map(\.name)
    }

    public func hash(into h: inout Hasher) {
        h.combine(masternodes)
        h.combine(bestChainLock)
        h.combine(quorums.map(\.name))
    }
}

// MARK: Calls

extension EngineClient {
    public nonisolated func coinJoinLimits() -> CoinJoinLimitValues {
        let l = DashWalletCore.coinjoinLimits()
        return CoinJoinLimitValues(
            denominations: l.denominations.map { Amount(duffs: Int64(clamping: $0)) },
            minimumMixingBalance: Amount(duffs: Int64(clamping: l.minMixingBalance)),
            rounds: l.minRounds...l.maxRounds, sessions: l.minSessions...l.maxSessions,
            targetAmountDash: l.minAmountDash...l.maxAmountDash, denoms: l.minDenoms...l.maxDenoms,
            defaults: CoinJoinOptions(l.defaults))
    }

    public func coinJoinOptions(on network: DashNetwork) throws(DashKitError) -> CoinJoinOptions {
        let session = try session(network)
        return CoinJoinOptions(try mapped { try session.coinjoinSettings() })
    }

    public func setCoinJoinOptions(on network: DashNetwork, _ options: CoinJoinOptions) async throws(DashKitError) {
        let session = try session(network)
        let ffi = options.ffi
        try await mapped { try await session.setCoinjoinSettings(settings: ffi) }
    }

    public func coinJoinStatus(on network: DashNetwork, wallet: WalletID) throws(DashKitError)
        -> CoinJoinWalletStatus
    {
        let session = try session(network)
        return try CoinJoinWalletStatus(try mapped { try session.coinjoinStatus(walletId: wallet.hex) })
    }

    public func startMixing(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.startMixing(walletId: wallet.hex) }
    }

    public func stopMixing(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.stopMixing(walletId: wallet.hex) }
    }

    public func coinJoinSalt(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> String {
        let session = try session(network)
        return try await mapped { try await session.coinjoinSalt(walletId: wallet.hex) }
    }

    public func setCoinJoinSalt(on network: DashNetwork, wallet: WalletID, salt: String) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.setCoinjoinSalt(walletId: wallet.hex, saltHex: salt) }
    }

    public func generateCoinJoinSalt(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> String {
        let session = try session(network)
        return try await mapped { try await session.generateCoinjoinSalt(walletId: wallet.hex) }
    }

    public func coinJoinRecoveryScan(on network: DashNetwork, wallet: WalletID) async throws(DashKitError)
        -> CoinJoinRecovery
    {
        let session = try session(network)
        let r = try await mapped { try await session.coinjoinRecoveryScan(walletId: wallet.hex) }
        return CoinJoinRecovery(
            coinJoinAddressesScanned: r.coinjoinAddressesScanned, bip44AddressesScanned: r.bip44AddressesScanned,
            coinJoinBalance: Amount(duffs: Int64(clamping: r.coinjoinBalance)), newTransactions: r.newTransactions)
    }

    public func mixedCoinsPlan(on network: DashNetwork, wallet: WalletID, target: MixedCoinsTarget)
        async throws(DashKitError) -> MixedCoinsPlan
    {
        let session = try session(network)
        let destination = target.ffi
        let p = try await mapped {
            try await session.mixedCoinsSweepPlan(walletId: wallet.hex, destination: destination)
        }
        return MixedCoinsPlan(
            total: Amount(duffs: Int64(clamping: p.total)),
            chunks: p.chunks.map {
                MixedCoinsPlanChunk(
                    inputs: $0.inputs, amount: Amount(duffs: Int64(clamping: $0.amount)),
                    fee: Amount(duffs: Int64(clamping: $0.fee)))
            })
    }

    public func moveMixedCoins(on network: DashNetwork, wallet: WalletID, target: MixedCoinsTarget, grantID: String)
        async throws(DashKitError) -> MixedCoinsOutcome
    {
        let session = try session(network)
        let destination = target.ffi
        let r = try await mapped {
            try await session.moveMixedCoins(walletId: wallet.hex, destination: destination, grantId: grantID)
        }
        return MixedCoinsOutcome(
            txids: r.txids, moved: Amount(duffs: Int64(clamping: r.moved)),
            remaining: Amount(duffs: Int64(clamping: r.remaining)), failureCode: r.failureCode)
    }

    public func networkStats(on network: DashNetwork) throws(DashKitError) -> NetworkStatsSnapshot {
        let session = try session(network)
        let s = try mapped { try session.networkStats() }
        return NetworkStatsSnapshot(
            creditPoolTotalLocked: s.creditPool.map { Amount(duffs: Int64(clamping: $0.totalLocked)) },
            instantSendVerified: s.instantsend?.verified,
            masternodes: s.masternodes.map { MasternodeCount(total: $0.total, enabled: $0.enabled) },
            evonodes: s.evonodes.map { MasternodeCount(total: $0.total, enabled: $0.enabled) },
            bestChainLock: s.bestChainlock.map {
                ChainLockInfo(height: $0.height, blockHash: $0.blockHash, blockTime: $0.blockTime.engineDate)
            },
            quorums: s.quorums.map {
                (name: $0.llmqName, type: $0.llmqType, active: $0.active, healthPercent: $0.healthPercent,
                 rotated: $0.rotated)
            })
    }
}
