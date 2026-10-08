// M3 demo state over the demo world: CoinJoin options and per-wallet mixing,
// the Network sub-tab's statistics and the masternode keychain. The rules
// are the engine's (m3-engine.md §2): option ranges, the 0.00140001 DASH
// minimum, the vault states mixing accepts, ≤ 100 keys per call and the
// `RevealSecret` grant of a key reveal. "Move mixed coins" builds
// transactions the demo cannot make and answers `not_implemented`.
import Foundation
import WalletFeatures
import WalletRuntime

@MainActor
final class DemoM3World {
    let world: DemoWorld
    let startedAt: Date

    var coinJoinSettings: [DashNetwork: CoinJoinSettings] = [:]
    var mixingSince: [WalletID: Date] = [:]
    var stopReasons: [WalletID: CoinJoinStopReason] = [:]
    var salts: [WalletID: String] = [:]

    nonisolated let coinJoinChanges = DemoBroadcaster<WalletID>()
    nonisolated let network: DashNetwork

    init(world: DemoWorld) {
        self.world = world
        network = world.network
        startedAt = world.now()
    }

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
        var rng = DemoRandom(text: "cj-session|" + network.description)
        let port = network == .mainnet ? 9_999 : 19_999
        let sessions: [CoinJoinSessionInfo] = mixing
            ? [CoinJoinSessionInfo(
                proTxHash: rng.hex(bytes: 32),
                service: "\(34 + rng.next() % 9).\(rng.next() % 200 + 20).\(rng.next() % 250).\(rng.next() % 250):\(port)",
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

    /// A mainnet-sized sample list (the demo has no masternode list).
    func statistics() -> NetworkStatistics {
        var rng = DemoRandom(text: "chainlock|\(tip)")
        return NetworkStatistics(
            creditPool: nil, instantSend: nil, masternodes: MasternodeCount(total: 2_120, enabled: 2_034),
            evonodes: MasternodeCount(total: 152, enabled: 149),
            bestChainLock: ChainLockInfo(height: tip, blockHash: rng.hex(bytes: 32), blockDate: world.now()),
            quorums: [
                QuorumSummary(name: "llmq_50_60", type: 1, active: 24, healthPercent: 98.4, rotated: false),
                QuorumSummary(name: "llmq_60_75", type: 5, active: 32, healthPercent: 97.1, rotated: true),
                QuorumSummary(name: "llmq_100_67", type: 4, active: 24, healthPercent: nil, rotated: false),
            ])
    }

    // MARK: Masternode keychain

    /// Derived provider keys of the wallet (public data), ≤ 100 per call;
    /// platform node keys stop at the 20 the engine pre-derives.
    func keys(_ wallet: WalletID, role: MasternodeKeyRole, range: Range<UInt32>) throws(ServiceError) -> [MasternodeKeyInfo] {
        guard range.count <= 100 else { throw .demo(.invalidArgument, "count > 100") }
        let info = try self.wallet(wallet)
        guard !info.watchOnly else { throw .demo(.masternodeWatchOnly) }
        let coin = network == .mainnet ? 5 : 1
        let account: Int =
            switch role {
            case .voting: 1
            case .owner: 2
            case .operator: 3
            case .platformNode: 4
            }
        let indexes = role == .platformNode ? range.clamped(to: 0..<20) : range
        return indexes.map { index in
            var rng = DemoRandom(text: "mnkey|\(wallet.hex)|\(role)|\(index)")
            let isSecp = role == .owner || role == .voting
            return MasternodeKeyInfo(
                role: role, index: index,
                derivationPath: "m/9'/\(coin)'/3'/\(account)'/\(index)" + (role == .platformNode ? "'" : ""),
                address: isSecp ? rng.address(on: network) : nil,
                publicKeyHex: role == .operator ? rng.hex(bytes: 48) : role == .platformNode ? rng.hex(bytes: 32) : "02" + rng.hex(bytes: 32),
                legacyPublicKeyHex: role == .operator ? rng.hex(bytes: 48) : nil,
                platformNodeID: role == .platformNode ? rng.hex(bytes: 20) : nil)
        }
    }

    func revealKey(_ wallet: WalletID, role: MasternodeKeyRole, index: UInt32, grant: AuthGrant) throws(ServiceError)
        -> RevealedMasternodeKey
    {
        try world.check(grant, .revealSecret, wallet: wallet, refuse: .masternode, locked: .masternodeVaultLocked)
        try world.redeem(grant, .revealSecret, wallet: wallet, refuse: .masternode)
        var rng = DemoRandom(text: "mnpriv|\(wallet.hex)|\(role)|\(index)")
        let key = rng.hex(bytes: 32)
        return RevealedMasternodeKey(
            privateKeyHex: DemoSecret(utf8: key),
            wif: role == .owner || role == .voting ? DemoSecret(utf8: "c" + DemoAddress.base58(rng.bytes(37))) : nil,
            tenderdashKey: role == .platformNode ? DemoSecret(utf8: key + rng.hex(bytes: 32)) : nil)
    }
}
