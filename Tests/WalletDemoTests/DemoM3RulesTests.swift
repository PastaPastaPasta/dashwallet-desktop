// The demo M3 services answer like the engine (docs/contracts/m3-engine.md):
// CoinJoin option ranges and start rules, dash-qt's progress formula, the
// 1-hour vote rule, proposal field rules and the 1 DASH collateral, the
// last-4 gate before a registration is sent, ≤ 100 keys, 2 MiB envelopes,
// and typed `not_implemented` where the demo builds no transaction or the
// engine call is Platform work. The M3 view models run against it.
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

    // MARK: Governance (QT-128…134)

    @Test func QT128_activeProposalsNeedGovSyncOn() async throws {
        let (env, _, m3) = make()
        #expect(await Self.code { _ = try await m3.governance.proposals(ProposalQuery(source: .active)) } == .governanceSyncDisabled)
        try await m3.governance.setSyncEnabled(true)
        let rows = try await m3.governance.proposals(ProposalQuery(source: .active))
        #expect(rows.count >= 5)
        #expect(rows.first?.status == .funded)
        let wallet = try #require(env.walletState.selectedWalletID)
        let mine = try await m3.governance.proposals(ProposalQuery(source: .mine(wallet)))
        #expect(mine.map(\.name) == ["demo-wallet-docs"])
        let filtered = try await m3.governance.proposals(ProposalQuery(source: .active, titleFilter: "DASHPAY"))
        #expect(filtered.map(\.name) == ["dashpay-ux-research"])
        let detail = try await m3.governance.detail(hash: rows[0].hash)
        #expect(detail.rawJSON.hasPrefix("{\"name\":\"dash-core-group-q4\",\"payment_address\":"))
    }

    @Test func QT131_votesFollowTheOneHourRule() async throws {
        let (env, _, m3) = make()
        try await m3.governance.setSyncEnabled(true)
        let wallet = try #require(env.walletState.selectedWalletID)
        let row = try #require(try await m3.governance.proposals(ProposalQuery(source: .active)).first { $0.status == .voting })
        let voters = try await m3.voting.votingMasternodes(proposal: row.hash, wallet: wallet)
        #expect(voters.map(\.weight).sorted() == [1, 4])
        let grant = try await env.auth.authorize(.governance, wallet: wallet, credential: .unencrypted)
        let results = try await m3.voting.cast(.yes, on: row.hash, with: voters.map(\.proTxHash), grant: grant)
        #expect(results.allSatisfy { $0.errorCode == nil })
        let after = try #require(try await m3.governance.proposals(ProposalQuery(source: .active)).first { $0.hash == row.hash })
        #expect(after.yes == row.yes + 5)
        #expect(after.myVotes == MyVotes(yes: 5, no: 0, abstain: 0, unvoted: 0))
        let again = try await env.auth.authorize(.governance, wallet: wallet, credential: .unencrypted)
        let refused = try await m3.voting.cast(.no, on: row.hash, with: voters.map(\.proTxHash), grant: again)
        #expect(refused.allSatisfy { $0.errorCode == .governanceVoteTooOften })
        // A used grant is refused.
        #expect(await Self.code { _ = try await m3.voting.cast(.no, on: row.hash, with: [], grant: grant) } == .governanceGrantInvalid)
    }

    @Test func QT132_QT133_createPaysTheCollateralThenBroadcastsAtOneConfirmation() async throws {
        let (env, _, m3) = make()
        try await m3.governance.setSyncEnabled(true)
        let wallet = try #require(env.walletState.selectedWalletID)
        let dates = try await m3.proposals.superblockDates(count: 12)
        #expect(dates.count == 12)
        #expect(dates[1].height - dates[0].height == 24)
        var draft = ProposalDraft(
            name: "Bad Name", url: "https://x.org", paymentAddress: "nope", paymentAmount: .zero, paymentCount: 13,
            firstSuperblockHeight: 1)
        #expect(try await m3.proposals.validate(draft) == [.name, .paymentAddress, .paymentAmount, .paymentCount, .firstPayment])
        let address = try await env.receive.currentAddress(wallet: wallet)
        draft = ProposalDraft(
            name: "demo-proposal", url: "https://x.org", paymentAddress: address.address,
            paymentAmount: Amount(duffs: 5 * Amount.duffsPerDash), paymentCount: 2, firstSuperblockHeight: dates[2].height)
        #expect(try await m3.proposals.validate(draft).isEmpty)
        let json = try await m3.proposals.json(draft)
        #expect(json.hasPrefix("{\"name\":\"demo-proposal\",\"payment_address\":"))
        #expect(json.hasSuffix(",\"type\":1}"))
        let before = try #require(env.walletState.balances).total
        let small = try await env.auth.authorize(.spend(max: Amount(duffs: 1)), wallet: wallet, credential: .unencrypted)
        #expect(await Self.code { _ = try await m3.proposals.create(wallet: wallet, draft: draft, grant: small) } == .governanceGrantInvalid)
        let grant = try await env.auth.authorize(
            .spend(max: Amount(duffs: Amount.duffsPerDash + 100_000)), wallet: wallet, credential: .unencrypted)
        let pending = try await m3.proposals.create(wallet: wallet, draft: draft, grant: grant)
        #expect(pending.collateralStatus == .pending)
        let after = try #require(env.walletState.balances).total
        #expect(before.duffs - after.duffs == Amount.duffsPerDash + 226)
        #expect(await Self.code { _ = try await m3.proposals.submit(wallet: wallet, hash: pending.hash) } == .governanceCollateralUnconfirmed)
        clock.advance(160)
        #expect(try await m3.proposals.pending(wallet: wallet).first?.collateralStatus == .ready)
        let hash = try await m3.proposals.submit(wallet: wallet, hash: pending.hash)
        #expect(hash == pending.hash)
        #expect(try await m3.proposals.pending(wallet: wallet).isEmpty)
        let mine = try await m3.governance.proposals(ProposalQuery(source: .mine(wallet)))
        #expect(mine.contains { $0.name == "demo-proposal" && $0.status == .confirming })
    }

    @Test func QT026_QT134_clockAndInfoComeFromTheTip() async throws {
        let (_, _, m3) = make()
        let unsynced = try await m3.governance.info()
        #expect(unsynced.passingThreshold == nil && unsynced.budgetAvailable == nil)
        try await m3.governance.setSyncEnabled(true)
        let info = try await m3.governance.info()
        #expect(info.superblockCycle == 24)
        #expect(info.votingCutoff == info.nextSuperblock.map { $0 - 8 })
        #expect(info.votesControlled == 5)
        let clock = try await m3.governance.clock()
        #expect(clock.nextSuperblock == info.nextSuperblock)
        #expect(clock.blocksToSuperblock <= 24)
        #expect(clock.budgetCommitted != nil)
    }

    // MARK: Masternodes (QT-118…127, IOS-080…083)

    @Test func QT118_QT120_listFiltersAndOwnedRows() async throws {
        let (_, _, m3) = make()
        let all = try await m3.masternodes.list(MasternodeQuery())
        #expect(all.count == 14)
        #expect(try await m3.masternodes.list(MasternodeQuery(typeFilter: .evo)).count == 4)
        #expect(try await m3.masternodes.list(MasternodeQuery(typeFilter: .shared)).count == 1)
        let owned = try await m3.masternodes.list(MasternodeQuery(ownedOnly: true))
        #expect(owned.count == 3)
        #expect(try await m3.masternodes.list(MasternodeQuery(hideBanned: true)).count == 12)
        // Foreign masternodes keep SPV-unknown values unknown.
        let foreign = try #require(all.first { $0.ownedRoles.isEmpty })
        #expect(foreign.poseScore == nil && foreign.ownerAddress == nil && foreign.operatorReward == nil)
        let state = try await m3.masternodes.state()
        #expect(state.total == 14 && state.evoTotal == 4)
    }

    @Test func QT123_QT124_registrationGateAndFunds() async throws {
        let (env, _, m3) = make()
        let wallet = try #require(env.walletState.selectedWalletID)
        let payout = try await env.receive.currentAddress(wallet: wallet).address
        func request(_ collateral: CollateralChoice) -> RegistrationRequest {
            RegistrationRequest(
                wallet: wallet, type: .regular, collateral: collateral, serviceAddresses: ["1.2.3.4:19999"],
                ownerAddress: nil, votingAddress: nil, operatorKey: .generate, payoutAddress: payout,
                operatorRewardX100: 0, platform: nil, feeSource: .automatic)
        }
        let grant = try await env.auth.authorize(.masternodeOperation, wallet: wallet, credential: .unencrypted)
        #expect(await Self.code { _ = try await m3.registration.prepare(request(.fundNew), grant: grant) } == .masternodeInsufficientFunds)
        let external = try await m3.registration.prepare(
            request(.external(OutPoint(txid: String(repeating: "ab", count: 32), vout: 1))), grant: grant)
        #expect(external.summary.operatorSecretRequired)
        #expect(external.summary.collateralSignMessage != nil)
        #expect(await Self.code { _ = try await m3.registration.submit(external, collateralSignature: "H" + String(repeating: "a", count: 40)) } == .masternodeOperatorSecretUnconfirmed)
        let secret = try await m3.registration.operatorSecret(external)
        let hex = secret.secretHex.withUnsafeBytes { String(decoding: $0, as: UTF8.self) }
        #expect(hex.count == 64)
        #expect(try await m3.registration.confirmOperatorSecret(external, last4: "zzzz") == false)
        #expect(try await m3.registration.confirmOperatorSecret(external, last4: String(hex.suffix(4)).uppercased()))
        #expect(await Self.code { _ = try await m3.registration.submit(external, collateralSignature: nil) } == .masternodeCollateralSignatureInvalid)
        let proTxHash = try await m3.registration.submit(external, collateralSignature: "H" + String(repeating: "a", count: 40))
        let row = try #require(try await m3.masternodes.list(MasternodeQuery(text: proTxHash)).first)
        #expect(row.ownedRoles.contains(.owner))
    }

    @Test func QT125_IOS081_unbanRevivesABannedNode() async throws {
        let (env, m2, m3) = make()
        let banned = try #require(try await m3.masternodes.list(MasternodeQuery()).first {
            if case .banned = $0.status { return true }
            return false
        })
        let model = MasternodeMaintenanceViewModel(kind: .unban, proTxHash: banned.proTxHash, env: env, m2: m2, m3: m3)
        await model.load()
        #expect(model.isBanned)
        await model.prepare()
        await model.broadcast()
        #expect(model.message == "Service update sent. The masternode stays banned until the transaction confirms.")
        let after = try await m3.masternodes.detail(proTxHash: banned.proTxHash)
        if case .active = after.row.status {} else { Issue.record("still banned: \(after.row.status)") }
    }

    @Test func IOS080_evonodePlatformCallsAreNotImplemented() async throws {
        let (env, _, m3) = make()
        let evo = try #require(try await m3.masternodes.list(MasternodeQuery(typeFilter: .evo)).first)
        #expect(await Self.code { _ = try await m3.evonodes.status(proTxHash: evo.proTxHash) } == .notImplemented)
        let wallet = try #require(env.walletState.selectedWalletID)
        let grant = try await env.auth.authorize(.masternodeOperation, wallet: wallet, credential: .unencrypted)
        #expect(await Self.code {
            _ = try await m3.evonodes.withdraw(proTxHash: evo.proTxHash, credits: 1, destination: .payoutAddress, grant: grant)
        } == .notImplemented)
    }

    @Test func IOS083_keychainLimitsAndReveal() async throws {
        let (env, _, m3) = make()
        let wallet = try #require(env.walletState.selectedWalletID)
        #expect(await Self.code { _ = try await m3.keychain.keys(wallet: wallet, role: .voting, range: 0..<101) } == .invalidArgument)
        let keys = try await m3.keychain.keys(wallet: wallet, role: .operator, range: 0..<20)
        #expect(keys.count == 20)
        #expect(keys[0].derivationPath == "m/9'/1'/3'/3'/0")
        #expect(keys[0].publicKeyHex.count == 96)
        #expect(!keys[0].usedBy.isEmpty)
        let spend = try await env.auth.authorize(.spend(max: Amount(duffs: 1)), wallet: wallet, credential: .unencrypted)
        #expect(await Self.code { _ = try await m3.keychain.reveal(wallet: wallet, role: .owner, index: 0, grant: spend) } == .masternodeGrantInvalid)
    }

    @Test func IOS082_trackAttachAndReveal() async throws {
        let (env, _, m3) = make()
        let wallet = try #require(env.walletState.selectedWalletID)
        let target = try #require(try await m3.masternodes.list(MasternodeQuery()).last)
        let found = try await m3.tracked.locate(String(target.service!.prefix(6)))
        #expect(found.contains { $0.proTxHash == target.proTxHash })
        _ = try await m3.tracked.track(proTxHash: target.proTxHash, label: "watch")
        #expect(await Self.code { _ = try await m3.tracked.track(proTxHash: target.proTxHash, label: nil) } == .masternodeAlreadyTracked)
        let grant = try await env.auth.authorize(.masternodeOperation, wallet: wallet, credential: .unencrypted)
        #expect(await Self.code {
            try await m3.tracked.attach(env.vault.makeSecret(utf8: "short"), role: .voting, proTxHash: target.proTxHash, grant: grant)
        } == .masternodeInvalidKey)
        try await m3.tracked.attach(
            env.vault.makeSecret(utf8: String(repeating: "1f", count: 32)), role: .voting, proTxHash: target.proTxHash,
            grant: grant)
        let node = try #require(try await m3.tracked.tracked().first)
        #expect(node.attachedRoles == [.voting] && node.capabilities.canVote)
        #expect(node.row.ownedRoles.contains(.tracked))
        #expect(try await m3.tracked.untrack(proTxHash: target.proTxHash))
    }

    @Test func QT126_QT127_sharedEnvelopesRouteAndLimits() async throws {
        let (env, _, m3) = make()
        let wallet = try #require(env.walletState.selectedWalletID)
        let terms = SharedMasternodeTerms(
            shares: [SharedShareTerms(amount: Amount(duffs: 600 * Amount.duffsPerDash), label: nil, mine: true),
                     SharedShareTerms(amount: Amount(duffs: 400 * Amount.duffsPerDash), label: nil, mine: false)],
            earlyPeriodBlocks: 1_000, earlyExitPenalty: Amount(duffs: 10 * Amount.duffsPerDash), serviceAddresses: [],
            operatorKey: .generate, operatorRewardX100: 0)
        let session = try await m3.shared.create(wallet: wallet, terms: terms)
        #expect(session.fingerprint.count == 9 && session.sessionCode.count == 6)
        let envelope = try await m3.shared.message(session: session.id)
        #expect(envelope.json.contains("\"type\":\"dash-shared-mn-session\""))
        let routed = try await m3.shared.importMessage(envelope.json, wallet: wallet)
        #expect(routed == .envelope(session))
        let foreign = envelope.json.replacingOccurrences(of: "\"testnet\"", with: "\"mainnet\"")
        #expect(await Self.code { _ = try await m3.shared.importMessage(foreign, wallet: wallet) } == .masternodeSharedNetworkMismatch)
        #expect(await Self.code {
            _ = try await m3.shared.importMessage(String(repeating: "x", count: 2 * 1024 * 1024 + 1), wallet: wallet)
        } == .masternodeSharedEnvelopeTooLarge)
        let standby = try await m3.shared.importMessage(String(repeating: "ab", count: 40) + "\n" + String(repeating: "cd", count: 40), wallet: wallet)
        if case .standbyDissolution(_, let txs) = standby { #expect(txs.count == 2) } else { Issue.record("not routed to standby") }
        #expect(await Self.code { _ = try await m3.shared.broadcast(session: session.id) } == .notImplemented)
        try await m3.shared.abandon(session: session.id)
        #expect(try await m3.shared.sessions(wallet: wallet).isEmpty)
    }
}
