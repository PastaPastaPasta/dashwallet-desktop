// CoinJoin view models (QT-041…051, QT-112, QT-135 CoinJoin tab, IOS-057)
// against the M3 fakes.
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("CoinJoin view models")
struct CoinJoinViewModelTests {
    let world = FakeWorld()
    let m2 = FakeM2World()
    let m3 = FakeM3World()

    init() {
        m3.coinJoin.settingsValue.withLock { $0 = .dashQtDefaults }
        m3.coinJoin.statuses.withLock { $0[walletA] = coinJoinStatus() }
        world.walletState.balances = balances(confirmed: 10_00000000)
        // The first-use hint was seen, unless a test says otherwise.
        m2.desktopPreferences.desktop.m3.coinJoinHintShown = true
    }

    func makePanel() -> CoinJoinPanelViewModel {
        CoinJoinPanelViewModel(env: world.environment(), m2: m2.services, m3: m3.services)
    }

    // MARK: Panel (QT-041…043, QT-047, QT-050)

    @Test func QT041_panelShowsStatusBalanceAndAmountAndRounds() async {
        let panel = makePanel()
        await panel.reload()
        #expect(panel.phase == .ready)
        #expect(panel.isVisible)
        #expect(panel.statusText == "Disabled")
        #expect(panel.buttonTitle == "Start CoinJoin")
        #expect(panel.balanceText.contains("1.50000000"))
        #expect(panel.amountAndRoundsText.hasPrefix("1"))
        #expect(panel.amountAndRoundsText.hasSuffix("DASH / 4 Rounds"))
        #expect(!panel.amountAndRoundsIsWarning)
        #expect(panel.amountAndRoundsTooltip?.hasPrefix("Found enough compatible inputs to mix") == true)
    }

    @Test func QT041_notEnoughInputsShowsTildeInRed() async {
        m3.coinJoin.statuses.withLock {
            $0[walletA] = coinJoinStatus(amount: 6_50000000, insufficientInputs: true)
        }
        let panel = makePanel()
        await panel.reload()
        #expect(panel.amountAndRoundsText == "~6 tDASH / 4 Rounds")
        #expect(panel.amountAndRoundsIsWarning)
        #expect(panel.amountAndRoundsTooltip?.hasPrefix("Not enough compatible inputs to mix") == true)
    }

    @Test func QT041_emptyWalletSaysNoInputsDetected() async {
        m3.coinJoin.statuses.withLock {
            $0[walletA] = coinJoinStatus(anonymizable: 0, fullyMixed: 0, denominated: 0)
        }
        let panel = makePanel()
        await panel.reload()
        #expect(panel.amountAndRoundsTooltip == "No inputs detected")
        #expect(panel.progressPercent == 0)
        #expect(panel.progressTooltipLines == ["No inputs detected"])
    }

    @Test func QT041_disabledFeaturesHideThePanel() async {
        var settings = CoinJoinSettings.dashQtDefaults
        settings.enabled = false
        m3.coinJoin.settingsValue.withLock { $0 = settings }
        let panel = makePanel()
        await panel.reload()
        #expect(!panel.isVisible)
    }

    @Test func QT042_progressTooltipFollowsDashQt() async {
        let panel = makePanel()
        await panel.reload()
        #expect(panel.progressPercent == 42)
        #expect(panel.progressTooltipLines == [
            "Overall progress: 42.50%", "Denominated: 60.00%", "Partially mixed: 30.00%", "Mixed: 25.00%",
            "Denominated inputs have 1.75 of 4 rounds on average",
        ])
    }

    @Test func QT047_advancedModeShowsKeysLeftAndSubmittedDenoms() async {
        m3.coinJoin.statuses.withLock { $0[walletA] = coinJoinStatus(state: .mixing, keysLeft: 80) }
        let panel = makePanel()
        await panel.reload()
        #expect(panel.statusText == "Enabled")
        m2.desktopPreferences.desktop.m3.showAdvancedCoinJoinUI = true
        #expect(panel.statusText == "Enabled, keys left: 80")
        #expect(panel.statusIsWarning)
        #expect(panel.submittedDenominationsText == "n/a")
    }

    @Test func QT048_HDWalletsShowNoKeysLeftAndOnlyEngineUnavailableStates() async {
        m2.desktopPreferences.desktop.m3.showAdvancedCoinJoinUI = true
        m3.coinJoin.statuses.withLock { $0[walletA] = coinJoinStatus(unavailable: .watchOnly) }
        let panel = makePanel()
        await panel.reload()
        #expect(panel.statusText == "Disabled")
        #expect(panel.buttonTitle == "(Disabled)")
        #expect(!panel.buttonEnabled)
        #expect(panel.buttonTooltip == "Watch-only wallets cannot mix.")
    }

    @Test func QT050_sessionStatusTextShowsCoreStatus() async {
        m3.coinJoin.statuses.withLock {
            $0[walletA] = coinJoinStatus(state: .mixing).with(state: .mixing)
        }
        let panel = makePanel()
        await panel.reload()
        #expect(panel.sessionStatusText == "Mixing in progress…")
        #expect(L10n.CoinJoin.status(.masternode(.queueFull)) == "Masternode: Masternode queue is full.")
        #expect(L10n.CoinJoin.status(.waitingInQueue) == "Submitted to masternode, waiting in queue .")
    }

    @Test func QT049_eventsOfOtherWalletsAreIgnored() async {
        m3.coinJoin.statuses.withLock { $0[walletB] = coinJoinStatus(walletB, state: .mixing) }
        let panel = makePanel()
        await panel.start()
        m3.coinJoin.statuses.withLock { $0[walletA] = coinJoinStatus(state: .mixing) }
        m3.coinJoin.changes.send(walletB)
        try? await Task.sleep(for: .milliseconds(20))
        #expect(panel.status?.state == .idle)
        m3.coinJoin.changes.send(walletA)
        await eventually { panel.status?.state == .mixing }
        panel.stop()
    }

    // MARK: Start / Stop (QT-044, QT-112)

    @Test func QT044_firstUseShowsMostCommonHintOnceThenStarts() async {
        m2.desktopPreferences.desktop.m3.coinJoinHintShown = false
        let panel = makePanel()
        await panel.reload()
        await panel.toggle()
        #expect(panel.prompt == .firstUseHint)
        #expect(panel.prompt?.message.contains("\"Most Common\"") == true)
        #expect(m2.desktopPreferences.desktop.m3.coinJoinHintShown)
        #expect(m3.coinJoin.starts.current.isEmpty)
        await panel.acknowledgeHint()
        #expect(panel.prompt == nil)
        #expect(m3.coinJoin.starts.current == [walletA])
        #expect(panel.buttonTitle == "Stop CoinJoin")
    }

    @Test func QT044_belowMinimumBalanceWarns() async {
        world.walletState.balances = balances(confirmed: 100_000)
        let panel = makePanel()
        await panel.reload()
        await panel.toggle()
        #expect(panel.prompt == .minimumBalance("0.00140001 tDASH"))
        #expect(panel.prompt?.message == "CoinJoin requires at least 0.00140001 tDASH to use.")
        #expect(m3.coinJoin.starts.current.isEmpty)
    }

    @Test func QT044_stopResetsAndStops() async {
        m3.coinJoin.statuses.withLock { $0[walletA] = coinJoinStatus(state: .mixing) }
        let panel = makePanel()
        await panel.reload()
        await panel.toggle()
        #expect(m3.coinJoin.stops.current == [walletA])
        #expect(panel.buttonTitle == "Start CoinJoin")
    }

    @Test func QT112_lockedVaultAsksToUnlockForMixingOnly() async {
        world.auth.lockState = .locked
        let panel = makePanel()
        await panel.reload()
        await panel.toggle()
        #expect(panel.prompt == .unlockForMixing)
        #expect(panel.prompt?.title == "Unlock wallet for mixing only")
        await panel.unlockForMixing(passphrase: "secret")
        #expect(world.auth.unlockCalls.last?.1 == .mixingOnly)
        #expect(world.auth.lockState == .unlockedMixingOnly)
        #expect(m3.coinJoin.starts.current == [walletA])
        #expect(panel.prompt == nil)
    }

    @Test func QT112_declinedUnlockDisablesCoinJoin() async {
        world.auth.lockState = .locked
        let panel = makePanel()
        await panel.reload()
        await panel.toggle()
        panel.dismissPrompt()
        #expect(panel.prompt == .declinedToUnlock)
        #expect(panel.prompt?.message == "Wallet is locked and user declined to unlock. Disabling CoinJoin.")
        #expect(m3.coinJoin.starts.current.isEmpty)
    }

    @Test func QT112_engineVaultLockedAnswerAlsoAsks() async {
        m3.coinJoin.vaultLocked.withLock { $0 = { true } }
        let panel = makePanel()
        await panel.reload()
        await panel.toggle()
        #expect(panel.prompt == .unlockForMixing)
    }

    @Test func QT112_vaultLockStopNoticeIsShown() async {
        m3.coinJoin.statuses.withLock { $0[walletA] = coinJoinStatus(stopReason: .vaultLocked) }
        let panel = makePanel()
        await panel.reload()
        #expect(panel.stopNotice == "Mixing stopped because the wallet was locked.")
    }

    @Test func QT041_notImplementedEngineShowsUnavailable() async {
        let panel = CoinJoinPanelViewModel(
            env: world.environment(), m2: m2.services, m3: M3Services.unavailable())
        await panel.reload()
        #expect(panel.phase == .unavailable)
        #expect(panel.isVisible)
        #expect(!panel.buttonEnabled)
    }

    // MARK: Options (QT-046, QT-047, QT-135)

    func makeOptions() -> OptionsViewModel {
        OptionsViewModel(env: world.environment(), m2: m2.services, m3: m3.services)
    }

    @Test func QT046_defaultsAndRangesComeFromDashQt() async {
        let options = makeOptions()
        await options.load()
        #expect(options.coinJoinAvailable)
        #expect(options.coinJoin.rounds == 4)
        #expect(options.coinJoin.targetAmountDash == 1000)
        #expect(options.coinJoin.maxSessions == 4)
        #expect(options.coinJoin.denomsGoal == 50)
        #expect(options.coinJoin.denomsHardCap == 300)
        #expect(options.coinJoin.enabled)
        #expect(!options.coinJoin.multiSession)
        #expect(options.coinJoinLimits.rounds == 2...16)
        #expect(options.coinJoinLimits.targetAmountDash == 2...21_000_000)
        #expect(options.coinJoinLimits.sessions == 1...10)
        #expect(options.coinJoinLimits.denoms == 10...100_000)
        #expect(options.showsCoinJoinTab)
    }

    @Test func QT135_coinJoinOptionsApplyOnOKAndRevertOnCancel() async throws {
        let options = makeOptions()
        await options.load()
        options.coinJoin.rounds = 8
        options.discard()
        #expect(options.coinJoin.rounds == 4)
        options.coinJoin.rounds = 8
        options.coinJoin.showAdvancedInterface = true
        try await options.apply()
        #expect(m3.coinJoin.settingsWrites.current.last?.rounds == 8)
        #expect(m2.desktopPreferences.desktop.m3.showAdvancedCoinJoinUI)
        // Unchanged values write nothing.
        try await options.apply()
        #expect(m3.coinJoin.settingsWrites.current.count == 1)
    }

    @Test func QT046_goalStaysAtOrBelowTheHardCapAndRangesClamp() async throws {
        let options = makeOptions()
        await options.load()
        options.coinJoin.denomsHardCap = 40
        options.coinJoin.denomsGoal = 90
        options.coinJoin.rounds = 99
        try await options.apply()
        let written = try #require(m3.coinJoin.settingsWrites.current.last)
        #expect(written.denomsGoal == 40)
        #expect(written.rounds == 16)
    }

    @Test func QT047_lowKeysWarningIsKeptForImportParity() async throws {
        let options = makeOptions()
        await options.load()
        #expect(options.coinJoin.lowKeysWarning)
        options.coinJoin.lowKeysWarning = false
        try await options.apply()
        #expect(!m2.desktopPreferences.desktop.m3.lowKeysWarning)
        #expect(L10n.CoinJoin.lowKeysNotApplicable.contains("HD wallets"))
    }

    @Test func QT046_engineWithoutCoinJoinLeavesTheTabUnavailable() async {
        let options = OptionsViewModel(
            env: world.environment(), m2: m2.services, m3: M3Services.unavailable())
        await options.load()
        #expect(!options.coinJoinAvailable)
        #expect(options.errorMessage == nil)
    }

    // MARK: CoinJoin send page (QT-051)

    @Test func QT051_coinJoinPageShowsTheMixedBalance() {
        world.walletState.balances = WalletBalances(
            confirmed: Amount(duffs: 10_00000000), unconfirmed: .zero, immature: .zero, locked: .zero,
            total: Amount(duffs: 10_00000000), coinjoin: Amount(duffs: 2_50000000))
        let page = SendViewModel(env: world.environment(), network: .testnet, page: .coinJoin)
        #expect(page.mixedBalance == Amount(duffs: 2_50000000))
        #expect(page.mixedBalanceText?.hasPrefix("CoinJoin Balance: ") == true)
        #expect(page.sendButtonTitle == "Send mixed funds")
        let regular = SendViewModel(env: world.environment(), network: .testnet)
        #expect(regular.mixedBalance == nil)
    }

    // MARK: Move mixed coins (IOS-057)

    func makeMixed() -> MixedCoinsViewModel {
        MixedCoinsViewModel(env: world.environment(), m2: m2.services, m3: m3.services)
    }

    @Test func IOS057_recoveryScanReportsWhatItFound() async {
        m3.coinJoin.report.withLock {
            $0 = CoinJoinRecoveryReport(
                coinJoinAddressesScanned: 1000, bip44AddressesScanned: 2000, coinJoinBalance: Amount(duffs: 3_00000000),
                newTransactions: 7)
        }
        let model = makeMixed()
        await model.scan()
        #expect(model.resultText == "Scanned 3000 addresses: 7 new transactions, CoinJoin balance 3.00000000 tDASH.")
    }

    @Test func IOS057_moveUsesASpendGrantForThePlanTotalAndRecordsWithdrawals() async {
        world.walletState.balances = WalletBalances(
            confirmed: .zero, unconfirmed: .zero, immature: .zero, locked: .zero, total: .zero,
            coinjoin: Amount(duffs: 2_00000000))
        let plan = MixedCoinsSweepPlan(
            destination: .wallet, total: Amount(duffs: 2_00000000),
            chunks: [MixedCoinsChunk(inputs: 40, amount: Amount(duffs: 2_00000000), fee: Amount(duffs: 6_000))])
        m3.coinJoin.sweepPlan.withLock { $0 = plan }
        m3.coinJoin.moveResult.withLock {
            $0 = MixedCoinsSweepResult(txids: [txid(9)], moved: Amount(duffs: 1_99994000), remaining: .zero, failureCode: nil)
        }
        let model = makeMixed()
        #expect(model.showsMoveBanner)
        await model.preparePlan()
        #expect(model.planText == "2.00000000 tDASH in 1 transaction, fee 0.00006000 tDASH")
        await model.move()
        #expect(m3.coinJoin.moveGrants.current.first?.purpose == .spend(max: Amount(duffs: 2_00000000)))
        #expect(model.resultText == "Moved 1.99994000 tDASH to your wallet.")
        #expect(m2.desktopPreferences.desktop.m3.coinJoinWithdrawals[walletA.hex] == [txid(9)])
    }

    @Test func IOS057_partialSweepSaysWhatIsLeft() async {
        m3.coinJoin.sweepPlan.withLock {
            $0 = MixedCoinsSweepPlan(destination: .wallet, total: Amount(duffs: 300), chunks: [])
        }
        m3.coinJoin.moveResult.withLock {
            $0 = MixedCoinsSweepResult(
                txids: [txid(1)], moved: Amount(duffs: 100), remaining: Amount(duffs: 200), failureCode: .coinjoinNoPeers)
        }
        let model = makeMixed()
        await model.preparePlan()
        await model.move()
        #expect(model.resultText?.contains("could not be moved yet") == true)
        #expect(model.resultText?.contains("No peers are connected") == true)
    }

    @Test func IOS057_encryptedVaultAsksForThePassphrase() async {
        world.auth.lockState = .unlocked
        m3.coinJoin.sweepPlan.withLock { $0 = MixedCoinsSweepPlan(destination: .wallet, total: Amount(duffs: 300), chunks: []) }
        let model = makeMixed()
        await model.preparePlan()
        await model.move()
        guard case .needsPassphrase = model.flow else {
            Issue.record("expected needsPassphrase, got \(model.flow)")
            return
        }
    }

    @Test func IOS057_laterHidesTheBannerUntilTheBalanceChanges() {
        world.walletState.balances = WalletBalances(
            confirmed: .zero, unconfirmed: .zero, immature: .zero, locked: .zero, total: .zero,
            coinjoin: Amount(duffs: 5_000))
        let model = makeMixed()
        #expect(model.showsMoveBanner)
        model.later()
        #expect(!model.showsMoveBanner)
        world.walletState.balances = WalletBalances(
            confirmed: .zero, unconfirmed: .zero, immature: .zero, locked: .zero, total: .zero,
            coinjoin: Amount(duffs: 6_000))
        #expect(model.showsMoveBanner)
    }

    @Test func IOS057_shieldedDestinationIsNotAvailableYet() async {
        let model = makeMixed()
        model.destination = .shielded
        await model.preparePlan()
        #expect(model.flow == .unavailable)
    }
}
