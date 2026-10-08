// The demo M3 services answer like the engine (docs/contracts/m3-engine.md):
// CoinJoin option ranges and start rules, dash-qt's progress formula, ≤ 100
// keys and the reveal's grant, and typed `not_implemented` where the demo
// builds no transaction. The M3 view models run against it.
import Foundation
import Testing
import WalletDemo
import WalletFeatures
import WalletRuntime

/// A clock the test moves (confirmations follow the demo's block clock).
final class DemoTestClock: @unchecked Sendable {
    private let lock = NSLock()
    private var date = Date(timeIntervalSince1970: 1_760_000_000)

    var now: Date { lock.withLock { date } }

    func advance(_ seconds: TimeInterval) {
        lock.withLock { date = date.addingTimeInterval(seconds) }
    }
}

@MainActor
struct DemoM3RulesTests {
    let clock = DemoTestClock()

    func make(_ scenario: DemoScenario = .funded) -> (env: AppEnvironment, m2: M2Services, m3: M3Services) {
        let clock = clock
        return DemoEnvironment.makeWithM3(
            scenario: scenario, timing: Timing(now: { clock.now }), desktopPlatform: .linux)
    }

    static func code(_ body: () async throws -> Void) async -> ServiceErrorCode? {
        do {
            try await body()
            return nil
        } catch {
            return (error as? ServiceError)?.code
        }
    }

    // MARK: CoinJoin (QT-041…046, QT-112, IOS-057)

    @Test func QT046_settingsAreRangeCheckedAndApplyLive() async throws {
        let (_, _, m3) = make()
        #expect(try await m3.coinJoin.settings() == .dashQtDefaults)
        var bad = CoinJoinSettings.dashQtDefaults
        bad.rounds = 17
        #expect(await Self.code { try await m3.coinJoin.setSettings(bad) } == .invalidArgument)
        bad = .dashQtDefaults
        bad.denomsGoal = 400
        #expect(await Self.code { try await m3.coinJoin.setSettings(bad) } == .invalidArgument)
        var good = CoinJoinSettings.dashQtDefaults
        good.rounds = 8
        try await m3.coinJoin.setSettings(good)
        #expect(try await m3.coinJoin.settings().rounds == 8)
    }

    @Test func QT042_progressFollowsDashQtsFormula() async throws {
        let (env, _, m3) = make()
        let wallet = try #require(env.walletState.selectedWalletID)
        let status = try await m3.coinJoin.status(wallet: wallet)
        #expect(status.state == .idle)
        #expect(status.unavailable == nil)
        #expect(status.keysLeft == nil)
        #expect(status.balances.fullyMixed > .zero)
        let b = status.balances
        let target = Int64(1000) * Amount.duffsPerDash
        let maxAmount = Double(min(b.anonymizable.duffs + b.fullyMixed.duffs, target))
        func part(_ v: Amount) -> Double { min(1, Double(v.duffs) / maxAmount) * 100 }
        func weighted(_ v: Double, _ w: Double) -> Double { (v * w / 7 * 100).rounded(.up) / 100 }
        let expected = weighted(part(b.denominated), 1) + weighted(part(b.normalizedAnonymized), 4)
            + weighted(part(b.fullyMixed), 2)
        #expect(abs(status.progress.overall - expected) < 0.0001)
        #expect(status.amountAndRounds.insufficientInputs)
    }

    @Test func QT044_startNeedsAKeyAndStopIsIdempotent() async throws {
        let (env, _, m3) = make(.locked)
        let wallet = try #require(env.walletState.selectedWalletID)
        #expect(await Self.code { try await m3.coinJoin.start(wallet: wallet) } == .coinjoinVaultLocked)
        try await env.auth.unlock(passphrase: env.vault.makeSecret(utf8: DemoEnvironment.passphrase), scope: .mixingOnly)
        try await m3.coinJoin.start(wallet: wallet)
        try await m3.coinJoin.start(wallet: wallet)
        #expect(try await m3.coinJoin.status(wallet: wallet).state == .mixing)
        #expect(try await m3.coinJoin.status(wallet: wallet).status == .waitingInQueue)
        try await m3.coinJoin.stop(wallet: wallet)
        try await m3.coinJoin.stop(wallet: wallet)
        let stopped = try await m3.coinJoin.status(wallet: wallet)
        #expect(stopped.state == .idle && stopped.stopReason == .userRequested)
    }

    @Test func QT112_lockingTheVaultStopsMixing() async throws {
        let (env, _, m3) = make(.locked)
        let wallet = try #require(env.walletState.selectedWalletID)
        try await env.auth.unlock(passphrase: env.vault.makeSecret(utf8: DemoEnvironment.passphrase), scope: .mixingOnly)
        try await m3.coinJoin.start(wallet: wallet)
        try await env.auth.lock()
        let status = try await m3.coinJoin.status(wallet: wallet)
        #expect(status.state == .idle)
        #expect(status.stopReason == .vaultLocked)
    }

    @Test func QT046_disablingCoinJoinStopsEveryWallet() async throws {
        let (env, _, m3) = make()
        let wallet = try #require(env.walletState.selectedWalletID)
        try await m3.coinJoin.start(wallet: wallet)
        var off = CoinJoinSettings.dashQtDefaults
        off.enabled = false
        try await m3.coinJoin.setSettings(off)
        let status = try await m3.coinJoin.status(wallet: wallet)
        #expect(status.state == .idle && status.stopReason == .disabled && status.unavailable == .disabled)
        #expect(await Self.code { try await m3.coinJoin.start(wallet: wallet) } == .coinjoinDisabled)
    }

    @Test func QT043_saltRules() async throws {
        let (env, _, m3) = make()
        let wallet = try #require(env.walletState.selectedWalletID)
        let salt = try await m3.coinJoin.salt(wallet: wallet)
        #expect(M3Defaults.isValidSalt(salt))
        #expect(try await m3.coinJoin.salt(wallet: wallet) == salt)
        #expect(await Self.code { try await m3.coinJoin.setSalt("ABC", wallet: wallet) } == .invalidArgument)
        try await m3.coinJoin.start(wallet: wallet)
        #expect(await Self.code { try await m3.coinJoin.setSalt(String(repeating: "a", count: 64), wallet: wallet) } == .invalidArgument)
    }

    @Test func IOS057_planWorksMoveAndShieldedAreNotImplemented() async throws {
        let (env, _, m3) = make()
        let wallet = try #require(env.walletState.selectedWalletID)
        let plan = try await m3.mixedCoins.plan(wallet: wallet, destination: .wallet)
        #expect(plan.total > .zero && !plan.chunks.isEmpty)
        #expect(await Self.code { _ = try await m3.mixedCoins.plan(wallet: wallet, destination: .shielded) } == .notImplemented)
        let grant = try await env.auth.authorize(.spend(max: plan.total), wallet: wallet, credential: .unencrypted)
        #expect(await Self.code { _ = try await m3.mixedCoins.move(wallet: wallet, destination: .wallet, grant: grant) } == .notImplemented)
    }

    @Test func QT041_panelRunsOverTheDemo() async {
        let (env, m2, m3) = make()
        let panel = CoinJoinPanelViewModel(env: env, m2: m2, m3: m3)
        await panel.reload()
        #expect(panel.phase == .ready)
        #expect(panel.buttonTitle == "Start CoinJoin")
        #expect(panel.amountAndRoundsIsWarning)
        await panel.toggle()
        await panel.acknowledgeHint()
        #expect(panel.isMixing)
        #expect(panel.sessionStatusText == "Submitted to masternode, waiting in queue .")
    }

    // MARK: Masternode keychain (IOS-083)

    @Test func IOS083_keychainLimitsAndReveal() async throws {
        let (env, _, m3) = make()
        let wallet = try #require(env.walletState.selectedWalletID)
        #expect(await Self.code { _ = try await m3.keychain.keys(wallet: wallet, role: .voting, range: 0..<101) } == .invalidArgument)
        let keys = try await m3.keychain.keys(wallet: wallet, role: .operator, range: 0..<20)
        #expect(keys.count == 20)
        #expect(keys[0].derivationPath == "m/9'/1'/3'/3'/0")
        #expect(keys[0].publicKeyHex.count == 96)
        // Platform node keys stop at the engine's 20 pre-derived ones.
        let nodes = try await m3.keychain.keys(wallet: wallet, role: .platformNode, range: 18..<25)
        #expect(nodes.map(\.index) == [18, 19])
        #expect(nodes[0].derivationPath == "m/9'/1'/3'/4'/18'")
        let spend = try await env.auth.authorize(.spend(max: Amount(duffs: 1)), wallet: wallet, credential: .unencrypted)
        #expect(await Self.code { _ = try await m3.keychain.reveal(wallet: wallet, role: .owner, index: 0, grant: spend) } == .masternodeGrantInvalid)
        let reveal = try await env.auth.authorize(.revealSecret, wallet: wallet, credential: .unencrypted)
        let key = try await m3.keychain.reveal(wallet: wallet, role: .owner, index: 0, grant: reveal)
        #expect(key.wif != nil && key.tenderdashKey == nil)
    }
}
