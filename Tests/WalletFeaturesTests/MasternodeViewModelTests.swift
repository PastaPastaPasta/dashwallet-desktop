// Masternode view models (QT-118…127, IOS-080…083) against the M3 fakes.
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Masternode view models")
struct MasternodeViewModelTests {
    let world = FakeWorld()
    let m2 = FakeM2World()
    let m3 = FakeM3World()

    let owned = masternodeRow(
        1, owned: [.owner, .voting], collateral: OutPoint(txid: txid(100), vout: 1),
        reward: OperatorReward(percentX100: 1_250, payoutAddress: "yOperatorPayout"))
    let evo = masternodeRow(2, type: .evo, service: "5.6.7.8:19999")
    let banned = masternodeRow(3, status: .banned(sinceHeight: 1_100))
    let shared = masternodeRow(
        4, owned: [.shareOwner], shared: SharedHolding(heldShares: 1, totalShares: 3), reward: nil)

    init() {
        m3.masternodes.listState.withLock {
            $0 = MasternodeListState(
                available: true, height: 1_200, total: 4, enabled: 3, evoTotal: 1, evoEnabled: 1, syncing: false)
        }
        m3.masternodes.rows.withLock { $0 = [owned, evo, banned, shared] }
    }

    func makeList() -> MasternodeListViewModel {
        MasternodeListViewModel(env: world.environment(), m2: m2.services, m3: m3.services)
    }

    // MARK: List (QT-118…121)

    @Test func QT118_listFiltersPersistAndTypeColumnHidesForRegularAndEvo() async {
        let list = makeList()
        await list.reload()
        #expect(list.nodeCountText == "4")
        #expect(list.columns.contains(.type))
        #expect(!list.columns.contains(.proTxHash))
        await list.setTypeFilter(.evo)
        #expect(list.rows.map(\.proTxHash) == [evo.proTxHash])
        #expect(!list.columns.contains(.type))
        #expect(m2.desktopPreferences.desktop.m3.masternodeTypeFilter == "evo")
        await list.setTypeFilter(.all)
        await list.setHideBanned(true)
        await list.setOwnedOnly(true)
        #expect(Set(list.rows.map(\.proTxHash)) == [owned.proTxHash, shared.proTxHash])
        await list.setFilterText("YVOTINGADDRESS1")
        #expect(list.rows.map(\.proTxHash) == [owned.proTxHash])
        // A new list starts from the stored filters (mnList* settings).
        let reopened = makeList()
        #expect(reopened.query.ownedOnly && reopened.query.hideBanned)
        #expect(reopened.query.text == "YVOTINGADDRESS1")
    }

    @Test func QT118_listIsBrowsableWithoutAWallet() async {
        world.walletState.wallets = []
        world.walletState.selectedWalletID = nil
        let list = makeList()
        await list.reload()
        #expect(list.rows.count == 4)
        #expect(!list.canRegister)
        #expect(list.registerTooltip == "Registering a masternode requires a wallet.")
    }

    @Test func QT119_columnsShowDashQtTextAndHonestDashes() async {
        let list = makeList()
        await list.reload()
        let row = list.rows.first { $0.proTxHash == owned.proTxHash }!
        #expect(list.text(.poseScore, row) == "—")
        #expect(list.tooltip(.poseScore, row) == "Requires full-node data source")
        #expect(list.text(.lastPaid, row) == "—")
        #expect(list.text(.registered, row) == "900")
        #expect(list.operatorRewardText(row) == "12.50% to yOperatorPayout")
        #expect(list.statusTooltip(row) == "Active for 8 hours")
        #expect(list.typeText(shared) == "Shared (you hold 1 of 3)")
        #expect(list.operatorRewardText(evo) == "NONE")
        #expect(list.text(.operatorReward, shared) == "—")
        #expect(list.statusTooltip(banned) == "Banned for 4 hours")
    }

    @Test func QT119_headerClickSortsAndFlips() async {
        let list = makeList()
        await list.reload()
        list.sort(by: .service)
        let ascending = list.rows.map(\.service)
        list.sort(by: .service)
        #expect(list.rows.map(\.service) == ascending.reversed() || ascending.allSatisfy { $0 == ascending.first })
        list.sort(by: .status)
        #expect(list.sort.column == .status && list.sort.ascending)
        #expect(list.rows.last?.proTxHash == banned.proTxHash)
    }

    @Test func QT120_ownedDetectionIsShownWithItsReasons() async {
        let list = makeList()
        await list.reload()
        #expect(list.ownedText(owned) == "Owner key, Voting key")
        #expect(list.ownedText(evo) == nil)
    }

    @Test func QT121_contextMenuCopiesAndFilters() async {
        let list = makeList()
        await list.reload()
        let titles = list.menu(for: owned).map(\.title)
        #expect(titles.contains("Copy ProTx Hash"))
        #expect(titles.contains("Update Registrar…"))
        #expect(!titles.contains("Copy IP"))
        await list.perform(.copyProTxHash, on: owned)
        #expect(m2.clipboard.text.current == owned.proTxHash)
        await list.perform(.copyCollateralOutpoint, on: owned)
        #expect(m2.clipboard.text.current == "\(txid(100))-1")
        await list.perform(.filterByVoting, on: owned)
        #expect(list.query.text == owned.votingAddress)
        await list.perform(.updateService, on: owned)
        #expect(list.presentation == .updateService(proTxHash: owned.proTxHash))
    }

    @Test func QT121_sharedMasternodesGetSharedActionsAndNoRegistrarUpdate() async {
        let list = makeList()
        let titles = list.menu(for: shared).map(\.title)
        #expect(!titles.contains("Update Registrar…"))
        #expect(titles.contains("Change Reward Address…"))
        #expect(titles.contains("Rotate Keys…"))
        #expect(titles.contains("Dissolve…"))
        let standby = list.menu(for: shared).first { $0.action == .createStandbyDissolution }
        #expect(standby?.tooltip == "Not created on this computer yet")
        let collateral = list.menu(for: evo).first { $0.action == .copyCollateralOutpoint }
        #expect(collateral?.isEnabled == false)
    }

    @Test func QT118_notImplementedEngineShowsUnavailable() async {
        let list = MasternodeListViewModel(
            env: world.environment(), m2: m2.services, m3: M3Services.unavailable(network: .testnet))
        await list.reload()
        #expect(list.phase == .unavailable)
    }

    // MARK: Details (QT-122, IOS-080)

    @Test func QT122_detailsShowEveryFieldWithHonestDashes() async {
        m3.masternodes.details.withLock { $0[owned.proTxHash] = masternodeDetail(owned) }
        let detail = MasternodeDetailViewModel(proTxHash: owned.proTxHash, env: world.environment(), m3: m3.services)
        await detail.load()
        #expect(detail.title == "Details for Masternode \(owned.proTxHash)")
        let titles = detail.lines.map(\.title)
        for field in ["ProTx Hash", "Public Key Operator", "Owner Address", "Voting Address", "Collateral Hash",
                      "Collateral Index", "Masternode Type", "Registered Height", "Last Paid Height",
                      "Consecutive Payments", "Operator Reward", "PoSe Penalty", "PoSe Ban Height",
                      "PoSe Revived Height"] {
            #expect(titles.contains(field), "missing \(field)")
        }
        #expect(detail.lines.first { $0.title == "PoSe Ban Height" }?.tooltip == "Requires full-node data source")
        #expect(detail.evonode == .notEvonode)
    }

    @Test func QT122_sharedDetailsListShares() async {
        let shares = [
            MasternodeShare(amount: Amount(duffs: 500_00000000), ownerAddress: "yA", payoutAddress: "yB", refundAddress: "yC", mine: true),
            MasternodeShare(amount: Amount(duffs: 500_00000000), ownerAddress: "yD", payoutAddress: "yE", refundAddress: "yF", mine: false),
        ]
        m3.masternodes.details.withLock { $0[shared.proTxHash] = masternodeDetail(shared, shares: shares) }
        let detail = MasternodeDetailViewModel(proTxHash: shared.proTxHash, env: world.environment(), m3: m3.services)
        await detail.load()
        #expect(detail.shares.count == 2)
        #expect(detail.shares[0].mine)
        #expect(detail.lines.contains { $0.title == "Early-exit penalty" })
    }

    @Test func IOS080_evonodePlatformStatusIsNotAvailableYet() async {
        m3.masternodes.details.withLock { $0[evo.proTxHash] = masternodeDetail(evo) }
        let detail = MasternodeDetailViewModel(proTxHash: evo.proTxHash, env: world.environment(), m3: m3.services)
        await detail.load()
        #expect(detail.evonode == .unavailable)
        #expect(detail.platformLines.map(\.value) == ["Not available yet", "Not available yet"])
    }

    @Test func IOS081_bannedMasternodeOffersUnbanWithPendingState() async {
        m3.masternodes.details.withLock { $0[banned.proTxHash] = masternodeDetail(banned) }
        let detail = MasternodeDetailViewModel(proTxHash: banned.proTxHash, env: world.environment(), m3: m3.services)
        await detail.load()
        #expect(detail.canUnban)
        let unban = makeMaintenance(.unban, banned)
        await unban.load()
        #expect(unban.warning == "Sending a service update revives a PoSe-banned masternode.")
        await unban.prepare()
        guard case .review = unban.step else {
            Issue.record("expected review, got \(unban.step)")
            return
        }
        await unban.broadcast()
        #expect(unban.message == "Service update sent. The masternode stays banned until the transaction confirms.")
    }

    // MARK: Register (QT-123, QT-124)

    func makeWizard() -> RegisterMasternodeWizardViewModel {
        m3.masternodes.candidates.withLock {
            $0 = [
                CollateralCandidate(
                    outpoint: OutPoint(txid: txid(200), vout: 0), address: testnetAddress2,
                    amount: Amount(duffs: 1000_00000000), confirmations: 3, refusal: nil),
                CollateralCandidate(
                    outpoint: OutPoint(txid: txid(201), vout: 0), address: testnetAddress2,
                    amount: Amount(duffs: 1000_00000000), confirmations: 0, refusal: .unconfirmed),
            ]
        }
        m3.masternodes.feeSources.withLock {
            $0 = [FeeSourceCandidate(address: testnetAddress1, spendable: Amount(duffs: 1_00000000), label: nil)]
        }
        return RegisterMasternodeWizardViewModel(env: world.environment(), m2: m2.services, m3: m3.services)
    }

    @Test func QT123_pagesStepTextAndDefaultPorts() async {
        let wizard = makeWizard()
        #expect(wizard.pages == [.type, .collateral, .service, .keys, .payout, .fee, .review, .saveKey, .complete])
        #expect(wizard.progressText == "Step 1 of 9 · Masternode type")
        #expect(wizard.defaultPortText == "Default port: 19999")
        wizard.type = .evo
        wizard.collateralMode = .external
        #expect(wizard.pages == RegisterPage.allCases)
        #expect(wizard.windowTitle == "Register EvoNode")
        #expect(wizard.collateralAmount == Amount(duffs: 4000_00000000))
    }

    @Test func QT123_fieldChecksFollowDashQt() async {
        let wizard = makeWizard()
        wizard.collateralMode = .external
        wizard.externalTxid = "xyz"
        #expect(wizard.validate(.collateral) == "Enter the collateral transaction id as 64 hexadecimal characters.")
        wizard.serviceText = "1.2.3.4:19999, nope"
        #expect(wizard.validate(.service) == "Enter service addresses as IP:port, separated by commas or spaces.")
        wizard.serviceText = ""
        #expect(wizard.validate(.service) == nil)
        wizard.operatorKeyMode = .existing
        wizard.operatorPublicKey = "abcd"
        #expect(wizard.validate(.keys)?.hasPrefix("Enter a valid operator BLS public key") == true)
        wizard.ownerAddress = "yOwner"
        wizard.payoutAddress = "yOwner"
        #expect(wizard.validate(.payout) == "The payout address must differ from the owner and voting addresses.")
        wizard.payoutAddress = testnetAddress1
        wizard.operatorRewardText = "100.01"
        #expect(wizard.validate(.payout) == "Enter an operator reward between 0.00 and 100.00 %.")
        wizard.operatorRewardText = "12.5"
        #expect(wizard.validate(.payout) == nil)
        #expect(wizard.rewardWarning != nil)
        wizard.platformNodeID = "12"
        #expect(wizard.validate(.platform) == "Enter the Platform node ID as 40 hexadecimal characters.")
    }

    @Test func QT124_secretGateComesBeforeBroadcast() async {
        let wizard = makeWizard()
        await wizard.next()                         // type
        #expect(wizard.page == .collateral)
        #expect(wizard.usableCollateral.count == 1)
        wizard.collateralMode = .existing
        wizard.selectedCollateral = OutPoint(txid: txid(200), vout: 0)
        await wizard.next()                         // collateral
        wizard.serviceText = "1.2.3.4:19999"
        await wizard.next()                         // service
        await wizard.next()                         // keys
        wizard.payoutAddress = testnetAddress1
        await wizard.next()                         // payout
        #expect(wizard.page == .fee)
        #expect(wizard.feeSource == testnetAddress1)
        await wizard.next()                         // fee → prepare
        #expect(wizard.page == .review)
        #expect(m3.masternodes.registrations.current.first?.collateral == .existingUTXO(OutPoint(txid: txid(200), vout: 0)))
        #expect(m3.masternodes.registrations.current.first?.operatorRewardX100 == 0)
        #expect(wizard.reviewLines.contains { $0.title == "Network fee" })
        #expect(wizard.nextTitle == "Continue")
        await wizard.next()                         // review → secret
        #expect(wizard.page == .saveKey)
        #expect(!wizard.canGoBack)
        #expect(wizard.operatorSecret != nil)
        wizard.copySecret(configLine: true)
        #expect(m2.clipboard.text.current?.hasPrefix("masternodeblsprivkey=") == true)
        #expect(!wizard.canGoNext)
        wizard.last4 = "0000"
        await wizard.next()
        #expect(wizard.errorMessage == "The characters do not match the end of the secret key.")
        #expect(m3.masternodes.submitted.current.isEmpty)
        wizard.last4 = "CD1F"
        await wizard.next()
        #expect(wizard.page == .complete)
        #expect(wizard.proTxHash == txid(500))
        #expect(wizard.operatorSecret == nil)
        #expect(wizard.progressText == "Complete")
    }

    @Test func QT123_externalCollateralAsksForTheSignature() async {
        let wizard = makeWizard()
        wizard.operatorKeyMode = .existing
        wizard.operatorPublicKey = String(repeating: "ab", count: 48)
        wizard.collateralMode = .external
        wizard.externalTxid = txid(300)
        wizard.payoutAddress = testnetAddress1
        for _ in 0..<5 { await wizard.next() }
        #expect(wizard.page == .fee)
        await wizard.next()
        #expect(wizard.page == .review)
        #expect(wizard.nextTitle == "Prepare")
        await wizard.next()
        #expect(wizard.page == .sign)
        #expect(wizard.collateralSignMessage == "payout|0|owner|voting|hash")
        await wizard.next()
        #expect(wizard.errorMessage?.hasPrefix("Paste the base64-encoded signature") == true)
        wizard.collateralSignature = "H+sig="
        await wizard.next()
        #expect(wizard.page == .complete)
        #expect(m3.masternodes.submitted.current.first?.1 == "H+sig=")
    }

    @Test func QT123_backFromReviewAbandonsAndCancelReleases() async {
        let wizard = makeWizard()
        wizard.payoutAddress = testnetAddress1
        for _ in 0..<6 { await wizard.next() }
        #expect(wizard.page == .review)
        await wizard.back()
        #expect(wizard.page == .fee)
        #expect(m3.masternodes.abandoned.current.count == 1)
        #expect(wizard.prepared == nil)
    }

    @Test func QT123_rejectReasonsGetDashQtExplanations() {
        let error = ServiceError(code: .masternodeBroadcastRejected, detail: "bad-protx-dup-addr")
        #expect(ErrorText.m3(error, amount: { _ in "" }) ==
            "The network rejected the transaction. The chosen service address is already in use by a registered masternode.")
        let unknown = ServiceError(code: .masternodeBroadcastRejected, detail: "<script>")
        #expect(ErrorText.m3(unknown, amount: { _ in "" }) == "The network rejected the transaction.")
    }

    // MARK: Maintenance (QT-125, QT-127)

    func makeMaintenance(_ kind: MaintenanceKind, _ row: MasternodeRow) -> MasternodeMaintenanceViewModel {
        m3.masternodes.details.withLock { $0[row.proTxHash] = masternodeDetail(row) }
        m3.masternodes.feeSources.withLock { $0 = [] }
        return MasternodeMaintenanceViewModel(kind: kind, proTxHash: row.proTxHash, env: world.environment(), m2: m2.services, m3: m3.services)
    }

    @Test func QT125_updateServiceTypesTheSecretEachTimeAndReviewsBeforeSending() async {
        let model = makeMaintenance(.updateService, owned)
        await model.load()
        #expect(model.serviceText == "1.2.3.4:19999")
        #expect(model.showsOperatorPayout)
        model.serviceText = "9.9.9.9:19999"
        model.operatorSecretText = "deadbeef"
        await model.prepare()
        #expect(model.operatorSecretText.isEmpty)
        let request = try! #require(m3.masternodes.serviceRequests.current.first)
        #expect(request.serviceAddresses == ["9.9.9.9:19999"])
        #expect(request.operatorSecret?.testString == "deadbeef")
        #expect(model.reviewLines == ["Network fee: 0.00000500 tDASH"])
        await model.broadcast()
        #expect(model.step == .sent(txid: txid(600)))
    }

    @Test func QT125_updateRegistrarSendsOnlyChangesAndWarnsOnOperatorKey() async {
        let model = makeMaintenance(.updateRegistrar, owned)
        await model.load()
        await model.prepare()
        #expect(model.errorMessage == "Nothing changed.")
        model.operatorPublicKey = String(repeating: "ef", count: 48)
        #expect(model.warning?.hasPrefix("Changing the operator key immediately PoSe-bans") == true)
        await model.prepare()
        let request = try! #require(m3.masternodes.registrarRequests.current.first)
        #expect(request.operatorPublicKey == String(repeating: "ef", count: 48))
        #expect(request.votingAddress == nil)
        #expect(model.reviewLines.contains { $0.hasPrefix("Changing the operator key") })
    }

    @Test func QT125_revokeCarriesTheReasonAndBackAbandons() async {
        let model = makeMaintenance(.revoke, owned)
        await model.load()
        model.reason = .compromisedKeys
        await model.prepare()
        #expect(m3.masternodes.providerRequests.current == ["revoke:compromisedKeys"])
        await model.backToEditing()
        #expect(model.step == .editing)
        #expect(m3.masternodes.abandoned.current.count == 1)
    }

    @Test func QT127_standbyDissolutionIsSavedAndRecorded() async {
        let model = makeMaintenance(.standbyDissolution, shared)
        await model.load()
        await model.prepare()
        #expect(model.standbyFileText == "0100aa\n0100bb\n")
        #expect(m2.desktopPreferences.desktop.m3.standbyDissolutions[shared.proTxHash] != nil)
        let list = makeList()
        let item = list.menu(for: shared).first { $0.action == .createStandbyDissolution }
        #expect(item?.tooltip?.hasPrefix("Already saved on this computer on") == true)
    }

    // MARK: Shared (QT-126, QT-127)

    func makeShared() -> SharedMasternodeViewModel {
        SharedMasternodeViewModel(env: world.environment(), m2: m2.services, m3: m3.services)
    }

    @Test func QT126_termsFollowTheShareRules() {
        let model = makeShared()
        #expect(model.termsError == nil)
        model.shares = [ShareDraft(amountText: "950"), ShareDraft(amountText: "50")]
        #expect(model.termsError == "Each share must be at least 100 DASH.")
        model.shares = [ShareDraft(amountText: "500"), ShareDraft(amountText: "400")]
        #expect(model.termsError == "The shares must add up to exactly 1000 DASH.")
        model.shares = [ShareDraft(amountText: "1000")]
        #expect(model.termsError == "A shared masternode has 2 to 8 shares.")
        model.shares = [ShareDraft(amountText: "500"), ShareDraft(amountText: "500")]
        model.earlyPeriodText = "500000"
        #expect(model.termsError == "The early period is at most 420480 blocks.")
        model.earlyPeriodText = "1000"
        model.penaltyText = "500"
        #expect(model.termsError == "The penalty must be smaller than the smallest share.")
    }

    @Test func QT126_coordinatorAndParticipantFlowOverEnvelopes() async {
        let model = makeShared()
        await model.create()
        #expect(model.session?.role == .coordinator)
        #expect(model.fingerprintText == "Fingerprint ABCD-1234")
        model.copyMessage()
        #expect(m2.clipboard.text.current?.contains("dash-shared-mn-session") == true)

        let participant = makeShared()
        await participant.importText("{\"type\":\"dash-shared-mn-session\"}")
        #expect(participant.session?.role == .participant)
        #expect(participant.canContribute)
        participant.contributionInputs = [OutPoint(txid: txid(1), vout: 0)]
        await participant.contribute()
        #expect(participant.session?.stage == .lockedTerms)
        await participant.approve()
        #expect(participant.session?.stage == .signingRequest)
        await participant.sign()
        #expect(participant.session?.stage == .signedContributions)
        // Close protection: reserved coins ask before closing.
        #expect(participant.requestClose() == false)
        #expect(participant.closeProtection)
        await participant.releaseAndClose()
        #expect(m3.masternodes.abandonedSessions.current == ["s2"])
    }

    @Test func QT127_pasteRoutingSendsStandbyTextToBroadcast() async {
        let model = makeShared()
        await model.importText("standby:0100aa 0100bb")
        #expect(model.mode == .standby(proTxHash: nil, transactionsHex: ["0100aa", "0100bb"]))
        await model.broadcastStandby()
        #expect(model.message?.hasPrefix("Standby dissolution sent:") == true)
    }

    @Test func QT127_oversizedPasteIsRefused() async {
        let model = makeShared()
        await model.importText(String(repeating: "x", count: 2 * 1024 * 1024 + 1))
        #expect(model.errorMessage?.hasPrefix("The message is too large") == true)
    }

    @Test func QT127_keyRotationAndDissolveTogetherStartSessions() async {
        let model = makeShared()
        await model.startKeyRotation(proTxHash: shared.proTxHash, operatorKey: .generate, votingAddress: nil)
        #expect(model.session?.purpose == .rotateKeys)
        await model.startDissolveTogether(proTxHash: shared.proTxHash)
        #expect(model.session?.purpose == .dissolveTogether)
    }

    // MARK: Keychain and tracked (IOS-082, IOS-083)

    @Test func IOS083_keychainPagesKeysAndRevealsBehindAGrant() async {
        m3.masternodes.keyInfos.withLock {
            $0 = (0..<25).map { index in
                MasternodeKeyInfo(
                    role: .voting, index: UInt32(index), derivationPath: "m/9'/1'/3'/1'/\(index)", address: "yVote\(index)",
                    publicKeyHex: "02ab", legacyPublicKeyHex: nil, platformNodeID: nil,
                    usedBy: index == 0 ? [MasternodeKeyUsage(proTxHash: txid(1), service: "1.2.3.4:19999", revoked: false)] : [])
            }
        }
        world.auth.lockState = .unlocked
        let model = MasternodeKeychainViewModel(env: world.environment(), m2: m2.services, m3: m3.services)
        await model.select(.voting)
        #expect(model.keys.count == 20)
        await model.loadMore()
        #expect(model.keys.count == 25)
        #expect(model.usageText(model.keys[0]) == "Used by \(txid(1)) (1.2.3.4:19999)")
        #expect(model.usageText(model.keys[1]) == "Unused")
        await model.reveal(model.keys[2])
        #expect(model.pendingReveal?.index == 2)
        await model.reveal(model.keys[2], passphrase: "pw")
        #expect(model.revealed["m/9'/1'/3'/1'/2"]?.privateKeyHex.testString == "priv-voting-2")
        #expect(world.auth.authorizeCalls.last?.purpose == .revealSecret)
        await model.select(.owner)
        #expect(model.revealed.isEmpty)
    }

    @Test func IOS082_trackLocateAttachAndWithdrawNotAvailableYet() async {
        m3.masternodes.trackedList.withLock { $0 = [] }
        let model = TrackedMasternodesViewModel(env: world.environment(), m3: m3.services)
        await model.reload()
        model.locateQuery = "5.6.7.8"
        await model.locate()
        #expect(model.results.map(\.proTxHash) == [evo.proTxHash])
        await model.track(evo.proTxHash, label: "friend's evonode")
        #expect(model.tracked.map(\.label) == ["friend's evonode"])
        await model.track(evo.proTxHash)
        #expect(model.errorMessage == "This masternode is already tracked.")
        await model.attach(keyText: "  cVoterWif  ", role: .voting, proTxHash: evo.proTxHash)
        #expect(m3.masternodes.attached.current.first?.2 == "cVoterWif")
        #expect(world.auth.authorizeCalls.last?.purpose == .masternodeOperation)
        await model.withdraw(proTxHash: evo.proTxHash, credits: 1_000, destination: .payoutAddress)
        #expect(model.errorMessage == "Not available yet")
    }
}
