// M3 demo state over the demo world: CoinJoin options and per-wallet mixing,
// a sample masternode list (two masternodes and a share owned by the sample
// wallet), sample proposals with tallies, votes cast in this run, pending
// proposals, tracked masternodes and shared sessions. The rules are the
// engine's (m3-engine.md §2): option ranges, the 0.00140001 DASH minimum,
// the vault states mixing accepts, the 1-hour vote rule, the proposal field
// rules, the last-4 gate before a registration is sent, ≤ 100 keys per call,
// 2 MiB envelopes. Calls whose engine work builds transactions the demo
// cannot make (move mixed coins, the multi-party shared broadcast, standby
// dissolutions) and the Platform calls answer `not_implemented`.
import Foundation
import WalletFeatures
import WalletRuntime

/// One sample masternode and what the demo knows about it.
struct DemoMasternode: Sendable {
    var row: MasternodeRow
    var detail: MasternodeDetail
    /// The demo wallet holds its voting key (it can vote with it).
    var votingKeyInWallet: Bool
}

/// A vote this run cast: per proposal and masternode.
struct DemoVote: Sendable {
    let outcome: VoteOutcome
    let time: Date
}

struct DemoProposal: Sendable {
    var row: ProposalRow
    var parentHash: String
    var collateralTxid: String
    var createdAt: Date
    var payments: Int
    var mine: Bool
}

struct DemoPendingProposal: Sendable {
    var proposal: PendingProposal
    var draft: ProposalDraft
    /// The collateral's height: confirmations follow the demo's block clock.
    var createdAt: Date
}

@MainActor
final class DemoM3World {
    let world: DemoWorld
    let m2: DemoM2World
    let startedAt: Date

    var coinJoinSettings: [DashNetwork: CoinJoinSettings] = [:]
    var mixingSince: [WalletID: Date] = [:]
    var stopReasons: [WalletID: CoinJoinStopReason] = [:]
    var salts: [WalletID: String] = [:]
    var governanceSyncOn = false
    var masternodes: [DemoMasternode] = []
    var proposals: [DemoProposal] = []
    var votes: [String: [String: DemoVote]] = [:]
    var pending: [WalletID: [DemoPendingProposal]] = [:]
    var tracked: [String: (label: String?, attached: [MasternodeKeyRole: String])] = [:]
    var sharedSessions: [String: SharedSessionInfo] = [:]
    var registrations: [UUID: (summary: RegistrationSummary, secret: String?, gateOpen: Bool, wallet: WalletID)] = [:]
    var providerTransactions: [UUID: (ProviderTransactionSummary, DemoProviderEffect)] = [:]

    nonisolated let coinJoinChanges = DemoBroadcaster<WalletID>()
    nonisolated let governanceChanges = DemoBroadcaster<Void>()
    nonisolated let masternodeChanges = DemoBroadcaster<Void>()
    nonisolated let network: DashNetwork

    init(world: DemoWorld, m2: DemoM2World) {
        self.world = world
        self.m2 = m2
        network = world.network
        startedAt = world.now()
        let sample = DemoM3Sample(network: network, now: startedAt)
        masternodes = sample.masternodes
        proposals = sample.proposals
    }

    var parameters: GovernanceParameters { M3Defaults.governanceParameters(network) }
    var tip: UInt32 { world.sync.tipHeight ?? DemoLedger.tipHeight }

    func wallet(_ id: WalletID) throws(ServiceError) -> WalletInfo {
        guard let info = world.current.wallets.first(where: { $0.id == id }) else { throw .demo(.walletNotFound) }
        return info
    }

    // MARK: CoinJoin

    var settings: CoinJoinSettings { coinJoinSettings[world.network] ?? .dashQtDefaults }

    func setSettings(_ settings: CoinJoinSettings) throws(ServiceError) {
        if let field = M3Defaults.invalidCoinJoinField(settings) { throw .demo(.invalidArgument, field) }
        coinJoinSettings[world.network] = settings
        if !settings.enabled {
            for id in mixingSince.keys { stop(id, reason: .disabled) }
        }
    }

    private func stop(_ id: WalletID, reason: CoinJoinStopReason) {
        guard mixingSince[id] != nil else { return }
        mixingSince[id] = nil
        stopReasons[id] = reason
        coinJoinChanges.send(id)
    }

    /// The sample wallet's CoinJoin coins: a third of its confirmed balance
    /// is denominated and a share of that is fully mixed, growing while
    /// mixing runs (one round per demo minute). Other wallets start empty.
    func status(_ id: WalletID) throws(ServiceError) -> CoinJoinStatus {
        let info = try wallet(id)
        if mixingSince[id] != nil, world.vault.state == .locked { stop(id, reason: .vaultLocked) }
        let settings = self.settings
        let limits = M3Defaults.coinJoinLimits
        let confirmed = info.balances?.confirmed.duffs ?? 0
        let isSample = world.phrase(of: id) == DemoEnvironment.sampleWalletPhrase
        let minutes = mixingSince[id].map { max(0, world.now().timeIntervalSince($0) / 60) } ?? 0
        let denominated = isSample ? confirmed / 3 : 0
        let roundsDone = min(Double(settings.rounds), 1.5 + minutes)
        let mixedShare = isSample ? min(1, 0.3 + minutes / 20) : 0
        let fullyMixed = Int64(Double(denominated) * mixedShare)
        let normalized = Int64(Double(denominated) * roundsDone / Double(settings.rounds))
        let anonymizable = max(0, confirmed - fullyMixed)
        let target = Int64(settings.targetAmountDash) * Amount.duffsPerDash
        let maxAmount = min(anonymizable + fullyMixed, target)
        let unavailable: CoinJoinUnavailable? =
            !settings.enabled ? .disabled
            : info.watchOnly ? .watchOnly
            : confirmed < limits.minimumMixingBalance.duffs ? .insufficientFunds(minimum: limits.minimumMixingBalance)
            : nil
        let mixing = mixingSince[id] != nil
        let progress = Self.progress(
            anonymizable: anonymizable, denominated: denominated, normalized: normalized, fullyMixed: fullyMixed,
            target: target, rounds: settings.rounds, averageRounds: isSample ? roundsDone : 0)
        let sessions: [CoinJoinSessionInfo] = mixing
            ? [CoinJoinSessionInfo(
                proTxHash: masternodes.first?.row.proTxHash, service: masternodes.first?.row.service,
                denomination: limits.denominations[2], state: .queue, entries: 1, lastMessage: .entriesAdded)]
            : []
        return CoinJoinStatus(
            wallet: id, state: mixing ? .mixing : .idle, stopReason: mixing ? nil : stopReasons[id],
            unavailable: unavailable,
            balances: CoinJoinBalances(
                anonymizable: Amount(duffs: anonymizable), denominated: Amount(duffs: denominated),
                normalizedAnonymized: Amount(duffs: normalized), fullyMixed: Amount(duffs: fullyMixed)),
            progress: progress,
            amountAndRounds: CoinJoinAmountAndRounds(
                amount: Amount(duffs: maxAmount), rounds: settings.rounds, insufficientInputs: maxAmount < target),
            submittedDenominations: mixing ? [limits.denominations[2]] : [], sessions: sessions,
            status: mixing ? .waitingInQueue : .idle, queueSize: mixing ? 2 : 0, keysLeft: nil)
    }

    /// dash-qt's progress formula (research 02 §9.2).
    nonisolated static func progress(
        anonymizable: Int64, denominated: Int64, normalized: Int64, fullyMixed: Int64, target: Int64, rounds: Int,
        averageRounds: Double
    ) -> CoinJoinProgress {
        let maxAmount = Double(min(anonymizable + fullyMixed, target))
        guard maxAmount > 0 else {
            return CoinJoinProgress(overall: 0, denominated: 0, partiallyMixed: 0, mixed: 0, averageRounds: 0)
        }
        func part(_ value: Int64) -> Double { min(1, Double(value) / maxAmount) * 100 }
        let denom = part(denominated), norm = part(normalized), full = part(fullyMixed)
        let divisor = Double(3 + rounds)
        func weighted(_ value: Double, _ weight: Double) -> Double { (value * weight / divisor * 100).rounded(.up) / 100 }
        let overall = min(100, weighted(denom, 1) + weighted(norm, Double(rounds)) + weighted(full, 2))
        return CoinJoinProgress(
            overall: overall, denominated: denom, partiallyMixed: norm, mixed: full, averageRounds: averageRounds)
    }

    /// `start_mixing`: idempotent; refused while disabled, watch-only, below
    /// the minimum, or with a locked vault (mixing-only unlock is enough).
    func start(_ id: WalletID) throws(ServiceError) {
        let current = try status(id)
        switch current.unavailable {
        case .disabled: throw .demo(.coinjoinDisabled)
        case .watchOnly: throw .demo(.coinjoinWatchOnly)
        case .insufficientFunds(let minimum): throw .demo(.coinjoinInsufficientFunds, parameters: ["min_duffs": minimum.duffs])
        case nil: break
        }
        switch world.vault.state {
        case .locked, .noVault: throw .demo(.coinjoinVaultLocked)
        case .noKeys, .unencrypted, .unlocked, .unlockedMixingOnly: break
        }
        guard mixingSince[id] == nil else { return }
        mixingSince[id] = world.now()
        stopReasons[id] = nil
        coinJoinChanges.send(id)
    }

    func stopMixing(_ id: WalletID) throws(ServiceError) {
        _ = try wallet(id)
        stop(id, reason: .userRequested)
    }

    func salt(_ id: WalletID) throws(ServiceError) -> String {
        _ = try wallet(id)
        if let salt = salts[id] { return salt }
        var rng = DemoRandom(text: "cj_salt|" + id.hex)
        let salt = rng.hex(bytes: 32)
        salts[id] = salt
        return salt
    }

    func setSalt(_ salt: String, _ id: WalletID) throws(ServiceError) {
        guard M3Defaults.isValidSalt(salt) else { throw .demo(.invalidArgument, "salt") }
        _ = try wallet(id)
        guard mixingSince[id] == nil else { throw .demo(.invalidArgument, "mixing") }
        salts[id] = salt
    }

    func generateSalt(_ id: WalletID) throws(ServiceError) -> String {
        guard mixingSince[id] == nil else { throw .demo(.invalidArgument, "mixing") }
        _ = try wallet(id)
        var rng = DemoRandom(text: UUID().uuidString)
        let salt = rng.hex(bytes: 32)
        salts[id] = salt
        return salt
    }

    func sweepPlan(_ id: WalletID, destination: MixedCoinsDestination) throws(ServiceError) -> MixedCoinsSweepPlan {
        if destination == .shielded { throw .demo(.notImplemented, "NetworkSession.mixed_coins_sweep_plan.shielded") }
        let mixed = try status(id).balances.fullyMixed
        guard mixed > .zero else { throw .demo(.coinjoinNothingToMove) }
        let inputs = max(1, Int(mixed.duffs / 100_001_000))
        let chunks = stride(from: 0, to: inputs, by: 500).map { start -> MixedCoinsChunk in
            let count = min(500, inputs - start)
            let fee = Int64(10 + 148 * count + 34)
            return MixedCoinsChunk(
                inputs: count, amount: Amount(duffs: mixed.duffs * Int64(count) / Int64(inputs) - fee),
                fee: Amount(duffs: fee))
        }
        return MixedCoinsSweepPlan(destination: destination, total: mixed, chunks: chunks)
    }

    // MARK: Network statistics

    func statistics() -> NetworkStatistics {
        let rows = masternodes.map(\.row)
        func count(_ type: MasternodeType) -> MasternodeCount {
            let typed = rows.filter { $0.type == type }
            return MasternodeCount(
                total: typed.count,
                enabled: typed.filter { if case .active = $0.status { return true } else { return false } }.count)
        }
        return NetworkStatistics(
            creditPool: nil, instantSend: nil, masternodes: count(.regular), evonodes: count(.evo),
            bestChainLock: ChainLockInfo(height: tip, blockHash: DemoM3Sample.hash("chainlock|\(tip)"), blockDate: world.now()),
            quorums: [
                QuorumSummary(name: "llmq_50_60", type: 1, active: 24, healthPercent: 98.4, rotated: false),
                QuorumSummary(name: "llmq_60_75", type: 5, active: 32, healthPercent: 97.1, rotated: true),
                QuorumSummary(name: "llmq_100_67", type: 4, active: 24, healthPercent: nil, rotated: false),
            ])
    }
}
