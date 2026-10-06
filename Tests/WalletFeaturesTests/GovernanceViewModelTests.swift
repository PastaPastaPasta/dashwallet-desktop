// Governance view models (QT-026, QT-128…134) against the M3 fakes.
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Governance view models")
struct GovernanceViewModelTests {
    let world = FakeWorld()
    let m2 = FakeM2World()
    let m3: FakeM3World

    static let synced = GovernanceSyncState(
        phase: .synced, objects: 40, votes: 9000, peers: 3, bytesReceived: 1_000_000, lastSyncedAt: nil)

    init() {
        let clock = world.clock
        m3 = FakeM3World(now: { clock.now })
        m3.governance.sync.withLock { $0 = Self.synced }
        m3.governance.rows.withLock {
            $0 = [
                proposalRow(1, name: "dash-hackathon", status: .passing, yes: 900, no: 10, margin: 40),
                proposalRow(2, name: "core-dev-q4", myVotes: MyVotes(yes: 4, no: 0, abstain: 1, unvoted: 2)),
            ]
        }
        // Testnet: cycle 24, window 8. Tip 1000, next superblock 1008, cutoff 1000…
        m3.governance.clockValue.withLock {
            $0 = GovernanceClock(
                cycleProgress: 0.5, nextSuperblock: 1_008, blocksToSuperblock: 12, superblockDate: Date(),
                votingCutoff: 1_000, votingOpen: true, budgetCommitted: 0.25)
        }
        world.walletState.balances = balances(confirmed: 5_00000000)
    }

    func makeList() -> ProposalListViewModel {
        ProposalListViewModel(env: world.environment(), m2: m2.services, m3: m3.services)
    }

    // MARK: List (QT-128…130)

    @Test func QT128_tabTurnsGovSyncOnAndListsActiveProposals() async {
        let list = makeList()
        await list.start()
        #expect(m3.governance.syncSwitches.current == [true])
        #expect(list.phase == .ready)
        #expect(list.rows.count == 2)
        #expect(list.countText == "2")
        #expect(list.syncText == "Governance data comes from peers and may lag behind the network.")
        list.stop()
    }

    @Test func QT128_myProposalsHidesVotesAndTitleFilterIsCaseInsensitive() async {
        let list = makeList()
        await list.reload()
        #expect(list.columns.contains(.votes))
        await list.setSource(.mine)
        #expect(!list.columns.contains(.votes))
        #expect(m3.governance.queries.current.last?.source == .mine(walletA))
        await list.setTitleFilter("HACK")
        #expect(list.rows.map(\.name) == ["dash-hackathon"])
        #expect(m3.governance.queries.current.last?.titleFilter == "HACK")
    }

    @Test func QT129_columnsUseDashQtFormats() async {
        let list = makeList()
        await list.reload()
        let passing = list.rows[0], voting = list.rows[1]
        #expect(list.votesText(passing) == "900Y, 10N, 1A (+40)")
        #expect(list.votesText(voting) == "10Y, 2N, 1A (-3)")
        #expect(list.votesTooltip(voting) == "10 Yes, 2 No, 1 Abstain, needs 3 more votes")
        #expect(list.myVotesText(voting) == "4Y, 0N, 1A / 2 unvoted")
        #expect(list.myVotesText(passing) == "No voting keys")
        #expect(list.statusTooltip(voting) == "Voting, needs 3 more votes for funding")
        #expect(list.statusTooltip(passing) == "Passing with 890 votes")
        #expect(list.statusText(passing) == "Passing")
        #expect(list.startText(passing) == "2025-10-09")
    }

    @Test func QT128_deadlineLabelFollowsTheClock() async {
        let list = makeList()
        await list.reload()
        // Tip 996 (1008 − 12): 4 blocks to the cutoff at 1000.
        #expect(list.deadlineText == "Voting deadline: ~10 minutes left (4 blocks, block 1000)")
        m3.governance.clockValue.withLock {
            $0 = GovernanceClock(
                cycleProgress: 0.9, nextSuperblock: 1_008, blocksToSuperblock: 4, superblockDate: Date(),
                votingCutoff: 1_000, votingOpen: false, budgetCommitted: 0.25)
        }
        await list.reload()
        #expect(list.deadlineText == "Voting deadline passed for this cycle (block 1000)")
        m3.governance.sync.withLock {
            $0 = GovernanceSyncState(phase: .syncingVotes, objects: 1, votes: 0, peers: 3, bytesReceived: 0, lastSyncedAt: nil)
        }
        await list.reload()
        #expect(list.deadlineText == "Voting deadline: waiting for sync…")
        #expect(!list.canCreate)
        #expect(list.createTooltip == "Cannot interact with governance before sync completes")
    }

    @Test func QT128_createNeedsTheProposalFee() async {
        world.walletState.balances = balances(confirmed: 50_000_000)
        let list = makeList()
        await list.reload()
        #expect(!list.canCreate)
        #expect(list.createTooltip == "Creating proposals costs 1.00000000 tDASH, insufficient balance")
    }

    @Test func QT130_copyRawJSONAndSafeURLOpen() async {
        let row = proposalRow(1)
        m3.governance.details.withLock {
            $0[row.hash] = ProposalDetail(
                row: row, parentHash: "0", collateralTxid: txid(3), createdAt: Date(timeIntervalSince1970: 0),
                payments: 3, rawJSON: "{\"name\":\"proposal\"}")
        }
        let list = makeList()
        await list.reload()
        await list.copyRawJSON(row.hash)
        #expect(m2.clipboard.text.current == "{\"name\":\"proposal\"}")

        list.requestOpenURL(proposalRow(5, url: "javascript:alert(1)"))
        #expect(list.errorMessage == "Cannot validate URL, potentially malformed or unknown protocol.")
        #expect(list.externalLink == nil)
        list.requestOpenURL(proposalRow(6, url: "https://www.dashcentral.org/p/x"))
        #expect(list.externalLink?.title == "External Link Warning")
        list.cancelOpenURL()
        #expect(list.urlToOpen == nil)
        list.requestOpenURL(proposalRow(6, url: "https://www.dashcentral.org/p/x"))
        list.confirmOpenURL()
        #expect(list.urlToOpen?.absoluteString == "https://www.dashcentral.org/p/x")
        #expect(ProposalListViewModel.safeURL("file:///etc/passwd") == nil)
        #expect(ProposalListViewModel.safeURL("http://") == nil)
    }

    @Test func QT130_detailsShowDashQtFields() async {
        let row = proposalRow(1)
        m3.governance.details.withLock {
            $0[row.hash] = ProposalDetail(
                row: row, parentHash: "0", collateralTxid: txid(3), createdAt: Date(timeIntervalSince1970: 0),
                payments: 3, rawJSON: "{}")
        }
        let list = makeList()
        await list.showDetails(row.hash)
        #expect(list.detailTitle == "Details for proposal")
        #expect(list.detailLines.map(\.title).contains("Collateral Hash"))
        #expect(list.detailLines.first { $0.title == "Payments Requested" }?.value == "3")
    }

    @Test func QT129_notImplementedEngineShowsUnavailable() async {
        let list = ProposalListViewModel(
            env: world.environment(), m2: m2.services, m3: M3Services.unavailable(network: .testnet))
        await list.start()
        #expect(list.phase == .unavailable)
    }

    // MARK: Vote (QT-131)

    func voters() -> [VotingMasternode] {
        [
            VotingMasternode(
                proTxHash: txid(10), collateral: "\(txid(11))-0", votingAddress: "yVote1", weight: 1, currentVote: nil,
                voteTime: nil, nextVoteAt: nil, label: nil),
            VotingMasternode(
                proTxHash: txid(12), collateral: "\(txid(13))-1", votingAddress: "yVote2", weight: 4, currentVote: .no,
                voteTime: world.clock.now.addingTimeInterval(-600), nextVoteAt: world.clock.now.addingTimeInterval(3000),
                label: "evo"),
        ]
    }

    @Test func QT131_voteDialogSelectsEligibleMasternodesAndSummarizesWeight() async {
        m3.governance.voters.withLock { $0 = voters() }
        let list = makeList()
        await list.reload()
        await list.openVote(list.rows[1].hash, outcome: .yes)
        let vote = try! #require(list.vote)
        #expect(vote.phase == .ready)
        #expect(vote.voteButtonTitle == "Vote Yes")
        #expect(vote.selected == [txid(10)])
        #expect(vote.summary == "Masternodes selected: 1 · Vote weight: 1")
        vote.selectAll()
        #expect(vote.summary == "Masternodes selected: 2 · Vote weight: 5")
        #expect(vote.waitText(vote.masternodes[1])?.hasPrefix("Can change its vote after") == true)
        vote.clearSelection()
        #expect(!vote.canVote)
    }

    @Test func QT131_castReportsSuccessesAndPerMasternodeErrors() async {
        m3.governance.voters.withLock { $0 = voters() }
        let list = makeList()
        await list.reload()
        await list.openVote(list.rows[1].hash, outcome: .no)
        let vote = try! #require(list.vote)
        vote.selectAll()
        await vote.vote()
        #expect(vote.phase == .done)
        let cast = try! #require(m3.governance.casts.current.first)
        #expect(cast.0 == .no)
        #expect(cast.2 == [txid(10), txid(12)])
        #expect(cast.3.purpose == .governance)
        #expect(vote.resultLines[0] == "Voted successfully 1 time")
        #expect(vote.resultLines[1] == "Failed to vote 1 time")
        #expect(vote.resultLines[3].hasPrefix("Masternode \(txid(12)): Masternode voting too often."))
    }

    @Test func QT131_lockedVaultNeedsAFullUnlockPassphrase() async {
        world.auth.lockState = .unlockedMixingOnly
        m3.governance.voters.withLock { $0 = voters() }
        let list = makeList()
        await list.reload()
        await list.openVote(list.rows[0].hash)
        let vote = try! #require(list.vote)
        await vote.vote()
        #expect(vote.phase == .needsPassphrase)
        await vote.vote(passphrase: "pw")
        #expect(vote.phase == .done)
        #expect(world.auth.authorizeCalls.last?.passphrase == "pw")
    }

    @Test func QT131_noVotingKeysSaysSo() async {
        m3.governance.voters.withLock { $0 = [] }
        let list = makeList()
        await list.reload()
        await list.openVote(list.rows[0].hash)
        #expect(list.vote?.phase == .failed("No masternode voting keys found in wallet."))
    }

    // MARK: Create (QT-132)

    func makeCreate() -> CreateProposalWizardViewModel {
        CreateProposalWizardViewModel(env: world.environment(), m3: m3.services)
    }

    func superblocks() -> [SuperblockDate] {
        (0..<12).map { SuperblockDate(height: 1_008 + UInt32($0) * 24, estimatedDate: Date(timeIntervalSince1970: 1_760_000_000 + Double($0) * 3600)) }
    }

    @Test func QT132_twelvePaymentDatesTotalAndMandatoryFields() async {
        m3.governance.superblocks.withLock { $0 = superblocks() }
        let wizard = makeCreate()
        await wizard.load()
        #expect(wizard.step == .editing)
        #expect(wizard.superblockOptions.count == 12)
        #expect(wizard.firstSuperblock == 1_008)
        #expect(wizard.paymentCountRange == 1...12)
        #expect(wizard.paymentDateHelp.contains("chosen superblock"))
        #expect(await wizard.validate() == false)
        #expect(wizard.errorMessage == "All fields are mandatory")
        wizard.name = "Bad Name"
        wizard.url = "https://x.org"
        wizard.paymentAddress = testnetAddress1
        wizard.amountText = "12.5"
        wizard.paymentCount = 3
        #expect(wizard.totalText == "37.50000000 tDASH")
        #expect(await wizard.validate() == false)
        #expect(wizard.fieldErrors == [.name])
        #expect(wizard.fieldErrorText(.name)?.hasPrefix("Name must be") == true)
    }

    @Test func QT132_createConfirmsTheNonRefundableFeeThenPays() async {
        m3.governance.superblocks.withLock { $0 = superblocks() }
        m3.governance.pendingList.withLock { $0 = [] }
        let wizard = makeCreate()
        await wizard.load()
        wizard.name = "dash-meetup"
        wizard.url = "https://x.org"
        wizard.paymentAddress = testnetAddress1
        wizard.amountText = "10"
        wizard.firstSuperblock = 1_032
        await wizard.viewJSON()
        #expect(wizard.preview?.contains("dash-meetup") == true)
        await wizard.requestCreate()
        #expect(wizard.step == .confirming)
        #expect(wizard.confirmationDetail ==
            "Creating a proposal pays 1.00000000 tDASH to the network. This fee is non-refundable regardless of outcome.")
        await wizard.confirm()
        guard case .created(let pending) = wizard.step else {
            Issue.record("expected created, got \(wizard.step)")
            return
        }
        let call = try! #require(m3.governance.created.current.first)
        #expect(call.0.firstSuperblockHeight == 1_032)
        #expect(call.1.purpose == .spend(max: Amount(duffs: 100_100_000)))
        #expect(pending.name == "dash-meetup")
        #expect(wizard.createdMessage?.hasPrefix("1.00000000 tDASH successfully sent for your proposal \"dash-meetup\".") == true)
    }

    // MARK: Resume (QT-133)

    @Test func QT133_broadcastWaitsForOneConfirmation() async {
        let pending = PendingProposal(
            hash: txid(40), name: "p", url: "https://x.org", paymentAmount: Amount(duffs: 1_00000000), paymentCount: 2,
            collateralTxid: txid(41), collateralStatus: .pending, confirmations: 0, createdAt: Date(), end: Date())
        m3.governance.pendingList.withLock { $0 = [pending] }
        let resume = ResumeProposalsViewModel(env: world.environment(), m3: m3.services)
        await resume.reload()
        #expect(!resume.canBroadcast(resume.pending[0]))
        #expect(resume.collateralStatusText(resume.pending[0]) == "Pending")
        #expect(resume.fundSummary(resume.pending[0]) == "For 2 payments of 1.00000000 tDASH to https://x.org")
        let ready = PendingProposal(
            hash: txid(40), name: "p", url: "https://x.org", paymentAmount: Amount(duffs: 1_00000000), paymentCount: 2,
            collateralTxid: txid(41), collateralStatus: .ready, confirmations: 1, createdAt: Date(), end: Date())
        m3.governance.pendingList.withLock { $0 = [ready] }
        await resume.reload()
        await resume.broadcast(txid(40))
        #expect(resume.message == "Proposal has been broadcasted to the network with hash \(txid(40))")
        #expect(resume.emptyText == "No pending proposals to broadcast.")
    }

    // MARK: Info and clock (QT-134, QT-026)

    func info(synced: Bool = true) -> GovernanceInfo {
        GovernanceInfo(
            sync: Self.synced, superblockCycle: 24, lastSuperblock: synced ? 984 : nil,
            nextSuperblock: synced ? 1_008 : nil, nextSuperblockDate: nil, votingCutoff: synced ? 1_000 : nil,
            masternodesVoting: synced ? 120 : nil, masternodesEligible: synced ? 300 : nil, evonodesVoting: synced ? 20 : nil,
            evonodesEligible: synced ? 40 : nil, passingThreshold: synced ? 46 : nil, masternodesControlled: 2,
            votesControlled: 5, proposalCount: synced ? 12 : nil, passing: synced ? 5 : nil, failing: synced ? 6 : nil,
            unfunded: synced ? 1 : nil, unfundedShort: synced ? Amount(duffs: 10_00000000) : nil,
            budgetAvailable: synced ? Amount(duffs: 100_00000000) : nil,
            budgetAllocated: synced ? Amount(duffs: 25_00000000) : nil)
    }

    @Test func QT134_infoPanelShowsParticipationThresholdAndBudget() async {
        m3.governance.infoValue.withLock { $0 = info() }
        let model = GovernanceInfoViewModel(env: world.environment(), m3: m3.services)
        await model.reload()
        let lines = model.sections.flatMap(\.lines)
        func value(_ title: String) -> String? { lines.first { $0.title == title }?.value }
        #expect(value("Voting cycles") == "24 blocks (~1 hour)")
        #expect(value("Masternodes voting") == "120 (300 eligible)")
        #expect(value("Passing threshold") == "46")
        #expect(value("Votes controlled") == "5")
        #expect(value("Budget allocated") == "25.00000000 tDASH / 100.00000000 tDASH (25%)")
        #expect(value("Unfunded proposals") == "1 (short 10.00000000 tDASH)")
        #expect(model.budgetFraction == 0.25)
    }

    @Test func QT134_unsyncedValuesAreNA() async {
        m3.governance.infoValue.withLock { $0 = info(synced: false) }
        let model = GovernanceInfoViewModel(env: world.environment(), m3: m3.services)
        await model.reload()
        let lines = model.sections.flatMap(\.lines)
        #expect(lines.first { $0.title == "Last superblock" }?.value == "N/A")
        #expect(lines.first { $0.title == "Budget allocated" }?.value == "N/A")
        #expect(model.budgetFraction == nil)
    }

    func makeClock() -> GovernanceClockViewModel {
        GovernanceClockViewModel(env: world.environment(), m2: m2.services, m3: m3.services, features: .m3)
    }

    @Test func QT026_clockFollowsTheDisplayOptionsAndTurnsGovSyncOn() async {
        m3.governance.infoValue.withLock { $0 = info() }
        let clock = makeClock()
        await clock.start()
        #expect(m3.governance.syncSwitches.current == [false])
        #expect(!clock.isVisible)
        m2.desktopPreferences.desktop.options.showGovernanceTab = true
        m2.desktopPreferences.desktop.options.showGovernanceClock = true
        await clock.optionsChanged()
        #expect(m3.governance.syncSwitches.current == [false, true])
        #expect(clock.isVisible)
        #expect(!clock.isAnimating)
        #expect(clock.cycleProgress == 0.5)
        #expect(clock.tooltipLines[0] == "~10 minutes (4 blocks) left for voting")
        #expect(clock.tooltipLines[1] == "~25% of budget committed (25.00000000 tDASH / 100.00000000 tDASH)")
        clock.activate()
        #expect(clock.route == .section(.governance))
        clock.stop()
    }

    @Test func QT026_clockSaysSuperblockImminentOrVotingEnded() {
        let parameters = M3Defaults.governanceParameters(.testnet)
        let awaiting = GovernanceClock(
            cycleProgress: 0.9, nextSuperblock: 1_008, blocksToSuperblock: 6, superblockDate: Date(), votingCutoff: 1_000,
            votingOpen: false, budgetCommitted: nil)
        #expect(GovernanceClockViewModel.cycleLine(awaiting, parameters: parameters) == "~15 minutes (6 blocks) left for superblock")
        let now = GovernanceClock(
            cycleProgress: 1, nextSuperblock: 1_008, blocksToSuperblock: 0, superblockDate: Date(), votingCutoff: 1_000,
            votingOpen: false, budgetCommitted: nil)
        #expect(GovernanceClockViewModel.cycleLine(now, parameters: parameters) == "Superblock imminent")
    }

    @Test func QT139_governanceTabAloneAlsoKeepsGovSyncOn() async {
        m2.desktopPreferences.desktop.options.showGovernanceTab = true
        let clock = makeClock()
        await clock.optionsChanged()
        #expect(m3.governance.syncSwitches.current == [true])
        #expect(!clock.isVisible)
    }
}
